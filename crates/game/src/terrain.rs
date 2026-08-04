//! Putting the world on screen: streamed chunk meshes, the sea and the sun.
//!
//! The world itself — the archipelago's layout, the height field, its colours
//! and the chunk geometry — lives in the `world` crate, re-exported here
//! wholesale so the rest of the app has one place to import terrain things
//! from. This module is the part Bevy sees, and its job is *streaming*: the
//! world is endless, so nothing can spawn it all. Chunks near the camera are
//! built in background tasks and spawned as they finish; chunks left behind
//! are despawned; the sea and the ocean floor are planes that simply travel
//! with the camera, since away from any island the world is exactly those two
//! surfaces.

use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::light::{CascadeShadowConfigBuilder, NotShadowCaster, NotShadowReceiver};
use bevy::mesh::PrimitiveTopology;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::tasks::{block_on, futures_lite::future, AsyncComputeTaskPool, Task};

pub use world::archipelago::*;
pub use world::terrain::*;

use crate::camera::MapCamera;
use crate::AppState;

/// How far the ocean floor plane hangs below [`MAX_DEPTH`], in metres. It only
/// has to back the water beyond the terrain meshes, so all this has to do is
/// keep the two surfaces from being coplanar — but it has to do it a kilometre
/// out, where the depth buffer is coarse, and the step it leaves at a chunk's
/// edge is under water.
const SEA_FLOOR_CLEARANCE: f32 = 2.0;

/// Width of the sea plane, in metres. It travels with the camera, so it only
/// has to reach past the far plane from wherever the camera is — not across
/// any particular stretch of world.
const SEA_EXTENT: f32 = 8000.0;

/// How far out from the camera's focus chunks are wanted, in metres. The eye
/// sits up to its zoom distance back from the focus and the haze swallows
/// everything [`crate::HAZE_END`] past the eye, so this is that reach plus a
/// margin — ground past it cannot be seen, whichever way the view is turned.
const STREAM_RADIUS: f32 = 1024.0;

/// How far out a chunk has to fall before it is despawned. The gap behind
/// [`STREAM_RADIUS`] is hysteresis: panning along a line must not shed and
/// rebuild the same ring of chunks with every step.
const DESPAWN_RADIUS: f32 = 1280.0;

/// How far out generated islands are kept in the archipelago's cache. Past the
/// chunk radii with a whole big island of slack, so an island is never dropped
/// while any of its chunks could still be wanted.
const ISLAND_CACHE_RADIUS: f32 = DESPAWN_RADIUS + 4096.0;

pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorldConfig>()
            .add_systems(OnEnter(AppState::InWorld), enter_world)
            .add_systems(OnExit(AppState::InWorld), leave_world)
            .add_systems(
                Update,
                (stream_in, receive_chunks, stream_out, follow_camera)
                    .chain()
                    .run_if(in_state(AppState::InWorld)),
            );
    }
}

/// The open world the match is played in, shared with the background tasks
/// that generate its islands and build its chunks.
#[derive(Resource, Clone)]
pub struct WorldTerrain(pub Arc<Archipelago>);

/// Every chunk that currently exists as an entity — spawned and meshed, or
/// still building in a task — and the one material they all share.
#[derive(Resource)]
struct ChunkIndex {
    chunks: HashMap<IVec2, Entity>,
    ground: Handle<StandardMaterial>,
}

/// Marks a terrain chunk entity, and records which world chunk it is.
#[derive(Component, Debug, Clone, Copy)]
pub struct TerrainChunk {
    /// The chunk's coordinate on the world grid. What streaming keys off, and
    /// later what level-of-detail selection and localised rebuilds will.
    pub coords: IVec2,
}

/// A chunk whose geometry is still being built off the main thread. The task
/// carries the whole cost: generating the owning island, if this is the first
/// chunk of it to be wanted, and then the mesh itself.
#[derive(Component)]
pub(crate) struct ChunkBuild(Task<Option<ChunkGeometry>>);

/// Marks the sea plane, which travels with the camera.
#[derive(Component)]
struct Sea;

/// Marks the ocean floor plane, likewise.
#[derive(Component)]
struct OceanFloor;

/// Wraps one chunk's geometry into the mesh Bevy draws. The geometry arrives
/// as finished vertex buffers, so all this adds is the labels — un-indexed,
/// for the reason [`ChunkGeometry`] gives.
fn chunk_mesh(geometry: ChunkGeometry) -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, geometry.positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, geometry.normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, geometry.uvs)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, geometry.colors)
}

// ---------------------------------------------------------------------------
// Entering and leaving the world
// ---------------------------------------------------------------------------

fn enter_world(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    config: Res<WorldConfig>,
) {
    // The world itself. Nothing is generated here — the streaming systems ask
    // for ground as the camera's neighbourhood needs it — so entering a match
    // costs a frame nothing, wherever in the world it starts.
    commands.insert_resource(WorldTerrain(Arc::new(Archipelago::new(&config))));
    info!("entered world (seed {})", config.seed);

    // Terrain material. Base colour is white so the vertex colours come
    // through unmodified — StandardMaterial multiplies the two together.
    // Every chunk shares the one material, so they still batch into a single
    // draw call each. Fully matte: a specular highlight is a gradient, and
    // gradients are the one thing this look can't have.
    commands.insert_resource(ChunkIndex {
        chunks: HashMap::new(),
        ground: materials.add(StandardMaterial {
            base_color: Color::WHITE,
            perceptual_roughness: 1.0,
            metallic: 0.0,
            reflectance: 0.0,
            ..default()
        }),
    });

    // Ocean floor. The sea is translucent, so without something opaque beneath
    // it the water beyond the terrain meshes blends against the sky and reads
    // as a pale band along the coastline. Between islands this plane *is* the
    // ground — open ocean has no chunk meshes at all — so it has to be
    // indistinguishable from the bed the chunks build: the same palette
    // colour through the same material, matte and unlit by anything the
    // chunks aren't. Every island's skirt bed reaches exactly [`OCEAN_DEPTH`]
    // flat, so with the colours agreeing the hand-over from mesh to backdrop
    // has nothing left to show. (It used to be darker, from when it only
    // appeared beyond a lone map's edge — against streamed islands that
    // printed every island's chunk rectangle onto the water.)
    //
    // It sits [`SEA_FLOOR_CLEARANCE`] below the terrain's own deepest point
    // rather than level with it. The two used to be at exactly the same
    // height, and since the height field spends whole square kilometres pinned
    // to its floor, that left two coplanar surfaces fighting over the depth
    // buffer — which read as a faint darker banding drifting across the open
    // sea as the camera moved. A couple of metres of parallax at the seam is
    // invisible with the colours matched; a shimmer is not.
    commands.spawn((
        Name::new("Ocean floor"),
        OceanFloor,
        DespawnOnExit(AppState::InWorld),
        // Nothing is below it to catch a shadow, so keep it out of the shadow
        // pass entirely.
        NotShadowCaster,
        // And nothing above it may throw one onto it. No honest shadow can
        // reach it — a clifftop's shadow dies within a few tens of metres of
        // its coast, and every island ends in a hundred metres of open skirt
        // — but the chunk meshes end in a two-metre step down to this plane,
        // and that step's own shadow drew a thin dark line along the western
        // edge of every island's chunk rectangle.
        NotShadowReceiver,
        Mesh3d(meshes.add(Plane3d::default().mesh().size(SEA_EXTENT, SEA_EXTENT))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(
                OCEAN_FLOOR_COLOR.x,
                OCEAN_FLOOR_COLOR.y,
                OCEAN_FLOOR_COLOR.z,
            ),
            perceptual_roughness: 1.0,
            metallic: 0.0,
            reflectance: 0.0,
            ..default()
        })),
        Transform::from_xyz(0.0, -MAX_DEPTH - SEA_FLOOR_CLEARANCE, 0.0),
    ));

    // Sea. Sized past the camera's far plane and moved along with it, so the
    // horizon is water fading into haze whichever way the view goes.
    commands.spawn((
        Name::new("Sea"),
        Sea,
        DespawnOnExit(AppState::InWorld),
        // Water casts no shadow. Bevy shadows a transparent surface as though
        // it were solid, so without this the sea throws its own shadow down
        // onto its own bed — a broad darkening of everything under water, with
        // a hard edge wherever the cascades stop resolving it, sliding about as
        // the camera moves. It still *receives* shadows, which is what puts a
        // cliff's shadow out across the water at its foot.
        NotShadowCaster,
        Mesh3d(meshes.add(Plane3d::default().mesh().size(SEA_EXTENT, SEA_EXTENT))),
        MeshMaterial3d(materials.add(StandardMaterial {
            // Flat and bright rather than glassy. Still partly transparent, so
            // the sand band running under the waterline shows through as a
            // turquoise ring around every coast — two flat tones of water,
            // which is the whole effect.
            base_color: Color::srgba(0.10, 0.42, 0.62, 0.84),
            perceptual_roughness: 1.0,
            metallic: 0.0,
            reflectance: 0.02,
            alpha_mode: AlphaMode::Blend,
            ..default()
        })),
        // Nudged above y = 0 so it doesn't z-fight with terrain sitting exactly
        // at sea level.
        Transform::from_xyz(0.0, 0.08, 0.0),
    ));

    // Sun.
    commands.spawn((
        Name::new("Sun"),
        DespawnOnExit(AppState::InWorld),
        DirectionalLight {
            // Lower than daylight, and paired with a much stronger ambient, so
            // the gap between a lit facet and a shadowed one is a couple of
            // steps rather than a full range. Flat shading gets its readability
            // from facets differing at all, not from deep contrast.
            illuminance: 8_500.0,
            shadow_maps_enabled: true,
            // The default biases cause bad self-shadowing acne on a heightfield
            // this large — dark speckle all over the hillsides.
            shadow_depth_bias: 0.06,
            shadow_normal_bias: 2.2,
            ..default()
        },
        // Cascades are fitted to the camera's own frustum, so the far end of the
        // shadowed region travels with the camera. It has to sit past everything
        // the camera can see, or that end lands on ground that is in shot and
        // whole hillsides gain and lose their shadows as the view moves. Out at
        // the haze it can't be seen doing it.
        CascadeShadowConfigBuilder {
            first_cascade_far_bound: 60.0,
            maximum_distance: crate::HAZE_END,
            ..default()
        }
        .build(),
        // Low-ish sun: long shadows pick out the relief far better than an
        // overhead one, which flattens everything.
        Transform::from_xyz(55.0, 42.0, 28.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

/// Chunk entities despawn themselves on exit; the resources that tracked them
/// have to go too, or a stale world would answer the next match's queries
/// until its own first frame replaced it.
fn leave_world(mut commands: Commands) {
    commands.remove_resource::<WorldTerrain>();
    commands.remove_resource::<ChunkIndex>();
}

// ---------------------------------------------------------------------------
// Streaming
// ---------------------------------------------------------------------------

/// Starts a build task for every chunk newly inside the streaming radius.
///
/// Only chunks an island is answerable for are ever built — the open ocean is
/// exactly the sea and floor planes, so spawning meshes of it would be paying
/// to draw a flat quad 24 thousand vertices at a time. Which chunks those are
/// is a layout question, answered without generating anything; the generation
/// itself happens in the tasks.
fn stream_in(
    mut commands: Commands,
    world: Res<WorldTerrain>,
    mut index: ResMut<ChunkIndex>,
    cameras: Query<&MapCamera>,
) {
    let Ok(camera) = cameras.single() else {
        return;
    };
    let focus = Vec2::new(camera.focus.x, camera.focus.z);
    let pool = AsyncComputeTaskPool::get();

    for spec in world
        .0
        .islands_within(focus - STREAM_RADIUS, focus + STREAM_RADIUS)
    {
        for chunk in chunks_of(&spec) {
            if index.chunks.contains_key(&chunk) || !within(chunk, focus, STREAM_RADIUS) {
                continue;
            }

            let world = world.0.clone();
            let task = pool.spawn(async move { world.chunk_geometry(chunk) });
            let entity = commands
                .spawn((
                    Name::new(format!("Terrain chunk {},{}", chunk.x, chunk.y)),
                    TerrainChunk { coords: chunk },
                    DespawnOnExit(AppState::InWorld),
                    ChunkBuild(task),
                    Transform::from_translation(Vec3::new(
                        chunk.x as f32 * CHUNK_METRES,
                        0.0,
                        chunk.y as f32 * CHUNK_METRES,
                    )),
                ))
                .id();
            index.chunks.insert(chunk, entity);
        }
    }
}

/// Puts finished geometry on its entity. Until this runs for a chunk, the
/// entity is a placeholder with a transform and a ticket.
fn receive_chunks(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut index: ResMut<ChunkIndex>,
    mut building: Query<(Entity, &TerrainChunk, &mut ChunkBuild)>,
) {
    for (entity, chunk, mut build) in &mut building {
        let Some(geometry) = block_on(future::poll_once(&mut build.0)) else {
            continue;
        };

        match geometry {
            Some(geometry) => {
                commands.entity(entity).remove::<ChunkBuild>().insert((
                    Mesh3d(meshes.add(chunk_mesh(geometry))),
                    MeshMaterial3d(index.ground.clone()),
                ));
            }
            // The layout said this chunk was an island's, the world said
            // otherwise — cannot happen while the two agree, but a mesh of
            // nothing is not worth keeping either way.
            None => {
                index.chunks.remove(&chunk.coords);
                commands.entity(entity).despawn();
            }
        }
    }
}

/// Despawns chunks the camera has left well behind, and lets the archipelago
/// forget islands none of them could belong to any more.
fn stream_out(
    mut commands: Commands,
    world: Res<WorldTerrain>,
    mut index: ResMut<ChunkIndex>,
    cameras: Query<&MapCamera>,
) {
    let Ok(camera) = cameras.single() else {
        return;
    };
    let focus = Vec2::new(camera.focus.x, camera.focus.z);

    index.chunks.retain(|chunk, entity| {
        if within(*chunk, focus, DESPAWN_RADIUS) {
            return true;
        }
        commands.entity(*entity).despawn();
        false
    });

    world.0.retain_near(focus, ISLAND_CACHE_RADIUS);
}

/// The transforms of the two planes that travel with the camera.
type TravellingPlanes<'w, 's> =
    Query<'w, 's, &'static mut Transform, Or<(With<Sea>, With<OceanFloor>)>>;

/// Keeps the sea and the ocean floor centred under the camera. They are
/// featureless planes, so nothing shows them moving — the water simply always
/// reaches the horizon.
fn follow_camera(cameras: Query<&MapCamera>, mut planes: TravellingPlanes) {
    let Ok(camera) = cameras.single() else {
        return;
    };
    for mut transform in &mut planes {
        transform.translation.x = camera.focus.x;
        transform.translation.z = camera.focus.z;
    }
}

/// Every world chunk of an island — its map and its skirt.
fn chunks_of(spec: &IslandSpec) -> impl Iterator<Item = IVec2> + '_ {
    let (min, max) = spec.covered();
    (min.y..max.y).flat_map(move |z| (min.x..max.x).map(move |x| IVec2::new(x, z)))
}

/// Whether any of a chunk lies within `radius` of `focus`.
fn within(chunk: IVec2, focus: Vec2, radius: f32) -> bool {
    let corner = chunk.as_vec2() * CHUNK_METRES;
    let nearest = focus.clamp(corner, corner + CHUNK_METRES);
    nearest.distance_squared(focus) <= radius * radius
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chunk_mesh_is_the_geometry_with_labels_on() {
        let config = MapConfig {
            chunks: UVec2::ONE,
            seed: 1,
        };
        let generator = TerrainGenerator::new(&config);
        let geometry = generator.build_chunk(UVec2::ZERO, config.tiles());
        let count = geometry.positions.len();
        let mesh = chunk_mesh(geometry);

        assert_eq!(mesh.count_vertices(), count);
        assert!(
            mesh.indices().is_none(),
            "flat shading needs no index buffer"
        );
    }

    #[test]
    fn the_colour_conversion_matches_the_renderers() {
        // The geometry's vertex colours are converted to linear space by the
        // world crate, which cannot ask Bevy how — so hold its conversion equal
        // to the one the renderer's own colour types would have done.
        for step in 0..=100 {
            let c = step as f32 / 100.0;
            assert_eq!(
                srgb_to_linear(c),
                Color::srgb(c, c, c).to_linear().red,
                "sRGB {c} decodes differently to Bevy"
            );
        }
    }

    #[test]
    fn an_islands_chunks_cover_its_map_and_skirt() {
        let spec = IslandSpec {
            origin: IVec2::new(4, -2),
            chunks: UVec2::new(2, 3),
            seed: 1,
        };
        let chunks: Vec<IVec2> = chunks_of(&spec).collect();

        // Map plus one chunk of skirt all round.
        assert_eq!(chunks.len(), 4 * 5);
        assert!(chunks.contains(&IVec2::new(3, -3)), "missing the skirt");
        assert!(chunks.contains(&IVec2::new(6, 1)), "missing the far corner");
        for chunk in &chunks {
            assert!(spec.covers_chunk(*chunk), "{chunk} is not the island's");
        }
    }

    #[test]
    fn a_chunk_is_within_reach_by_its_nearest_point() {
        // Chunk (0,0) spans 0..128 on both axes.
        let chunk = IVec2::ZERO;
        assert!(within(chunk, Vec2::new(64.0, 64.0), 1.0), "inside is near");
        assert!(
            within(chunk, Vec2::new(200.0, 64.0), 80.0),
            "80 m off the east edge"
        );
        assert!(
            !within(chunk, Vec2::new(200.0, 64.0), 60.0),
            "60 m short of the east edge"
        );
    }
}
