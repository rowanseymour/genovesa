//! The waterline: the plane the hulls are actually solved on, and the seam
//! between it and the world's three axes.
//!
//! A boat's story happens in two dimensions. It floats, so it never falls;
//! it heels and it pitches, but only to be looked at. Where a hull *is* and
//! which way it points are settled on the flat, and every other thing the
//! eye gets is hung off that afterwards — see [`crate::boat::float`], which
//! puts the hull on the water and tilts it to the swell once this plane has
//! said where it lies.
//!
//! So the solver is a two-dimensional one, and everything here is the
//! translation. The plane is the world with its height dropped: `x` is `x`
//! and `y` is `z`. A hull's own frame lands tidily on that — the world's -Z
//! is the bow and its +X is starboard, so on the plane the bow is -Y and
//! starboard is +X. A hull is a rectangle a beam wide and a length tall, and
//! a drive along the keel is a force along its own -Y.
//!
//! That leaves one sign, and it is kept in exactly one place — [`across`].
//! A yaw about the vertical spins the opposite way from an angle read on the
//! (x, z) plane, because dropping a left-handed axis is what the mapping
//! does. A codebase with that flip written in two places is one with it
//! written wrong in one of them.
//!
//! # Why the solver runs in `Update`
//!
//! Avian's own default is a fixed timestep in `FixedPostUpdate`, with the
//! drawn pose interpolated between steps. This runs it in `Update` on the
//! frame's own delta instead, which is what every other easing in this
//! client is already timed by — the sailing, the swell, the moorings, the
//! oars. One clock is worth more here than the fidelity a fixed step buys:
//! there is no stack to settle and no tower to topple, just a few hulls
//! nudging each other, and a second clock would mean a drawn hull that is
//! interpolated while the wake laid off it is not.
//!
//! # What is a body and what is not
//!
//! Only hulls. The ground is not a collider — it is a height field, and
//! `boat::hold_the_ground` keeps the keel out of it by the same probe that
//! always did, run as a correction after the solver rather than as a gate
//! before it. Contouring every chunk into a polyline as it streamed in
//! would be a second answer to a question the height field already answers,
//! and the two would disagree at the shoreline, which is the one place a
//! player is looking.
//!
//! Every hull is the same kind of body — [`avian2d::prelude::RigidBody::Dynamic`],
//! carrying its own displacement — including the ones nobody here is
//! steering. That is not a hole in the authority split; it is what makes the
//! split cost nothing. A collision is decided by two masses and two
//! velocities, and a hull the wire moved used to have neither: it was an
//! infinite mass lying still, so a dinghy checked a ship exactly as hard as
//! a ship checked a dinghy, and two boats closing at twenty knots met as one
//! boat hitting a moored one.
//!
//! So every client solves every hull, and keeps only its own answer. What a
//! client may not do is *decide* where somebody else's hull ends up:
//! [`crate::boat::follow_the_telling`] steers a told hull onto the last word
//! about it, so the give it took a moment ago decays into what the wire says
//! — which is usually the same give, the client whose boat it is having
//! solved the same collision from the other side. The one hull with no
//! authority behind it is an empty one, and this client takes that up for as
//! long as it is pushing it; see [`crate::boat::claim_the_shoved`].

use avian2d::physics_transform::PhysicsTransformConfig;
use avian2d::prelude::*;
use bevy::prelude::*;

use crate::AppState;

/// How bouncy a hull is against another, from nothing to perfectly
/// elastic. Low: two wooden boats coming together stop, they do not
/// twang apart, and what a player should get from a bump is the bow
/// knocked off its course rather than the pair of them flung.
const PLANKING_BOUNCE: f32 = 0.15;

/// How much a hull's planking drags along another's as they slide past.
/// Wet timber on wet timber, and low for the reason the bounce is: what a
/// hull alongside should do is slide clear, not catch.
const PLANKING_DRAG: f32 = 0.2;

/// A yaw about the vertical, as an angle on the water plane — and back
/// again, the two being the same negation.
///
/// The whole of the sign this mapping costs. Bevy's yaw turns +X towards
/// -Z, and an angle read on the plane this module lays down turns +X
/// towards +Z, because the plane is the world with a left-handed axis
/// dropped out of it. So the two run opposite, and every crossing goes
/// through here rather than through a minus sign somebody has to notice.
pub fn across(radians: f32) -> f32 {
    -radians
}

/// Where a point in the world lies on the plane.
pub fn on_the_plane(at: Vec3) -> Vec2 {
    Vec2::new(at.x, at.z)
}

/// Which way a hull's bow points, as a direction on the plane.
///
/// Which is also its heading in the world's x and z, and not by
/// coincidence: the mapping drops an axis and renames another, so it leaves
/// *vectors* alone and touches only the angle that spins them. That is why
/// this can be handed straight to the sail and oar polars in `boat`, which
/// were written against a transform's forward and have not had to change.
pub fn bow(rotation: &Rotation) -> Vec2 {
    let (sin, cos) = rotation.as_radians().sin_cos();
    Vec2::new(sin, -cos)
}

/// The angle a hull pointed along `along` holds — the inverse of [`bow`],
/// for the one hull whose heading is decided by where it is being dragged.
pub fn pointing(along: Vec2) -> f32 {
    f32::atan2(along.x, -along.y)
}

/// The plane, as the rest of the client meets it: the solver, wound down
/// to the two dimensions a boat is decided in, with its own idea of where
/// things are kept off the transforms.
pub struct WaterlinePlugin;

impl Plugin for WaterlinePlugin {
    fn build(&self, app: &mut App) {
        // Before the plugins, which read it as they are built: Avian's own
        // sync writes a plane position into a transform's x and y, which
        // for a world whose up is y would lay every boat on its side in the
        // sky. Both directions come off, replaced by `crate::boat`'s own
        // sync, which knows that the plane's y is the world's z and that
        // the height and the tilt belong to somebody else.
        app.insert_resource(PhysicsTransformConfig {
            transform_to_position: false,
            position_to_transform: false,
            ..default()
        })
        // A hull floats: it is held up by water that is not modelled. Down
        // is not a direction on this plane at all.
        .insert_resource(Gravity(Vec2::ZERO))
        .add_plugins(PhysicsPlugins::new(Update))
        .add_systems(
            Update,
            // The pause menu stops the water as well as the helm. Nothing
            // else here needs telling: the drawing systems re-derive
            // themselves from poses that have stopped changing, which is
            // what they already did when the way was a number that froze.
            hold_the_clock.before(PhysicsSystems::Prepare),
        )
        .add_systems(OnExit(AppState::InWorld), still_the_water);
    }
}

/// What makes a hull a body on the plane: its shape, its weight and its
/// manners in a collision. `length` and `beam` are the hull's own, in
/// metres, and `displacement` its mass in kilogrammes — the one number
/// here that does nothing until two hulls touch.
///
/// The rectangle is the planking read from above, which is the shape a
/// boat meeting a boat is: fine ends earn nothing at the speeds anything
/// closes at here, and cost a contact manifold that jitters where two
/// curved bows meet.
pub fn afloat(length: f32, beam: f32, displacement: f32) -> impl Bundle {
    (
        RigidBody::Dynamic,
        Collider::rectangle(beam, length),
        Mass(displacement),
        Restitution::new(PLANKING_BOUNCE),
        Friction::new(PLANKING_DRAG),
        // Opt-in, and asked for on every hull because what reads it is a
        // question about pairs: which empty boat this client has run into
        // and is therefore answering for — see
        // [`crate::boat::claim_the_shoved`].
        CollidingEntities::default(),
        // A solver puts a body that has stopped moving to sleep, to spare
        // itself the arithmetic. There are a handful of hulls on this
        // water and the arithmetic is nothing, while a boat asleep is a
        // boat that has to be woken by whatever wants to move it — and the
        // thing most likely to want to is a painter coming taut, which
        // wakes nobody. A dinghy left sleeping astern of a ship getting
        // under way simply stays where it was and the rope runs through
        // it, which is what this cost before it was turned off.
        SleepingDisabled,
    )
}

/// Where a body is and how it points, as the plane holds it — what a hull
/// is put down at when something other than the solver decides its pose.
pub fn laid_at(at: Vec2, yaw: f32) -> (Position, Rotation) {
    (Position(at), Rotation::radians(across(yaw)))
}

/// Holds the water still while the game is paused.
///
/// The hulls used to stop with the helm for nothing: the way was a number
/// on a component and the system that integrated it was the one the pause
/// switched off. A solver has its own clock and would sail a boat across
/// the chart the player was reading, so the clock is what is stopped now.
///
/// Not by taking the physics systems out of the schedule, which is the
/// other way to do it: the step's own sets have an order they rely on, and
/// a condition hung on the outside of them is a condition on somebody
/// else's arrangement.
fn hold_the_clock(helm: Option<Res<State<crate::Helm>>>, mut clock: ResMut<Time<Physics>>) {
    let sailing = helm.is_none_or(|helm| *helm.get() == crate::Helm::Sailing);
    if sailing == clock.is_paused() {
        if sailing {
            clock.unpause();
        } else {
            clock.pause();
        }
    }
}

/// Stops every hull dead when the world closes.
///
/// The bodies go with their entities, being `DespawnOnExit` like every
/// other piece of a world. What does not is the solver's own idea of what
/// is touching what, and a contact left standing between two despawned
/// hulls is a contact the next world's first frame resolves.
fn still_the_water(mut bodies: Query<(&mut LinearVelocity, &mut AngularVelocity)>) {
    for (mut linear, mut angular) in &mut bodies {
        linear.0 = Vec2::ZERO;
        angular.0 = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plane_and_the_world_agree_about_which_way_a_hull_faces() {
        // The one sign this mapping costs, pinned at both ends. A hull
        // pointed some way in the world has a bow on the plane, and the
        // plane's own answer — the body's local -Y, turned by its angle —
        // has to be the same vector.
        for yaw in [0.0, 0.7, -1.9, std::f32::consts::PI, 2.5] {
            let in_the_world = Quat::from_rotation_y(yaw) * Vec3::NEG_Z;
            let bow = on_the_plane(in_the_world);

            let angle = across(yaw);
            let (sin, cos) = angle.sin_cos();
            // The body's own -Y, turned by the angle the plane holds.
            let on_the_plane = Vec2::new(sin, -cos);

            assert!(
                bow.distance(on_the_plane) < 1e-5,
                "a hull at {yaw} rad has its bow at {bow} in the world and {on_the_plane} on the plane"
            );
        }
    }

    #[test]
    fn a_crossing_and_a_crossing_back_is_no_crossing_at_all() {
        // [`across`] is its own inverse, which is what lets one function
        // serve both directions — and is the whole reason there is one.
        for yaw in [0.0, 0.7, -1.9, 2.5] {
            assert_eq!(across(across(yaw)), yaw);
        }
    }
}
