//! Putting the terrain on screen: meshes, materials, the sea and the sun.
//!
//! The terrain itself — the height field, its colours and the chunk geometry —
//! lives in the `world` crate, re-exported here wholesale so the rest of the app
//! has one place to import terrain things from. This module is the part Bevy
//! sees: it wraps each chunk's geometry into a `Mesh` and spawns the world.

use bevy::asset::RenderAssetUsages;
use bevy::light::{CascadeShadowConfigBuilder, NotShadowCaster};
use bevy::mesh::PrimitiveTopology;
use bevy::prelude::*;
use bevy::tasks::ComputeTaskPool;

pub use world::terrain::*;

use crate::AppState;

/// How far the ocean floor plane hangs below [`MAX_DEPTH`], in metres. It only
/// has to back the water beyond the terrain mesh, so all this has to do is keep
/// the two surfaces from being coplanar — but it has to do it a kilometre out,
/// where the depth buffer is coarse, and the step it leaves at the map's rim is
/// under water and off the edge of anywhere the camera can go.
const SEA_FLOOR_CLEARANCE: f32 = 2.0;

/// Width of the sea plane, in metres. Has to reach past the camera's far plane
/// from any point on the largest map, so that water always runs to the horizon.
const SEA_EXTENT: f32 = 8000.0;

pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MapConfig>()
            .add_systems(OnEnter(AppState::InWorld), spawn_world);
    }
}

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

/// Marks a terrain chunk entity, and records which chunk of the map it is.
/// Later work — level of detail, rebuilding deformed ground — keys off this.
#[derive(Component, Debug, Clone, Copy)]
pub struct TerrainChunk {
    /// Which chunk of the map this is. Not read yet — it's what level-of-detail
    /// selection and localised rebuilds will key off.
    #[allow(dead_code)]
    pub coords: UVec2,
}

/// One chunk's mesh, and where in the world it belongs.
struct ChunkMesh {
    coords: UVec2,
    origin: Vec3,
    mesh: Mesh,
}

/// Builds every chunk of the map, in parallel across the task pool.
fn build_chunks(generator: &TerrainGenerator, config: &MapConfig) -> Vec<ChunkMesh> {
    let half = config.half_extent();
    let tiles = config.tiles();

    ComputeTaskPool::get().scope(|scope| {
        for cz in 0..config.chunks.y {
            for cx in 0..config.chunks.x {
                scope.spawn(async move {
                    let chunk = UVec2::new(cx, cz);
                    let origin_tiles = chunk * CHUNK_TILES;

                    ChunkMesh {
                        coords: chunk,
                        origin: Vec3::new(
                            origin_tiles.x as f32 * TILE_SIZE - half.x,
                            0.0,
                            origin_tiles.y as f32 * TILE_SIZE - half.y,
                        ),
                        mesh: chunk_mesh(generator.build_chunk(origin_tiles, tiles)),
                    }
                });
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Spawning
// ---------------------------------------------------------------------------

fn spawn_world(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    config: Res<MapConfig>,
) {
    let started = std::time::Instant::now();
    let generator = TerrainGenerator::new(&config);
    let chunks = build_chunks(&generator, &config);
    let elapsed = started.elapsed();

    // Ocean floor. The sea is translucent, so without something opaque beneath
    // it the water past the edge of the terrain mesh blends against the sky and
    // reads as a pale band along the coastline.
    //
    // It sits [`SEA_FLOOR_CLEARANCE`] below the terrain's own deepest point
    // rather than level with it. The two used to be at exactly the same height,
    // and since the height field spends whole square kilometres pinned to its
    // floor, that left two coplanar surfaces fighting over the depth buffer —
    // which read as a faint darker banding drifting across the open sea as the
    // camera moved.
    commands.spawn((
        Name::new("Ocean floor"),
        DespawnOnExit(AppState::InWorld),
        // Nothing is below it to catch a shadow, so keep it out of the shadow
        // pass entirely.
        NotShadowCaster,
        Mesh3d(meshes.add(Plane3d::default().mesh().size(SEA_EXTENT, SEA_EXTENT))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.08, 0.24, 0.32),
            perceptual_roughness: 1.0,
            reflectance: 0.0,
            ..default()
        })),
        Transform::from_xyz(0.0, -MAX_DEPTH - SEA_FLOOR_CLEARANCE, 0.0),
    ));

    // Terrain. Base colour is white so the vertex colours come through
    // unmodified — StandardMaterial multiplies the two together. Every chunk
    // shares the one material, so they still batch into a single draw call each.
    // Fully matte: a specular highlight is a gradient, and gradients are the
    // one thing this look can't have.
    let ground = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 1.0,
        metallic: 0.0,
        reflectance: 0.0,
        ..default()
    });

    let chunk_count = chunks.len();
    for chunk in chunks {
        commands.spawn((
            Name::new(format!(
                "Terrain chunk {},{}",
                chunk.coords.x, chunk.coords.y
            )),
            TerrainChunk {
                coords: chunk.coords,
            },
            DespawnOnExit(AppState::InWorld),
            Mesh3d(meshes.add(chunk.mesh)),
            MeshMaterial3d(ground.clone()),
            Transform::from_translation(chunk.origin),
        ));
    }

    info!(
        "generated {} x {} m map (seed {seed}) as {chunk_count} chunks in {elapsed:.1?}",
        config.tiles().x,
        config.tiles().y,
        seed = config.seed,
    );

    // Sea. Sized past the camera's far plane rather than relative to the map, so
    // its own edge is never in frame on a small map — the horizon should be
    // water fading into haze.
    commands.spawn((
        Name::new("Sea"),
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

    commands.insert_resource(generator);
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
}
