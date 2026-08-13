//! Dressing a spawned glTF scene: the world's tones, and finding what a
//! loaded model belongs to.
//!
//! Most of what the game draws is pulled out of a file mesh by mesh — see
//! [`crate::model_mesh`] — and painted where it is spawned. A *rigged* model
//! cannot be: a skinned mesh has to arrive as a whole scene or it is a shape
//! with no skeleton behind it, so the file's own PBR materials come along
//! with it, and the painting has to happen afterwards, as the meshes turn
//! up. Both rigged models here were doing that for themselves with the same
//! system written twice, each scanning every arriving mesh in the world to
//! find its own handful.
//!
//! So the palette is one table. A module that dresses a model registers what
//! its meshes are called and what each is painted, and [`paint`] does the
//! rest for every model there will ever be — the third rigged thing the
//! world grows is a table, not a third copy of a system.
//!
//! Registering also means the material is made *once*. Painting mesh by mesh
//! as it arrived minted a fresh `StandardMaterial` per animal, so every shark
//! that swam past was its own bind group and its own batch break, and the
//! assets piled up for as long as the sharks lived. The rigid kinds already
//! knew better — a whole pod of dolphins draws off one handle — and this is
//! that arrangement extended to the ones that arrive as scenes.

use bevy::gltf::GltfMeshName;
use bevy::prelude::*;

use crate::{matte, AppState};

/// The tones every model in the world is painted in, by the mesh name that
/// asks for each.
///
/// One table across all models, so the names are one namespace: two models
/// wanting different colours must not both call a mesh `hide`. That is a
/// condition worth being loud about rather than one to be discovered by a
/// shark coming out the colour of a coat, so [`Tones::register`] refuses a
/// name twice.
#[derive(Resource, Default)]
pub struct Tones(Vec<(&'static str, Handle<StandardMaterial>)>);

impl Tones {
    /// Adds a model's palette: the mesh names it carries, and what each is
    /// painted. Called once at startup by whichever module dresses the model
    /// — the materials are made here, and every copy of that model shares
    /// them.
    pub fn register(
        &mut self,
        materials: &mut Assets<StandardMaterial>,
        tones: &[(&'static str, Color)],
    ) {
        for (name, colour) in tones {
            assert!(
                !self.0.iter().any(|(known, _)| known == name),
                "two models both call a mesh `{name}` — the tones are one namespace"
            );
            self.0.push((name, materials.add(matte(*colour))));
        }
    }

    /// What a mesh of that name is painted, if anything here paints it. A
    /// mesh nobody claims keeps whatever the file gave it, which is how it
    /// shows: it arrives with a highlight on it, and nothing else in the
    /// world has one.
    fn of(&self, mesh: &str) -> Option<Handle<StandardMaterial>> {
        self.0
            .iter()
            .find(|(named, _)| *named == mesh)
            .map(|(_, tone)| tone.clone())
    }
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

/// Paints arriving meshes in the world's own tones, throwing away what came
/// out of the file.
///
/// glTF materials are PBR — a roughness, a metalness, a specular response —
/// and the look here is a small fixed palette under [`matte`], so a model lit
/// the way its file asked for would be the one surface in the world with a
/// highlight on it.
fn paint(
    tones: Res<Tones>,
    mut arrivals: Query<
        (&GltfMeshName, &mut MeshMaterial3d<StandardMaterial>),
        Added<GltfMeshName>,
    >,
) {
    for (mesh, mut material) in &mut arrivals {
        if let Some(tone) = tones.of(&mesh.0) {
            *material = MeshMaterial3d(tone);
        }
    }
}

pub struct ModelsPlugin;

impl Plugin for ModelsPlugin {
    fn build(&self, app: &mut App) {
        // In the world only: every rigged model here is spawned into a match,
        // so there is nothing to paint on a menu screen. A frame this sits
        // out is not a frame of arrivals missed — `Added` is measured from
        // when this system last ran, not from last frame.
        app.init_resource::<Tones>()
            .add_systems(Update, paint.run_if(in_state(AppState::InWorld)));
    }
}
