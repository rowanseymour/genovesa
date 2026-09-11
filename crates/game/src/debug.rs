//! The debug readout: what the frame draws, and the switches that change it.
//!
//! A small readout pinned to a corner of the window, over whatever screen the
//! app is on, a subject to a line. The frame rate comes from Bevy's own
//! frame-time diagnostics, already smoothed; the geometry under it is what the
//! camera actually draws, so it moves with the view as well as with the
//! streaming, and reads zero on the menu, where nothing 3D exists at all.
//! The chunk line shows the streamer's own bookkeeping instead,
//! which is about what this machine *holds* rather than what it draws: how
//! much of the world it has, how much of that was open water, and how much
//! ground it is still waiting on. The last line reads the world and the view
//! back in the words that put them there — `--seed`, then `goto`, `yaw` and
//! `zoom` — so a screenshot of the overlay is the whole of what it takes to
//! stand here again. The same words go into every picture as [`stamp`], which
//! is that promise kept for the pictures nobody remembered to turn the readout
//! on for — see [`crate::shots`].
//!
//! The switches are [`Toggles`], and they are set from the console — see
//! [`crate::console`], whose `client` lines are their only writer. They used to
//! be number keys; the console replaced them because a vocabulary outgrows a
//! number row, but reading a count and changing what is counted are still one
//! job, which is why the state stays in this module with the readout: the
//! reason to turn the haze off is to watch the far ground answer. When
//! any switch is away from the world's own state the readout says so on a
//! last line, since a doctored picture that did not admit it would be worth
//! less than no picture at all.

use std::any::TypeId;

use bevy::camera::visibility::VisibleEntities;
use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::ecs::system::SystemParam;
use bevy::pbr::wireframe::{WireframeConfig, WireframePlugin};
use bevy::pbr::DistanceFog;
use bevy::prelude::*;
use bevy::text::FontSize;

use crate::camera::{MapCamera, View};
use crate::chart::Chart;
use crate::net::Hosting;
use crate::terrain::{Ground, Tally};
use crate::AppState;
use protocol::survey::SurveyTally;

pub(crate) const TEXT: Color = Color::srgb(0.88, 0.87, 0.80);
pub(crate) const BACKDROP: Color = Color::srgba(0.0, 0.0, 0.0, 0.55);

pub struct DebugOverlayPlugin;

impl Plugin for DebugOverlayPlugin {
    fn build(&self, app: &mut App) {
        // Wireframes come from the renderer, and the readout's own tests run
        // an app that has none — text is text without a GPU. Asking whether
        // the renderer is there keeps every overlay system in this file
        // rather than scattering half of them into the binary to dodge that.
        if app.is_plugin_added::<bevy::render::RenderPlugin>() {
            app.add_plugins(WireframePlugin::default());
        }
        app.add_plugins(FrameTimeDiagnosticsPlugin::default())
            .init_resource::<Toggles>()
            // The hour the readout prints — shared with the plugin that
            // draws the sky, each initialising it for its own tests.
            .init_resource::<crate::sky::Sky>()
            .init_resource::<NextRefresh>()
            .add_systems(Startup, spawn_overlay)
            .add_systems(Update, (apply_toggles, refresh_overlay).chain());
    }
}

/// How often the readout is worked out again, in seconds.
///
/// Not every frame, for two reasons that point the same way. The numbers are
/// printed as whole figures, and a frame rate hovering between two of them
/// rewrites the line every frame — which reads as a flicker rather than as a
/// measurement, and is the one thing a readout must not do when what it is
/// being read for is *whether the frame rate is steady*. And working it out
/// costs a walk of every visible mesh, so a readout that refreshed with the
/// frame would charge the frame for saying how long the frame took.
///
/// Four times a second is fast enough that a sag is visible as it happens and
/// slow enough to read. The frame rate itself is smoothed long before it gets
/// here — see [`bevy::diagnostic::Diagnostic::smoothed`] — so this samples an
/// average rather than decimating a raw signal.
const REFRESH_SECONDS: f32 = 0.25;

/// When the readout is next due. Zero on the first frame, so a run that has
/// just turned `stats` on gets its numbers immediately rather than after a
/// wait.
#[derive(Resource, Default)]
struct NextRefresh(f32);

/// Marks the overlay's panel, so showing and hiding it can find it.
#[derive(Component)]
struct DebugPanel;

/// Marks the overlay's one text block, so the refresh can find it.
#[derive(Component)]
struct DebugText;

/// The variables the console's `client` lines reach — see [`crate::console`],
/// which owns the grammar, while this module owns making them true of the
/// scene. None of them reaches the world or anybody else's picture of it,
/// which is what separates a `client` line from the console's other language.
///
/// Most are about what this machine *draws*. `resolution` is the exception
/// and lives here anyway: it belongs to [`crate::control`], which a run
/// without a socket has none of, and `client` needs somewhere it can always
/// reach.
#[derive(Resource, PartialEq, Clone, Debug)]
pub struct Toggles {
    /// `client stats` — whether the readout itself is on screen. The one switch
    /// here that changes nothing about the picture, so the last line never
    /// mentions it: a readout that is visible has already admitted to being
    /// on.
    pub stats: bool,
    /// `client shadows` — off, and the sun stops casting. Only what moves is
    /// in the pass now — the boat, the plants, the player, the beasts; the
    /// terrain wears its own baked shadow and does not answer to this. So
    /// what the switch shows is the cast half alone, which is exactly what
    /// makes it worth having: whether a hull is sitting on its shadow or
    /// floating over the ground is hard to see until the shadow goes.
    pub shadows: bool,
    /// `client haze` — off, and the aerial haze comes away, so what the
    /// distant ground is actually doing can be seen.
    pub haze: bool,
    /// `client wireframe` — every triangle drawn as lines, which is how the
    /// fixed 8,192 a chunk carries stops being a number and becomes a
    /// picture.
    pub wireframe: bool,
    /// `client resolution` — how tall a picture `shot` writes is, in rows off
    /// [`crate::settings::LADDER`]; the width follows from
    /// [`crate::settings::WIDESCREEN`], there being no display to take a
    /// shape from. `None` in a run that has a window, where a picture is the
    /// window's own size and there is nothing here to choose — which is why
    /// it is an `Option` rather than a number a windowed run would carry and
    /// never read.
    pub resolution: Option<u32>,
}

impl Default for Toggles {
    fn default() -> Self {
        Self {
            stats: false,
            shadows: true,
            haze: true,
            wireframe: false,
            // A window until [`crate::control::ControlPlugin`] says
            // otherwise, that being the only thing in the game that knows
            // whether this run has one.
            resolution: None,
        }
    }
}

/// One of the boolean switches, as everything that needs to know about them
/// reads it: what it is called, how to reach it in [`Toggles`], and whether a
/// departure from its default is worth confessing on the readout.
///
/// A table because the alternative was five copies of the same list of names
/// — the console's completion, its listing, its reader, its writer and the
/// line below — with nothing keeping them in step, so a switch added to one
/// of them tab-completed to a variable that did not exist, or existed and
/// could not be completed. A switch is now a row.
pub struct Switch {
    pub name: &'static str,
    pub of: fn(&mut Toggles) -> &mut bool,
    /// `stats` is the one that says no: it changes nothing about the picture,
    /// and a readout that is visible has already admitted to being on.
    confessed: bool,
}

pub const SWITCHES: [Switch; 4] = [
    Switch {
        name: "stats",
        of: |toggles| &mut toggles.stats,
        confessed: false,
    },
    Switch {
        name: "shadows",
        of: |toggles| &mut toggles.shadows,
        confessed: true,
    },
    Switch {
        name: "haze",
        of: |toggles| &mut toggles.haze,
        confessed: true,
    },
    Switch {
        name: "wireframe",
        of: |toggles| &mut toggles.wireframe,
        confessed: true,
    },
];

/// The variables that are not switches. They stay out of [`SWITCHES`]
/// because that table carries `confessed` for [`Toggles::line`], which walks
/// booleans and would have to unwrap a kind to do it; what they join instead
/// is [`Picture::variable`], which is what the grammar asks.
const RESOLUTION: &str = "resolution";
const ZOOM: &str = "zoom";
const YAW: &str = "yaw";
const POSITION: &str = "position";

/// What a `client` line reaches: the switches this machine draws by, and the
/// view it draws through.
///
/// The view is here rather than in [`Toggles`] because it is not this
/// module's to hold — the camera is [`crate::camera`]'s, and a `zoom` typed
/// at the console is the same zoom the mouse wheel does. What makes it a
/// `client` variable all the same is the rule the word stands for: it is this
/// machine's picture and nobody else's, which is exactly what separates these
/// lines from the ones that cross the wire.
pub struct Picture<'a> {
    pub toggles: &'a mut Toggles,
    /// `None` where there is no world being looked at — every screen but the
    /// helm, which is a refusal to read rather than a variable nobody has.
    /// See [`Machine::afloat`], which is what decides it: the camera outlives
    /// every world, so a reading off it on a menu would be a number about
    /// nowhere.
    pub looking: Option<Looking<'a>>,
}

/// Where this machine is looking from, as a `client` line reaches it.
///
/// Two things, because a view is written in one place and true in another.
/// The camera is what is actually drawn — eased, grounded on the surface the
/// player rides, and what the readout prints — so it is what a reading has to
/// come off. [`View`] is what a camera is spawned and recentred from, so a
/// line that moved the view has to move it too, or entering a world would put
/// back a view somebody had typed their way out of.
pub struct Looking<'a> {
    pub view: &'a mut View,
    pub camera: &'a mut MapCamera,
}

impl Looking<'_> {
    /// The view as it stands, which is the camera's own.
    pub fn now(&self) -> View {
        View {
            focus: self.camera.focus,
            distance: self.camera.distance,
            yaw: self.camera.yaw,
        }
    }

    /// Puts the view where a line asked for it. Only ever the *view* — where
    /// the player is is the server's to say, and the camera follows whatever
    /// carries them of its own accord.
    pub fn look(&mut self, wanted: View) {
        *self.view = wanted;
        self.camera.snap_to(wanted);
    }
}

/// What a variable holds, for [`Picture::variable`] to answer with.
///
/// The *wording* of each is [`crate::console`]'s, which owns the grammar —
/// this only says which kind the name found, so the console can read and
/// write every variable by asking once instead of testing for each kind
/// everywhere it walks them.
pub enum Value<'a> {
    Switch(&'a mut bool),
    /// `None` in a run with a window, which has no off-screen picture to
    /// size — see [`Toggles::resolution`].
    Rows(&'a mut Option<u32>),
    /// One of the view's own, and the view to read or move — `None` on a
    /// screen with no world under it. One variant for the three of them so
    /// that *there is no view here* is worded once rather than three times.
    Looking(Look, Option<Looking<'a>>),
}

/// Which of the view's variables a name found.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Look {
    Zoom,
    Yaw,
    /// Reads and does not turn. Where somebody is is not a dial: they get
    /// there by sailing, by walking, or by being taken — see the server's
    /// `goto`, which is one of those ways and not the definition.
    Position,
}

/// Which kind a variable is, told without a [`Picture`] to hand.
///
/// [`Value`] answers the same question by handing over the thing itself,
/// which is what reading and writing want and what completing a line cannot
/// use: tab is offered while a player types, against no particular state, and
/// what it needs to know is that `on` and `off` stand after a switch and the
/// ladder's rungs after `resolution`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Switch,
    Rows,
    /// A number of the player's own, or nothing at all: the view's variables
    /// take metres and degrees, and `position` takes nothing.
    Looking,
}

/// This machine, as a system reaches it: everything a `client` line can read
/// or move. One parameter rather than three because [`Picture`] wants all
/// three together, and both mouths that speak the grammar — the keyboard and
/// the socket — would otherwise carry the same three and build the same
/// thing.
#[derive(SystemParam)]
pub struct Machine<'w, 's> {
    pub toggles: ResMut<'w, Toggles>,
    view: ResMut<'w, View>,
    cameras: Query<'w, 's, &'static mut MapCamera>,
    /// Whether there is a world being looked at.
    ///
    /// The camera is spawned at startup and never despawned, so its existing
    /// says nothing about whether it is showing anybody anything: on a menu
    /// screen it is still pointed wherever it was last left. Without this a
    /// `client position` typed at the title screen would answer `0 0` — a
    /// place, in the words a place is given, for a player who is nowhere.
    ///
    /// `None` in a run with no states at all, which is a test harness rather
    /// than a screen, and is read as no world for the same reason.
    afloat: Option<Res<'w, State<AppState>>>,
    /// The world this machine is serving itself, where it is serving one. Only
    /// for [`Machine::stamp`] — see [`server::Host::seed`] for why a guest has
    /// none to read.
    hosting: Option<Res<'w, Hosting>>,
}

/// The same reading as [`Machine::stamp`], for a caller that only wants to
/// *read* it — which is [`crate::shots`], on the frame a key goes down.
///
/// A parameter of its own rather than a method on [`Machine`], because a
/// system's access is declared for the whole run and not for the frames it
/// does anything: taking `Machine` to stamp a picture would hold the switches,
/// the view and every camera against the systems that move them, on every
/// frame of a game, to serve a key that is hardly ever pressed.
///
/// The two cannot be one parameter and cannot be held together — a system
/// carrying both would be asking for [`Toggles`] mutably and immutably at once
/// — so what they share is [`stamp`], which does the work for either.
#[derive(SystemParam)]
pub struct Stamp<'w, 's> {
    toggles: Res<'w, Toggles>,
    cameras: Query<'w, 's, &'static MapCamera>,
    /// Read exactly as [`Machine::afloat`] reads it, and for the same reason.
    afloat: Option<Res<'w, State<AppState>>>,
    hosting: Option<Res<'w, Hosting>>,
}

impl Stamp<'_, '_> {
    /// What a picture taken now is stamped with — see [`stamp`].
    pub fn text(&self) -> Option<String> {
        let view = afloat(self.afloat.as_deref())
            .then(|| self.cameras.single().ok())
            .flatten()?;
        stamp(
            self.hosting.as_ref().map(|hosting| hosting.0.seed()),
            Some(View {
                focus: view.focus,
                distance: view.distance,
                yaw: view.yaw,
            }),
            &self.toggles,
        )
    }
}

/// What a picture of a view is stamped with — see [`crate::shots`], which is
/// what puts it in the file.
///
/// The overlay's own last two lines, and deliberately not a second account of
/// them: the words that would stand somebody here again are [`view_line`]'s,
/// and what admits to a doctored picture is [`Toggles::line`]'s. A stamp
/// saying it its own way would be a fact written twice, and the one nobody is
/// looking at is the one that rots.
///
/// `None` where there is no world being looked at, a menu screen being a
/// picture of nowhere: better no stamp than a place for a player who is not
/// anywhere.
fn stamp(seed: Option<u32>, view: Option<View>, toggles: &Toggles) -> Option<String> {
    Some(
        std::iter::once(view_line(seed, view?))
            .chain(toggles.line())
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// Whether there is a world under the camera — see [`Machine::afloat`], whose
/// question this is, asked where both parameters can reach it.
fn afloat(state: Option<&State<AppState>>) -> bool {
    state.is_some_and(|state| *state.get() == AppState::InWorld)
}

impl Machine<'_, '_> {
    /// This machine's picture, as a `client` line reaches it.
    pub fn picture(&mut self) -> Picture<'_> {
        let camera = self
            .afloat()
            .then(|| self.cameras.single_mut().ok())
            .flatten();
        Picture {
            toggles: &mut self.toggles,
            looking: camera.map(|camera| Looking {
                view: &mut self.view,
                camera: camera.into_inner(),
            }),
        }
    }

    /// The view as it stands, or `None` where there is no world being looked
    /// at — what a line that may have moved the picture is measured against.
    /// The camera's own rather than the [`View`], for the reason [`Looking`]
    /// gives.
    pub fn seen(&self) -> Option<View> {
        if !self.afloat() {
            return None;
        }
        self.cameras.single().ok().map(|camera| View {
            focus: camera.focus,
            distance: camera.distance,
            yaw: camera.yaw,
        })
    }

    /// What a picture of this view is stamped with — see [`stamp`], which is
    /// the whole of it. Here as well as on [`Stamp`] because the socket's
    /// `shot` already carries a [`Machine`] and cannot carry both.
    pub fn stamp(&self) -> Option<String> {
        stamp(
            self.hosting.as_ref().map(|hosting| hosting.0.seed()),
            self.seen(),
            &self.toggles,
        )
    }

    /// Whether there is a world under the camera — see [`afloat`].
    fn afloat(&self) -> bool {
        afloat(self.afloat.as_deref())
    }
}

impl<'a> Picture<'a> {
    /// The switches alone, for a caller with no camera to offer — the tests,
    /// and any screen the console can be reached from that has no view.
    pub fn of(toggles: &'a mut Toggles) -> Self {
        Self {
            toggles,
            looking: None,
        }
    }

    /// The variable a name asks for, whatever kind it holds, or `None` for a
    /// name that is not one.
    pub fn variable(&mut self, name: &str) -> Option<Value<'_>> {
        let look = match name {
            ZOOM => Look::Zoom,
            YAW => Look::Yaw,
            POSITION => Look::Position,
            RESOLUTION => return Some(Value::Rows(&mut self.toggles.resolution)),
            _ => return self.toggles.switch(name).map(Value::Switch),
        };
        // Reborrowed rather than handed over, so the answer borrows this
        // picture for as long as it is used and no longer.
        let looking = self.looking.as_mut().map(|it| Looking {
            view: &mut *it.view,
            camera: &mut *it.camera,
        });
        Some(Value::Looking(look, looking))
    }

    /// Every variable there is, in the order a bare `client` lists them, each
    /// with the kind it holds — here rather than in the console because this
    /// is where they live, and a list kept beside the grammar would be a
    /// second place to add one.
    pub fn every() -> impl Iterator<Item = (&'static str, Kind)> {
        SWITCHES
            .iter()
            .map(|switch| (switch.name, Kind::Switch))
            .chain([
                (RESOLUTION, Kind::Rows),
                (ZOOM, Kind::Looking),
                (YAW, Kind::Looking),
                (POSITION, Kind::Looking),
            ])
    }

    /// Every variable's name, for the listing and the refusal that offer them.
    pub fn names() -> impl Iterator<Item = &'static str> {
        Self::every().map(|(name, _)| name)
    }

    /// What a name holds, or `None` for a name that is not a variable at all.
    pub fn kind(name: &str) -> Option<Kind> {
        Self::every()
            .find(|(it, _)| *it == name)
            .map(|(_, kind)| kind)
    }
}

impl Toggles {
    /// The boolean switch a name asks for, or `None` where the name is not
    /// one of them.
    fn switch(&mut self, name: &str) -> Option<&mut bool> {
        let switch = SWITCHES.iter().find(|switch| switch.name == name)?;
        Some((switch.of)(self))
    }

    /// How the overlay owns up to being in a doctored state — `None` when
    /// nothing has been touched, which is the usual case and costs the readout
    /// no line at all.
    ///
    /// It has to say. The module above promises that a screenshot of this
    /// overlay is the whole of what it takes to stand here again, and a
    /// picture taken with the haze stripped off would quietly break that
    /// promise in exactly the place it gets used: an argument about how the
    /// renderer should be set up.
    ///
    /// What it says is the *departure*, which each switch's own default
    /// decides the wording of: one that is normally on reads as "no haze"
    /// when it is off, one that is normally off reads as its own name when it
    /// is on.
    fn line(&self) -> Option<String> {
        let (mut mine, mut usual) = (self.clone(), Self::default());
        let mut on = Vec::new();
        for switch in &SWITCHES {
            let (set, default) = (*(switch.of)(&mut mine), *(switch.of)(&mut usual));
            if switch.confessed && set != default {
                on.push(if set {
                    switch.name.to_string()
                } else {
                    format!("no {}", switch.name)
                });
            }
        }
        (!on.is_empty()).then(|| format!("debug: {}", on.join(" / ")))
    }
}

/// What the readout can say about the world under the picture: how much of it
/// this machine holds, and what time it is there. Together because it is one
/// question — a menu screen has neither a world to count nor an hour to be at.
#[derive(SystemParam)]
struct WorldUnder<'w> {
    ground: Option<Res<'w, Ground>>,
    chart: Option<Res<'w, Chart>>,
    sky: Res<'w, crate::sky::Sky>,
}

/// What this machine holds of the world: the ground a server has sent it, and
/// the coast it has drawn out of that ground.
///
/// One argument rather than two because they are one subject — how much world
/// is in hand — and because the readout's own line is long enough already.
#[derive(Clone, Copy, Default)]
struct Held<'a> {
    ground: Option<&'a Tally>,
    /// What the chart holds, counted — see [`Chart::tally`].
    charted: Option<SurveyTally>,
}

/// Makes the scene agree with [`Toggles`].
///
/// Written as "set it to what it should be" rather than "change it when the
/// switch is thrown". The `is_changed` guard is only to keep it from writing
/// the same values every frame, which would have every camera register as
/// changed for anything else watching.
fn apply_toggles(
    mut commands: Commands,
    toggles: Res<Toggles>,
    wireframe: Option<ResMut<WireframeConfig>>,
    mut suns: Query<&mut DirectionalLight, With<crate::sky::SkyLight>>,
    cameras: Query<(Entity, Has<DistanceFog>), With<MapCamera>>,
) {
    if !toggles.is_changed() {
        return;
    }

    // The one writer of this field, which is why the switch can simply say
    // what it should be: two systems setting it would each undo the other on
    // alternate frames. The light is hung casting — see
    // [`crate::sky::hang_the_light`] — and this only doctors it.
    for mut sun in &mut suns {
        sun.shadow_maps_enabled = toggles.shadows;
    }

    if let Some(mut wireframe) = wireframe {
        wireframe.global = toggles.wireframe;
        // Dark lines. The default is white, which disappears against sand and
        // surf — the two places the mesh is most worth looking at.
        wireframe.default_color = Color::srgb(0.05, 0.05, 0.05);
    }

    for (camera, has_fog) in &cameras {
        match (toggles.haze, has_fog) {
            (false, true) => {
                commands.entity(camera).remove::<DistanceFog>();
            }
            (true, false) => {
                commands.entity(camera).insert(crate::camera::haze());
            }
            _ => {}
        }
    }
}

fn spawn_overlay(mut commands: Commands) {
    commands
        .spawn((
            Name::new("Debug overlay"),
            DebugPanel,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(8.0),
                left: Val::Px(8.0),
                padding: UiRect::axes(Val::Px(10.0), Val::Px(6.0)),
                ..default()
            },
            BackgroundColor(BACKDROP),
            // The menus are full-screen panels spawned later; without this the
            // readout would sit underneath them.
            GlobalZIndex(1),
        ))
        .with_children(|panel| {
            panel.spawn((
                DebugText,
                Text::new(""),
                TextFont {
                    font_size: FontSize::Px(14.0),
                    ..default()
                },
                TextColor(TEXT),
            ));
        });
}

/// The overlay's two entities, as the refresh reaches them — the panel that
/// shows or hides and the text that says everything — and the clock that says
/// whether this frame is one of the four a second that print.
#[derive(SystemParam)]
struct Overlay<'w, 's> {
    panels: Query<'w, 's, &'static mut Visibility, With<DebugPanel>>,
    texts: Query<'w, 's, &'static mut Text, With<DebugText>>,
    time: Res<'w, Time>,
    due: ResMut<'w, NextRefresh>,
}

fn refresh_overlay(
    diagnostics: Res<DiagnosticsStore>,
    scene: Scene,
    toggles: Res<Toggles>,
    world: WorldUnder,
    hosting: Option<Res<Hosting>>,
    cameras: Query<&MapCamera>,
    mut overlay: Overlay,
) {
    for mut visibility in &mut overlay.panels {
        *visibility = if toggles.stats {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
    // Hidden is hidden: the counting below walks every visible mesh, and a
    // readout nobody can see should cost what it shows — nothing.
    if !toggles.stats {
        // Left due, so turning it back on prints at once rather than after a
        // wait measured from whenever it was last on.
        overlay.due.0 = 0.0;
        return;
    }

    // And a readout that is on still only costs four times a second — see
    // [`REFRESH_SECONDS`]. The visibility above is settled every frame; it is
    // the counting and the printing that wait.
    let now = overlay.time.elapsed_secs();
    if now < overlay.due.0 {
        return;
    }
    overlay.due.0 = now + REFRESH_SECONDS;

    let fps = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|fps| fps.smoothed());

    let counts = scene.counts();

    // What this machine has of the world, and what it is still waiting for.
    // Absent outside a match, where the readout has no world to count.
    let tally = world.ground.as_ref().map(|ground| ground.tally());

    // And what time it is there. On the same condition, the hour being the
    // world's rather than the app's: a menu screen is at no time at all.
    let hour = tally.is_some().then(|| world.sky.phase());

    let view = cameras.single().ok().map(|camera| View {
        focus: camera.focus,
        distance: camera.distance,
        yaw: camera.yaw,
    });

    // Only a world made on this machine can be named. A guest is sent ground
    // and never the recipe, so there is no seed here to print.
    let seed = hosting.map(|hosting| hosting.0.seed());

    let held = Held {
        ground: tally.as_ref(),
        charted: world.chart.as_ref().map(|chart| chart.tally()),
    };

    for mut text in &mut overlay.texts {
        text.0 = overlay_text(fps, &counts, held, hour, seed, view, &toggles);
    }
}

/// Everything drawn this frame, from the camera's point of view.
///
/// The count is read off a cull somebody else has already done — the camera's
/// [`VisibleEntities`] — rather than worked out again here, which means a
/// wrong number here would mean a wrong picture too, and that is the kind of
/// wrong that gets noticed.
///
/// The list is built in `PostUpdate` and read here in `Update`, so it is a
/// frame old — near enough for a readout refreshed four times a second.
#[derive(SystemParam)]
struct Scene<'w, 's> {
    meshes: Res<'w, Assets<Mesh>>,
    drawn: Query<'w, 's, &'static Mesh3d>,
    cameras: Query<'w, 's, &'static VisibleEntities, With<MapCamera>>,
}

/// The geometry line of the readout: what the eye is given.
struct Counts {
    triangles: usize,
    meshes: usize,
}

impl Scene<'_, '_> {
    fn counts(&self) -> Counts {
        let mut counts = Counts {
            triangles: 0,
            meshes: 0,
        };
        for camera in &self.cameras {
            for entity in camera.iter(TypeId::of::<Mesh3d>()) {
                let Some(mesh) = self.mesh_of(*entity) else {
                    continue;
                };
                counts.triangles += triangles_in(mesh);
                counts.meshes += 1;
            }
        }
        counts
    }

    /// The mesh an entity draws, if it has one and it has landed — an asset
    /// still loading draws nothing, so it counts as nothing.
    fn mesh_of(&self, entity: Entity) -> Option<&Mesh> {
        self.drawn
            .get(entity)
            .ok()
            .and_then(|handle| self.meshes.get(&handle.0))
    }
}

/// Triangles a mesh draws. An index buffer overrules the vertex buffer
/// wherever there is one, which is everywhere the terrain and the water are —
/// a mesh with no indices is one whose vertices are its triangles, three at a
/// time.
fn triangles_in(mesh: &Mesh) -> usize {
    match mesh.indices() {
        Some(indices) => indices.len() / 3,
        None => mesh.count_vertices() / 3,
    }
}

/// What the readout says. The frame rate has no value at all for the first
/// frames, before the diagnostic has anything to average.
///
/// One line per subject, slash-separated within it, so the block stays four
/// lines however much it has to say. Every number carries its own noun —
/// nothing is positional — because the lines come and go with what exists to
/// count.
///
/// The chunk line is three numbers about two different things:
///
/// - the **total** is every chunk this machine has an answer about, of which
///   **ocean** is the share answered with nothing — sea costs nothing to hold,
///   so the rest is where the memory went;
/// - **requested** is ground asked for and not answered, deliberately outside
///   the total and the only number here about the *server*.
///
/// There is no count of meshes still being assembled, though there is a stage
/// for it: it is the moment between a payload landing and its vertex buffers
/// being filled, which an arrival's worth of chunks passes through in about six
/// frames. The failure it would have caught shows up as the count above this
/// one standing still while the chunks climb.
fn overlay_text(
    fps: Option<f64>,
    counts: &Counts,
    held: Held<'_>,
    hour: Option<f32>,
    seed: Option<u32>,
    view: Option<View>,
    toggles: &Toggles,
) -> String {
    let fps = fps.map_or_else(|| "--".to_string(), |fps| format!("{fps:.0}"));
    let mut lines = vec![format!(
        "{fps} fps / {} meshes / {} triangles",
        counts.meshes,
        thousands(counts.triangles)
    )];
    if let Some(tally) = held.ground {
        lines.push(format!(
            "{} chunks / {} ocean / {} requested",
            tally.ground + tally.ocean,
            tally.ocean,
            tally.requested
        ));
    }
    // Under the chunks, because it is drawn out of them and grows behind them.
    // The last number is the one gameplay hangs off: closed coastlines ringing
    // land big enough to letter — see `protocol::survey::SurveyTally`.
    if let Some(chart) = held.charted {
        lines.push(format!(
            "{} surveyed / {} with coast / {} closed / {} landmasses",
            chart.surveyed, chart.coastal, chart.complete, chart.landmasses
        ));
    }
    if let Some(hour) = hour {
        // As a clock rather than as the fraction the wire carries: what the
        // eye is checking this against is a sky, and a sky reads as an hour.
        lines.push(format!("sky {}", protocol::clock(hour)));
    }
    if let Some(view) = view {
        lines.push(view_line(seed, view));
    }
    // Last, so it reads as a footnote against everything above it rather than
    // as another thing being counted.
    if let Some(doctored) = toggles.line() {
        lines.push(doctored);
    }
    lines.join("\n")
}

/// The world and the view in the terms that take them back in: `--seed` on
/// the command line, the console's `goto`, and then its `client yaw` and
/// `client zoom` — metres, degrees and metres. Written as the *arguments* are
/// typed, so that a picture of this line is enough to stand here again; the
/// console will also say all four back on request, which is the same numbers
/// through a channel something other than an eye can read. The yaw runs
/// unbounded on the camera — easing never wants to wrap — so it is folded to
/// a bearing here.
///
/// The seed leads because it is the part that cannot be guessed from the
/// picture, and it is absent in somebody else's world: a guest can say where
/// it stood but not which world it stood in, that never having crossed the
/// wire.
fn view_line(seed: Option<u32>, view: View) -> String {
    let mut parts = Vec::new();
    if let Some(seed) = seed {
        parts.push(format!("seed {seed}"));
    }
    parts.push(format!("goto {:.0} {:.0}", view.focus.x, view.focus.z));
    parts.push(format!(
        "yaw {:.0}",
        view.yaw.to_degrees().rem_euclid(360.0)
    ));
    parts.push(format!("zoom {:.0}", view.distance));
    parts.join(" / ")
}

/// `1234567` -> `1,234,567`, since triangle counts run to seven digits.
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use bevy::asset::RenderAssetUsages;
    use bevy::mesh::{Indices, PrimitiveTopology};
    use bevy::time::TimeUpdateStrategy;

    /// A chunk of flat ground, which is all this needs of one: the readout
    /// counts chunks, it does not look at them.
    fn a_chunk() -> protocol::ChunkPayload {
        use protocol::ground::{quantize, Material, CELL_COUNT, CORNERS, LIT_ALL_DAY};
        protocol::ChunkPayload {
            heights: vec![quantize(1.0); CORNERS * CORNERS],
            materials: vec![Material::Grass; CELL_COUNT],
            lit: vec![LIT_ALL_DAY; CORNERS * CORNERS],
            water: None,
            plants: Vec::new(),
        }
    }

    /// A mesh of `vertices` positions and no index buffer.
    fn unindexed(vertices: usize) -> Mesh {
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0f32, 0.0, 0.0]; vertices])
    }

    /// The switches with the readout on, which is what most of these tests
    /// are looking at. Not what a run starts with — `client stats on` is.
    fn showing() -> Toggles {
        Toggles {
            stats: true,
            ..default()
        }
    }

    #[test]
    fn an_unindexed_mesh_counts_by_its_vertices() {
        assert_eq!(triangles_in(&unindexed(6)), 2);
    }

    #[test]
    fn an_indexed_mesh_counts_by_its_indices() {
        // Four vertices drawn as two triangles, the way the sea plane is.
        let mesh = unindexed(4).with_inserted_indices(Indices::U32(vec![0, 1, 2, 0, 2, 3]));
        assert_eq!(triangles_in(&mesh), 2);
    }

    #[test]
    fn the_readout_puts_each_subject_on_its_own_line() {
        let view = View {
            focus: Vec3::new(98.4, 3.0, -316.7),
            distance: 42.4,
            yaw: 45f32.to_radians(),
        };
        // 173 of ground and 58 of water make the 231; the 12 requested are
        // not in it, having been answered with nothing yet.
        let tally = Tally {
            ground: 173,
            ocean: 58,
            requested: 12,
        };
        let counts = Counts {
            triangles: 1_234_567,
            meshes: 214,
        };
        assert_eq!(
            overlay_text(
                Some(59.6),
                &counts,
                Held {
                    ground: Some(&tally),
                    charted: Some(SurveyTally {
                        surveyed: 96,
                        coastal: 21,
                        complete: 3,
                        open: 2,
                        landmasses: 1,
                    }),
                },
                Some(0.35),
                Some(20_040_112),
                Some(view),
                &showing()
            ),
            "60 fps / 214 meshes / 1,234,567 triangles\n\
             231 chunks / 58 ocean / 12 requested\n\
             96 surveyed / 21 with coast / 3 closed / 1 landmasses\n\
             sky 08:24\n\
             seed 20040112 / goto 98 -317 / yaw 45 / zoom 42"
        );
    }

    #[test]
    fn the_readout_survives_having_no_frame_rate_yet_and_no_world() {
        // On a menu screen there is no world to count and no camera to
        // describe — so the readout falls back to its one line that is true
        // anywhere.
        let counts = Counts {
            triangles: 0,
            meshes: 0,
        };
        assert_eq!(
            overlay_text(None, &counts, Held::default(), None, None, None, &showing()),
            "-- fps / 0 meshes / 0 triangles"
        );
    }

    #[test]
    fn the_view_line_folds_the_yaw_to_a_bearing() {
        // The camera's yaw runs unbounded, but the line has to say something
        // `yaw` would read back as the same view.
        let at = |yaw: f32| View {
            focus: Vec3::ZERO,
            distance: 100.0,
            yaw: yaw.to_radians(),
        };
        assert_eq!(
            view_line(Some(7), at(-90.0)),
            "seed 7 / goto 0 0 / yaw 270 / zoom 100"
        );
        assert_eq!(
            view_line(Some(7), at(450.0)),
            "seed 7 / goto 0 0 / yaw 90 / zoom 100"
        );
    }

    #[test]
    fn a_guests_view_line_names_no_world() {
        // In somebody else's world there is no seed to give: the wire carries
        // ground, not the recipe. Where the player stands is still worth
        // saying.
        assert_eq!(
            view_line(
                None,
                View {
                    focus: Vec3::new(98.4, 3.0, -316.7),
                    distance: 150.0,
                    yaw: 0.0,
                }
            ),
            "goto 98 -317 / yaw 0 / zoom 150"
        );
    }

    /// A stamp is not a second account of the view: it is the overlay's own
    /// last lines, the ones a driver would type back. This holds it to that —
    /// the words the readout draws in the corner and the words that go into
    /// the file are one string, doctoring confessed and all.
    #[test]
    fn a_stamp_is_the_overlays_own_words() {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .insert_state(AppState::InWorld)
            .init_resource::<View>()
            // Haze off, so the stamp has something to own up to: a picture
            // taken with the far ground stripped bare has to say so wherever
            // it says anything at all.
            .insert_resource(Toggles {
                haze: false,
                ..default()
            });
        let host = server::Server::bind(("127.0.0.1", 0), 4242)
            .expect("a server should bind")
            .spawn()
            .expect("a server should serve");
        app.insert_resource(Hosting(host));
        app.world_mut().spawn(MapCamera::looking(View {
            focus: Vec3::new(480.0, 0.0, -1200.0),
            distance: 240.0,
            yaw: std::f32::consts::FRAC_PI_2,
        }));

        let said = "seed 4242 / goto 480 -1200 / yaw 90 / zoom 240\ndebug: no haze";
        assert_eq!(machines_stamp(&mut app).as_deref(), Some(said));
        // And the read-only parameter the key takes says it too. Two ways in
        // because a system carrying `Machine` cannot also carry `Stamp` — see
        // [`Stamp`] — which is exactly the arrangement that could drift, so
        // both are asked the same question here.
        assert_eq!(stamps_text(&mut app).as_deref(), Some(said));
    }

    /// A picture of a menu screen is a picture of nowhere, and says so by
    /// saying nothing — see [`Machine::stamp`].
    #[test]
    fn a_picture_of_nowhere_is_stamped_with_nothing() {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .insert_state(AppState::MainMenu)
            .init_resource::<View>()
            .init_resource::<Toggles>();
        // A camera all the same, pointed wherever it was last left: its
        // existing is what the stamp must not read as a place.
        app.world_mut().spawn(MapCamera::default());

        assert_eq!(machines_stamp(&mut app), None);
        assert_eq!(stamps_text(&mut app), None);
    }

    /// The stamp as the socket's `shot` reaches it, carrying a [`Machine`].
    fn machines_stamp(app: &mut App) -> Option<String> {
        app.world_mut()
            .run_system_cached(|machine: Machine| machine.stamp())
            .expect("the stamp to be read")
    }

    /// And as the key reaches it, carrying only [`Stamp`].
    fn stamps_text(app: &mut App) -> Option<String> {
        app.world_mut()
            .run_system_cached(|stamp: Stamp| stamp.text())
            .expect("the stamp to be read")
    }

    #[test]
    fn the_overlay_spawns_and_counts_the_scene() {
        // A headless app with just enough plumbing for the overlay's systems:
        // time and a frame count for the FPS diagnostic, and a mesh store for
        // the triangle count. No renderer — the overlay is text either way.
        let mut app = App::new();
        app.add_plugins((
            bevy::time::TimePlugin,
            bevy::diagnostic::FrameCountPlugin,
            DebugOverlayPlugin,
        ))
        .insert_resource(Assets::<Mesh>::default())
        .insert_resource(showing());

        // Two meshes of two triangles each — and only one of them in shot.
        // Both are spawned, so a readout that counted what exists rather than
        // what is drawn would say four triangles here.
        let handle = app
            .world_mut()
            .resource_mut::<Assets<Mesh>>()
            .add(unindexed(6));
        let drawn = app.world_mut().spawn(Mesh3d(handle.clone())).id();
        app.world_mut().spawn(Mesh3d(handle));

        // A world holding one chunk of ground and one of open water — a mesh
        // and a fact, which is the distinction the chunk line exists to draw,
        // and which is why that line stays about what is *held* while the two
        // above it are about what is drawn.
        let mut ground = Ground::default();
        ground.deliver(IVec2::ZERO, None, Some(a_chunk()));
        ground.deliver(IVec2::new(1, 0), None, None);
        app.insert_resource(ground);

        // And a world of this machine's own behind it, which is what the seed
        // is read off. Bound and never accepted from: the readout asks the
        // handle which world it is, and nothing here has to join it.
        let host = server::Server::bind(("127.0.0.1", 0), 4242)
            .expect("a server should bind")
            .spawn()
            .expect("a server should serve");
        app.insert_resource(Hosting(host));

        // And a camera, for the view line and for the scene count — which is
        // its cull, so it is the camera that decides what the geometry line
        // says. Filled by hand here: `check_visibility` belongs to the render
        // plugins, and this app has none.
        let mut seen = VisibleEntities::default();
        seen.push(drawn, TypeId::of::<Mesh3d>());
        app.world_mut().spawn((
            MapCamera::looking(View {
                focus: Vec3::new(10.0, 0.0, -20.0),
                distance: 150.0,
                yaw: 0.0,
            }),
            seen,
        ));

        // The first update spawns the overlay; the rest give the FPS
        // diagnostic frames with a measurable delta, with real time between
        // them — back-to-back updates can land zero frame time, which the
        // diagnostic refuses to count.
        //
        // Then a wait past [`REFRESH_SECONDS`] and one more, because the
        // readout is worked out four times a second rather than every frame:
        // the frames above are what give the diagnostic something to say, and
        // this last one is the one that prints it.
        for _ in 0..4 {
            app.update();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        std::thread::sleep(std::time::Duration::from_secs_f32(REFRESH_SECONDS));
        app.update();

        let text = app
            .world_mut()
            .query_filtered::<&Text, With<DebugText>>()
            .single(app.world())
            .expect("the overlay never spawned")
            .0
            .clone();
        assert!(
            text.ends_with(
                "1 meshes / 2 triangles\n\
                 2 chunks / 1 ocean / 0 requested\n\
                 sky 08:24\n\
                 seed 4242 / goto 10 -20 / yaw 0 / zoom 150"
            ),
            "overlay reads: {text}"
        );
        assert!(!text.starts_with("-- fps"), "FPS never got a value: {text}");
    }

    /// One frame's worth of time, handed to the app rather than waited out —
    /// see the test below. Deliberately not a divisor of [`REFRESH_SECONDS`],
    /// so no frame lands on the exact moment the readout is due and nothing
    /// here rests on two floats coming out equal.
    const FRAME: Duration = Duration::from_millis(8);

    #[test]
    fn the_readout_is_worked_out_four_times_a_second_and_not_every_frame() {
        // The line is printed in whole figures, so a frame rate sitting
        // between two of them would rewrite it every frame and read as a
        // flicker — in a readout whose whole job is to say whether the frame
        // rate is steady. Working it out also walks every visible mesh and
        // every cascade's, which is a cost the frame should not pay for being
        // told how long it took.
        //
        // The app is given its time a fixed step at a time rather than left
        // to read the wall clock, so which side of the interval a frame falls
        // on is arithmetic. A readout refreshed four times a second cannot
        // otherwise be tested in under a second of anybody's patience without
        // asserting that the machine got through a stretch of frames quickly
        // enough, which is a claim about the machine and not about the code.
        let mut app = App::new();
        app.add_plugins((
            bevy::time::TimePlugin,
            bevy::diagnostic::FrameCountPlugin,
            DebugOverlayPlugin,
        ))
        .insert_resource(Assets::<Mesh>::default())
        .insert_resource(TimeUpdateStrategy::ManualDuration(FRAME));
        app.world_mut().resource_mut::<Toggles>().stats = true;
        let camera = app
            .world_mut()
            .spawn((
                MapCamera::looking(View {
                    focus: Vec3::ZERO,
                    distance: 100.0,
                    yaw: 0.0,
                }),
                VisibleEntities::default(),
            ))
            .id();

        let printed = |app: &mut App| {
            app.world_mut()
                .query_filtered::<&Text, With<DebugText>>()
                .single(app.world())
                .expect("the overlay never spawned")
                .0
                .clone()
        };

        // The first update prints, because a run that has just turned stats
        // on should not wait a quarter second to see anything. The frame rate
        // is not a number yet: the diagnostic's measurements land at the end
        // of the schedule that took them, so the first frame has none to read.
        app.update();
        let first = printed(&mut app);
        assert_eq!(
            first,
            "-- fps / 0 meshes / 0 triangles\ngoto 0 0 / yaw 0 / zoom 100"
        );

        // Give the readout something it would say differently, so that the
        // frames below are silent because the readout waited and not because
        // there was no news. The view moves, and the frame rate becomes a
        // number — a steady one, every frame being [`FRAME`] long.
        app.world_mut()
            .get_mut::<MapCamera>(camera)
            .expect("the camera")
            .focus = Vec3::new(40.0, 0.0, -60.0);

        // Frames inside the interval leave the line alone, however many there
        // are and however much has changed under them.
        let interval = Duration::from_secs_f32(REFRESH_SECONDS);
        let mut since = Duration::ZERO;
        while since + FRAME < interval {
            app.update();
            since += FRAME;
            assert_eq!(
                printed(&mut app),
                first,
                "the readout was rewritten inside its own refresh interval"
            );
        }

        // And the frame that carries the clock past the interval prints
        // again, in the new numbers — a readout that had simply stopped would
        // pass everything above.
        while since < interval {
            app.update();
            since += FRAME;
        }
        assert_eq!(
            printed(&mut app),
            "125 fps / 0 meshes / 0 triangles\ngoto 40 -60 / yaw 0 / zoom 100",
            "the readout stopped rather than slowed"
        );
    }

    #[test]
    fn the_readout_hides_until_stats_is_set() {
        // The panel exists either way — the console can turn it on at any
        // moment — but with `stats` off it is invisible and writes nothing.
        let mut app = App::new();
        app.add_plugins((
            bevy::time::TimePlugin,
            bevy::diagnostic::FrameCountPlugin,
            DebugOverlayPlugin,
        ))
        .insert_resource(Assets::<Mesh>::default());
        app.update();

        let hidden = app
            .world_mut()
            .query_filtered::<&Visibility, With<DebugPanel>>()
            .single(app.world())
            .expect("the overlay never spawned");
        assert_eq!(*hidden, Visibility::Hidden);

        app.world_mut().resource_mut::<Toggles>().stats = true;
        app.update();
        let shown = app
            .world_mut()
            .query_filtered::<&Visibility, With<DebugPanel>>()
            .single(app.world())
            .expect("the overlay never spawned");
        assert_eq!(*shown, Visibility::Inherited);
    }

    /// Nothing is said when nothing has been touched — the common case must
    /// not cost the readout a line. The readout being on is itself not a
    /// doctoring: it changes nothing about the picture under it.
    #[test]
    fn an_untouched_run_admits_to_nothing() {
        assert_eq!(Toggles::default().line(), None);
        assert_eq!(showing().line(), None);
    }

    #[test]
    fn a_doctored_run_says_so() {
        // Every switch at once, to pin the order and the separator as well as
        // the wording.
        let all = Toggles {
            stats: true,
            shadows: false,
            haze: false,
            wireframe: true,
            // Not a doctoring of the picture but the size of it, which a
            // picture cannot hide, so the line never mentions it.
            resolution: Some(1080),
        };
        assert_eq!(
            all.line().as_deref(),
            Some("debug: no shadows / no haze / wireframe")
        );
    }

    /// The switch reaches the light, both ways. `apply_toggles` is the one
    /// writer of the field — see there — so this is the whole of what the
    /// switch does, and without it a rename or a dropped query would leave
    /// `client shadows off` answering cheerfully and changing nothing.
    #[test]
    fn the_console_takes_the_sun_s_casting_away_and_gives_it_back() {
        let mut app = App::new();
        app.add_plugins((
            bevy::time::TimePlugin,
            bevy::diagnostic::FrameCountPlugin,
            DebugOverlayPlugin,
        ))
        .insert_resource(Assets::<Mesh>::default());

        // The light as `hang_the_light` hangs it: casting, for the things
        // that move.
        app.world_mut().spawn((
            crate::sky::SkyLight,
            DirectionalLight {
                shadow_maps_enabled: true,
                ..default()
            },
        ));
        app.update();
        assert!(sun_casts(&mut app), "the sun was hung not casting");

        app.world_mut().resource_mut::<Toggles>().shadows = false;
        app.update();
        assert!(!sun_casts(&mut app), "the console lost its own switch");

        app.world_mut().resource_mut::<Toggles>().shadows = true;
        app.update();
        assert!(sun_casts(&mut app), "the sun never came back");
    }

    fn sun_casts(app: &mut App) -> bool {
        app.world_mut()
            .query::<&DirectionalLight>()
            .iter(app.world())
            .next()
            .expect("a sun")
            .shadow_maps_enabled
    }

    #[test]
    fn thousands_are_grouped() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }
}
