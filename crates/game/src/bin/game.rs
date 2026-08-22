//! The game itself: an endless ocean of generated islands to look around.
//!
//! What `cargo run` runs, being the crate's `default-run`. The others are
//! `mapgen`, which draws the same terrain from above without opening a
//! window, and `server`, which hosts a world without drawing it at all.
//!
//! `game --help` lists what it can be asked for, which is not much on
//! purpose: how a run *behaves* is said down the debug socket rather than on
//! the command line — see `game::control`.

use std::process::ExitCode;
use std::time::Duration;

use bevy::app::ScheduleRunnerPlugin;
use bevy::prelude::*;
use bevy::window::{ExitCondition, WindowResolution};

use game::ambience::AmbiencePlugin;
use game::backdrop::BackdropPlugin;
use game::beasts::BeastsPlugin;
use game::boat::BoatPlugin;
use game::cairn::CairnPlugin;
use game::camera::MapCameraPlugin;
use game::chart::ChartPlugin;
use game::cli::{self, Args};
use game::compass::CompassPlugin;
use game::console::ConsolePlugin;
use game::control::{self, ControlPlugin};
use game::debug::DebugOverlayPlugin;
use game::logbook::{self, LogbookPlugin};
use game::menu::MenuPlugin;
use game::models::ModelsPlugin;
use game::net::{Hosting, NetPlugin, Online, Reach, Session};
use game::player::PlayerPlugin;
use game::settings::{self, DisplaySettings, SettingsPlugin};
use game::sky::SkyPlugin;
use game::stopping::StoppingPlugin;
use game::terrain::TerrainPlugin;
use game::trees::TreesPlugin;
use game::wake::WakePlugin;
use game::wildlife::WildlifePlugin;
use game::{AppState, Helm};

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
        // Alone and unkept, because a run that asked for a world on the
        // command line asked for one to look at rather than one to be joined
        // or returned to: sharing and keeping are the menu's business, and a
        // dedicated `server` is the other binary.
        (None, AppState::InWorld) => Some(Session::open(
            args.config,
            Reach::Alone,
            server::OPENING,
            false,
        )),
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

    // Before the app, for the reason the session is: a port already in use is
    // a plain failure to start, and finding that out from inside a running
    // game would mean a window opening on a run that cannot be driven.
    let control = match args.debug.map(control::open).transpose() {
        Ok(control) => control,
        Err(message) => {
            eprintln!("game: {message}");
            return ExitCode::FAILURE;
        }
    };

    run(args, session, control);
    ExitCode::SUCCESS
}

fn run(args: Args, session: Option<Session>, control: Option<control::Control>) {
    let mut app = App::new();
    let headless = args.is_headless();

    // Read off the file before the window is built, because the window is
    // built out of it — see [`settings::opening`]. A windowless run reads
    // nothing: a picture asked for by the command line has to come out the
    // same on any machine, and the settings are this machine's.
    let display = if args.is_headless() {
        DisplaySettings::default()
    } else {
        settings::load()
    };
    app.insert_resource(display);

    app.add_plugins(
        DefaultPlugins
            .set(window_plugin(&args, &display))
            .set(asset_plugin()),
    );

    // A windowless run has nothing driving the frame loop — winit's runner has
    // no events to wait on — so it runs its own, paced like a game rather than
    // spun as fast as the machine goes. It is a game being played by something
    // that is not a person, and everything it is asked to do is timed in
    // seconds: a press held for twenty of them wants twenty seconds of world,
    // not as many frames as a core can be burned producing.
    if args.is_headless() {
        app.add_plugins(ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(
            1.0 / 60.0,
        )));
    }

    // Both in every run, windowed or not. The readout starts hidden and the
    // console starts closed, so neither costs a run that never asks for them —
    // and a run being driven down the socket can ask: `set stats on` puts the
    // readout up, which is how the frame rate and the chunk counts get into a
    // picture at all. There used to be a `--debug` flag that turned it on from
    // the command line, and it is gone for the reason the rest of them are:
    // the socket says it, and can also say it back off again.
    app.add_plugins((DebugOverlayPlugin, ConsolePlugin));

    // Not in a windowless run either: nobody is at a run with no window, and
    // it would open an audio device for no one.
    if !args.is_headless() {
        app.add_plugins(AmbiencePlugin);
    }

    if let Some(session) = session {
        // The logbook before the pieces of the session are given up: a
        // joined world is one this machine remembers, and the book is what
        // the chart and the boat read on the way in.
        if let Some(logbook) = logbook::for_session(&session) {
            app.insert_resource(logbook);
        }
        if let Some(host) = session.hosting {
            app.insert_resource(Hosting(host));
        }
        app.insert_resource(Online::new(session.connection));
    }

    app.insert_state(args.state)
        // Comes into being with the world and goes with it, so it cannot be
        // inserted the way a top-level state is. A run always enters a world
        // at the helm — the pause menu and the screens under it are walked
        // into, never opened on — so the default is the whole of it.
        .add_sub_state::<Helm>()
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
            // The boats, and the cairns a claim leaves standing. Paired only
            // because a plugin tuple holds fifteen.
            (BoatPlugin, CairnPlugin),
            // The white water the boat leaves, painted by the sea itself.
            WakePlugin,
            PlayerPlugin,
            MapCameraPlugin,
            CompassPlugin,
            ChartPlugin,
            // The logbook — harmless in a world nobody remembers, its systems
            // conditioning on a book being open — then the sheet the menus
            // stand on, and the menus. Nested only because a plugin tuple
            // holds fifteen.
            (LogbookPlugin, BackdropPlugin, MenuPlugin, SettingsPlugin),
            // The session — harmless offline, its systems conditioning on a
            // joined one — and the machine's own way of asking this to quit,
            // which matters most in a run that is hosting: the world is
            // written down in the drop an ordinary exit reaches and a killed
            // process does not. Paired only because a plugin tuple holds
            // fifteen.
            (NetPlugin, StoppingPlugin),
        ));

    // Last, and only when asked for: the socket is a mouth on everything above
    // rather than a part of any of it, and a run without one should be the run
    // it would have been before this existed.
    if let Some(control) = control {
        app.insert_resource(control)
            .add_plugins(ControlPlugin { headless });
    }

    app.run();
}

/// Where to read assets from, which is only ever a question inside a bundle.
///
/// Bevy resolves `assets/` against `BEVY_ASSET_ROOT`, then `CARGO_MANIFEST_DIR`,
/// and failing both against the directory holding the executable. Under cargo
/// the first is set — `.cargo/config.toml` points it at the workspace root, so
/// that the assets beside the crates are the ones a run reads — and a
/// double-clicked application has neither, leaving it looking beside the binary
/// in `Contents/MacOS`. That is not where they are: macOS puts everything a
/// program only reads in `Contents/Resources`, and it is the bundle's shape
/// that has to give, not Apple's.
fn asset_plugin() -> AssetPlugin {
    let mut plugin = AssetPlugin::default();
    #[cfg(target_os = "macos")]
    if let Some(bundled) = bundled_assets() {
        plugin.file_path = bundled;
    }
    plugin
}

/// The assets in the bundle this is running from, if it is running from one.
///
/// An absolute path rather than a `../Resources/assets` relative to wherever
/// Bevy was going to look, because then it is the whole answer: a bundle that
/// happens to be launched with `BEVY_ASSET_ROOT` set in the environment reads
/// its own assets rather than half of somebody else's checkout's.
#[cfg(target_os = "macos")]
fn bundled_assets() -> Option<String> {
    // .../Genovesa.app/Contents/MacOS/Genovesa, and nothing else counts: a bare
    // binary run out of `target/release` is not in a bundle and must keep
    // falling through to the environment the way every other platform does.
    let exe = std::env::current_exe().ok()?;
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    if macos.file_name()? != "MacOS" || contents.file_name()? != "Contents" {
        return None;
    }
    let assets = contents.join("Resources/assets");
    Some(assets.to_string_lossy().into_owned())
}

/// A window to play in, or none at all when the run is here to be driven down
/// a socket and looked at through the pictures it writes.
fn window_plugin(args: &Args, display: &DisplaySettings) -> WindowPlugin {
    if args.is_headless() {
        return WindowPlugin {
            primary_window: None,
            // Nothing to close, so closing cannot be what ends the run.
            exit_condition: ExitCondition::DontExit,
            close_when_requested: false,
            ..default()
        };
    }

    let (mode, size) = settings::opening(display);
    WindowPlugin {
        primary_window: Some(Window {
            title: "Genovesa".into(),
            mode,
            resolution: WindowResolution::new(size.x, size.y),
            ..default()
        }),
        ..default()
    }
}
