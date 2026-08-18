//! What this machine draws with, and the file it is remembered in.
//!
//! Three things, and they are three because they are what a player on a slow
//! machine reaches for in the order they reach for them: fill the screen, draw
//! fewer pixels, stop casting shadows.
//!
//! **The resolution is the real one.** It is the size of the surface the frame
//! is drawn on — the window's, or the display's own mode in fullscreen — and
//! not a render scale laid over a full-size one. Bevy has a piece for that
//! ([`bevy::camera::MainPassResolutionOverride`]) and it is not this: it draws
//! the world's passes into a corner of a full-size target and leaves the blit
//! to the screen sampling the whole of it, so without an upscaler behind it —
//! the DLSS-shaped thing it was built to sit in front of — the picture comes
//! out in the top-left with rubbish around it. Asking the *window* for fewer
//! pixels needs nothing behind it and is what the setting says it is: at 720p
//! the machine draws 921,600 pixels, whatever else is going on.
//!
//! Which is why fullscreen and resolution are one question here rather than
//! two. A borderless fullscreen window is always the size of the desktop, so a
//! resolution asked for while fullscreen has to be a *display mode*, and the
//! monitor is the one that says which of those exist — see [`mode_for`]. A
//! display with nothing that size keeps its own, and the screen says so rather
//! than pretending.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy::window::{
    Monitor, MonitorSelection, PrimaryMonitor, PrimaryWindow, VideoMode, VideoModeSelection,
    WindowMode,
};

use crate::{AppState, Helm};

/// The format this build writes, named in the file's first line.
const FORMAT: u32 = 1;

/// What the display options are set to. Always present — the file is a memory
/// of this, not the other way round, so a machine with nowhere to keep one
/// still plays with every switch working.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub struct DisplaySettings {
    /// Whether the game fills the screen.
    pub fullscreen: bool,
    /// How many pixels it is drawn in — see [`Resolution`].
    pub resolution: Resolution,
    /// Whether the sun casts. The first thing worth turning off on a machine
    /// that cannot keep up, since the shadow pass draws the whole scene again
    /// once per cascade.
    pub shadows: bool,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            // A window, because a game that seized the whole screen the first
            // time it was run would be a game you had to know a key to get out
            // of before you had seen it.
            fullscreen: false,
            resolution: Resolution::Native,
            shadows: true,
        }
    }
}

/// How tall the picture is drawn, in rows of real pixels.
///
/// Height alone, because the width follows from the shape of the screen and
/// asking the player for both would be asking them to know their own aspect
/// ratio. The rungs are the ones a player already has names for.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Resolution {
    /// Whatever the window or the display is already at — full sharpness, and
    /// on a retina screen the most pixels anything here will ever draw.
    #[default]
    Native,
    /// A number of rows off [`LADDER`].
    Rows(u32),
}

/// The rungs, in the order the button cycles them: down from native, since
/// that is the direction somebody opening this screen is going.
const LADDER: [Resolution; 5] = [
    Resolution::Native,
    // 4K is a rung rather than a synonym for native, and on a retina panel it
    // is a long way below one: a 6400x3600 display draws twenty-three million
    // pixels a frame, and 2160p is a third of that.
    Resolution::Rows(2160),
    Resolution::Rows(1440),
    Resolution::Rows(1080),
    Resolution::Rows(720),
];

impl Resolution {
    /// What the button reads. A switch has to say which way it is set, not
    /// what pressing it would do.
    pub fn label(&self) -> String {
        match self {
            Self::Native => "Native".to_string(),
            Self::Rows(rows) => format!("{rows}p"),
        }
    }

    /// The next rung down, wrapping back to native off the bottom. A rung a
    /// file named and this build has stopped offering cycles to the top rather
    /// than nowhere.
    pub fn next(self) -> Self {
        let at = LADDER.iter().position(|rung| *rung == self);
        LADDER[at.map_or(0, |at| (at + 1) % LADDER.len())]
    }
}

/// How wide a picture `rows` tall is on a screen of `shape` — the aspect ratio
/// the display has, so a resolution is a *count* of pixels and never a change
/// of shape. Even, because a video mode is.
fn width_for(shape: UVec2, rows: u32) -> u32 {
    let wide = (rows as f32 * shape.x.max(1) as f32 / shape.y.max(1) as f32).round() as u32;
    wide.max(2) & !1
}

// ---------------------------------------------------------------------------
// Making it so
// ---------------------------------------------------------------------------

pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        // Read once, here, rather than in a startup system: the binary builds
        // its window out of these — see `window_plugin` — so they have to be
        // known before there is an app to run a system in.
        app.init_resource::<DisplaySettings>().add_systems(
            Update,
            dress_the_window.run_if(resource_exists::<DisplaySettings>),
        );
        // Written when the screen that changes them is left, which is one
        // write per visit rather than one per press of a cycling button. Both
        // ways to the screen — see [`crate::menu`].
        app.add_systems(OnExit(AppState::Display), keep_settings)
            .add_systems(OnExit(Helm::Display), keep_settings);
    }
}

/// Makes the window agree with the settings.
///
/// The *mode* is set the way the debug overlay's own apply system sets things:
/// to what it should be, every pass, rather than when a switch is thrown. It
/// has to be, because what it should be can change with nothing here changing
/// at all — a run comes up before winit has reported a single monitor, so a
/// resolution wanting an exclusive mode has to settle for borderless on frame
/// one and get what it asked for a frame or two later. Comparing before writing
/// is what keeps that from marking `Window` changed forever, which the backdrop
/// would answer by re-ruling its sheet forever.
///
/// The *size* is the opposite and deliberately so: written only when the
/// settings themselves change. Asserted every pass, it would undo the player
/// dragging the window's own corner, over and over, as fast as they could drag
/// it.
fn dress_the_window(
    settings: Res<DisplaySettings>,
    monitors: Query<(Entity, &Monitor), With<PrimaryMonitor>>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    // A capture run has no window at all, and asks for its size on the command
    // line instead — see [`crate::capture`].
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    let monitor = monitors.iter().next();

    let wanted = if settings.fullscreen {
        fullscreen_mode(settings.resolution, monitor)
    } else {
        WindowMode::Windowed
    };
    if window.mode != wanted {
        window.mode = wanted;
    }

    // In a window the resolution is the window's own size, which is the same
    // count of pixels by another route. Left alone while fullscreen, where the
    // mode above has already said how big the surface is and writing a size
    // over it would only fight winit.
    if settings.is_changed() && !settings.fullscreen {
        let shape = monitor.map_or(UVec2::new(16, 9), |(_, monitor)| monitor.physical_size());
        let size = match settings.resolution {
            Resolution::Native => crate::WINDOW,
            Resolution::Rows(rows) => UVec2::new(width_for(shape, rows), rows),
        };
        window
            .resolution
            .set_physical_resolution(size.x.max(1), size.y.max(1));
    }
}

/// How the window should first come up, for the binary that builds it before
/// there is an app at all.
///
/// Neither answer can be the final one. Winit reports no monitors until it has
/// a window, so the exclusive mode a resolution wants cannot be chosen yet and
/// borderless stands in for it; the shape of the screen is unknown for the same
/// reason, so a windowed size is guessed widescreen. [`dress_the_window`] puts
/// both right on the first frame that has a monitor to ask. What this buys is
/// the frame *before* that one: a run that came up windowed and went fullscreen
/// a moment later would flash the desktop at somebody who had already said what
/// they wanted.
pub fn opening(settings: &DisplaySettings) -> (WindowMode, UVec2) {
    let mode = if settings.fullscreen {
        // `Primary` rather than the `Current` everything after this uses, and
        // the difference is real: "current" means the monitor the window is
        // on, and a window being created is not on one yet. Asking anyway gets
        // "cannot find current monitor" out of winit and a guess for an answer.
        WindowMode::BorderlessFullscreen(MonitorSelection::Primary)
    } else {
        WindowMode::Windowed
    };
    let size = match settings.resolution {
        Resolution::Native => crate::WINDOW,
        Resolution::Rows(rows) => UVec2::new(width_for(UVec2::new(16, 9), rows), rows),
    };
    (mode, size)
}

/// The fullscreen this resolution asks for.
///
/// Borderless for native, which is the mode that behaves itself: it takes the
/// desktop as it finds it and gives it back the same way. A resolution below
/// native has to be an *exclusive* mode, because that is the only fullscreen
/// in which the display itself changes size — and a display with nothing that
/// tall keeps borderless rather than being forced into a shape it has not
/// offered.
fn fullscreen_mode(resolution: Resolution, monitor: Option<(Entity, &Monitor)>) -> WindowMode {
    let borderless = WindowMode::BorderlessFullscreen(MonitorSelection::Current);
    let Resolution::Rows(rows) = resolution else {
        return borderless;
    };
    let Some((entity, monitor)) = monitor else {
        return borderless;
    };
    match mode_for(monitor, rows) {
        // Named by the monitor it was read off rather than by `Current`, so
        // the mode and the screen it is being asked of are the same screen.
        Some(mode) => WindowMode::Fullscreen(
            MonitorSelection::Entity(entity),
            VideoModeSelection::Specific(mode),
        ),
        None => borderless,
    }
}

/// The best mode this monitor has at `rows` tall, or `None` where it has none.
///
/// Widest first, so a display offering both 4:3 and 16:9 at a height gives the
/// one that fills it; then fastest, since two modes of a size differ only in
/// refresh rate and nobody wants the slower one.
fn mode_for(monitor: &Monitor, rows: u32) -> Option<VideoMode> {
    monitor
        .video_modes
        .iter()
        .filter(|mode| mode.physical_size.y == rows)
        .max_by_key(|mode| (mode.physical_size.x, mode.refresh_rate_millihertz))
        .copied()
}

/// Whether a display can actually be put into this resolution — what the
/// screen reads out beside a rung it is offering but the monitor has never
/// heard of.
pub fn available(resolution: Resolution, monitor: Option<&Monitor>) -> bool {
    match resolution {
        Resolution::Native => true,
        Resolution::Rows(rows) => monitor.is_some_and(|monitor| mode_for(monitor, rows).is_some()),
    }
}

// ---------------------------------------------------------------------------
// Kept on file
// ---------------------------------------------------------------------------

/// Where the settings live on this machine, or `None` where there is nowhere
/// to keep them — then they last as long as the run does.
fn place() -> Option<PathBuf> {
    Some(server::data_dir()?.join("settings"))
}

/// What was set last time, or the defaults on a machine that has never been
/// played on.
///
/// Read before there is an app, because the binary builds its first window out
/// of the answer: a run that came up windowed and went fullscreen a frame later
/// would flash the desktop at somebody who had already said what they wanted.
pub fn load() -> DisplaySettings {
    let Some(path) = place() else {
        return DisplaySettings::default();
    };
    let Ok(text) = fs::read_to_string(&path) else {
        return DisplaySettings::default();
    };
    match parse(&text) {
        Ok(settings) => settings,
        Err(why) => {
            // Left where it is rather than written over, the way a logbook this
            // build cannot read is: the file is somebody's preferences, and
            // failing to understand it is no licence to throw it away. The run
            // plays on the defaults, and only a visit to the screen — which is
            // somebody saying what they want in so many words — replaces it.
            warn!("cannot read the display settings: {why}");
            DisplaySettings::default()
        }
    }
}

fn keep_settings(settings: Res<DisplaySettings>) {
    let Some(path) = place() else {
        return;
    };
    if let Err(error) = keep(&path, &settings) {
        warn!("the display settings could not be kept: {error}");
    }
}

/// Composed whole beside the file and renamed over it, like the logbook and
/// the server's world file: at no instant is the name pointing at half of it.
fn keep(path: &Path, settings: &DisplaySettings) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let fresh = path.with_extension("new");
    fs::write(&fresh, compose(settings))?;
    fs::rename(&fresh, path)
}

fn compose(settings: &DisplaySettings) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "genovesa settings {FORMAT}");
    let _ = writeln!(out, "fullscreen {}", switch(settings.fullscreen));
    let _ = writeln!(
        out,
        "resolution {}",
        match settings.resolution {
            Resolution::Native => "native".to_string(),
            Resolution::Rows(rows) => rows.to_string(),
        }
    );
    let _ = writeln!(out, "shadows {}", switch(settings.shadows));
    out
}

fn switch(on: bool) -> &'static str {
    if on {
        "on"
    } else {
        "off"
    }
}

/// Reads a file back.
///
/// A key this build has never heard of is refused, exactly as the logbook
/// refuses one: it means a file from a later build, and the honest answer is
/// to leave it alone. A key that is simply *absent* is not the same thing and
/// keeps its default — every setting here stands on its own, so a file written
/// before one of them existed still says what it does say.
fn parse(text: &str) -> Result<DisplaySettings, String> {
    let mut lines = text.lines();
    match lines.next() {
        Some(header) if header == format!("genovesa settings {FORMAT}") => {}
        Some(other) => return Err(format!("not settings this build keeps: `{other}`")),
        None => return Err("an empty file".to_string()),
    }

    let mut settings = DisplaySettings::default();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (key, value) = line
            .split_once(' ')
            .ok_or_else(|| format!("a line with no value: `{line}`"))?;
        match key {
            "fullscreen" => settings.fullscreen = on(value)?,
            "shadows" => settings.shadows = on(value)?,
            "resolution" => {
                settings.resolution = if value == "native" {
                    Resolution::Native
                } else {
                    let rows = Resolution::Rows(
                        value
                            .parse()
                            .map_err(|_| format!("`{value}` is not a number of rows"))?,
                    );
                    // A rung this build has stopped offering reads as native
                    // rather than as an error: the file is still settings, and
                    // the one thing every display can do is its own size.
                    if LADDER.contains(&rows) {
                        rows
                    } else {
                        Resolution::Native
                    }
                }
            }
            other => return Err(format!("unknown key `{other}`")),
        }
    }
    Ok(settings)
}

fn on(value: &str) -> Result<bool, String> {
    match value {
        "on" => Ok(true),
        "off" => Ok(false),
        other => Err(format!("`{other}` is not on or off")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_monitor(modes: &[(u32, u32, u32)]) -> Monitor {
        Monitor {
            name: None,
            physical_width: 3840,
            physical_height: 2160,
            physical_position: IVec2::ZERO,
            refresh_rate_millihertz: Some(60_000),
            scale_factor: 2.0,
            video_modes: modes
                .iter()
                .map(|(w, h, hz)| VideoMode {
                    physical_size: UVec2::new(*w, *h),
                    bit_depth: 32,
                    refresh_rate_millihertz: *hz,
                })
                .collect(),
        }
    }

    #[test]
    fn the_settings_survive_the_round_trip() {
        for settings in [
            DisplaySettings::default(),
            DisplaySettings {
                fullscreen: true,
                resolution: Resolution::Rows(720),
                shadows: false,
            },
        ] {
            let read = parse(&compose(&settings)).expect("parse what was composed");
            assert_eq!(read, settings);
        }
    }

    #[test]
    fn a_setting_a_file_never_mentions_keeps_its_default() {
        // Every switch here stands on its own, so a file written before one of
        // them existed is still a good file for the ones it does name.
        let read = parse("genovesa settings 1\nfullscreen on\n").expect("a partial file reads");
        assert!(read.fullscreen);
        assert_eq!(read.shadows, DisplaySettings::default().shadows);
        assert_eq!(read.resolution, DisplaySettings::default().resolution);
    }

    #[test]
    fn a_rung_this_build_no_longer_offers_reads_as_native() {
        let read = parse("genovesa settings 1\nresolution 480\n").expect("an old rung reads");
        assert_eq!(read.resolution, Resolution::Native);
    }

    #[test]
    fn what_is_not_settings_is_refused() {
        for (text, what) in [
            ("", "an empty file"),
            ("genovesa settings 999\nshadows on\n", "a format from later"),
            (
                "genovesa settings 1\nshadows maybe\n",
                "a switch that is neither way",
            ),
            (
                "genovesa settings 1\nbloom on\n",
                "a key this build has never heard of",
            ),
        ] {
            assert!(parse(text).is_err(), "swallowed {what}");
        }
    }

    #[test]
    fn the_ladder_comes_back_round_to_native() {
        let mut rung = Resolution::Native;
        let mut seen = vec![rung];
        for _ in 0..LADDER.len() - 1 {
            rung = rung.next();
            assert!(!seen.contains(&rung), "{rung:?} came round twice");
            seen.push(rung);
        }
        assert_eq!(
            rung.next(),
            Resolution::Native,
            "the ladder has no way back"
        );
    }

    #[test]
    fn a_resolution_the_display_has_no_mode_for_stays_borderless() {
        // The honest answer to "1080p" from a screen that has never offered
        // 1080p: fill it at its own size rather than force a shape it does not
        // have. The screen says so too — see [`available`].
        let monitor = a_monitor(&[(3840, 2160, 60_000)]);
        assert!(!available(Resolution::Rows(1080), Some(&monitor)));
        assert!(available(Resolution::Native, Some(&monitor)));
        // And a machine that has not reported its monitors yet claims nothing.
        assert!(!available(Resolution::Rows(1080), None));
    }

    #[test]
    fn the_widest_and_fastest_mode_of_a_height_is_the_one_taken() {
        // Two modes of a height differ in shape and in speed, and neither
        // choice is a matter of taste: the wider one fills the screen, and
        // nobody wants the slower one.
        let monitor = a_monitor(&[
            (1440, 1080, 60_000),
            (1920, 1080, 60_000),
            (1920, 1080, 120_000),
            (1280, 720, 60_000),
        ]);
        let mode = mode_for(&monitor, 1080).expect("a mode at 1080 rows");
        assert_eq!(mode.physical_size, UVec2::new(1920, 1080));
        assert_eq!(mode.refresh_rate_millihertz, 120_000);
    }

    #[test]
    fn a_window_keeps_the_shape_of_the_screen_it_is_on() {
        // A resolution is a count of pixels, never a change of shape: 1080
        // rows on a 16:10 display is 1728 wide, not 1920.
        assert_eq!(width_for(UVec2::new(2560, 1600), 1080), 1728);
        // And a video mode's width is even.
        assert_eq!(width_for(UVec2::new(1365, 768), 720) % 2, 0);
    }

    /// A headless app with a window in it. There is no winit here to answer,
    /// but `Window` is a plain component and what this system does to one is
    /// the whole of what it does.
    fn a_windowed_app(settings: DisplaySettings) -> App {
        let mut app = App::new();
        app.insert_resource(settings)
            .add_systems(Update, dress_the_window);
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        app.update();
        app
    }

    fn the_window(app: &mut App) -> Window {
        app.world_mut()
            .query_filtered::<&Window, With<PrimaryWindow>>()
            .iter(app.world())
            .next()
            .expect("a window")
            .clone()
    }

    #[test]
    fn fullscreen_fills_the_screen_and_windowed_gives_it_back() {
        let mut app = a_windowed_app(DisplaySettings::default());
        assert_eq!(the_window(&mut app).mode, WindowMode::Windowed);

        app.world_mut().resource_mut::<DisplaySettings>().fullscreen = true;
        app.update();
        // Borderless, no monitor here having offered a mode to be exclusive
        // about — see [`fullscreen_mode`].
        assert_eq!(
            the_window(&mut app).mode,
            WindowMode::BorderlessFullscreen(MonitorSelection::Current)
        );

        app.world_mut().resource_mut::<DisplaySettings>().fullscreen = false;
        app.update();
        assert_eq!(the_window(&mut app).mode, WindowMode::Windowed);
    }

    /// A window dragged to a new size stays that size. The setting says what
    /// to open at, not what to be held at, and a system that asserted the size
    /// every pass would undo the drag as fast as it could be made.
    #[test]
    fn a_window_the_player_has_resized_is_left_alone() {
        let mut app = a_windowed_app(DisplaySettings {
            resolution: Resolution::Rows(720),
            ..DisplaySettings::default()
        });
        assert_eq!(the_window(&mut app).resolution.physical_size().y, 720);

        // The corner, dragged.
        let window = app
            .world_mut()
            .query_filtered::<Entity, With<PrimaryWindow>>()
            .iter(app.world())
            .next()
            .expect("a window");
        app.world_mut()
            .entity_mut(window)
            .get_mut::<Window>()
            .expect("a window")
            .resolution
            .set_physical_resolution(1000, 800);
        app.update();
        assert_eq!(
            the_window(&mut app).resolution.physical_size(),
            UVec2::new(1000, 800),
            "the drag was undone"
        );

        // And asking for a resolution still takes, the settings having
        // changed this time.
        app.world_mut().resource_mut::<DisplaySettings>().resolution = Resolution::Rows(1080);
        app.update();
        assert_eq!(the_window(&mut app).resolution.physical_size().y, 1080);
    }

    /// The whole point of keeping a file: what was set last time is what the
    /// game comes up in next time. One test rather than two of the file and
    /// the screen separately, because both would be reaching for the same
    /// path — [`place`] answers once per process, and two tests racing over
    /// one file would fail each other on whichever machine ran them at once.
    ///
    /// Written on the way *out* of the screen rather than on every press, so
    /// what this has to show is that leaving is enough, and that a run which
    /// never opened the screen writes nothing over what is already there.
    #[test]
    fn leaving_the_display_screen_keeps_the_settings_for_next_time() {
        use bevy::state::app::StatesPlugin;

        crate::testing::quarantine_data_dir();
        let path = place().expect("a place to keep them");
        let _ = fs::remove_file(&path);

        let mut app = App::new();
        app.add_plugins((StatesPlugin, SettingsPlugin))
            .init_state::<AppState>()
            .add_sub_state::<Helm>();
        app.update();

        // Wandering about the rest of the menus writes nothing.
        for screen in [AppState::Options, AppState::Controls, AppState::MainMenu] {
            app.world_mut()
                .resource_mut::<NextState<AppState>>()
                .set(screen);
            app.update();
        }
        assert!(!path.exists(), "a file was written by nobody");

        // Opening the screen, changing every setting on it and leaving does.
        let wanted = DisplaySettings {
            fullscreen: true,
            resolution: Resolution::Rows(720),
            shadows: false,
        };
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::Display);
        app.update();
        *app.world_mut().resource_mut::<DisplaySettings>() = wanted;
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::Options);
        app.update();

        assert_eq!(load(), wanted, "next run would come up on the wrong screen");
        let _ = fs::remove_file(&path);
    }
}
