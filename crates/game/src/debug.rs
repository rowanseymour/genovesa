//! The debug readout: what each pass draws, and the switches that change it.
//!
//! A small readout pinned to a corner of the window, over whatever screen the
//! app is on, a subject to a line. The frame rate comes from Bevy's own
//! frame-time diagnostics, already smoothed; the geometry under it is what the
//! camera actually draws, so it moves with the view as well as with the
//! streaming, and reads zero on the menu, where nothing 3D exists at all. The
//! shadow line is the same frame drawn again as the sun sees it, once per
//! cascade — a larger number than the one above it, and meant to be read
//! against it. The chunk line shows the streamer's own bookkeeping instead,
//! which is about what this machine *holds* rather than what it draws: how
//! much of the world it has, how much of that was open water, and how much
//! ground it is still waiting on. The last line reads the world and the view
//! back in the terms the command line takes them —
//! `--seed`, `--focus`, `--yaw`, `--zoom` — so a screenshot of the overlay is
//! the whole of what it takes to stand here again, in a `--shot` run or
//! otherwise.
//!
//! The switches are [`Toggles`], and they are set from the console — see
//! [`crate::console`], whose `set` lines are their only writer. They used to
//! be number keys; the console replaced them because a vocabulary outgrows a
//! number row, but reading a count and changing what is counted are still one
//! job, which is why the state stays in this module with the readout: the
//! reason to turn the shadows off is to watch the shadow line answer. When
//! any switch is away from the world's own state the readout says so on a
//! last line, since a doctored picture that did not admit it would be worth
//! less than no picture at all.

use std::any::TypeId;

use bevy::camera::visibility::{CascadesVisibleEntities, VisibleEntities};
use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::ecs::system::SystemParam;
use bevy::light::CascadeShadowConfig;
use bevy::pbr::wireframe::{WireframeConfig, WireframePlugin};
use bevy::pbr::DistanceFog;
use bevy::prelude::*;
use bevy::text::FontSize;

use crate::camera::{MapCamera, View};
use crate::chart::{Chart, ChartTally};
use crate::net::Hosting;
use crate::terrain::{Ground, Tally};

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
            .add_systems(Startup, spawn_overlay)
            .add_systems(Update, (apply_toggles, refresh_overlay).chain());
    }
}

/// Marks the overlay's panel, so showing and hiding it can find it.
#[derive(Component)]
struct DebugPanel;

/// Marks the overlay's one text block, so the refresh can find it.
#[derive(Component)]
struct DebugText;

/// The switches the console's `set` lines throw — see [`crate::console`],
/// which owns the grammar, while this module owns making them true of the
/// scene. All of them are about what *this machine* draws: nothing here
/// reaches the world or anybody else's picture of it, which is what
/// separates a `set` from the console's other language.
#[derive(Resource, PartialEq, Clone, Debug)]
pub struct Toggles {
    /// `set stats` — whether the readout itself is on screen. The one switch
    /// here that changes nothing about the picture, so the last line never
    /// mentions it: a readout that is visible has already admitted to being
    /// on.
    pub stats: bool,
    /// `set shadows` — off, and the sun stops casting. The shadow line
    /// disappears with it, Bevy clearing the cascade cull when a light's
    /// shadows are off, so the readout cannot claim work that is no longer
    /// being done.
    pub shadows: bool,
    /// `set haze` — off, and the aerial haze comes away, so what the distant
    /// ground is actually doing can be seen. Mostly worth having next to
    /// `set reach`: judging what a shorter shadow reach costs is impossible
    /// while the haze is hiding the far end of it.
    pub haze: bool,
    /// `set wireframe` — every triangle drawn as lines, which is how the
    /// fixed 8,192 a chunk carries stops being a number and becomes a
    /// picture.
    pub wireframe: bool,
    /// `set reach` — how far the sun's cascades go, in metres. The default
    /// is [`crate::HAZE_END`], where the haze has closed and nothing can be
    /// seen to lose its shadow; the question the switch exists to answer is
    /// whether a shorter reach is *visibly* worse, and walking it in until
    /// the far edge of the shadows sits in plain sight is where the answer
    /// is.
    pub reach: f32,
}

impl Default for Toggles {
    fn default() -> Self {
        Self {
            stats: false,
            shadows: true,
            haze: true,
            wireframe: false,
            reach: crate::HAZE_END,
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

/// The one variable that is not a switch — metres rather than on and off, so
/// it is a special case wherever the switches are walked rather than a row
/// that would have to carry a second kind of value.
pub const REACH: &str = "reach";

impl Toggles {
    /// The boolean switch a name asks for, or `None` where the name is not
    /// one of them.
    pub fn switch(&mut self, name: &str) -> Option<&mut bool> {
        let switch = SWITCHES.iter().find(|switch| switch.name == name)?;
        Some((switch.of)(self))
    }

    /// How the overlay owns up to being in a doctored state — `None` when
    /// nothing has been touched, which is the usual case and costs the readout
    /// no line at all.
    ///
    /// It has to say. The module above promises that a screenshot of this
    /// overlay is the whole of what it takes to stand here again, and a
    /// picture taken with the sun switched off would quietly break that
    /// promise in exactly the place it gets used: an argument about how the
    /// renderer should be set up.
    ///
    /// What it says is the *departure*, which each switch's own default
    /// decides the wording of: one that is normally on reads as "no shadows"
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
        if self.reach != Self::default().reach {
            on.push(format!("shadow {REACH} {:.0}m", self.reach));
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
    charted: Option<ChartTally>,
}

/// Makes the scene agree with [`Toggles`].
///
/// Written as "set it to what it should be" rather than "change it when the
/// switch is thrown", so that a sun or a camera spawned *after* the console
/// spoke — leaving a world and entering another does exactly that — comes up
/// in the state the readout claims it is in. The `is_changed` guard is only to
/// keep it from writing the same values every frame, which would have every
/// light and camera register as changed for anything else watching.
fn apply_toggles(
    mut commands: Commands,
    toggles: Res<Toggles>,
    wireframe: Option<ResMut<WireframeConfig>>,
    mut suns: Query<(&mut DirectionalLight, &mut CascadeShadowConfig)>,
    cameras: Query<(Entity, Has<DistanceFog>), With<MapCamera>>,
    mut lit: Local<bool>,
) {
    let fresh_sun = !suns.is_empty() && !*lit;
    *lit = !suns.is_empty();
    if !toggles.is_changed() && !fresh_sun {
        return;
    }

    if let Some(mut wireframe) = wireframe {
        wireframe.global = toggles.wireframe;
        // Dark lines. The default is white, which disappears against sand and
        // surf — the two places the mesh is most worth looking at.
        wireframe.default_color = Color::srgb(0.05, 0.05, 0.05);
    }

    for (mut sun, mut cascades) in &mut suns {
        sun.shadow_maps_enabled = toggles.shadows;
        *cascades = crate::terrain::cascades(toggles.reach);
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

/// The overlay's two entities, as the refresh reaches them: the panel that
/// shows or hides, and the text that says everything.
#[derive(SystemParam)]
struct Overlay<'w, 's> {
    panels: Query<'w, 's, &'static mut Visibility, With<DebugPanel>>,
    texts: Query<'w, 's, &'static mut Text, With<DebugText>>,
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
        return;
    }

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

/// Everything drawn this frame, from the two points of view that draw it: the
/// camera's, and the sun's once per cascade.
///
/// Both counts are read off culls somebody else has already done — the
/// camera's [`VisibleEntities`] and the light's [`CascadesVisibleEntities`] —
/// rather than worked out again here. That is what makes them comparable with
/// each other, and it means a wrong number here would mean a wrong picture
/// too, which is the kind of wrong that gets noticed.
///
/// Both lists are built in `PostUpdate` and read here in `Update`, so both are
/// a frame old. Equally so, which is what matters for reading one against the
/// other.
#[derive(SystemParam)]
struct Scene<'w, 's> {
    meshes: Res<'w, Assets<Mesh>>,
    drawn: Query<'w, 's, &'static Mesh3d>,
    cameras: Query<'w, 's, &'static VisibleEntities, With<MapCamera>>,
    lights: Query<'w, 's, &'static CascadesVisibleEntities>,
}

/// The geometry lines of the readout: what the eye is given, and what the sun
/// asks for on top of it.
struct Counts {
    triangles: usize,
    meshes: usize,
    /// Absent where nothing is casting — see [`Scene::shadows`].
    shadows: Option<ShadowLoad>,
}

impl Scene<'_, '_> {
    /// What the camera draws, and what the sun redraws on top of it.
    ///
    /// Deliberately not [`ViewVisibility`], which looks like the same question
    /// asked per entity and is not: the light sets it as well, so that a
    /// caster standing behind the camera still reaches the shadow maps.
    /// Counting that way puts geometry the camera never draws into the
    /// camera's own line — which is the exact confusion these two lines exist
    /// to resolve.
    fn counts(&self) -> Counts {
        let mut counts = Counts {
            triangles: 0,
            meshes: 0,
            shadows: self.shadows(),
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

    /// `None` where there is no sun — every menu screen, the light being
    /// spawned with the world and despawned with it.
    fn shadows(&self) -> Option<ShadowLoad> {
        let mut load = ShadowLoad {
            draws: 0,
            triangles: 0,
            cascades: 0,
        };
        let mut lit = false;
        for light in &self.lights {
            // Keyed by the view the cascades were fitted to, and there is one
            // camera — but a light with no view yet has an empty map, which is
            // what nothing to report looks like on the first frames.
            for cascades in light.entities.values() {
                lit = true;
                load.cascades = load.cascades.max(cascades.len());
                for cascade in cascades {
                    load.draws += cascade.entities.len();
                    for entity in &cascade.entities {
                        let Some(mesh) = self.mesh_of(*entity) else {
                            continue;
                        };
                        load.triangles += triangles_in(mesh);
                    }
                }
            }
        }
        lit.then_some(load)
    }
}

/// What the sun's cascades ask for in a frame.
///
/// The shadow pass is the frame drawn over again from the sun, once per
/// cascade the mesh falls inside — so this is not the count above it with a
/// constant on the front, and the two do not even move together. The cascades
/// are fitted to the camera's frustum out to `maximum_distance`, and that
/// reach does not shrink when the view does: zoomed out over an island the
/// camera drew 29 meshes and the sun 67, but zoomed in on the boat the camera
/// drew 7 and the sun still 100. Close to the ground the shadow pass does
/// thirty times the camera's geometry, which is not what anybody guesses.
///
/// Which is the reason for the line. A frame rate that sags says nothing about
/// *which* pass grew, and this is the half of the frame the count above it
/// cannot see.
struct ShadowLoad {
    /// Mesh draws submitted, a mesh counted once per cascade that wants it.
    draws: usize,
    /// Triangles in those draws, on the same footing — a mesh in three
    /// cascades is rasterised three times and counted three times.
    triangles: usize,
    cascades: usize,
}

/// Triangles a mesh draws. The terrain's chunk meshes are un-indexed — flat
/// shading shares no vertices — so theirs is a vertex count; the sea and
/// ocean-floor planes are indexed, and an index buffer overrules the vertex
/// buffer wherever there is one.
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
/// lines however much it has to say: what the frame cost, what the sun added
/// to it, what the streamer holds, and where this is. Every number carries its
/// own noun — nothing is positional — because the lines come and go with what
/// exists to count, and a reader should not have to know which line is missing
/// to know what they are looking at.
///
/// The chunk line is three numbers about two different things:
///
/// - the **total** is every chunk this machine has an answer about, ground and
///   water together, of which **ocean** is the share that was answered with
///   nothing — sea costs nothing to hold, so the rest is where the memory
///   went;
/// - **requested** is ground asked for and not answered. It is deliberately
///   outside the total, nothing being known about it yet, and it is the only
///   number here about the *server* rather than about this machine.
///
/// There is no count of meshes still being assembled, though there is a stage
/// for it. Before the world crossed the wire that number was the whole of the
/// streaming backlog, because building a chunk was generating it; now it is
/// the moment between a payload landing and its vertex buffers being filled,
/// which an arrival's worth of chunks passes through in about six frames. A
/// number that reads zero but for a tenth of a second, once, is not worth the
/// line — and the failure it would have caught, meshes not landing, shows up
/// as the count above this one standing still while the chunks climb.
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
    if let Some(shadows) = &counts.shadows {
        lines.push(format!(
            "{} shadow tris / {} draws / {} cascades",
            thousands(shadows.triangles),
            shadows.draws,
            shadows.cascades
        ));
    }
    if let Some(tally) = held.ground {
        lines.push(format!(
            "{} chunks / {} ocean / {} requested",
            tally.ground + tally.ocean,
            tally.ocean,
            tally.requested
        ));
    }
    // Under the chunks, because it is drawn out of them and grows behind them.
    // The last number is the one gameplay will hang off: closed coastlines
    // big enough, and the right way round, to be islands a player could claim.
    if let Some(chart) = held.charted {
        lines.push(format!(
            "{} surveyed / {} with coast / {} closed / {} claimable",
            chart.surveyed, chart.coastal, chart.complete, chart.islands
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

/// The world and the view in the terms `--seed`, `--focus`, `--yaw` and
/// `--zoom` take them back in: a seed, then metres, degrees and metres. The
/// yaw runs unbounded on the camera — easing never wants to wrap — so it is
/// folded to a bearing here.
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
    parts.push(format!("focus {:.0},{:.0}", view.focus.x, view.focus.z));
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
    use super::*;
    use bevy::asset::RenderAssetUsages;
    use bevy::camera::visibility::VisibleMeshEntities;
    use bevy::mesh::{Indices, PrimitiveTopology};

    /// A chunk of flat ground, which is all this needs of one: the readout
    /// counts chunks, it does not look at them.
    fn a_chunk() -> protocol::ChunkPayload {
        use protocol::ground::{quantize, Surface, Tone, FACET_TRIS, FACET_VERTS};
        protocol::ChunkPayload {
            heights: vec![quantize(1.0); FACET_VERTS * FACET_VERTS],
            surfaces: vec![Surface::plain(Tone::Grass); FACET_TRIS],
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

    /// The switches with the readout on — what most of these tests are
    /// looking at, and exactly what `--debug` starts a run with.
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
        // The sun asks for more than the eye does, the outer cascades not
        // narrowing when the view does — so the two triangle counts are
        // deliberately unlike each other, and a formatter that muddled them
        // would be caught here.
        let counts = Counts {
            triangles: 1_234_567,
            meshes: 214,
            shadows: Some(ShadowLoad {
                draws: 623,
                triangles: 3_298_112,
                cascades: 4,
            }),
        };
        assert_eq!(
            overlay_text(
                Some(59.6),
                &counts,
                Held {
                    ground: Some(&tally),
                    charted: Some(ChartTally {
                        surveyed: 96,
                        coastal: 21,
                        complete: 3,
                        open: 2,
                        islands: 1,
                    }),
                },
                Some(0.35),
                Some(20_040_112),
                Some(view),
                &showing()
            ),
            "60 fps / 214 meshes / 1,234,567 triangles\n\
             3,298,112 shadow tris / 623 draws / 4 cascades\n\
             231 chunks / 58 ocean / 12 requested\n\
             96 surveyed / 21 with coast / 3 closed / 1 claimable\n\
             sky 08:24\n\
             seed 20040112 / focus 98,-317 / yaw 45 / zoom 42"
        );
    }

    #[test]
    fn the_readout_survives_having_no_frame_rate_yet_and_no_world() {
        // On a menu screen there is no world to count, no sun casting and no
        // camera to describe — so the readout falls back to its one line that
        // is true anywhere.
        let counts = Counts {
            triangles: 0,
            meshes: 0,
            shadows: None,
        };
        assert_eq!(
            overlay_text(None, &counts, Held::default(), None, None, None, &showing()),
            "-- fps / 0 meshes / 0 triangles"
        );
    }

    #[test]
    fn the_view_line_folds_the_yaw_to_a_bearing() {
        // The camera's yaw runs unbounded, but the line has to say something
        // `--yaw` would read back as the same view.
        let at = |yaw: f32| View {
            focus: Vec3::ZERO,
            distance: 100.0,
            yaw: yaw.to_radians(),
        };
        assert_eq!(
            view_line(Some(7), at(-90.0)),
            "seed 7 / focus 0,0 / yaw 270 / zoom 100"
        );
        assert_eq!(
            view_line(Some(7), at(450.0)),
            "seed 7 / focus 0,0 / yaw 90 / zoom 100"
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
            "focus 98,-317 / yaw 0 / zoom 150"
        );
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
        let caster = app.world_mut().spawn(Mesh3d(handle.clone())).id();
        app.world_mut().spawn(Mesh3d(handle));

        // A sun whose cascades both want that one mesh, which is the whole
        // point of counting the shadow pass separately: two triangles in the
        // scene are four triangles of shadow work, because a mesh inside two
        // cascades is rasterised into both.
        let view = app.world_mut().spawn_empty().id();
        let mut cascades = CascadesVisibleEntities::default();
        cascades.entities.insert(
            view,
            vec![
                VisibleMeshEntities {
                    entities: vec![caster],
                },
                VisibleMeshEntities {
                    entities: vec![caster],
                },
            ],
        );
        app.world_mut().spawn(cascades);

        // A world holding one chunk of ground and one of open water — a mesh
        // and a fact, which is the distinction the chunk line exists to draw,
        // and which is why that line stays about what is *held* while the two
        // above it are about what is drawn.
        let mut ground = Ground::default();
        ground.deliver(IVec2::ZERO, Some(a_chunk()));
        ground.deliver(IVec2::new(1, 0), None);
        app.insert_resource(ground);

        // And a world of this machine's own behind it, which is what the seed
        // is read off. Bound and never accepted from: the readout asks the
        // handle which world it is, and nothing here has to join it.
        let host = server::Server::bind(("127.0.0.1", 0), server::WorldConfig { seed: 4242 })
            .expect("a server should bind")
            .spawn()
            .expect("a server should serve");
        app.insert_resource(Hosting(host));

        // And a camera, for the view line and for the scene count — which is
        // its cull, so it is the camera that decides what the geometry line
        // says. Filled by hand here: `check_visibility` belongs to the render
        // plugins, and this app has none.
        let mut seen = VisibleEntities::default();
        seen.push(caster, TypeId::of::<Mesh3d>());
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
        for _ in 0..4 {
            app.update();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }

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
                 4 shadow tris / 2 draws / 2 cascades\n\
                 2 chunks / 1 ocean / 0 requested\n\
                 sky 08:24\n\
                 seed 4242 / focus 10,-20 / yaw 0 / zoom 150"
            ),
            "overlay reads: {text}"
        );
        assert!(!text.starts_with("-- fps"), "FPS never got a value: {text}");
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
            reach: 225.0,
        };
        assert_eq!(
            all.line().as_deref(),
            Some("debug: no shadows / no haze / wireframe / shadow reach 225m")
        );

        // And the reach only speaks up when it is not the world's own, since
        // the default is what a normal run already has.
        let default_reach = Toggles {
            reach: crate::HAZE_END,
            ..all.clone()
        };
        assert!(!default_reach.line().unwrap().contains("reach"));
    }

    #[test]
    fn thousands_are_grouped() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }
}
