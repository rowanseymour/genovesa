//! Development helper: render a few frames, save a screenshot, then quit.
//!
//! Enabled by setting `KASSITER_SCREENSHOT=<path>`. Useful for eyeballing
//! terrain generation changes without sitting in front of the window.

use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::input::InputSystems;
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};

#[derive(Resource)]
struct ScreenshotRun {
    path: String,
    frame: u32,
}

/// Frames to render before capturing, so shadows and asset uploads have settled.
const WARMUP_FRAMES: u32 = 240;
/// Frames to wait after capturing, so the file is written before we exit.
const COOLDOWN_FRAMES: u32 = 30;

pub struct ScreenshotPlugin;

impl Plugin for ScreenshotPlugin {
    fn build(&self, app: &mut App) {
        let Ok(path) = std::env::var("KASSITER_SCREENSHOT") else {
            return;
        };

        app.insert_resource(ScreenshotRun { path, frame: 0 })
            .add_systems(PreUpdate, ignore_input.after(InputSystems))
            .add_systems(Update, capture);
    }
}

/// Swallows every input for the duration of a capture run, so that a keypress
/// landing in the window while it is up cannot pan the camera or leave the
/// match. Without it two runs of the same command can frame differently, which
/// makes before-and-after screenshots useless for judging a change.
///
/// Done centrally, by clearing the input resources once the input plugin has
/// filled them, rather than by gating each system that reads them — that way
/// nothing new can start responding to input by accident.
fn ignore_input(
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut buttons: ResMut<ButtonInput<MouseButton>>,
    mut scroll: ResMut<AccumulatedMouseScroll>,
) {
    keys.reset_all();
    keys.clear();
    buttons.reset_all();
    buttons.clear();
    *scroll = AccumulatedMouseScroll::default();
}

fn capture(
    mut commands: Commands,
    mut run: ResMut<ScreenshotRun>,
    mut exit: MessageWriter<AppExit>,
) {
    run.frame += 1;

    if run.frame == WARMUP_FRAMES {
        info!("capturing screenshot to {}", run.path);
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(run.path.clone()));
    }

    if run.frame == WARMUP_FRAMES + COOLDOWN_FRAMES {
        exit.write(AppExit::Success);
    }
}
