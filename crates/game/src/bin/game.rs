//! The game itself: an endless ocean of generated islands to look around.
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

use game::ambience::AmbiencePlugin;
use game::boat::BoatPlugin;
use game::camera::MapCameraPlugin;
use game::capture::CapturePlugin;
use game::cli::{self, Args};
use game::compass::CompassPlugin;
use game::debug::DebugOverlayPlugin;
use game::menu::MenuPlugin;
use game::net::{Hosting, NetPlugin, Online, Reach, Session};
use game::terrain::TerrainPlugin;
use game::{AppState, SKY, WINDOW};

fn main() -> ExitCode {
    let mut args = match cli::parse(std::env::args().skip(1).collect()) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("game: {message}");
            return ExitCode::FAILURE;
        }
    };

    // A run that named no seed is in a world picked off the clock, so say
    // which: without it a shot worth keeping, or a landscape worth walking
    // back into, could never be asked for a second time. A joined run is in
    // somebody else's world and has no seed of its own to name.
    if !args.seed_given && args.join.is_none() {
        println!("world {}", args.config.seed);
    }

    // A run that starts in a world gets one before the app exists, because
    // what the handshake learns — where this player stands, and what to look
    // at — is what the view is built from. Either somebody else's world or one
    // opened here; a run that starts on a menu screen has neither yet, and
    // waits for a button.
    let session = match (&args.join, args.state) {
        (Some(addr), _) => Some(Session::joining(addr)),
        // Alone, because a run that asked for a world on the command line
        // asked for one to look at rather than one to be joined: sharing is
        // the menu's switch, and a dedicated `server` is the other binary.
        (None, AppState::InWorld) => Some(Session::open(args.config, Reach::Alone)),
        (None, _) => None,
    };
    let session = match session.transpose() {
        Ok(session) => session,
        Err(message) => {
            eprintln!("game: {message}");
            return ExitCode::FAILURE;
        }
    };
    if let Some(session) = &session {
        args.opened_on(session.connection.spawn, session.connection.facing);
    }

    run(args, session);
    ExitCode::SUCCESS
}

fn run(args: Args, session: Option<Session>) {
    let mut app = App::new();

    app.add_plugins(DefaultPlugins.set(window_plugin(&args)));

    // Capturing has no window, so nothing drives the frame loop — winit's
    // runner has no events to wait on. Run frames back to back instead, as
    // fast as they render, and let the capture quit when it is done.
    if args.is_capture() {
        app.add_plugins(ScheduleRunnerPlugin::run_loop(Duration::ZERO));
    }

    // Not while capturing: the UI renders to the captured image, so the
    // readout would be baked into every shot.
    if args.debug && !args.is_capture() {
        app.add_plugins(DebugOverlayPlugin);
    }

    // Not while capturing either: a run that writes pictures and quits has no
    // menu to sit behind, and would open an audio device for nobody.
    if !args.is_capture() {
        app.add_plugins(AmbiencePlugin);
    }

    if let Some(session) = session {
        if let Some(host) = session.hosting {
            app.insert_resource(Hosting(host));
        }
        app.insert_resource(Online::new(session.connection));
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
        .insert_resource(args.starting_view())
        .add_plugins((
            TerrainPlugin,
            BoatPlugin,
            MapCameraPlugin,
            CompassPlugin,
            MenuPlugin,
            // Harmless offline: its systems condition on the joined session.
            NetPlugin,
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
            title: "Genovesa".into(),
            resolution: (WINDOW.x, WINDOW.y).into(),
            ..default()
        }),
        ..default()
    }
}
