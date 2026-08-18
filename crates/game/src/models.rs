//! Dressing a spawned glTF scene: keeping the files' materials out of the
//! world, and finding what a loaded model belongs to.
//!
//! Most of what the game draws is pulled out of a file mesh by mesh — see
//! [`crate::model_mesh`] — and handed its material where it is spawned. A
//! *rigged* model cannot be: a skinned mesh has to arrive as a whole scene or
//! it is a shape with no skeleton behind it, so the file's own PBR materials
//! come along with it, and the dressing has to happen afterwards, as the
//! meshes turn up.
//!
//! Every model carries its own colours on its vertices, so there is nothing to
//! choose: whatever arrives is repainted with the one white matte that lets
//! those colours through. A model that lost its colour attribute would arrive
//! white, which is why each one's tests hold `COLOR_0` to being there.
//!
//! The material is made once and shared. Painting mesh by mesh as it arrived
//! minted a fresh `StandardMaterial` per animal — a bind group and a batch
//! break each, piling up for as long as the sharks lived.

use bevy::gltf::GltfMeshName;
use bevy::prelude::*;

use crate::{matte, AppState};

/// The one material every scene-spawned mesh is dressed in: white, matte,
/// and nothing but a window for the colours a model carries itself.
#[derive(Resource)]
struct Painted(Handle<StandardMaterial>);

fn mix(mut commands: Commands, mut materials: ResMut<Assets<StandardMaterial>>) {
    commands.insert_resource(Painted(materials.add(matte(Color::WHITE))));
}

/// The nearest ancestor of `entity` carrying `M` — what a loaded scene's
/// insides are asked to know about themselves.
///
/// The loader builds a model as a tree of nodes and hangs its
/// [`bevy::animation::AnimationPlayer`] on whatever it found animated, which
/// is somewhere in the middle of it. Everything that wants to do something
/// with an arriving model therefore has to climb out of it to find what it
/// is part of — whether that is "a figure, so this gait is the walker's" or
/// "a beast, so this tail is paced by its swimming".
pub fn above<M: Component>(
    hierarchy: &Query<&ChildOf>,
    marked: &Query<(), With<M>>,
    entity: Entity,
) -> Option<Entity> {
    hierarchy
        .iter_ancestors(entity)
        .find(|above| marked.contains(*above))
}

/// Repaints every arriving mesh white, throwing away what came out of the
/// file.
///
/// glTF materials are PBR — a roughness, a metalness, a specular response —
/// and the look here is flat tones on the models' own vertices under
/// [`matte`], so a model lit the way its file asked for would be the one
/// surface in the world with a highlight on it.
fn paint(
    painted: Res<Painted>,
    mut arrivals: Query<&mut MeshMaterial3d<StandardMaterial>, Added<GltfMeshName>>,
) {
    for mut material in &mut arrivals {
        *material = MeshMaterial3d(painted.0.clone());
    }
}

pub struct ModelsPlugin;

impl Plugin for ModelsPlugin {
    fn build(&self, app: &mut App) {
        // In the world only: every rigged model here is spawned into a match,
        // so there is nothing to paint on a menu screen. A frame this sits
        // out is not a frame of arrivals missed — `Added` is measured from
        // when this system last ran, not from last frame.
        app.add_systems(Startup, mix)
            .add_systems(Update, paint.run_if(in_state(AppState::InWorld)));
    }
}
