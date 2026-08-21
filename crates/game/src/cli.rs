//! The game binary's command line.
//!
//! Deliberately short. What a run needs to be *started* with is here — which
//! world, which screen, whether to join somebody else's — and everything
//! about how a run behaves once it is going is said down the debug socket
//! instead, in [`crate::control`]'s grammar.
//!
//! It used to carry the second kind too: a point to look at, a distance, a
//! bearing, a list of pictures to take, an hour to open at. Each of those
//! could be said once, before anything existed, and never again — so a sweep
//! of views was a row of `--shot`s read left to right, and anything wanting
//! the world to have *happened* first was out of reach entirely. A socket
//! says all of it at any point and as often as it likes, which is strictly
//! more, so the options went rather than staying on as a second way of saying
//! a subset.
//!
//! What is left is what a socket cannot say, because it is settled before
//! there is a game to say it to: the world, the screen, and the port the
//! socket itself listens on.

use args::pair;
use bevy::math::{UVec2, Vec2, Vec3};
use protocol::DEFAULT_PORT;

use crate::camera::View;
use crate::{AppState, Helm};
use server::{random_seed, WorldConfig};

/// Size of a picture the socket's `shot` writes in a run with no window.
/// Matches the shots already in `screenshots/`, which came off a 1280x720
/// window on a doubled display.
const DEFAULT_RESOLUTION: UVec2 = UVec2::new(2560, 1440);

/// What the command line asked for.
pub struct Args {
    pub state: AppState,
    /// What the player is doing in the world, when the screen asked for is one
    /// inside a world. Ignored otherwise — there is no helm to be at on a menu
    /// screen — so it costs the menu screens nothing to carry it.
    pub helm: Helm,
    pub config: WorldConfig,
    /// Server to join, as `host` or `host:port`. A joined run takes the
    /// world — and where to look — from the server's welcome.
    pub join: Option<String>,
    /// Whether `--seed` was given. A run that did not name one is in a world
    /// picked off the clock, which is worth saying out loud: otherwise a place
    /// worth going back to could never be asked for a second time.
    pub seed_given: bool,
    /// Port the debug socket listens on, if this run was asked to open one.
    /// `None` in every ordinary run — see [`crate::control`].
    pub debug: Option<u16>,
    /// Whether a debugged run does without a window. Meaningless on its own: a
    /// run nobody can drive has nothing to be windowless *for*.
    pub headless: bool,
    /// Size of each picture the socket writes. Ignored in a windowed run,
    /// where a picture is the size of the window.
    pub resolution: UVec2,
    /// Where the camera opens. Not settable here any more — it is whatever the
    /// welcome says, and moving it afterwards is the socket's `focus`, `zoom`
    /// and `yaw`.
    view: View,
}

impl Args {
    /// Whether this run opens a window at all. Only one way not to, and it has
    /// to be asked for.
    ///
    /// What hangs off it is everything a window would have been for — the
    /// display settings, the audio device, and the frame loop, which winit
    /// drives for a window and nothing drives without one.
    pub fn is_headless(&self) -> bool {
        self.debug.is_some() && self.headless
    }

    /// Where the camera opens on.
    pub fn starting_view(&self) -> View {
        self.view
    }

    /// Points the run at the world a server has just described: where it put
    /// this player down, and the land it said to look at.
    ///
    /// The spawn stands `SPAWN_OFFSHORE` metres off the coast precisely so
    /// that land fills the opening screen. An island's portrait — somewhere
    /// else, from some other bearing — is the socket's business now, and it
    /// can be taken at any point rather than only at the start.
    pub fn opened_on(&mut self, spawn: Vec2, facing: Vec2) {
        self.view.focus = Vec3::new(spawn.x, 0.0, spawn.y);
        self.view.face(facing);
    }
}

/// Built rather than written out so the defaults it quotes are read from the
/// code itself and cannot drift.
fn usage() -> String {
    format!(
        "\
Genovesa — an endless ocean of generated islands to look around.

Usage: game [options]

Options:
  --state <screen>  start on `mainmenu`, `setsail`, `newworld`, `joinworld`,
                    `options`, `display`, `controls`, `inworld`, `paused`,
                    `pausedoptions`, `pauseddisplay`, `pausedcontrols` or
                    `chart`
                    [default: mainmenu, or inworld when a server or a debug
                    socket is asked for]
  --seed <n>        the world to open [default: a new one every run, and the
                    run says which so it can be asked for again]
  --join <host[:port]>  play in somebody else's world instead of opening one;
                    the server says where the world is entered, and the run
                    starts in that world rather than on a screen
                    [port: {DEFAULT_PORT}]

Every world is served. A run that opens one runs a server for itself, reachable
from this machine only; a world started from the menu can be shared instead,
which hosts it on port {DEFAULT_PORT} for others to `--join`. Either way it is
the same session a dedicated `server` serves.

Debugging:
  --debug <port>    take console lines on 127.0.0.1:<port>, one per line,
                    answering each — once its work is done — with what it did
                    and a blank line. Everything the console takes: `set` for
                    this client's own switches, anything else for the server.
                    Plus the words a keyboard never needed — `shot`, `press`,
                    `focus`, `zoom`, `yaw`, `hold` and `quit`. Send `help` for
                    the whole vocabulary
  --headless        no window: draw off screen and be driven down the socket
                    alone
  --resolution <WxH>  size of the pictures `shot` writes in a run with no
                    window [default: {}x{}]

The view, the weather, the clock and the controls are reached down the socket
and nowhere else — an option could only ever say them once, and before
anything existed:

  game --seed 7 --debug 7777 --headless
  printf 'focus 98,-317\\nzoom 120\\nshot near.png\\nquit\\n' | nc 127.0.0.1 7777
",
        DEFAULT_RESOLUTION.x, DEFAULT_RESOLUTION.y,
    )
}

/// Reads the arguments the binary was started with, printing usage and quitting
/// if that is all that was asked for.
pub fn parse(argv: Vec<String>) -> Result<Args, String> {
    if argv.iter().any(|a| a == "-h" || a == "--help") {
        print!("{}", usage());
        std::process::exit(0);
    }

    // An unasked-for world is a new one rather than the same one every time:
    // the ocean is endless and there is nothing special about any seed in it,
    // so a run that says nothing is better off somewhere it has not been.
    let mut args = Args {
        state: AppState::MainMenu,
        helm: Helm::Sailing,
        config: WorldConfig {
            seed: random_seed(),
        },
        join: None,
        seed_given: false,
        debug: None,
        headless: false,
        resolution: DEFAULT_RESOLUTION,
        view: View::default(),
    };
    let mut state_given = false;

    // `--headless` is the one flag that stands on its own; every other option
    // takes a value, so past it an option in the last position is always a
    // missing value.
    let mut rest = argv.iter();
    while let Some(flag) = rest.next() {
        if flag == "--headless" {
            args.headless = true;
            continue;
        }
        let value = rest
            .next()
            .ok_or_else(|| format!("`{flag}` needs a value"))?;
        match flag.as_str() {
            "--state" => {
                (args.state, args.helm) = state(value)?;
                state_given = true;
            }
            "--seed" => {
                args.config.seed = value
                    .parse()
                    .map_err(|_| format!("`{value}` is not a seed"))?;
                args.seed_given = true;
            }
            "--join" => args.join = Some(value.clone()),
            "--resolution" => args.resolution = resolution(value)?,
            "--debug" => {
                args.debug = Some(
                    value
                        .parse()
                        .map_err(|_| format!("`{value}` is not a port"))?,
                );
            }
            other => return Err(format!("unknown option `{other}`\n\n{}", usage())),
        }
    }

    // A joined world is somebody else's, whole: a seed given alongside would
    // have nothing to open, since this run is not the one making a world.
    if args.join.is_some() && args.seed_given {
        return Err(
            "`--seed` picks a world to open, but joining plays in one somebody \
             else has already opened"
                .into(),
        );
    }

    // And a joined session is only a session in the served world. Starting on
    // any other screen leaves it running behind a menu whose "new world" opens
    // a second one — the run would go on reporting its position into an ocean
    // it is no longer standing in, and draw the other players' markers on
    // ground that isn't theirs.
    if args.join.is_some() && state_given && args.state != AppState::InWorld {
        return Err(
            "`--join` plays in the served world, so a joined run cannot start on another screen"
                .into(),
        );
    }

    // Windowless is a thing a debugged run can be, not a thing a run can be on
    // its own: without the socket there would be nothing to drive it and no
    // way to see that it was running at all.
    if args.headless && args.debug.is_none() {
        return Err(
            "`--headless` is for a `--debug` run to be driven without a window, \
             and there is nothing else to do with one"
                .into(),
        );
    }

    // Joining a server means playing in its world, and a socket has nothing to
    // drive on a menu — so save every caller from saying so.
    if (args.join.is_some() || args.debug.is_some()) && !state_given {
        args.state = AppState::InWorld;
    }

    // Where to look is not settled here, because nothing on this side knows
    // where anything is: the world comes from a server, and the server is the
    // one that can say where it is entered. See [`Args::opened_on`], which the
    // binary calls once a welcome has arrived.
    Ok(args)
}

/// A screen by name, as the two states it takes to be on one. The screens
/// inside a world are named in their own right rather than behind a second
/// flag: opening on the pause menu is as fair a thing to ask for as opening on
/// any other, and it wants a world drawn behind it.
fn state(value: &str) -> Result<(AppState, Helm), String> {
    match value {
        "mainmenu" => Ok((AppState::MainMenu, Helm::Sailing)),
        "setsail" => Ok((AppState::SetSail, Helm::Sailing)),
        "newworld" => Ok((AppState::NewWorld, Helm::Sailing)),
        "joinworld" => Ok((AppState::JoinWorld, Helm::Sailing)),
        "options" => Ok((AppState::Options, Helm::Sailing)),
        "display" => Ok((AppState::Display, Helm::Sailing)),
        "controls" => Ok((AppState::Controls, Helm::Sailing)),
        "inworld" => Ok((AppState::InWorld, Helm::Sailing)),
        "paused" => Ok((AppState::InWorld, Helm::Paused)),
        "pausedoptions" => Ok((AppState::InWorld, Helm::Options)),
        "pauseddisplay" => Ok((AppState::InWorld, Helm::Display)),
        "pausedcontrols" => Ok((AppState::InWorld, Helm::Controls)),
        "chart" => Ok((AppState::InWorld, Helm::Chart)),
        other => Err(format!("`{other}` is not a screen\n\n{}", usage())),
    }
}

/// A size in pixels, as `WxH`.
fn resolution(value: &str) -> Result<UVec2, String> {
    let (w, h) = pair(
        value,
        'x',
        |s| s.parse::<u32>().ok().filter(|v| *v > 0),
        "a size in pixels, e.g. 2560x1440",
    )?;
    Ok(UVec2::new(w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(line: &str) -> Result<Args, String> {
        parse(line.split_whitespace().map(String::from).collect())
    }

    fn ok(line: &str) -> Args {
        parse_args(line).expect("should parse")
    }

    #[test]
    fn defaults_play_the_game_from_the_main_menu() {
        let args = ok("");
        assert_eq!(args.state, AppState::MainMenu);
        assert_eq!(args.debug, None);
        assert!(!args.headless);
        assert!(!args.seed_given);
    }

    #[test]
    fn sets_up_the_world() {
        let args = ok("--state inworld --seed 7");
        assert_eq!(args.state, AppState::InWorld);
        assert_eq!(args.config.seed, 7);
        assert!(args.seed_given);
    }

    #[test]
    fn opens_on_any_of_the_screens_by_name() {
        assert_eq!(ok("--state mainmenu").state, AppState::MainMenu);
        assert_eq!(ok("--state newworld").state, AppState::NewWorld);
        assert_eq!(ok("--state options").state, AppState::Options);
        assert_eq!(ok("--state display").state, AppState::Display);
        assert_eq!(ok("--state controls").state, AppState::Controls);
        assert_eq!(ok("--state inworld").state, AppState::InWorld);
    }

    /// The screens inside a world are a world plus what the player is doing in
    /// it, so naming one has to set both — a pause menu with no world under it
    /// would be a picture of nothing.
    #[test]
    fn the_screens_over_a_world_open_with_the_world_under_them() {
        assert_eq!(ok("--state inworld").helm, Helm::Sailing);

        let paused = ok("--state paused");
        assert_eq!(paused.state, AppState::InWorld);
        assert_eq!(paused.helm, Helm::Paused);

        let controls = ok("--state pausedcontrols");
        assert_eq!(controls.state, AppState::InWorld);
        assert_eq!(controls.helm, Helm::Controls);

        let display = ok("--state pauseddisplay");
        assert_eq!(display.state, AppState::InWorld);
        assert_eq!(display.helm, Helm::Display);
    }

    /// Pausing is inside the served world, so it is one of the few screens a
    /// joined run may start on — unlike the menus, which it may not.
    #[test]
    fn a_joined_run_may_start_paused() {
        assert_eq!(ok("--state paused --join x").helm, Helm::Paused);
    }

    /// A spawn and a facing as a server would have named them: afloat, with
    /// land off to one side.
    #[test]
    fn the_view_opens_where_the_world_says_it_is_entered() {
        let mut args = ok("--seed 777 --state inworld");
        args.opened_on(Vec2::new(20.0, -40.0), Vec2::new(120.0, -40.0));
        let view = args.starting_view();
        assert_eq!(view.focus, Vec3::new(20.0, 0.0, -40.0));
        assert_eq!(
            view.yaw.to_degrees().round(),
            -90.0,
            "turned to face the land the welcome named"
        );
    }

    #[test]
    fn joining_enters_the_served_world() {
        let args = ok("--join example.com:4000");
        assert_eq!(args.join.as_deref(), Some("example.com:4000"));
        assert_eq!(args.state, AppState::InWorld);
    }

    #[test]
    fn a_joined_run_leaves_the_seed_to_the_server() {
        assert!(
            parse_args("--join x --seed 7").is_err(),
            "a joined run is in a world it did not make"
        );
        assert!(
            parse_args("--state mainmenu --join x").is_err(),
            "and cannot start on a menu over the top of it"
        );
    }

    /// A debugged run takes a port and stays up to be driven.
    #[test]
    fn reads_a_debug_port() {
        assert_eq!(ok("--debug 7777").debug, Some(7777));
        assert!(!ok("--debug 7777").headless);
        assert!(ok("--debug 7777 --headless").headless);
        assert_eq!(ok("--seed 7").debug, None);
    }

    /// And it opens in a world, the way a joined run does: there is nothing to
    /// drive on a menu screen, and saying so every time would be a toll on the
    /// common case.
    #[test]
    fn a_debugged_run_opens_in_a_world() {
        assert_eq!(ok("--debug 7777").state, AppState::InWorld);
        assert_eq!(
            ok("--debug 7777 --state mainmenu").state,
            AppState::MainMenu,
            "a driver that asked for a menu gets one"
        );
    }

    /// Windowless has to be asked for, and only means something with a socket
    /// to be driven down.
    #[test]
    fn windowless_needs_something_to_drive_it() {
        assert!(ok("--debug 7777 --headless").is_headless());
        assert!(
            !ok("--debug 7777").is_headless(),
            "a window to watch it being driven in"
        );
        assert!(!ok("--seed 7").is_headless());
        assert!(parse_args("--headless").is_err());
    }

    /// The view, the hour and the pictures are the socket's now, and an option
    /// that quietly did nothing would be worse than one that is refused.
    #[test]
    fn the_options_the_socket_replaced_are_gone() {
        for line in [
            "--shot a.png",
            "--focus 98,-317",
            "--zoom 120",
            "--yaw 45",
            "--time 6",
        ] {
            assert!(
                parse_args(line).is_err(),
                "`{line}` should be refused — it is said down the socket now"
            );
        }
    }

    #[test]
    fn rejects_what_it_cannot_make_sense_of() {
        assert!(parse_args("--nonsense 1").is_err());
        assert!(parse_args("--seed").is_err(), "a value is required");
        assert!(parse_args("--seed twelve").is_err());
        assert!(parse_args("--state elsewhere").is_err());
        assert!(parse_args("--debug eleven").is_err());
        assert!(parse_args("--resolution 2560").is_err());
        assert!(parse_args("--resolution 0x1440").is_err());
    }
}
