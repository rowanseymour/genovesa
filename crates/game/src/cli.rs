//! The game binary's command line.
//!
//! `game` is both the thing you play and the thing that renders pictures of
//! the terrain, so its options cover how a match is set up, where the camera
//! starts, and a list of shots to take before quitting.
//!
//! View options and `--shot` are read strictly left to right: each `--shot`
//! captures the view as the options before it have left it, and the ones after
//! it adjust the view for the shots that follow. So a sweep is a plain
//! sequence, with no nesting and nothing to quote:
//!
//! ```sh
//! cargo run --bin game -- --seed 7 --focus 98,-317 --yaw 45 \
//!   --zoom 120 --shot near.png \
//!   --zoom 340 --shot far.png \
//!   --yaw 225 --shot behind.png
//! ```
//!
//! One process, one world, three pictures.

use bevy::math::{UVec2, Vec2, Vec3};
use protocol::DEFAULT_PORT;
use world::args::{metres, pair};

use crate::camera::{View, MAX_DISTANCE, MIN_DISTANCE};
use crate::terrain::{random_seed, Archipelago, WorldConfig};
use crate::AppState;

/// Size of a captured picture, in pixels. Matches the shots already in
/// `screenshots/`, which came off a 1280x720 window on a doubled display.
const DEFAULT_RESOLUTION: UVec2 = UVec2::new(2560, 1440);

/// What the command line asked for.
pub struct Args {
    pub state: AppState,
    pub config: WorldConfig,
    /// Server to join, as `host` or `host:port`. A joined run takes the
    /// world's seed — and, unless `--focus` says otherwise, where to look —
    /// from the server's welcome rather than from this command line.
    pub join: Option<String>,
    /// Overlay frame rate and geometry counts on the window.
    pub debug: bool,
    /// Where the camera starts. With shots to take this is where the last of
    /// them left it, which nobody sees, since capturing quits at the end.
    pub view: View,
    /// Whether `--focus` was given — what lets [`Args::centre_on`] tell
    /// "nobody chose" from "somebody chose the origin".
    focus_given: bool,
    /// Whether `--yaw` was given, so [`Args::enter`] only turns a view
    /// nobody aimed.
    yaw_given: bool,
    /// Whether `--seed` was given. A run that did not name one is in a world
    /// picked off the clock, which is worth saying out loud: otherwise a
    /// picture worth keeping could never be taken twice.
    pub seed_given: bool,
    /// Pictures to take, in order. Empty means play the game.
    pub shots: Vec<Shot>,
    /// Size of each captured picture. Ignored when there are no shots — a
    /// window that is played in gets its size from [`crate::WINDOW`].
    pub resolution: UVec2,
}

/// One picture to take, and the view to take it from.
pub struct Shot {
    pub path: String,
    pub view: View,
}

impl Args {
    /// Whether this run exists to write pictures rather than to be played. A
    /// capturing run needs no window and quits on its own.
    pub fn is_capture(&self) -> bool {
        !self.shots.is_empty()
    }

    /// Where the camera opens on. Capturing starts on the first shot, so that
    /// the frames spent warming up draw the world about to be photographed.
    pub fn starting_view(&self) -> View {
        self.shots.first().map_or(self.view, |shot| shot.view)
    }

    /// Points the whole run — the starting view and every shot — at a ground
    /// point, unless `--focus` already chose one. Parsing uses it to point a
    /// capture run at the island nearest the origin; [`Args::enter`] rides
    /// on it for everything else.
    pub fn centre_on(&mut self, centre: Vec2) {
        if self.focus_given {
            return;
        }
        let focus = Vec3::new(centre.x, 0.0, centre.y);
        self.view.focus = focus;
        for shot in &mut self.shots {
            shot.view.focus = focus;
        }
    }

    /// Opens the run on where a world is entered — what [`View::enter`] does
    /// to one view, done to the starting view and every shot alike, and read
    /// the same way: `point` is the spawn a server already named, and `None`
    /// takes the world's own.
    ///
    /// Each half yields to the command line: a `--focus` keeps the whole view
    /// where it was put, and a `--yaw` keeps its own bearing.
    pub fn enter(&mut self, world: &Archipelago, point: Option<Vec2>) {
        if self.focus_given {
            return;
        }
        let spawn = world.spawn();
        self.centre_on(point.or(spawn.map(|s| s.point)).unwrap_or(Vec2::ZERO));
        if self.yaw_given {
            return;
        }
        if let Some(island) = spawn.map(|s| s.island.centre()) {
            self.view.face(island);
            for shot in &mut self.shots {
                shot.view.face(island);
            }
        }
    }
}

/// Built rather than written out so the defaults it quotes are read from the
/// code itself and cannot drift.
fn usage() -> String {
    let view = View::default();
    format!(
        "\
Genovesa — an endless ocean of generated islands to look around.

Usage: game [options]

Options:
  --state <screen>  start on `mainmenu`, `newworld`, `joinworld`, `settings`
                    or `inworld` [default: mainmenu, or inworld when shots or
                    a server are asked for]
  --seed <n>        the world to generate [default: a new one every run, and
                    the run says which so it can be asked for again]
  --join <host[:port]>  play in a served world instead of a local one; the
                    server provides the seed and where the world is entered,
                    and the run starts in that world rather than on a screen
                    [port: {DEFAULT_PORT}]

A world started from the menu can be shared instead of kept, which hosts it on
port {DEFAULT_PORT} for others to `--join` — the same session a dedicated
`server` serves, run alongside the game that started it.
  --debug           overlay frame rate, geometry counts and the current view
                    on the window; ignored when capturing, so shots stay clean

View options, applied in the order given:
  --focus <x,z>     world point to put the player down at and centre the view
                    on, in metres [default: where the world is entered — open
                    water just off its first island; shots default to the
                    island nearest the origin]
  --zoom <m>        camera distance in metres, {MIN_DISTANCE} to {MAX_DISTANCE}
                    [default: {}]
  --yaw <deg>       bearing to look from [default: facing the island the
                    entry stands off, or {} degrees wherever --focus points]

Capture options:
  --shot <path>     write a PNG of the view as the options so far have left
                    it; may be given many times, and quits after the last
  --resolution <WxH>  size of each shot in pixels [default: {}x{}]

Capturing needs no window: the shots are rendered off screen, so a run can
take its pictures without stealing the display.
",
        view.distance,
        view.yaw.to_degrees(),
        DEFAULT_RESOLUTION.x,
        DEFAULT_RESOLUTION.y,
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
        config: WorldConfig {
            seed: random_seed(),
        },
        join: None,
        debug: false,
        view: View::default(),
        focus_given: false,
        yaw_given: false,
        seed_given: false,
        shots: Vec::new(),
        resolution: DEFAULT_RESOLUTION,
    };
    let mut state_given = false;

    // `--debug` is the one flag that stands on its own; every other option
    // takes a value, so past it an option in the last position is always a
    // missing value.
    let mut rest = argv.iter();
    while let Some(flag) = rest.next() {
        if flag == "--debug" {
            args.debug = true;
            continue;
        }
        let value = rest
            .next()
            .ok_or_else(|| format!("`{flag}` needs a value"))?;
        match flag.as_str() {
            "--state" => {
                args.state = state(value)?;
                state_given = true;
            }
            "--seed" => {
                args.config.seed = value
                    .parse()
                    .map_err(|_| format!("`{value}` is not a seed"))?;
                args.seed_given = true;
            }
            "--join" => args.join = Some(value.clone()),
            "--focus" => {
                args.view.focus = focus(value)?;
                args.focus_given = true;
            }
            "--zoom" => args.view.distance = zoom(value)?,
            "--yaw" => {
                args.view.yaw = yaw(value)?;
                args.yaw_given = true;
            }
            "--resolution" => args.resolution = resolution(value)?,
            // Takes a copy of the view as it stands, which is what makes the
            // options before a shot its own and the ones after it the next
            // shot's.
            "--shot" => args.shots.push(Shot {
                path: value.clone(),
                view: args.view,
            }),
            other => return Err(format!("unknown option `{other}`\n\n{}", usage())),
        }
    }

    // A joined world is the server's world, whole: a seed given alongside
    // would either be ignored or generate a different ocean, and both are
    // worse than saying so.
    if args.join.is_some() && args.seed_given {
        return Err(
            "`--seed` picks a world to generate, but a joined world is the server's — \
             its seed arrives with the welcome"
                .into(),
        );
    }

    // And a joined session is only a session in the served world. Starting on
    // any other screen leaves it running behind a menu whose "new world"
    // builds a *local* one — the run would go on reporting its position into
    // an ocean it is no longer standing in, and draw the other players' markers
    // on ground that isn't theirs.
    if args.join.is_some() && state_given && args.state != AppState::InWorld {
        return Err(
            "`--join` plays in the served world, so a joined run cannot start on another screen"
                .into(),
        );
    }

    // Pictures of the menu are a fair thing to want, but a shot nearly always
    // means a shot of the map — and joining a server means playing in its
    // world — so save every caller from saying so.
    if (args.is_capture() || args.join.is_some()) && !state_given {
        args.state = AppState::InWorld;
    }

    // A run that said nothing about where to look opens where its world is
    // entered: the spawn point just off the first island, facing it. A
    // *capture* run is different — a shot nearly always means a shot of
    // terrain, and photographing an island means centring on its middle
    // rather than floating off its coast — so shots go to the nearest land.
    //
    // Once, and for the whole command line, rather than per shot: the shots
    // are a sweep over one world, and moving each of them to its own nearest
    // island would break a sequence that says "here, then a bit further" into
    // an unrelated set of pictures. The world built here is thrown away and
    // the match builds its own — cheap for a capture run, which only asks
    // the layout; a played run's spawn generates its entry island, a one-off
    // cost the match was about to pay for the same island anyway.
    //
    // A joined run skips this entirely: its world is the server's, so a
    // focus from a locally laid-out ocean would point at the wrong one. The
    // game binary calls `enter` with the served spawn point instead.
    if args.join.is_none() {
        let world = Archipelago::new(&args.config);
        if args.is_capture() {
            if let Some(centre) = world.nearest_island(Vec2::ZERO).map(|spec| spec.centre()) {
                args.centre_on(centre);
            }
        } else {
            args.enter(&world, None);
        }
    }
    Ok(args)
}

fn state(value: &str) -> Result<AppState, String> {
    match value {
        "mainmenu" => Ok(AppState::MainMenu),
        "newworld" => Ok(AppState::NewWorld),
        "joinworld" => Ok(AppState::JoinWorld),
        "settings" => Ok(AppState::Settings),
        "inworld" => Ok(AppState::InWorld),
        other => Err(format!(
            "`{other}` is not a screen — try mainmenu, newworld, joinworld, settings or inworld"
        )),
    }
}

/// Reads an `x,z` pair of metres. The height is left at zero: the camera puts
/// itself down on the ground on its first frame.
fn focus(value: &str) -> Result<Vec3, String> {
    let (x, z) = pair(value, ',', metres, "an x,z point in metres, e.g. 98,-317")?;
    Ok(Vec3::new(x, 0.0, z))
}

fn zoom(value: &str) -> Result<f32, String> {
    let distance: f32 = value
        .parse()
        .map_err(|_| format!("`{value}` is not a distance in metres"))?;
    if !(MIN_DISTANCE..=MAX_DISTANCE).contains(&distance) {
        return Err(format!(
            "--zoom must be between {MIN_DISTANCE} and {MAX_DISTANCE} metres, but was {distance}"
        ));
    }
    Ok(distance)
}

fn yaw(value: &str) -> Result<f32, String> {
    let degrees: f32 = value
        .parse()
        .map_err(|_| format!("`{value}` is not a bearing in degrees"))?;
    if !degrees.is_finite() {
        return Err(format!("`{value}` is not a bearing in degrees"));
    }
    Ok(degrees.to_radians())
}

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
        parse(line.split_whitespace().map(str::to_string).collect())
    }

    fn ok(line: &str) -> Args {
        parse_args(line).expect("should parse")
    }

    #[test]
    fn defaults_play_the_game_from_the_main_menu() {
        let args = ok("");
        assert_eq!(args.state, AppState::MainMenu);
        assert!(!args.is_capture());

        // And in a world nobody chose, which is a new one each run rather
        // than one seed forever.
        assert!(!args.seed_given);
        assert_ne!(args.config.seed, ok("").config.seed);
    }

    #[test]
    fn sets_up_the_world_and_the_view() {
        let args = ok("--state inworld --seed 7 --focus 98,-317 --zoom 150 --yaw 90");
        assert_eq!(args.state, AppState::InWorld);
        assert_eq!(args.config.seed, 7);
        assert_eq!(args.view.focus, Vec3::new(98.0, 0.0, -317.0));
        assert_eq!(args.view.distance, 150.0);
        assert_eq!(args.view.yaw, std::f32::consts::FRAC_PI_2);
    }

    #[test]
    fn opens_on_any_of_the_screens_by_name() {
        assert_eq!(ok("--state mainmenu").state, AppState::MainMenu);
        assert_eq!(ok("--state newworld").state, AppState::NewWorld);
        assert_eq!(ok("--state settings").state, AppState::Settings);
        assert_eq!(ok("--state inworld").state, AppState::InWorld);
    }

    #[test]
    fn with_no_focus_given_a_played_run_opens_on_the_spawn() {
        // A run that said nothing about where to look starts the view, and
        // with it the boat, exactly where the world is entered — the spawn
        // point off the first island — with the bow aimed at the island, so
        // holding forward is the whole of the first sail.
        let args = ok("--seed 777 --state inworld");
        let spawn = Archipelago::new(&args.config)
            .spawn()
            .expect("seed 777 should offer somewhere to enter");

        assert_eq!(
            args.view.focus,
            Vec3::new(spawn.point.x, 0.0, spawn.point.y)
        );

        // The yaw convention the boat pins: forward is (-sin, -cos).
        let ahead = Vec2::new(-args.view.yaw.sin(), -args.view.yaw.cos());
        let towards = (spawn.island.centre() - spawn.point).normalize();
        assert!(
            ahead.dot(towards) > 0.999,
            "the view opens looking {ahead}, not at the island {towards}"
        );
    }

    #[test]
    fn an_explicit_yaw_keeps_its_bearing_over_the_spawn_facing() {
        let args = ok("--seed 777 --state inworld --yaw 90");
        assert_eq!(args.view.yaw, std::f32::consts::FRAC_PI_2);
    }

    #[test]
    fn with_no_focus_given_shots_are_taken_of_the_nearest_island() {
        // The origin is guaranteed open water, so a capture run that said
        // nothing about where to look has to be moved onto land — both the
        // starting view and every shot, so a sweep stays a sweep.
        let args = ok("--seed 777 --shot a.png --zoom 300 --shot b.png");
        let world = Archipelago::new(&args.config);
        let island = world
            .nearest_island(Vec2::ZERO)
            .expect("seed 777 should have an island near the origin");

        assert_ne!(args.view.focus, Vec3::ZERO, "the view still opens on water");
        for view in [args.view, args.shots[0].view, args.shots[1].view] {
            let focus = Vec2::new(view.focus.x, view.focus.z);
            let out = (focus - island.centre()).abs() - island.extent() * 0.5;
            assert!(
                out.max_element() <= 0.0,
                "{focus} is outside the nearest island's frame"
            );
        }
        // And only the focus moved — the shots keep their own zooms.
        assert_eq!(args.shots[0].view.distance, View::default().distance);
        assert_eq!(args.shots[1].view.distance, 300.0);
    }

    #[test]
    fn an_explicit_focus_is_honoured_wherever_it_points() {
        // Including at the origin, which is what the snap above would
        // otherwise have moved: somebody asking for open water is entitled to
        // a picture of open water.
        assert_eq!(ok("--focus 0,0").view.focus, Vec3::ZERO);
        assert_eq!(
            ok("--focus 98,-317 --shot a.png").shots[0].view.focus,
            Vec3::new(98.0, 0.0, -317.0)
        );
        // A focus given after a shot still counts as one being given, so the
        // shot before it keeps the default view rather than a snapped one.
        assert_eq!(
            ok("--shot a.png --focus 10,20").shots[0].view.focus,
            Vec3::ZERO
        );
    }

    #[test]
    fn a_shot_takes_the_view_as_the_options_before_it_left_it() {
        let args = ok("--focus 10,20 --zoom 100 --shot near.png --zoom 300 --shot far.png");
        let paths: Vec<&str> = args.shots.iter().map(|s| s.path.as_str()).collect();
        assert_eq!(paths, ["near.png", "far.png"]);
        assert_eq!(args.shots[0].view.distance, 100.0);
        assert_eq!(args.shots[1].view.distance, 300.0);
        // Everything not overridden carries on to the shots that follow.
        assert_eq!(args.shots[1].view.focus, Vec3::new(10.0, 0.0, 20.0));
    }

    #[test]
    fn options_after_the_last_shot_do_not_reach_it() {
        let args = ok("--zoom 100 --shot near.png --zoom 300");
        assert_eq!(args.shots[0].view.distance, 100.0);
    }

    #[test]
    fn shots_imply_a_match_unless_a_screen_was_named() {
        assert_eq!(ok("--shot a.png").state, AppState::InWorld);
        assert_eq!(
            ok("--state mainmenu --shot a.png").state,
            AppState::MainMenu
        );
    }

    #[test]
    fn joining_enters_the_served_world() {
        let args = ok("--join example.com:4000");
        assert_eq!(args.join.as_deref(), Some("example.com:4000"));
        assert_eq!(args.state, AppState::InWorld);

        // Saying so as well is fine; saying anything else is not, since a
        // joined session has nowhere but the served world to be.
        assert_eq!(ok("--state inworld --join x").state, AppState::InWorld);
        assert!(
            parse_args("--state mainmenu --join x").is_err(),
            "a joined run cannot start behind a menu"
        );
    }

    #[test]
    fn a_joined_run_leaves_the_seed_and_the_view_to_the_server() {
        assert!(
            parse_args("--join x --seed 7").is_err(),
            "a joined world cannot also be a chosen one"
        );

        // No island snap either: the layout it would come from is the wrong
        // world's. The binary centres on the served spawn instead, and that
        // reaches the shots exactly as the snap would have.
        let mut args = ok("--join x --shot a.png");
        assert_eq!(args.view.focus, Vec3::ZERO);
        args.centre_on(Vec2::new(10.0, 20.0));
        assert_eq!(args.view.focus, Vec3::new(10.0, 0.0, 20.0));
        assert_eq!(args.shots[0].view.focus, Vec3::new(10.0, 0.0, 20.0));
    }

    #[test]
    fn an_explicit_focus_outranks_the_served_spawn() {
        // Both halves of the opening: the view stays where `--focus` put it,
        // and stays looking the way the default looks — facing an island
        // from a spot the player chose would be facing it from the wrong
        // place.
        let mut args = ok("--join x --focus 5,6");
        let world = Archipelago::new(&args.config);
        args.enter(&world, Some(Vec2::new(10.0, 20.0)));
        assert_eq!(args.view.focus, Vec3::new(5.0, 0.0, 6.0));
        assert_eq!(args.view.yaw, View::default().yaw);
    }

    #[test]
    fn debug_is_off_unless_asked_for_and_takes_no_value() {
        assert!(!ok("").debug);

        // Standing between two valued options, so a parse that gave it a
        // value would swallow `--seed`.
        let args = ok("--zoom 150 --debug --seed 7");
        assert!(args.debug);
        assert_eq!(args.config.seed, 7);
    }

    #[test]
    fn rejects_what_it_cannot_make_sense_of() {
        assert!(parse_args("--nonsense 1").is_err());
        assert!(parse_args("--zoom").is_err(), "a value is required");
        assert!(parse_args("--zoom wide").is_err());
        assert!(
            parse_args("--zoom 5").is_err(),
            "closer than the camera goes"
        );
        assert!(parse_args("--zoom 5000").is_err(), "further than it goes");
        assert!(parse_args("--focus 98").is_err(), "needs both axes");
        assert!(parse_args("--focus north,south").is_err());
        assert!(parse_args("--yaw sideways").is_err());
        assert!(parse_args("--state elsewhere").is_err());
        assert!(parse_args("--resolution 2560").is_err());
        assert!(parse_args("--resolution 0x1440").is_err());
    }
}
