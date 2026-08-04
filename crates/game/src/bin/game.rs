//! The game itself: generates a map and lets you look around it.
//!
//! What `cargo run` runs, being the crate's `default-run`. The other binary,
//! `mapgen`, draws the same terrain from above without opening a window.
//!
//! `game --help` lists what it can be asked for. Given `--shot` it renders
//! pictures off screen and quits instead of opening a window at all.

use std::process::ExitCode;
use std::time::Duration;

use bevy::app::ScheduleRunnerPlugin;
use bevy::prelude::*;
use bevy::window::ExitCondition;

use game::camera::MapCameraPlugin;
use game::capture::CapturePlugin;
use game::cli::{self, Args};
use game::menu::MenuPlugin;
use game::terrain::TerrainPlugin;
use game::{SKY, WINDOW};

fn main() -> ExitCode {
    let args = match cli::parse(std::env::args().skip(1).collect()) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("game: {message}");
            return ExitCode::FAILURE;
        }
    };

    run(args);
    ExitCode::SUCCESS
}

fn run(args: Args) {
    let mut app = App::new();

    app.add_plugins(DefaultPlugins.set(window_plugin(&args)));

    // Capturing has no window, so nothing drives the frame loop — winit's
    // runner has no events to wait on. Run frames back to back instead, as
    // fast as they render, and let the capture quit when it is done.
    if args.is_capture() {
        app.add_plugins(ScheduleRunnerPlugin::run_loop(Duration::ZERO));
    }

    app.insert_state(args.state)
        .insert_resource(ClearColor(SKY))
        // Sky fill. Deliberately strong relative to the sun — this look wants
        // shadows that read as a second flat tone, not as darkness.
        .insert_resource(GlobalAmbientLight {
            color: Color::srgb(0.82, 0.89, 1.0),
            brightness: 1_400.0,
            ..default()
        })
        .insert_resource(args.config)
        .insert_resource(args.starting_view())
        .add_plugins((
            TerrainPlugin,
            MapCameraPlugin,
            MenuPlugin,
            CapturePlugin {
                resolution: args.resolution,
                shots: args.shots,
            },
        ));

    app.run();
}

/// A window to play in, or none at all when the run is only here to write
/// pictures.
fn window_plugin(args: &Args) -> WindowPlugin {
    if args.is_capture() {
        return WindowPlugin {
            primary_window: None,
            // Nothing to close, so closing cannot be what ends the run.
            exit_condition: ExitCondition::DontExit,
            close_when_requested: false,
            ..default()
        };
    }

    WindowPlugin {
        primary_window: Some(Window {
            title: "Kassiter".into(),
            resolution: (WINDOW.x, WINDOW.y).into(),
            ..default()
        }),
        ..default()
    }
}
