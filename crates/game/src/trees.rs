//! The palms along the back of a beach.
//!
//! The server says where they stand; this decides what one looks like. That
//! split is the same one the ground is drawn on — a chunk arrives as heights
//! and palette entries, and what green means is the client's business — and it
//! is why a palm crosses the wire as a position, a bearing and a size rather
//! than as anything to do with trunks or fronds.
//!
//! A palm hangs off the chunk its foot stands in, so it is drawn when that
//! ground is drawn and goes away with it. Nothing here streams, caches or
//! culls on its own: a tree is part of a chunk, and the chunk already knows
//! when it is wanted.

use bevy::prelude::*;
use protocol::ground::{Palm, CHUNK_METRES};

use crate::terrain::{Ground, PendingPalms, TerrainChunk};
use crate::{matte, model_mesh, AppState};

/// The palm, as a file. Built from `assets-src/models/palm/palm.blend` by
/// `assets-src/models/export.sh`, which is where the export settings the look
/// depends on are written down.
const MODEL: &str = "models/palm.glb";

/// Which mesh in [`MODEL`] is which — positions in the file, as the boat's
/// are, and pinned by `the_model_is_a_crown_and_a_trunk_fit_to_draw` for the same
/// reason. Note the order: the exporter writes meshes by name rather than in
/// the order the objects were made, so the fronds come first.
const FRONDS_MESH: usize = 0;
const TRUNK_MESH: usize = 1;

/// Timber, a shade browner and darker than the sand a palm stands on so the
/// trunk reads against it at any zoom. Deliberately not the boat's — a hull is
/// meant to be findable in a landscape and a tree is meant to belong to one.
const TRUNK_COLOR: Color = Color::srgb(0.42, 0.33, 0.24);

/// Frond green. Darker and yellower than the grass behind a beach and lighter
/// than the forest above it, so a stand of palms is its own band of colour
/// rather than an outcrop of whatever it is standing in front of.
const FROND_COLOR: Color = Color::srgb(0.31, 0.50, 0.20);

/// The two meshes and two materials every palm in the world shares, loaded
/// once. Sharing them is what lets the whole beach draw in as few calls as
/// there are materials, rather than one apiece.
#[derive(Resource)]
struct PalmModel {
    fronds: Handle<Mesh>,
    trunk: Handle<Mesh>,
    frond_material: Handle<StandardMaterial>,
    trunk_material: Handle<StandardMaterial>,
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
    commands.insert_resource(PalmModel {
        fronds: assets.load(model_mesh(MODEL, FRONDS_MESH)),
        trunk: assets.load(model_mesh(MODEL, TRUNK_MESH)),
        frond_material: materials.add(matte(FROND_COLOR)),
        trunk_material: materials.add(matte(TRUNK_COLOR)),
    });
}

/// Turns the palms a chunk arrived carrying into trees standing on it.
///
/// The height comes from [`Ground`] rather than from the wire: a palm's foot
/// has to sit on the surface the client draws and the boat floats on, which is
/// the interpolated height field, and that is the only place that answer
/// lives. A chunk being planted has by definition arrived, so the lookup
/// cannot miss — but it is written to skip rather than to unwrap, because a
/// palm quietly not planted is a better failure than a panicked frame.
fn plant(
    mut commands: Commands,
    model: Res<PalmModel>,
    ground: Res<Ground>,
    pending: Query<(Entity, &PendingPalms, &TerrainChunk)>,
) {
    for (chunk, palms, at_chunk) in &pending {
        // Taken off the entity whether or not every tree on it stood up, so a
        // chunk is never planted twice.
        commands.entity(chunk).remove::<PendingPalms>();

        let origin = at_chunk.coords.as_vec2() * CHUNK_METRES;
        for palm in &palms.0 {
            let at = stands_at(origin, palm);
            let Some(surface) = ground.surface(at.x, at.y) else {
                continue;
            };
            // Where the tree stands, in the chunk's own frame. Both halves of
            // it are given the same one: a palm is two entities rather than a
            // parent and two children, because it is two meshes only for as
            // long as it is two materials, and once they both know where they
            // stand there is nothing left for a third entity to hold. A tree
            // is a hundred-odd triangles either way, and it is entities a
            // world full of plants runs out of first.
            let stands = Transform::from_xyz(palm.at.x, surface, palm.at.y)
                .with_rotation(Quat::from_rotation_y(palm.yaw))
                .with_scale(Vec3::splat(palm.scale));
            // Placed under the chunk they belong to, so they are despawned
            // with the ground rather than needing a lifetime of their own.
            commands.entity(chunk).with_children(|under| {
                under.spawn((
                    Name::new("Palm trunk"),
                    stands,
                    Mesh3d(model.trunk.clone()),
                    MeshMaterial3d(model.trunk_material.clone()),
                ));
                under.spawn((
                    Name::new("Palm crown"),
                    stands,
                    Mesh3d(model.fronds.clone()),
                    MeshMaterial3d(model.frond_material.clone()),
                ));
            });
        }
    }
}

/// Where a palm's foot stands in the world, for a chunk at `origin`.
///
/// Here rather than inline so the tests can say the same thing the drawing
/// does — a palm's position is chunk-local on the wire and world-absolute on
/// the ground, and the two differ by exactly the chunk's own corner.
pub fn stands_at(origin: Vec2, palm: &Palm) -> Vec2 {
    origin + palm.at
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::testing::{assert_model_draws, model, triangles};

    #[test]
    fn the_model_is_a_crown_and_a_trunk_fit_to_draw() {
        // See `assert_model_draws`. The order has a sharper edge here than on
        // the boat: glTF numbers its meshes in the order the *exporter* wrote
        // them, which is by name — so the fronds come first despite the trunk
        // being modelled first, and anything that reasoned from the modelling
        // order would paint the crown in bark.
        assert_model_draws(MODEL, &[(FRONDS_MESH, "fronds"), (TRUNK_MESH, "trunk")]);
    }

    #[test]
    fn the_palm_stands_on_its_own_origin() {
        // What `plant` assumes when it puts a palm's foot at the surface
        // height: the model's own foot is at its origin, so a trunk whose
        // geometry started a metre up would hover, and one that started below
        // would be driven into the sand.
        let (json, _) = model(MODEL);
        let trunk = &json["meshes"][TRUNK_MESH]["primitives"][0];
        let positions =
            &json["accessors"][trunk["attributes"]["POSITION"].as_u64().unwrap() as usize];
        let low = positions["min"][1].as_f64().unwrap();
        assert!(
            low.abs() < 1e-4,
            "the trunk starts at {low} rather than at the ground"
        );
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
        let corners: Vec<Vec3> = triangles(MODEL, FRONDS_MESH, "POSITION")
            .into_iter()
            .flatten()
            .collect();
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
        let palm = Palm {
            at: Vec2::new(3.0, 120.0),
            yaw: 0.0,
            scale: 1.0,
        };
        assert_eq!(stands_at(origin, &palm), Vec2::new(-253.0, 504.0));
    }
}
