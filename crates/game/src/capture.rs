//! Development helper: render a list of views to PNGs, then quit.
//!
//! Driven by `--shot`, which the command line turns into a [`Shot`] apiece.
//! The whole list is taken in one run, so the map is generated once and every
//! picture is of the same world — which is what makes a pair of them worth
//! comparing.
//!
//! Nothing is rendered to a window. The camera is pointed at an off-screen
//! image instead, so a run can take its pictures without a window appearing
//! and stealing the display.

use bevy::asset::RenderAssetUsages;
use bevy::camera::RenderTarget;
use bevy::image::Image;
use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::input::InputSystems;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
use bevy::render::view::screenshot::{save_to_disk, Screenshot};

use crate::boat::Boat;
use crate::camera::{MapCamera, View};
use crate::cli::Shot;
use crate::terrain::ChunkBuild;

/// Frames to render before the first shot, so terrain meshes have reached the
/// GPU and the shadow cascades have settled. Counted only while no chunk is
/// still streaming in — the world builds itself in background tasks, so the
/// clock starts once the ground being photographed has actually arrived.
const WARMUP_FRAMES: u32 = 120;
/// Frames between pointing the camera somewhere and capturing it. The camera
/// snaps rather than eases, so this only has to cover dropping the focus back
/// onto the ground and redrawing the shadow maps for the new view.
const SETTLE_FRAMES: u32 = 8;
/// Frames to leave a picture alone after asking for it. A screenshot is taken
/// from the camera as the frame renders, which is after the systems that could
/// move it have run — so moving on to the next shot straight away would frame
/// this one as that one.
const CAPTURE_FRAMES: u32 = 2;
/// Frames to wait after the last capture, so its file is written before we
/// exit. Bevy reads the picture back off the GPU over the following frames.
const COOLDOWN_FRAMES: u32 = 30;

/// What the run is waiting for.
enum Phase {
    /// Letting the world settle before taking anything.
    WarmUp,
    /// The camera is pointed at the next shot; letting that take effect.
    Settling,
    /// A picture has been asked for; letting the frame it belongs to render.
    Capturing,
    /// Every shot taken; letting the last file reach disk.
    Finishing,
}

/// The list of shots, and how far through it we are.
#[derive(Resource)]
struct Capture {
    shots: Vec<Shot>,
    /// The shot to take next.
    next: usize,
    phase: Phase,
    /// Frames rendered since this phase began.
    waited: u32,
    /// Where the camera renders, in place of a window.
    target: Handle<Image>,
}

/// Renders each `--shot` in turn and quits. Does nothing at all when there are
/// none, which is every run that is actually played.
pub struct CapturePlugin {
    pub shots: Vec<Shot>,
    pub resolution: UVec2,
}

impl Plugin for CapturePlugin {
    fn build(&self, app: &mut App) {
        if self.shots.is_empty() {
            return;
        }

        let target = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(offscreen(self.resolution));

        app.insert_resource(Capture {
            shots: self.shots.iter().map(clone_shot).collect(),
            next: 0,
            phase: Phase::WarmUp,
            waited: 0,
            target: target.clone(),
        })
        .add_systems(PreUpdate, ignore_input.after(InputSystems))
        // After `Startup`, so the camera the map plugin spawns there exists to
        // be pointed somewhere.
        .add_systems(PostStartup, render_off_screen)
        .add_systems(Update, capture);
    }
}

/// `Shot` is plain data, but deriving `Clone` on it only to move it out of the
/// plugin would put a derive on the command line's types for this file's sake.
fn clone_shot(shot: &Shot) -> Shot {
    Shot {
        path: shot.path.clone(),
        view: shot.view,
    }
}

/// An image for the camera to draw into. `COPY_SRC` is what lets the finished
/// picture be read back off the GPU.
fn offscreen(resolution: UVec2) -> Image {
    let size = Extent3d {
        width: resolution.x,
        height: resolution.y,
        depth_or_array_layers: 1,
    };
    let mut image = Image::new_fill(
        size,
        TextureDimension::D2,
        &[0, 0, 0, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.texture_descriptor.usage =
        TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC | TextureUsages::TEXTURE_BINDING;
    image
}

/// Points the camera at the off-screen image. Runs after the camera has been
/// spawned rather than as part of spawning it, so that the camera itself knows
/// nothing about being captured.
fn render_off_screen(
    mut commands: Commands,
    capture: Res<Capture>,
    mut cameras: Query<(Entity, &mut RenderTarget), With<MapCamera>>,
) {
    for (entity, mut target) in &mut cameras {
        *target = capture.target.clone().into();
        // Menus normally find their camera by looking for the one drawing the
        // primary window, and there isn't one. Saying outright which camera
        // the UI belongs to is what keeps a shot of a menu from coming out as
        // an empty sky.
        commands.entity(entity).insert(IsDefaultUiCamera);
    }
}

/// Swallows every input for the duration of a capture run, so that a keypress
/// landing while it is running cannot steer the boat or leave the match.
/// Without it two runs of the same command can frame differently, which makes
/// before-and-after screenshots useless for judging a change.
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

/// Walks the list: settle, take the picture, point the camera at the next one.
fn capture(
    mut commands: Commands,
    mut capture: ResMut<Capture>,
    mut cameras: Query<&mut MapCamera>,
    mut boats: Query<&mut Transform, With<Boat>>,
    building: Query<(), With<ChunkBuild>>,
    mut view: ResMut<View>,
    mut exit: MessageWriter<AppExit>,
) {
    // Ground still streaming in means the picture is not of the world yet —
    // hold the phase clock at zero until the last build lands, so the wait
    // that follows is all settling and none of it generation.
    if matches!(capture.phase, Phase::WarmUp | Phase::Settling) && !building.is_empty() {
        capture.waited = 0;
        return;
    }

    let wait = match capture.phase {
        Phase::WarmUp => WARMUP_FRAMES,
        Phase::Settling => SETTLE_FRAMES,
        Phase::Capturing => CAPTURE_FRAMES,
        Phase::Finishing => COOLDOWN_FRAMES,
    };
    capture.waited += 1;
    if capture.waited < wait {
        return;
    }
    capture.waited = 0;

    match capture.phase {
        // Both waits end the same way: what is on screen is the view wanted,
        // so ask for the picture. Warming up is the longer of the two because
        // the camera starts at the first shot's view — nothing has to move for
        // it, but the world it is looking at has to finish arriving.
        Phase::WarmUp | Phase::Settling => {
            let Some(shot) = capture.shots.get(capture.next) else {
                capture.phase = Phase::Finishing;
                return;
            };
            info!(
                "shot {} of {} -> {}",
                capture.next + 1,
                capture.shots.len(),
                shot.path
            );
            let path = shot.path.clone();
            commands
                .spawn(Screenshot::image(capture.target.clone()))
                .observe(save_to_disk(path));
            capture.phase = Phase::Capturing;
        }
        // The picture has been rendered, so the camera is free to move on.
        Phase::Capturing => {
            capture.next += 1;
            match capture.shots.get(capture.next) {
                Some(shot) => {
                    let wanted = shot.view;
                    // Kept in step with the camera so that anything else
                    // reading the view agrees with what is being captured.
                    *view = wanted;
                    for mut camera in &mut cameras {
                        camera.snap_to(wanted);
                    }
                    // The camera is pinned to the boat, so a sweep moves the
                    // boat and the view follows — teleported, there being
                    // nobody to watch it sail there. The height is stale
                    // until `float` sees the new ground, which the settling
                    // frames absorb. A shot of a menu has no boat, and the
                    // camera then stands wherever it was snapped.
                    for mut boat in &mut boats {
                        boat.translation.x = wanted.focus.x;
                        boat.translation.z = wanted.focus.z;
                    }
                    capture.phase = Phase::Settling;
                }
                None => capture.phase = Phase::Finishing,
            }
        }
        Phase::Finishing => {
            exit.write(AppExit::Success);
        }
    }
}
