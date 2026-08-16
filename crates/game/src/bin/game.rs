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
use game::beasts::BeastsPlugin;
use game::boat::BoatPlugin;
use game::camera::MapCameraPlugin;
use game::capture::CapturePlugin;
use game::chart::ChartPlugin;
use game::cli::{self, Args};
use game::compass::CompassPlugin;
use game::console::ConsolePlugin;
use game::debug::DebugOverlayPlugin;
use game::menu::MenuPlugin;
use game::models::ModelsPlugin;
use game::net::{Hosting, NetPlugin, Online, Reach, Session};
use game::player::PlayerPlugin;
use game::sky::SkyPlugin;
use game::terrain::TerrainPlugin;
use game::trees::TreesPlugin;
use game::wake::WakePlugin;
use game::wildlife::WildlifePlugin;
use game::{AppState, Helm, WINDOW};

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
        (None, AppState::InWorld) => Some(Session::open(args.config, Reach::Alone, args.opening)),
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
    // readout — or a console left open — would be baked into every shot. In
    // every windowed run the console is present and the readout with it,
    // hidden until `set stats on`; `--debug` just starts with it showing.
    if !args.is_capture() {
        app.add_plugins((DebugOverlayPlugin, ConsolePlugin));
        if args.debug {
            app.world_mut().resource_mut::<game::debug::Toggles>().stats = true;
        }
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
        // Comes into being with the world and goes with it, so it cannot be
        // inserted the way a top-level state is. The pending value is read
        // when the state is first created as well as on every change after,
        // which is what lets `--state paused` open on the pause menu; a run
        // that asked for no such thing sets the default it would have had.
        .add_sub_state::<Helm>()
        .insert_resource(NextState::Pending(args.helm))
        .insert_resource(args.starting_view())
        .add_plugins((
            // Before anything that draws a model: it owns the world's tones,
            // which the modules dressing rigged models register into.
            ModelsPlugin,
            TerrainPlugin,
            // Before the terrain and the rest only by convention; what it
            // owns — the clear colour, the ambient light and the one light in
            // the sky — is the world's whole lighting.
            SkyPlugin,
            TreesPlugin,
            WildlifePlugin,
            BeastsPlugin,
            BoatPlugin,
            // The white water the boat leaves, painted by the sea itself.
            WakePlugin,
            PlayerPlugin,
            MapCameraPlugin,
            CompassPlugin,
            ChartPlugin,
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
