//! The `--debug` overlay: frame rate and how much geometry is in the scene.
//!
//! A small readout pinned to a corner of the window, over whatever screen the
//! app is on. The frame rate comes from Bevy's own frame-time diagnostics,
//! already smoothed; the triangle count is summed from the meshes actually
//! spawned, so it rises and falls as chunks stream in and out and reads zero
//! on the menu, where nothing 3D exists at all. The chunk line shows the
//! streamer's own bookkeeping: how many chunks it holds, and how many of
//! those are still building off the main thread. The last line reads the
//! view back in the terms the command line takes it — `--focus`, `--zoom`,
//! `--yaw` — so a view worth keeping can be pasted straight into a `--shot`
//! run.

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use bevy::text::FontSize;

use crate::camera::{MapCamera, View};
use crate::terrain::{Ground, Tally};

const TEXT: Color = Color::srgb(0.88, 0.87, 0.80);
const BACKDROP: Color = Color::srgba(0.0, 0.0, 0.0, 0.55);

pub struct DebugOverlayPlugin;

impl Plugin for DebugOverlayPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(FrameTimeDiagnosticsPlugin::default())
            .add_systems(Startup, spawn_overlay)
            .add_systems(Update, refresh_overlay);
    }
}

/// Marks the overlay's one text block, so the refresh can find it.
#[derive(Component)]
struct DebugText;

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
    meshes: Res<Assets<Mesh>>,
    drawn: Query<&Mesh3d>,
    ground: Option<Res<Ground>>,
    cameras: Query<&MapCamera>,
    mut texts: Query<&mut Text, With<DebugText>>,
) {
    let fps = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|fps| fps.smoothed());

    let mut triangles = 0;
    let mut count = 0;
    for handle in &drawn {
        // A handle whose asset hasn't landed yet draws nothing, so it counts
        // as nothing.
        let Some(mesh) = meshes.get(&handle.0) else {
            continue;
        };
        triangles += triangles_in(mesh);
        count += 1;
    }

    // What this machine has of the world, and what it is still waiting for.
    // Absent outside a match, where the readout has no world to count.
    let tally = ground.map(|ground| ground.tally());

    let view = cameras.single().ok().map(|camera| View {
        focus: camera.focus,
        distance: camera.distance,
        yaw: camera.yaw,
    });

    for mut text in &mut texts {
        text.0 = overlay_text(fps, triangles, count, tally.as_ref(), view);
    }
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
    triangles: usize,
    meshes: usize,
    tally: Option<&Tally>,
    view: Option<View>,
) -> String {
    let fps = fps.map_or_else(|| "--".to_string(), |fps| format!("{fps:.0}"));
    let mut text = format!(
        "{fps} fps\n{} triangles\n{meshes} meshes",
        thousands(triangles)
    );
    if let Some(tally) = tally {
        text.push('\n');
        text.push_str(&format!(
            "{} chunks ({} ocean, {} requested)",
            tally.ground + tally.ocean,
            tally.ocean,
            tally.requested
        ));
    }
    if let Some(view) = view {
        text.push('\n');
        text.push_str(&view_line(view));
    }
    text
}

/// The view in the terms `--focus`, `--zoom` and `--yaw` take it back in:
/// metres, metres and degrees. The yaw runs unbounded on the camera — easing
/// never wants to wrap — so it is folded to a bearing here.
fn view_line(view: View) -> String {
    format!(
        "focus {:.0},{:.0}  zoom {:.0}  yaw {:.0}",
        view.focus.x,
        view.focus.z,
        view.distance,
        view.yaw.to_degrees().rem_euclid(360.0)
    )
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
    use bevy::mesh::{Indices, PrimitiveTopology};

    /// A chunk of flat ground, which is all this needs of one: the readout
    /// counts chunks, it does not look at them.
    fn a_chunk() -> protocol::ChunkPayload {
        use protocol::ground::{quantize, Surface, Tone, FACET_TRIS, FACET_VERTS};
        protocol::ChunkPayload {
            heights: vec![quantize(1.0); FACET_VERTS * FACET_VERTS],
            surfaces: vec![Surface::plain(Tone::Grass); FACET_TRIS],
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
    fn the_readout_puts_each_count_on_its_own_line() {
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
        assert_eq!(
            overlay_text(Some(59.6), 1_234_567, 214, Some(&tally), Some(view)),
            "60 fps\n1,234,567 triangles\n214 meshes\n\
             231 chunks (58 ocean, 12 requested)\n\
             focus 98,-317  zoom 42  yaw 45"
        );
    }

    #[test]
    fn the_readout_survives_having_no_frame_rate_yet_and_no_world() {
        // On a menu screen there is no world to count and no camera to
        // describe, so the readout is the three lines that are always true.
        assert_eq!(
            overlay_text(None, 0, 0, None, None),
            "-- fps\n0 triangles\n0 meshes"
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
        assert_eq!(view_line(at(-90.0)), "focus 0,0  zoom 100  yaw 270");
        assert_eq!(view_line(at(450.0)), "focus 0,0  zoom 100  yaw 90");
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

        // Two triangles' worth of scene, and a world holding one chunk of
        // ground and one of open water — a mesh and a fact, which is the
        // distinction the chunk line exists to draw.
        let handle = app
            .world_mut()
            .resource_mut::<Assets<Mesh>>()
            .add(unindexed(6));
        app.world_mut().spawn(Mesh3d(handle));

        let mut ground = Ground::default();
        ground.deliver(IVec2::ZERO, Some(a_chunk()));
        ground.deliver(IVec2::new(1, 0), None);
        app.insert_resource(ground);

        // And a camera, for the view line.
        app.world_mut().spawn(MapCamera::looking(View {
            focus: Vec3::new(10.0, 0.0, -20.0),
            distance: 150.0,
            yaw: 0.0,
        }));

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
                "2 triangles\n1 meshes\n\
                 2 chunks (1 ocean, 0 requested)\n\
                 focus 10,-20  zoom 150  yaw 0"
            ),
            "overlay reads: {text}"
        );
        assert!(!text.starts_with("-- fps"), "FPS never got a value: {text}");
    }

    #[test]
    fn thousands_are_grouped() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }
}
