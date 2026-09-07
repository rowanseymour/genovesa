//! The game binary's command line.
//!
//! Deliberately short. What a run needs to be *started* with is here — which
//! world, whether to join somebody else's — and everything about how a run
//! behaves once it is going is said down the debug socket instead, in
//! [`crate::control`]'s grammar.
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
//! The screens went the same way, and last. They were thirteen names for
//! *opening on* a screen, which is not the same as reaching one: opening on
//! the controls screen teleports past the code that opens it, so it could
//! show that screen but never a row of it armed and waiting for a key, nor
//! any other state a screen can only be walked into. The socket's `click`
//! walks in, so the names bought a shortcut past the interesting half and
//! nothing else.
//!
//! What is left is what a socket cannot say, because it is settled before
//! there is a game to say it to: the world, whether there is a window at
//! all, and the port the socket itself listens on. Which screen a run opens
//! on is not among them — it follows from whether a world was asked for.

use bevy::math::{Vec2, Vec3};
use protocol::DEFAULT_PORT;

use crate::camera::View;
use crate::AppState;
use server::random_seed;

/// What the command line asked for.
pub struct Args {
    /// Where the run opens, which is not chosen but *followed*: a run that
    /// named a world to be in opens in it, and a run that named none opens on
    /// the main menu with a socket — if it has one — able to click its way
    /// wherever it likes.
    pub state: AppState,
    /// The world to open. A seed is a world; what a generator makes of one
    /// is behind the server, not here.
    pub seed: u32,
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
    /// Where the camera opens. Not settable here any more — it is whatever the
    /// welcome says, and moving it afterwards is the console's `goto` — the
    /// camera going with whatever carries the player — and the socket's
    /// `zoom` and `yaw`.
    view: View,
}

impl Args {
    /// Whether this run opens a window at all. Only one way not to, and it has
    /// to be asked for.
    ///
    /// What hangs off it is everything a window would have been for — the
    /// display settings, the audio device, and the frame loop, which winit
    /// drives for a window and nothing drives without one.
    ///
    /// The flag alone, without a second look at `--debug`: [`parse`] refuses
    /// windowless without a socket outright, so a run that gets this far and
    /// says it is windowless has one. Asking again here would be a second
    /// place the rule lived, and the two could disagree.
    pub fn is_headless(&self) -> bool {
        self.headless
    }

    /// Where the camera opens on.
    pub fn starting_view(&self) -> View {
        self.view
    }

    /// Points the run at the world a server has just described: where it put
    /// this player down, and the land it said to look at.
    ///
    /// The entry is open sea with the nearest land a few hundred metres off,
    /// so nothing is on the opening screen but the boat. An island's portrait
    /// — somewhere else, from some other bearing — is the socket's business,
    /// and it can be taken at any point rather than only at the start.
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
  --seed <n>        a world to open, and the run opens in it rather than on
                    the menu [default: no world — the menu, where one is
                    chosen; a world started there says which seed it got, so
                    it can be asked for again]
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
                    and a blank line. Everything the console takes: `client`
                    for this machine's own switches, anything else for the
                    server.
                    Plus the words a keyboard never needed — `shot`, `press`,
                    `click`, `hold` and `quit`. Send `help` for the whole
                    vocabulary
  --headless        no window: draw off screen and be driven down the socket
                    alone

A debugged run opens where any other run would. There used to be a `--state`
for the screen to start on, and `click` replaced it: a screen opened outright
is a screen with none of its history, and the socket can walk to any of them
— through the transitions, which is where the interesting states are.

The view, the weather, the clock, the controls and the size of the pictures are
reached down the socket and nowhere else — an option could only ever say them
once, and before anything existed:

  game --seed 7 --debug 7777 --headless
  printf 'client resolution 1080\\ngoto 98 -317\\nshot near.png\\nquit\\n' | nc 127.0.0.1 7777
"
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
        // Settled at the end, out of what was asked for.
        state: AppState::MainMenu,
        seed: random_seed(),
        join: None,
        seed_given: false,
        debug: None,
        headless: false,
        view: View::default(),
    };

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
            "--seed" => {
                args.seed = value
                    .parse()
                    .map_err(|_| format!("`{value}` is not a seed"))?;
                args.seed_given = true;
            }
            "--join" => args.join = Some(value.clone()),
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

    // A run that named a world opens in it, and a run that named none opens on
    // the menu. That is the whole rule, and it is a rule rather than an option
    // because the two halves of it were never really separable: a `--seed`
    // that left the run on a menu was a world nobody opened, which is what it
    // used to be without `--state inworld` beside it.
    //
    // A debug socket is not part of it. It used to force a world, on the
    // grounds that there was nothing to drive on a menu; there is now — see
    // [`crate::control`]'s `click` — so a debugged run opens where any other
    // run would and clicks its way in if it wants a world.
    if args.join.is_some() || args.seed_given {
        args.state = AppState::InWorld;
    }

    // Where to look is not settled here, because nothing on this side knows
    // where anything is: the world comes from a server, and the server is the
    // one that can say where it is entered. See [`Args::opened_on`], which the
    // binary calls once a welcome has arrived.
    Ok(args)
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

    /// A seed is a world to open, and opening it is what naming it means.
    /// It used to need `--state inworld` beside it, without which the run sat
    /// on the menu and the seed did nothing at all.
    #[test]
    fn a_named_world_is_a_world_the_run_opens_in() {
        let args = ok("--seed 7");
        assert_eq!(args.state, AppState::InWorld);
        assert_eq!(args.seed, 7);
        assert!(args.seed_given);
    }

    /// And a run that named none opens on the menu, socket or no socket. The
    /// socket used to force a world on the grounds that a menu had nothing to
    /// drive; `click` drives one.
    #[test]
    fn a_run_that_named_no_world_opens_on_the_menu() {
        assert_eq!(ok("--debug 7777").state, AppState::MainMenu);
        assert_eq!(ok("--debug 7777 --headless").state, AppState::MainMenu);
        assert_eq!(ok("--seed 7 --debug 7777").state, AppState::InWorld);
    }

    /// The screens are the socket's to walk to now, and an option that
    /// quietly did nothing would be worse than one that is refused.
    #[test]
    fn the_screens_are_not_asked_for_here() {
        for line in ["--state mainmenu", "--state inworld", "--state paused"] {
            assert!(
                parse_args(line).is_err(),
                "`{line}` should be refused — `click` walks there now"
            );
        }
    }

    /// A spawn and a facing as a server would have named them: afloat, with
    /// land off to one side.
    #[test]
    fn the_view_opens_where_the_world_says_it_is_entered() {
        let mut args = ok("--seed 777");
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
    }

    /// A debugged run takes a port and stays up to be driven.
    #[test]
    fn reads_a_debug_port() {
        assert_eq!(ok("--debug 7777").debug, Some(7777));
        assert!(!ok("--debug 7777").headless);
        assert!(ok("--debug 7777 --headless").headless);
        assert_eq!(ok("--seed 7").debug, None);
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
            "--resolution 2560x1440",
        ] {
            assert!(
                parse_args(line).is_err(),
                "`{line}` should be refused — it is said down the socket now"
            );
        }
    }

    /// The `--debug` paragraph names the socket's own words, and a sentence is
    /// the one thing [`crate::control`]'s table cannot generate — so this is
    /// what holds it. It had already drifted: the list went on offering `zoom`
    /// and `yaw` for a while after the view became `client zoom` and `client
    /// yaw`, which is a run refused at the socket by the option that told you
    /// to try it.
    ///
    /// `client` and `help` are left out of the comparison because the prose
    /// names them itself, on either side of the list.
    #[test]
    fn the_usage_names_the_words_the_socket_serves() {
        let listed: Vec<String> = usage()
            .split_once("a keyboard never needed — ")
            .expect("the usage should say what the socket adds")
            .1
            .split_once('.')
            .expect("and stop at the end of the sentence")
            .0
            .split('`')
            .skip(1)
            .step_by(2)
            .map(String::from)
            .collect();
        let served: Vec<String> = crate::control::verbs()
            .filter(|word| !matches!(*word, "client" | "help"))
            .map(String::from)
            .collect();
        assert_eq!(listed, served);
    }

    #[test]
    fn rejects_what_it_cannot_make_sense_of() {
        assert!(parse_args("--nonsense 1").is_err());
        assert!(parse_args("--seed").is_err(), "a value is required");
        assert!(parse_args("--seed twelve").is_err());
        assert!(parse_args("--state elsewhere").is_err());
        assert!(parse_args("--debug eleven").is_err());
    }
}
