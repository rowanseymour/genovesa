//! What this machine draws with, and the file it is remembered in.
//!
//! Two things, and they are two because they are what a player on a slow
//! machine reaches for in the order they reach for them: fill the screen,
//! draw fewer pixels.
//!
//! **The resolution is the real one** — the size of the surface the frame is
//! drawn on, not a render scale laid over a full-size one. Bevy's piece for
//! that ([`bevy::camera::MainPassResolutionOverride`]) draws into a corner of
//! a full-size target and leaves the blit sampling the whole of it, so
//! without the upscaler it was built to sit in front of the picture comes out
//! in the top-left with rubbish around it.
//!
//! Which is why fullscreen and resolution are one question here rather than
//! two. A borderless fullscreen window is always the size of the desktop, so a
//! resolution asked for while fullscreen has to be a *display mode*, and the
//! monitor is the one that says which of those exist — see [`mode_for`]. A
//! display with nothing that size keeps its own, and the screen says so rather
//! than pretending.
//!
//! And "the monitor" means one monitor, the one the window is on, for every
//! question asked of it here and on the display screen — see [`showing_on`].
//!
//! None of it happens the moment a switch is thrown. The screen edits
//! [`Wanted`] and Apply is what makes that [`DisplaySettings`], which is the
//! only thing [`dress_the_window`] reads — and an applied change is then put
//! back on its own unless it is stood by. See [`Wanted`] and [`OnTrial`] for
//! why a display setting of all things is worth that much ceremony.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use bevy::prelude::*;
use bevy::window::{
    Monitor, MonitorSelection, PrimaryMonitor, PrimaryWindow, VideoMode, VideoModeSelection,
    WindowMode, WindowPosition, WindowResized,
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
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            // A window, because a game that seized the whole screen the first
            // time it was run would be a game you had to know a key to get out
            // of before you had seen it.
            fullscreen: false,
            resolution: Resolution::Native,
        }
    }
}

/// What the display screen is set to, which is not yet what the machine is
/// doing.
///
/// The screen edits this; [`dress_the_window`] reads [`DisplaySettings`]. The
/// gap between the two is the whole point of an Apply button, and it is the
/// resolution that earns it: below native in fullscreen a rung is an
/// *exclusive display mode*, and taking one is a real mode switch — the screen
/// goes black and the monitor resyncs. A control that applied as it was
/// touched spent four of those getting from native to 720p, three of them
/// modes nobody had asked for.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Wanted(pub DisplaySettings);

/// How long an applied change has to be stood by before it is put back.
///
/// Ten seconds because the first two or three are not the player's: a mode
/// switch blacks the monitor out while it resyncs, and only then is there
/// anything to judge. What is left is enough to read a screen and reach for a
/// button, and little enough that somebody looking at a display they cannot
/// read is not looking at it for long.
const TRIAL: Duration = Duration::from_secs(10);

/// A change that has been applied but not yet stood by.
///
/// Why Apply is not the last word: the setting most worth having is also the
/// one that can leave the player unable to read the screen they would have to
/// use to undo it — a fullscreen mode the display accepts and then draws
/// badly, or not at all. So the change is made and then put back on its own,
/// unless somebody who can evidently still see it says to keep it.
#[derive(Resource, Default)]
pub struct OnTrial(pub Option<Trial>);

/// A change under way, and the clock it has to be stood by within.
pub struct Trial {
    /// What to go back to.
    was: DisplaySettings,
    left: Timer,
}

impl Trial {
    pub fn new(was: DisplaySettings) -> Self {
        Self {
            was,
            left: Timer::new(TRIAL, TimerMode::Once),
        }
    }

    /// Whole seconds still on the clock, for the button to count down in.
    /// Rounded up, so a trial with any time left at all never reads as none.
    pub fn seconds_left(&self) -> u64 {
        self.left.remaining().as_secs_f32().ceil() as u64
    }

    /// Ticks the clock, and gives back the settings to return to once it has
    /// run out — `None` for as long as it has not.
    pub fn ran_out(&mut self, delta: Duration) -> Option<DisplaySettings> {
        self.left.tick(delta).is_finished().then_some(self.was)
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

/// The shape a width is worked out on when there is no screen to ask — before
/// winit has reported a monitor, and in a run with no window at all, where
/// there is never going to be one. Widescreen because everything is.
pub(crate) const WIDESCREEN: UVec2 = UVec2::new(16, 9);

/// The rungs, in the order the screen lists them: down from native, since that
/// is the direction somebody opening it is going.
pub const LADDER: [Resolution; 5] = [
    Resolution::Native,
    // 4K is a rung rather than a synonym for native, and on a retina panel it
    // is a long way below one: a 6400x3600 display draws twenty-three million
    // pixels a frame, and 2160p is a third of that.
    Resolution::Rows(2160),
    Resolution::Rows(1440),
    Resolution::Rows(1080),
    Resolution::Rows(720),
];

/// The rungs as plain rows of pixels, in the order [`LADDER`] lists them —
/// what the console's `client resolution` takes, native being no answer in a run
/// with no display to be native to.
pub(crate) fn rungs() -> Vec<u32> {
    LADDER
        .iter()
        .filter_map(|rung| match rung {
            Resolution::Native => None,
            Resolution::Rows(rows) => Some(*rows),
        })
        .collect()
}

impl Resolution {
    /// What the button reads. A switch has to say which way it is set, not
    /// what pressing it would do.
    pub fn label(&self) -> String {
        match self {
            Self::Native => "Native".to_string(),
            Self::Rows(rows) => format!("{rows}p"),
        }
    }
}

/// How wide a picture `rows` tall is on a screen of `shape` — the aspect ratio
/// the display has, so a resolution is a *count* of pixels and never a change
/// of shape. Even, because a video mode is.
pub(crate) fn width_for(shape: UVec2, rows: u32) -> u32 {
    let wide = (rows as f32 * shape.x.max(1) as f32 / shape.y.max(1) as f32).round() as u32;
    wide.max(2) & !1
}

// ---------------------------------------------------------------------------
// Making it so
// ---------------------------------------------------------------------------

pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        // Shared with the menu that changes them and the overlay that draws
        // beside them — see [`crate::menu`] and [`crate::debug`] — each
        // initialising it for its own tests. A real run has them off the file
        // before there is an app at all, the first window being built out of
        // them: `bin/game.rs` reads them and inserts them over this.
        app.init_resource::<DisplaySettings>()
            .init_resource::<AsOpened>()
            // What the screen is editing towards, and a change it has made
            // but not yet been stood by. The screen itself drives both — see
            // [`crate::menu`], which initialises them for its own tests the
            // way it does the settings.
            .init_resource::<Wanted>()
            .init_resource::<OnTrial>()
            // Normally `UiPlugin`'s, initialised here the way the menu does
            // its shared resources, so the tests have one too.
            .init_resource::<UiScale>()
            // Normally `WindowPlugin`'s, registered here for the same reason:
            // [`dress_the_window`] writes it.
            .add_message::<WindowResized>()
            // Chained so the scale reads the size the window was just asked
            // for rather than last frame's.
            .add_systems(Update, (dress_the_window, scale_the_ui).chain());
        // Noted on the way into the screen that changes them and written on
        // the way out, which is one write per visit rather than one per Apply
        // — and none at all for a visit that applied nothing, or applied
        // something and let it be put back, which is what [`load`] promises a
        // file this build cannot read. Both ways to the screen.
        app.add_systems(OnEnter(AppState::Display), note_settings)
            .add_systems(OnEnter(Helm::Display), note_settings)
            .add_systems(OnExit(AppState::Display), keep_settings)
            .add_systems(OnExit(Helm::Display), keep_settings);
    }
}

/// Makes the window agree with the settings.
///
/// The *mode* is set the way the debug overlay's own apply system sets things:
/// to what it should be, every pass, rather than when a switch is thrown. It
/// has to be, because what it should be can change with nothing here changing
/// at all — a run comes up before winit has reported a single monitor, so a
/// resolution wanting an exclusive mode has to settle for borderless until
/// there is a screen to read one off. Comparing before writing is what keeps
/// that from marking `Window` changed forever, which the backdrop would answer
/// by re-ruling its sheet forever.
///
/// A window born unseen — see [`opening`] — is the one exception to settling:
/// it takes no mode at all until there is a screen, and is shown the frame
/// after it has the one it was waiting for.
///
/// The *size* is compared too, but against the size this system last asked
/// for rather than against the window's own. That difference is the whole of
/// what leaves a dragged window alone: a drag changes the window and not what
/// this wants, so nothing is written and the drag stands. What does change
/// what this wants is the screen turning up — a width follows from the shape
/// of the display, and the guess made before there was one to ask is only
/// right on a widescreen — so that lands a frame or two in, once, and then
/// stops.
fn dress_the_window(
    settings: Res<DisplaySettings>,
    monitors: Query<(Entity, &Monitor, Has<PrimaryMonitor>)>,
    mut windows: Query<(Entity, &mut Window), With<PrimaryWindow>>,
    mut resized: MessageWriter<WindowResized>,
    mut asked_for: Local<Option<UVec2>>,
) {
    // A windowless run has no window to dress, and asks for the size of its
    // pictures on the command line instead — see [`crate::control`].
    let Ok((entity, mut window)) = windows.single_mut() else {
        return;
    };
    let monitor = showing_on(Some(&window), monitors.iter());

    // Unseen because it is not to pass through borderless on its way to an
    // exclusive mode, so it waits, windowed, for a screen to read one off.
    if !window.visible && monitor.is_none() {
        return;
    }

    let wanted = if settings.fullscreen {
        fullscreen_mode(settings.resolution, monitor)
    } else {
        WindowMode::Windowed
    };
    // Dressed already means asked for a frame ago, which winit has acted on by
    // now — the frame it is asked for, it has not, and a window shown then
    // would be seen changing.
    let dressed = window.mode == wanted;
    if !dressed {
        window.mode = wanted;
    }
    if dressed && !window.visible {
        window.visible = true;
    }

    // In a window the resolution is the window's own size, which is the same
    // count of pixels by another route. Left alone while fullscreen, where the
    // mode above has already said how big the surface is and writing a size
    // over it would only fight winit.
    if settings.fullscreen {
        return;
    }
    // A shape to work a width out of, and 16:9 until there is a screen to ask
    // — the same guess [`opening`] makes, and replaced by the real one on the
    // frame a monitor arrives.
    let shape = monitor.map_or(WIDESCREEN, |(_, screen)| screen.physical_size());
    let size = match settings.resolution {
        Resolution::Native => crate::WINDOW,
        Resolution::Rows(rows) => UVec2::new(width_for(shape, rows), rows),
    };
    if *asked_for == Some(size) {
        return;
    }
    *asked_for = Some(size);
    let size = size.max(UVec2::ONE);
    // A window already that size — as on the first pass of every default run,
    // the window having been built at the size Native wants — has nothing to
    // resize, and must not be told it did: the announcement below would be of
    // a resize that never happened.
    if window.resolution.physical_size() == size {
        return;
    }
    window.resolution.set_physical_resolution(size.x, size.y);
    // Announced now rather than left to winit, whose own message lands a
    // frame late for a resize the app began: the surface follows the `Window`
    // the frame it changes, but the camera sizes the depth texture off this
    // message — and one frame of the two disagreeing is a wgpu validation
    // error the renderer answers by quitting. Winit still echoes the resize a
    // frame later; its one reader re-reads the same size, so the echo is idle.
    resized.write(WindowResized {
        window: entity,
        width: window.width(),
        height: window.height(),
    });
}

/// How small the UI will let itself be drawn, as a fraction of its laid-out
/// size. Below half the lettering stops being lettering, so a window shorter
/// than that gets a UI too big for it instead — which the controls screen, the
/// one screen that can outgrow a window, answers by scrolling.
const SMALLEST_UI: f32 = 0.5;

/// Keeps the UI in proportion to the window.
///
/// Every menu is laid out in fixed `Val::Px` sizes drawn up to fit
/// [`crate::WINDOW`] read as *logical* pixels. The resolution setting deals in
/// physical ones — see the module doc — so on a high-density display a 720p
/// window is only 360 logical rows, half the room the layouts were drawn for,
/// and the controls screen runs off both ends of it. Scaling the UI by how
/// much window there really is makes every screen the same fraction of it
/// instead: the menu at 720p is the menu at native with fewer pixels, which is
/// all the setting ever said it changed.
///
/// Fitted to whichever way the window is tighter, so a window dragged tall and
/// narrow shrinks the menus rather than cropping their sides. The scale is the
/// whole UI's — the compass and the console wear it too — and the two sheets
/// drawn *under* UI keep themselves in step: the backdrop enlarges its
/// engraving to match (see [`crate::backdrop`]), and the chart pins its
/// furniture to its own pixels instead (see [`crate::chart`]).
fn scale_the_ui(windows: Query<&Window, With<PrimaryWindow>>, mut scale: ResMut<UiScale>) {
    // A windowless run has no window to read, and fits the UI to the picture
    // it is drawing into instead — see [`crate::control`], which sets the
    // scale once from the same [`fitted_to`] this uses.
    let Ok(window) = windows.single() else {
        return;
    };
    let fitted = fitted_to(window.resolution.size());
    // Compared before written: a scale rewritten every frame reads as changed,
    // and the layout system would lay the whole UI out again for it.
    if scale.0 != fitted {
        scale.0 = fitted;
    }
}

/// The scale the UI wears when it is being drawn at `size` physical pixels.
///
/// Split out because a run with no window draws UI all the same — the readout
/// and the compass are in every picture it writes — and has to fit it to the
/// image it draws into. Two spellings of this would be two opinions about how
/// big a menu is, and the windowless one is the half nobody would be looking
/// at while it drifted.
pub(crate) fn fitted_to(size: Vec2) -> f32 {
    let room = size / crate::WINDOW.as_vec2();
    room.min_element().max(SMALLEST_UI)
}

/// The monitor a window is on, named by the entity that carries it.
///
/// "The screen" has to mean one screen and go on meaning the same one: the
/// modes a resolution is checked against, the monitor a fullscreen is asked
/// of, the shape a windowed width follows from and the caveat the display
/// screen prints are four questions about the display the player is looking
/// at, and on a second monitor they have different answers from the primary's.
/// Asking [`PrimaryMonitor`] for all four is how a game fullscreen on the
/// second screen answers "1080p" by taking a mode off the first and dragging
/// itself over there to wear it.
///
/// Worked out from the geometry, winit's own idea of a current monitor not
/// being readable from here: the displays lie side by side in one space of
/// physical pixels, so the screen a window is on is the one it covers most of.
/// A window whose corner is not a number yet — one still being created —
/// covers nothing, and the primary monitor stands in for it, as it does for a
/// run with no window of its own at all.
pub fn showing_on<'a>(
    window: Option<&Window>,
    monitors: impl IntoIterator<Item = (Entity, &'a Monitor, bool)>,
) -> Option<(Entity, &'a Monitor)> {
    let rect = window.and_then(seen_at);
    let (mut best, mut most) = (None, 0);
    let mut primary = None;
    for (entity, monitor, is_primary) in monitors {
        if is_primary {
            primary = Some((entity, monitor));
        }
        let covered = rect.map_or(0, |rect| overlap(rect, monitor));
        if covered > most {
            (best, most) = (Some((entity, monitor)), covered);
        }
    }
    best.or(primary)
}

/// Where a window's pixels are in the space the monitors share, or `None` for
/// one whose corner the window manager has not answered for yet.
fn seen_at(window: &Window) -> Option<IRect> {
    let WindowPosition::At(corner) = window.position else {
        return None;
    };
    Some(IRect::from_corners(
        corner,
        corner + window.resolution.physical_size().as_ivec2(),
    ))
}

/// How many of a window's pixels fall on this monitor.
fn overlap(window: IRect, monitor: &Monitor) -> i64 {
    let screen = IRect::from_corners(
        monitor.physical_position,
        monitor.physical_position + monitor.physical_size().as_ivec2(),
    );
    let shared = window.intersect(screen);
    i64::from(shared.width()) * i64::from(shared.height())
}

/// How the window is born, for the binary that builds it before there is an
/// app at all.
pub struct Opening {
    pub mode: WindowMode,
    pub size: UVec2,
    /// Whether it is seen from its first frame, or held back until
    /// [`dress_the_window`] has given it the mode it was born waiting for.
    pub visible: bool,
}

/// How the window should first come up.
///
/// Neither answer can be the final one. Winit reports no monitors until it has
/// a window, so the exclusive mode a resolution wants cannot be chosen yet; the
/// shape of the screen is unknown for the same reason, so a windowed size is
/// guessed widescreen. [`dress_the_window`] puts both right on the first frame
/// that has a monitor to ask.
///
/// Native fullscreen is asked for at birth, so nobody who said fullscreen is
/// shown the desktop first. A resolution below native is born *windowed and
/// unseen* instead, and takes its mode once there is a screen. Not borderless
/// meanwhile: on macOS a window switched to an exclusive mode in its first
/// frames is still in the borderless transition, and winit leaves it framed
/// for the desktop it no longer has — the picture a strip too high, black
/// beneath. From windowed the switch is framed right, and a window nobody has
/// seen can go that way unseen.
pub fn opening(settings: &DisplaySettings) -> Opening {
    let size = match settings.resolution {
        Resolution::Native => crate::WINDOW,
        Resolution::Rows(rows) => UVec2::new(width_for(WIDESCREEN, rows), rows),
    };
    let (mode, visible) = match (settings.fullscreen, settings.resolution) {
        (false, _) => (WindowMode::Windowed, true),
        // `Primary`, because a window being created is not on a monitor yet
        // and there is no other honest answer: naming the one it is on takes
        // a monitor to name. It is the answer [`fullscreen_mode`] gives while
        // no monitor has been reported, too, so the first pass over this
        // window finds the mode already right and writes nothing — a write of
        // a *different* guess would only be asking winit the same unanswerable
        // question a frame later.
        (true, Resolution::Native) => (
            WindowMode::BorderlessFullscreen(MonitorSelection::Primary),
            true,
        ),
        (true, Resolution::Rows(_)) => (WindowMode::Windowed, false),
    };
    Opening {
        mode,
        size,
        visible,
    }
}

/// The fullscreen this resolution asks for, on the screen the window is on.
///
/// Borderless for native, which is the mode that behaves itself: it takes the
/// desktop as it finds it and gives it back the same way. A resolution below
/// native has to be an *exclusive* mode, because that is the only fullscreen
/// in which the display itself changes size — and a display with nothing that
/// tall keeps borderless rather than being forced into a shape it has not
/// offered.
///
/// Every answer names the screen it was worked out on rather than saying
/// `Current` and trusting winit to agree, so a mode and the monitor it is
/// asked of are the same monitor. Before any has been reported there is
/// nothing to name and `Primary` stands in — see [`opening`].
fn fullscreen_mode(resolution: Resolution, monitor: Option<(Entity, &Monitor)>) -> WindowMode {
    let Some((entity, screen)) = monitor else {
        return WindowMode::BorderlessFullscreen(MonitorSelection::Primary);
    };
    let borderless = WindowMode::BorderlessFullscreen(MonitorSelection::Entity(entity));
    let Resolution::Rows(rows) = resolution else {
        return borderless;
    };
    match mode_for(screen, rows) {
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
            // plays on the defaults, and only somebody saying what they want in
            // so many words — a switch actually thrown, not merely a screen
            // opened and left — replaces it. See [`keep_settings`].
            warn!("cannot read the display settings: {why}");
            DisplaySettings::default()
        }
    }
}

/// What the settings were when the display screen was opened, so that leaving
/// it can tell a change from a look.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug, Default)]
struct AsOpened(DisplaySettings);

fn note_settings(settings: Res<DisplaySettings>, mut opened: ResMut<AsOpened>) {
    opened.0 = *settings;
}

/// Writes what the screen was left set to, and only if leaving it left
/// anything different. A visit that threw no switch has said nothing, and
/// there is a file this build could not read that it must not answer with the
/// defaults it fell back on — see [`load`].
fn keep_settings(settings: Res<DisplaySettings>, opened: Res<AsOpened>) {
    if *settings == opened.0 {
        return;
    }
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
    use bevy::window::WindowResolution;

    use super::*;

    fn a_monitor(modes: &[(u32, u32, u32)]) -> Monitor {
        a_monitor_at(IVec2::ZERO, UVec2::new(3840, 2160), modes)
    }

    /// A monitor somewhere in the space the displays share, which is what
    /// tells one from another — see [`showing_on`].
    fn a_monitor_at(at: IVec2, size: UVec2, modes: &[(u32, u32, u32)]) -> Monitor {
        Monitor {
            name: None,
            physical_width: size.x,
            physical_height: size.y,
            physical_position: at,
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

    /// A window's corner and how big it is, which between them say which
    /// screen it is on.
    fn a_window_at(at: IVec2, size: UVec2) -> Window {
        Window {
            position: WindowPosition::At(at),
            resolution: WindowResolution::new(size.x, size.y),
            ..default()
        }
    }

    #[test]
    fn the_settings_survive_the_round_trip() {
        for settings in [
            DisplaySettings::default(),
            DisplaySettings {
                fullscreen: true,
                resolution: Resolution::Rows(720),
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
            (
                "genovesa settings 999\nfullscreen on\n",
                "a format from later",
            ),
            (
                "genovesa settings 1\nfullscreen maybe\n",
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

    /// Two displays side by side, and a window on the right-hand one. Every
    /// question this file asks about "the screen" has to be asked of that one:
    /// answering with the primary is how a fullscreen picked on the second
    /// monitor takes a mode off the first and hauls the window over to it.
    #[test]
    fn the_screen_is_the_one_the_window_is_on_rather_than_the_first_one() {
        let left = a_monitor_at(IVec2::ZERO, UVec2::new(1920, 1080), &[(1920, 1080, 60_000)]);
        let right = a_monitor_at(
            IVec2::new(1920, 0),
            UVec2::new(2560, 1600),
            &[(2560, 1600, 60_000)],
        );
        let (first, second) = (
            Entity::from_raw_u32(1).expect("an entity"),
            Entity::from_raw_u32(2).expect("an entity"),
        );
        let screens = [(first, &left, true), (second, &right, false)];

        let over_there = a_window_at(IVec2::new(2100, 200), UVec2::new(1280, 720));
        let (entity, monitor) = showing_on(Some(&over_there), screens).expect("a screen to be on");
        assert_eq!(entity, second);
        assert_eq!(monitor.physical_size(), UVec2::new(2560, 1600));

        // And a window mostly on the first is on the first, a window straddling
        // the two counting as on whichever it shows more of.
        let mostly_here = a_window_at(IVec2::new(1600, 200), UVec2::new(600, 400));
        assert_eq!(
            showing_on(Some(&mostly_here), screens).map(|(entity, _)| entity),
            Some(first)
        );
    }

    /// The screen cannot always be told, and the primary is the stand-in: a
    /// window still being created has no corner to compare, and a run drawing
    /// pictures off screen has no window at all.
    #[test]
    fn a_window_that_is_on_no_screen_yet_falls_back_to_the_first() {
        let primary = a_monitor(&[]);
        let other = a_monitor_at(IVec2::new(4000, 0), UVec2::new(1920, 1080), &[]);
        let (first, second) = (
            Entity::from_raw_u32(1).expect("an entity"),
            Entity::from_raw_u32(2).expect("an entity"),
        );
        let screens = [(first, &primary, true), (second, &other, false)];

        let placeless = Window::default();
        assert_eq!(
            showing_on(Some(&placeless), screens).map(|(entity, _)| entity),
            Some(first)
        );
        assert_eq!(
            showing_on(None, screens).map(|(entity, _)| entity),
            Some(first)
        );
        // And a machine that has reported no monitors has nothing to answer
        // with, which every caller here already knows how to hear.
        let none: [(Entity, &Monitor, bool); 0] = [];
        assert!(showing_on(Some(&placeless), none).is_none());
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
            .add_message::<WindowResized>()
            .add_systems(Update, dress_the_window);
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        app.update();
        app
    }

    /// The sizes [`dress_the_window`] has announced since last asked.
    fn announced(app: &mut App) -> Vec<Vec2> {
        app.world_mut()
            .resource_mut::<Messages<WindowResized>>()
            .drain()
            .map(|message| Vec2::new(message.width, message.height))
            .collect()
    }

    fn the_window(app: &mut App) -> Window {
        app.world_mut()
            .query_filtered::<&Window, With<PrimaryWindow>>()
            .iter(app.world())
            .next()
            .expect("a window")
            .clone()
    }

    /// The window's own corner, dragged.
    fn drag_to(app: &mut App, size: UVec2) {
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
            .set_physical_resolution(size.x, size.y);
        app.update();
    }

    #[test]
    fn fullscreen_fills_the_screen_and_windowed_gives_it_back() {
        let mut app = a_windowed_app(DisplaySettings::default());
        assert_eq!(the_window(&mut app).mode, WindowMode::Windowed);

        app.world_mut().resource_mut::<DisplaySettings>().fullscreen = true;
        app.update();
        // Borderless, no monitor here having offered a mode to be exclusive
        // about — and on the primary, there being none to name and nothing
        // else honest to say — see [`fullscreen_mode`].
        assert_eq!(
            the_window(&mut app).mode,
            WindowMode::BorderlessFullscreen(MonitorSelection::Primary)
        );

        app.world_mut().resource_mut::<DisplaySettings>().fullscreen = false;
        app.update();
        assert_eq!(the_window(&mut app).mode, WindowMode::Windowed);
    }

    /// A resolution below native is born windowed and unseen — [`opening`]
    /// says why it must not pass through borderless — takes its mode the
    /// frame a screen is reported, and is shown the frame after that.
    #[test]
    fn a_window_born_unseen_is_shown_once_it_has_its_mode() {
        let settings = DisplaySettings {
            fullscreen: true,
            resolution: Resolution::Rows(1440),
        };
        let born = opening(&settings);
        assert_eq!(born.mode, WindowMode::Windowed);
        assert!(!born.visible);

        let mut app = App::new();
        app.insert_resource(settings)
            .add_message::<WindowResized>()
            .add_systems(Update, dress_the_window);
        app.world_mut().spawn((
            Window {
                mode: born.mode,
                visible: born.visible,
                ..default()
            },
            PrimaryWindow,
        ));
        // No screen yet, so no mode to take and nothing to show — however
        // long that lasts.
        app.update();
        app.update();
        let window = the_window(&mut app);
        assert_eq!(
            window.mode,
            WindowMode::Windowed,
            "went fullscreen with no screen to read a mode off"
        );
        assert!(!window.visible, "shown before it had its mode");

        let screen = app
            .world_mut()
            .spawn((
                a_monitor_at(IVec2::ZERO, UVec2::new(5120, 2880), &[(2560, 1440, 60_000)]),
                PrimaryMonitor,
            ))
            .id();
        app.update();
        let window = the_window(&mut app);
        assert_eq!(
            window.mode,
            WindowMode::Fullscreen(
                MonitorSelection::Entity(screen),
                VideoModeSelection::Specific(VideoMode {
                    physical_size: UVec2::new(2560, 1440),
                    bit_depth: 32,
                    refresh_rate_millihertz: 60_000,
                }),
            )
        );
        assert!(
            !window.visible,
            "shown the frame the mode was asked for, before winit had it"
        );

        app.update();
        assert!(the_window(&mut app).visible);
    }

    /// Every other window is seen from its first frame, and one that said
    /// fullscreen at native is fullscreen from it too.
    #[test]
    fn every_other_window_is_born_seen() {
        let native_fullscreen = DisplaySettings {
            fullscreen: true,
            resolution: Resolution::Native,
        };
        let born = opening(&native_fullscreen);
        assert!(born.visible);
        assert_eq!(
            born.mode,
            WindowMode::BorderlessFullscreen(MonitorSelection::Primary)
        );
        for settings in [
            DisplaySettings::default(),
            DisplaySettings {
                fullscreen: false,
                resolution: Resolution::Rows(720),
            },
        ] {
            let born = opening(&settings);
            assert!(born.visible, "{settings:?}");
            assert_eq!(born.mode, WindowMode::Windowed, "{settings:?}");
        }
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

        drag_to(&mut app, UVec2::new(1000, 800));
        assert_eq!(
            the_window(&mut app).resolution.physical_size(),
            UVec2::new(1000, 800),
            "the drag was undone"
        );

        // And asking for a resolution still takes, this being a size the
        // system has not asked for before rather than the one it already has.
        app.world_mut().resource_mut::<DisplaySettings>().resolution = Resolution::Rows(1080);
        app.update();
        assert_eq!(the_window(&mut app).resolution.physical_size().y, 1080);
    }

    /// A resize this system makes has to be announced the frame it is made,
    /// and a resize it did not make must not be — the comments at the
    /// announcement in [`dress_the_window`] say why each half was a crash or
    /// would be a lie.
    #[test]
    fn a_change_of_size_is_announced_the_frame_it_is_made() {
        let mut app = a_windowed_app(DisplaySettings {
            resolution: Resolution::Rows(1080),
            ..DisplaySettings::default()
        });
        assert_eq!(announced(&mut app), vec![Vec2::new(1920.0, 1080.0)]);

        // Said once, when it happens: a frame that changed nothing has
        // nothing to announce.
        app.update();
        assert_eq!(announced(&mut app), vec![]);

        app.world_mut().resource_mut::<DisplaySettings>().resolution = Resolution::Rows(720);
        app.update();
        assert_eq!(announced(&mut app), vec![Vec2::new(1280.0, 720.0)]);

        // A drag is winit's news and winit has already told it — repeating it
        // here would be a second report of the same resize.
        drag_to(&mut app, UVec2::new(1000, 800));
        assert_eq!(announced(&mut app), vec![]);

        // And a run whose window opens at the very size it wants — every
        // default run does — has no resize to announce on its first frame.
        let mut untouched = a_windowed_app(DisplaySettings::default());
        assert_eq!(announced(&mut untouched), vec![]);
    }

    /// The width follows from the shape of the screen, and the screen turns up
    /// late: a run comes up before winit has reported a monitor, so the first
    /// size is worked out on a guess of 16:9. The frame the real shape arrives
    /// the window has to be put right, though not a setting has moved — the
    /// whole reason [`width_for`] exists is a display that is not widescreen.
    #[test]
    fn a_window_is_resized_when_the_screen_it_is_on_becomes_known() {
        let mut app = a_windowed_app(DisplaySettings {
            resolution: Resolution::Rows(1080),
            ..DisplaySettings::default()
        });
        // The guess, nothing having yet said this is not a widescreen.
        assert_eq!(
            the_window(&mut app).resolution.physical_size(),
            UVec2::new(1920, 1080)
        );

        app.world_mut().spawn((
            a_monitor_at(IVec2::ZERO, UVec2::new(2560, 1600), &[]),
            PrimaryMonitor,
        ));
        app.update();
        assert_eq!(
            the_window(&mut app).resolution.physical_size(),
            UVec2::new(1728, 1080),
            "the window kept a width guessed before there was a screen to ask"
        );

        // Once, and no more: with the shape settled, a corner dragged after it
        // stands, exactly as it does on a machine that never had a monitor to
        // report.
        drag_to(&mut app, UVec2::new(1000, 800));
        assert_eq!(
            the_window(&mut app).resolution.physical_size(),
            UVec2::new(1000, 800),
            "the drag was undone"
        );
    }

    /// Sets a window's size the way the platform reports it: so many physical
    /// pixels, at so many of them per logical one. What the scale answers to.
    fn scaled(app: &mut App, window: Entity, physical: UVec2, factor: f32) -> f32 {
        let mut resolution = WindowResolution::new(physical.x, physical.y);
        resolution.set_scale_factor_override(Some(factor));
        app.world_mut()
            .entity_mut(window)
            .get_mut::<Window>()
            .expect("a window")
            .resolution = resolution;
        app.update();
        app.world().resource::<UiScale>().0
    }

    /// The menus were laid out for [`crate::WINDOW`] of *logical* pixels, and
    /// the resolution setting deals in physical ones — so on a dense display
    /// the same setting leaves the layouts half the room, and the scale is
    /// what squares the two.
    #[test]
    fn the_ui_is_scaled_to_the_room_the_window_gives_it() {
        let mut app = App::new();
        app.init_resource::<UiScale>()
            .add_systems(Update, scale_the_ui);
        let window = app
            .world_mut()
            .spawn((Window::default(), PrimaryWindow))
            .id();

        // The laid-out size itself, and everything in proportion to it.
        assert_eq!(scaled(&mut app, window, UVec2::new(1280, 720), 1.0), 1.0);
        assert_eq!(scaled(&mut app, window, UVec2::new(2560, 1440), 1.0), 2.0);
        // The window the scale exists for: 720 physical rows on a two-to-one
        // display is 360 logical, and the UI halves with them.
        assert_eq!(scaled(&mut app, window, UVec2::new(1280, 720), 2.0), 0.5);
        // A window dragged tall and narrow is fitted to its narrowness rather
        // than cropped at its sides.
        assert_eq!(scaled(&mut app, window, UVec2::new(640, 720), 1.0), 0.5);
        // And below the floor the UI stops shrinking — see [`SMALLEST_UI`].
        assert_eq!(scaled(&mut app, window, UVec2::new(1280, 180), 1.0), 0.5);
    }

    /// The whole point of keeping a file: what was set last time is what the
    /// game comes up in next time. One test rather than two of the file and
    /// the screen separately, because both would be reaching for the same
    /// path — [`place`] answers once per process, and two tests racing over
    /// one file would fail each other on whichever machine ran them at once.
    ///
    /// Written on the way *out* of the screen rather than on every press, so
    /// what this has to show is that leaving is enough — and that nothing is
    /// written by a run that never opened the screen, or by a visit that
    /// looked and changed nothing, since a file this build cannot read is
    /// promised it will not be answered with the defaults it fell back on.
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

        // Nor does opening the screen itself, and leaving without touching a
        // row. Looking is not saying.
        for screen in [AppState::Display, AppState::Options] {
            app.world_mut()
                .resource_mut::<NextState<AppState>>()
                .set(screen);
            app.update();
        }
        assert!(
            !path.exists(),
            "a file was written by a visit that said nothing"
        );

        // Nor does a visit that applied something and let it be put back —
        // what the trial does over in the menu, here as the two writes it
        // makes. The file is promised a write per change that outlasted the
        // screen, and this is not one.
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::Display);
        app.update();
        *app.world_mut().resource_mut::<DisplaySettings>() = DisplaySettings {
            fullscreen: true,
            ..DisplaySettings::default()
        };
        *app.world_mut().resource_mut::<DisplaySettings>() = DisplaySettings::default();
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::Options);
        app.update();
        assert!(
            !path.exists(),
            "a file was written by a change that was put back"
        );

        // Opening the screen, changing every setting on it and leaving does.
        let wanted = DisplaySettings {
            fullscreen: true,
            resolution: Resolution::Rows(720),
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
