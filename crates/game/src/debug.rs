//! The `--debug` overlay: what each pass draws, and switches to change it.
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
//! The number keys are switches for whoever is working on the renderer — see
//! [`Toggles`] — and they are here rather than in their own module because
//! reading a count and changing what is counted are one job: the reason to
//! turn the shadows off is to watch the shadow line answer. When any of them
//! is set the readout says so on a last line, since a doctored picture that
//! did not admit it would be worth less than no picture at all.

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
use crate::net::Hosting;
use crate::terrain::{Ground, Tally};
use crate::Helm;

const TEXT: Color = Color::srgb(0.88, 0.87, 0.80);
const BACKDROP: Color = Color::srgba(0.0, 0.0, 0.0, 0.55);

pub struct DebugOverlayPlugin;

impl Plugin for DebugOverlayPlugin {
    fn build(&self, app: &mut App) {
        // Wireframes come from the renderer, and the readout's own tests run
        // an app that has none — text is text without a GPU. Asking whether
        // the renderer is there keeps every `--debug` system in this file
        // rather than scattering half of them into the binary to dodge that.
        if app.is_plugin_added::<bevy::render::RenderPlugin>() {
            app.add_plugins(WireframePlugin::default());
        }
        app.add_plugins(FrameTimeDiagnosticsPlugin::default())
            .init_resource::<Toggles>()
            .add_systems(Startup, spawn_overlay)
            // Only at the helm. Digits are typed into the seed and address
            // fields on the menus, and the controls screen binds whatever key
            // it is given — and that screen is *inside* a world, so gating on
            // being in one is not enough to keep a toggle out of a binding.
            .add_systems(Update, take_toggles.run_if(in_state(Helm::Sailing)))
            .add_systems(Update, (apply_toggles, refresh_overlay).chain());
    }
}

/// Marks the overlay's one text block, so the refresh can find it.
#[derive(Component)]
struct DebugText;

/// How far the sun's cascades reach when `4` has been leaned on, in metres.
///
/// The first is the world's own — [`crate::HAZE_END`], where the haze has
/// closed and nothing can be seen to lose its shadow. The rest walk in far
/// enough to be worth looking at rather than in polite steps: the question the
/// key exists to answer is whether a shorter reach is *visibly* worse, and
/// halving twice puts the far edge of the shadows inside the haze and then
/// inside plain sight, which is where the answer is.
const REACHES: [f32; 3] = [crate::HAZE_END, 450.0, 225.0];

/// The switches `--debug` puts on the number keys.
///
/// Hard-wired rather than bound through [`crate::bindings`], and deliberately:
/// these are for whoever is working on the renderer, they never appear on the
/// controls screen, and a player who has rebound every key they can see still
/// cannot reach them — the whole plugin is absent without `--debug`.
#[derive(Resource, Default, PartialEq, Clone, Debug)]
struct Toggles {
    /// `1` — the sun stops casting. The shadow line disappears with it, Bevy
    /// clearing the cascade cull when a light's shadows are off, so the
    /// readout cannot claim work that is no longer being done.
    no_shadows: bool,
    /// `2` — the aerial haze comes off, so what the distant ground is actually
    /// doing can be seen. Mostly worth having next to `4`: judging what a
    /// shorter shadow reach costs is impossible while the haze is hiding the
    /// far end of it.
    no_haze: bool,
    /// `3` — every triangle drawn as lines, which is how the fixed 8,192 a
    /// chunk carries stops being a number and becomes a picture.
    wireframe: bool,
    /// `4` — index into [`REACHES`], cycling.
    reach: usize,
}

impl Toggles {
    /// How the overlay owns up to being in a doctored state — `None` when
    /// nothing has been touched, which is the usual case and costs the readout
    /// no line at all.
    ///
    /// It has to say. The module above promises that a screenshot of this
    /// overlay is the whole of what it takes to stand here again, and a
    /// picture taken with the sun switched off would quietly break that
    /// promise in exactly the place it gets used: an argument about how the
    /// renderer should be set up.
    fn line(&self) -> Option<String> {
        let mut on = Vec::new();
        if self.no_shadows {
            on.push("no shadows".to_string());
        }
        if self.no_haze {
            on.push("no haze".to_string());
        }
        if self.wireframe {
            on.push("wireframe".to_string());
        }
        if self.reach != 0 {
            on.push(format!("shadow reach {:.0}m", REACHES[self.reach]));
        }
        (!on.is_empty()).then(|| format!("debug: {}", on.join(" / ")))
    }
}

/// Reads the number keys. Nothing here touches the world — it only moves
/// [`Toggles`], which [`apply_toggles`] then makes true of the scene, so that
/// the state and the acting on it stay one thing each.
fn take_toggles(keys: Res<ButtonInput<KeyCode>>, mut toggles: ResMut<Toggles>) {
    if keys.just_pressed(KeyCode::Digit1) {
        toggles.no_shadows = !toggles.no_shadows;
    }
    if keys.just_pressed(KeyCode::Digit2) {
        toggles.no_haze = !toggles.no_haze;
    }
    if keys.just_pressed(KeyCode::Digit3) {
        toggles.wireframe = !toggles.wireframe;
    }
    if keys.just_pressed(KeyCode::Digit4) {
        toggles.reach = (toggles.reach + 1) % REACHES.len();
    }
    // Everything back, for when a session has drifted somewhere nobody can
    // remember the way out of.
    if keys.just_pressed(KeyCode::Digit0) {
        *toggles = Toggles::default();
    }
}

/// Makes the scene agree with [`Toggles`].
///
/// Written as "set it to what it should be" rather than "change it when the
/// key is pressed", so that a sun or a camera spawned *after* the key was
/// pressed — leaving a world and entering another does exactly that — comes up
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
        sun.shadow_maps_enabled = !toggles.no_shadows;
        *cascades = crate::terrain::cascades(REACHES[toggles.reach]);
    }

    for (camera, has_fog) in &cameras {
        match (toggles.no_haze, has_fog) {
            (true, true) => {
                commands.entity(camera).remove::<DistanceFog>();
            }
            (false, false) => {
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

fn refresh_overlay(
    diagnostics: Res<DiagnosticsStore>,
    scene: Scene,
    toggles: Res<Toggles>,
    ground: Option<Res<Ground>>,
    hosting: Option<Res<Hosting>>,
    cameras: Query<&MapCamera>,
    mut texts: Query<&mut Text, With<DebugText>>,
) {
    let fps = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|fps| fps.smoothed());

    let counts = scene.counts();

    // What this machine has of the world, and what it is still waiting for.
    // Absent outside a match, where the readout has no world to count.
    let tally = ground.map(|ground| ground.tally());

    let view = cameras.single().ok().map(|camera| View {
        focus: camera.focus,
        distance: camera.distance,
        yaw: camera.yaw,
    });

    // Only a world made on this machine can be named. A guest is sent ground
    // and never the recipe, so there is no seed here to print.
    let seed = hosting.map(|hosting| hosting.0.seed());

    for mut text in &mut texts {
        text.0 = overlay_text(fps, &counts, tally.as_ref(), seed, view, &toggles);
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
    tally: Option<&Tally>,
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
    if let Some(tally) = tally {
        lines.push(format!(
            "{} chunks / {} ocean / {} requested",
            tally.ground + tally.ocean,
            tally.ocean,
            tally.requested
        ));
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
            palms: Vec::new(),
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
                Some(&tally),
                Some(20_040_112),
                Some(view),
                &Toggles::default()
            ),
            "60 fps / 214 meshes / 1,234,567 triangles\n\
             3,298,112 shadow tris / 623 draws / 4 cascades\n\
             231 chunks / 58 ocean / 12 requested\n\
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
            overlay_text(None, &counts, None, None, None, &Toggles::default()),
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
        .insert_resource(Assets::<Mesh>::default());

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
                 seed 4242 / focus 10,-20 / yaw 0 / zoom 150"
            ),
            "overlay reads: {text}"
        );
        assert!(!text.starts_with("-- fps"), "FPS never got a value: {text}");
    }

    /// Nothing is said when nothing has been touched — the common case must
    /// not cost the readout a line.
    #[test]
    fn an_untouched_run_admits_to_nothing() {
        assert_eq!(Toggles::default().line(), None);
    }

    #[test]
    fn a_doctored_run_says_so() {
        // Every switch at once, to pin the order and the separator as well as
        // the wording.
        let all = Toggles {
            no_shadows: true,
            no_haze: true,
            wireframe: true,
            reach: 2,
        };
        assert_eq!(
            all.line().as_deref(),
            Some("debug: no shadows / no haze / wireframe / shadow reach 225m")
        );

        // And the reach only speaks up when it is not the world's own, since
        // the first entry is what a normal run already has.
        let default_reach = Toggles {
            reach: 0,
            ..all.clone()
        };
        assert!(!default_reach.line().unwrap().contains("reach"));
    }

    /// `4` walks the reaches and comes back to the world's own, so leaning on
    /// it can never strand the sun somewhere there is no key to leave.
    #[test]
    fn the_shadow_reach_cycles_back_to_the_worlds_own() {
        let mut app = App::new();
        app.init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<Toggles>()
            .add_systems(Update, take_toggles);

        for expected in [1, 2, 0, 1] {
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(KeyCode::Digit4);
            app.update();
            assert_eq!(app.world().resource::<Toggles>().reach, expected);
            // `clear` would only drop the just-pressed edge and leave the key
            // held, so the next press would not read as a new one.
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .reset(KeyCode::Digit4);
        }
        assert_eq!(REACHES[0], crate::HAZE_END);
    }

    /// `0` is the way out of any state the other keys can reach.
    #[test]
    fn zero_puts_everything_back() {
        let mut app = App::new();
        app.init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<Toggles>()
            .add_systems(Update, take_toggles);

        fn press(app: &mut App, key: KeyCode) {
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(key);
            app.update();
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .reset(key);
        }
        for key in [
            KeyCode::Digit1,
            KeyCode::Digit2,
            KeyCode::Digit3,
            KeyCode::Digit4,
        ] {
            press(&mut app, key);
        }
        assert!(app.world().resource::<Toggles>().line().is_some());

        press(&mut app, KeyCode::Digit0);
        assert_eq!(*app.world().resource::<Toggles>(), Toggles::default());
        assert_eq!(app.world().resource::<Toggles>().line(), None);
    }

    #[test]
    fn thousands_are_grouped() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }
}
