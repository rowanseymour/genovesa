//! The plants standing on the ground: palms along the back of a beach,
//! bananas on the valley floors behind them.
//!
//! The server says where they stand; this decides what one looks like. That
//! split is the same one the ground is drawn on — a chunk arrives as heights
//! and palette entries — and it is why a plant crosses the wire as a kind, a
//! position, a bearing and a size rather than as anything to do with trunks
//! or fronds.
//!
//! What green means is the *model's* business, not this module's. A plant's
//! colours ride on its own vertices and the material here is white, so
//! nothing in this file knows a frond from a trunk. The ground's palette is
//! the other way about for a reason that does not apply here: a tone is a
//! number on the wire, shared by two machines that must agree, while a mesh
//! is an asset the client already holds and can perfectly well be handed
//! painted.
//!
//! A palm hangs off the chunk its foot stands in, so it is drawn when that
//! ground is drawn and goes away with it. Nothing here streams, caches or
//! culls on its own: a tree is part of a chunk, and the chunk already knows
//! when it is wanted.

use bevy::prelude::*;
use protocol::ground::{Kind, Plant, CHUNK_METRES};

use crate::terrain::{Ground, PendingPlants, TerrainChunk};
use crate::{matte, model_mesh, AppState};

/// The plants, as files. Each is built from the master of the same name under
/// `assets-src/models/` by `assets-src/models/export.sh`, which is where the
/// export settings the look depends on are written down — including the ones
/// that carry a plant's own colours through.
const PALM: &str = "models/palm.glb";
const BANANA: &str = "models/banana.glb";

/// Which mesh in each of them the plant is. A plant is one mesh, so there is
/// only ever the one — pinned, with the name, by
/// `every_model_is_one_painted_plant_fit_to_draw`.
const THE_PLANT: usize = 0;

/// One mesh per kind, and one material for every plant in the world, loaded
/// once.
///
/// The material is white and does nothing but let the vertices through. That
/// is what makes a plant one entity: a tree was two meshes because it was two
/// materials, and a model that carries its own colours is neither.
#[derive(Resource)]
struct PlantModels {
    palm: Handle<Mesh>,
    banana: Handle<Mesh>,
    /// White, matte, and shared by every kind — see [`matte`].
    painted: Handle<StandardMaterial>,
}

pub struct TreesPlugin;

impl Plugin for TreesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, load_the_model)
            .add_systems(Update, plant.run_if(in_state(AppState::InWorld)));
    }
}

fn load_the_model(
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    assets: Res<AssetServer>,
) {
    commands.insert_resource(PlantModels {
        palm: assets.load(model_mesh(PALM, THE_PLANT)),
        banana: assets.load(model_mesh(BANANA, THE_PLANT)),
        painted: materials.add(matte(Color::WHITE)),
    });
}

/// Turns the plants a chunk arrived carrying into trees standing on it.
///
/// The height comes from [`Ground`] rather than from the wire: a plant's foot
/// has to sit on the surface the client draws and the boat floats on, which is
/// the interpolated height field, and that is the only place that answer
/// lives. A chunk being planted has by definition arrived, so the lookup
/// cannot miss — but it is written to skip rather than to unwrap, because a
/// tree quietly not planted is a better failure than a panicked frame.
fn plant(
    mut commands: Commands,
    model: Res<PlantModels>,
    ground: Res<Ground>,
    pending: Query<(Entity, &PendingPlants, &TerrainChunk)>,
) {
    for (chunk, plants, at_chunk) in &pending {
        // Taken off the entity whether or not every tree on it stood up, so a
        // chunk is never planted twice.
        commands.entity(chunk).remove::<PendingPlants>();

        let origin = at_chunk.coords.as_vec2() * CHUNK_METRES;
        for plant in &plants.0 {
            // Which model to stand here. A match rather than a table lookup
            // so that a kind added to the wire cannot compile until this
            // module has decided what it looks like — the alternative is a
            // client that quietly stands nothing where a mangrove was sent.
            let (name, mesh) = match plant.kind {
                Kind::Palm => ("Palm", &model.palm),
                Kind::Banana => ("Banana", &model.banana),
            };
            let at = stands_at(origin, plant);
            let Some(surface) = ground.surface(at.x, at.y) else {
                continue;
            };
            // One entity: one mesh, one material, and the chunk for a parent
            // so that a tree is despawned with the ground it stands on rather
            // than needing a lifetime of its own. A tree is a hundred-odd
            // triangles however it is drawn, and it is entities a world full
            // of plants runs out of first.
            commands.entity(chunk).with_children(|under| {
                under.spawn((
                    Name::new(name),
                    Transform::from_xyz(plant.at.x, surface, plant.at.y)
                        .with_rotation(Quat::from_rotation_y(plant.yaw))
                        .with_scale(Vec3::splat(plant.scale)),
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(model.painted.clone()),
                ));
            });
        }
    }
}

/// Where a plant's foot stands in the world, for a chunk at `origin`.
///
/// Here rather than inline so the tests can say the same thing the drawing
/// does — a plant's position is chunk-local on the wire and world-absolute on
/// the ground, and the two differ by exactly the chunk's own corner.
pub fn stands_at(origin: Vec2, plant: &Plant) -> Vec2 {
    origin + plant.at
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::testing::{assert_model_draws, creature_named_by, mesh_names, model, triangles};

    #[test]
    fn every_model_is_one_painted_plant_fit_to_draw() {
        // See `assert_model_draws` for the three conditions every model in the
        // game is held to. The two after it are this module's own: a plant is
        // one mesh, and it carries its own colours — the material it is drawn
        // with is white, so an unpainted mesh would arrive as a white tree
        // rather than as an obviously broken one.
        for file in [PALM, BANANA] {
            let named = creature_named_by(file);
            assert_model_draws(file, &[(THE_PLANT, named)]);
            assert_eq!(mesh_names(file), [named], "a plant is one mesh");

            let (json, _) = model(file);
            let attributes = &json["meshes"][THE_PLANT]["primitives"][0]["attributes"];
            assert!(
                !attributes["COLOR_0"].is_null(),
                "{file} carries no colours — see its NOTES.md, and the export \
                 settings that pass them through"
            );
        }
    }

    #[test]
    fn every_plant_stands_on_its_own_origin() {
        // What `plant` assumes when it puts a foot at the surface height: the
        // model's own foot is at its origin, so a trunk whose geometry started
        // a metre up would hover, and one that started below would be driven
        // into the ground.
        for file in [PALM, BANANA] {
            let (json, _) = model(file);
            let tree = &json["meshes"][THE_PLANT]["primitives"][0];
            let positions =
                &json["accessors"][tree["attributes"]["POSITION"].as_u64().unwrap() as usize];
            let low = positions["min"][1].as_f64().unwrap();
            assert!(
                low.abs() < 1e-4,
                "{file} starts at {low} rather than at the ground"
            );
        }
    }

    #[test]
    fn the_crown_maps_onto_itself_at_no_rotation() {
        // What stops a stand of palms reading as one tree stamped repeatedly:
        // seven fronds at exactly a seventh of a turn apart look the same from
        // seven directions, and the server only ever turns a palm about the
        // vertical — so a regular crown would tile however it was placed.
        //
        // Tested as the property itself rather than by any proxy for it. An
        // earlier version measured the crown's centre of mass off the trunk
        // and read 14mm on a crown that is visibly lopsided: fronds radiate,
        // so averaging their corners hides exactly what is being asked about.
        // This turns the crown instead and asks how well it lands on itself.
        //
        // Which corners are the crown is a thing the file now says rather than
        // a mesh apart: a palm is one painted mesh, and a corner painted
        // greener than it is red is a frond where the trunk's timber is the
        // other way about. That is a better question than the old one anyway —
        // it asks after the green of the tree, which is what tiles visibly.
        let corners: Vec<Vec3> = triangles(PALM, THE_PLANT, "POSITION")
            .into_iter()
            .flatten()
            .zip(triangles(PALM, THE_PLANT, "COLOR_0").into_iter().flatten())
            .filter(|(_, paint)| paint.y > paint.x)
            .map(|(corner, _)| corner)
            .collect();
        assert!(
            corners.len() > 20,
            "only {} corners of the palm are green — the crown has lost its \
             paint, or the trunk has taken it",
            corners.len()
        );
        let axis = corners.iter().sum::<Vec3>() / corners.len() as f32;

        for seventh in 1..7 {
            let angle = std::f32::consts::TAU * seventh as f32 / 7.0;
            let (sin, cos) = angle.sin_cos();
            let mismatch = corners
                .iter()
                .map(|corner| {
                    let (dx, dz) = (corner.x - axis.x, corner.z - axis.z);
                    let turned = Vec3::new(
                        axis.x + dx * cos - dz * sin,
                        corner.y,
                        axis.z + dx * sin + dz * cos,
                    );
                    // How far the nearest original corner is from where this
                    // one landed. Near zero for every corner would mean the
                    // turn had reproduced the crown.
                    corners
                        .iter()
                        .map(|other| (*other - turned).length())
                        .fold(f32::MAX, f32::min)
                })
                .fold(f32::MIN, f32::max);

            assert!(
                mismatch > 0.25,
                "turning the crown by {seventh}/7 lands it back on itself to \
                 within {mismatch}m — it would tile when the server turns it"
            );
        }
    }

    #[test]
    fn a_palm_stands_where_the_wire_put_it() {
        let origin = Vec2::new(-256.0, 384.0);
        let palm = Plant {
            kind: Kind::Palm,
            at: Vec2::new(3.0, 120.0),
            yaw: 0.0,
            scale: 1.0,
        };
        assert_eq!(stands_at(origin, &palm), Vec2::new(-253.0, 504.0));
    }
}
