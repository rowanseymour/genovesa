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

use bevy::math::{UVec2, Vec3};

use crate::camera::{View, MAX_DISTANCE, MIN_DISTANCE};
use crate::terrain::WorldConfig;
use crate::AppState;

/// Size of a captured picture, in pixels. Matches the shots already in
/// `screenshots/`, which came off a 1280x720 window on a doubled display.
const DEFAULT_RESOLUTION: UVec2 = UVec2::new(2560, 1440);

/// What the command line asked for.
pub struct Args {
    pub state: AppState,
    pub config: WorldConfig,
    /// Where the camera starts. With shots to take this is where the last of
    /// them left it, which nobody sees, since capturing quits at the end.
    pub view: View,
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
}

/// Built rather than written out so the defaults it quotes are read from the
/// code itself and cannot drift.
fn usage() -> String {
    let world = WorldConfig::default();
    let view = View::default();
    format!(
        "\
Genovesa — an endless ocean of generated islands to look around.

Usage: game [options]

Options:
  --state <screen>  start on `mainmenu`, `newworld`, `settings` or `inworld`
                    [default: mainmenu, or inworld when shots are asked for]
  --seed <n>        the world to generate [default: {}]

View options, applied in the order given:
  --focus <x,z>     world point to look at, in metres [default: {},{}]
  --zoom <m>        camera distance in metres, {MIN_DISTANCE} to {MAX_DISTANCE}
                    [default: {}]
  --yaw <deg>       bearing to look from [default: {}]

Capture options:
  --shot <path>     write a PNG of the view as the options so far have left
                    it; may be given many times, and quits after the last
  --resolution <WxH>  size of each shot in pixels [default: {}x{}]

Capturing needs no window: the shots are rendered off screen, so a run can
take its pictures without stealing the display.
",
        world.seed,
        view.focus.x,
        view.focus.z,
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

    let mut args = Args {
        state: AppState::MainMenu,
        config: WorldConfig::default(),
        view: View::default(),
        shots: Vec::new(),
        resolution: DEFAULT_RESOLUTION,
    };
    let mut state_given = false;

    // Every option here takes a value, so an option in the last position is
    // always a missing value rather than a flag that stands on its own.
    let mut rest = argv.iter();
    while let Some(flag) = rest.next() {
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
            }
            "--focus" => args.view.focus = focus(value)?,
            "--zoom" => args.view.distance = zoom(value)?,
            "--yaw" => args.view.yaw = yaw(value)?,
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

    // Pictures of the menu are a fair thing to want, but a shot nearly always
    // means a shot of the map, so save every caller from saying so.
    if args.is_capture() && !state_given {
        args.state = AppState::InWorld;
    }
    Ok(args)
}

fn state(value: &str) -> Result<AppState, String> {
    match value {
        "mainmenu" => Ok(AppState::MainMenu),
        "newworld" => Ok(AppState::NewWorld),
        "settings" => Ok(AppState::Settings),
        "inworld" => Ok(AppState::InWorld),
        other => Err(format!(
            "`{other}` is not a screen — try mainmenu, newworld, settings or inworld"
        )),
    }
}

/// Reads an `x,z` pair of metres. The height is left at zero: the camera puts
/// itself down on the ground on its first frame.
fn focus(value: &str) -> Result<Vec3, String> {
    let bad = || format!("`{value}` is not an x,z point in metres, e.g. 98,-317");
    let (x, z) = value.split_once(',').ok_or_else(bad)?;
    let axis = |s: &str| s.trim().parse::<f32>().ok().filter(|v| v.is_finite());
    Ok(Vec3::new(
        axis(x).ok_or_else(bad)?,
        0.0,
        axis(z).ok_or_else(bad)?,
    ))
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
    let bad = || format!("`{value}` is not a size in pixels, e.g. 2560x1440");
    let (w, h) = value.split_once('x').ok_or_else(bad)?;
    let axis = |s: &str| s.trim().parse::<u32>().ok().filter(|v| *v > 0);
    Ok(UVec2::new(
        axis(w).ok_or_else(bad)?,
        axis(h).ok_or_else(bad)?,
    ))
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
        assert_eq!(args.config.seed, WorldConfig::default().seed);
        assert!(!args.is_capture());
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
