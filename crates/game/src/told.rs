//! Drawing a thing the world owns: eased onto the last word about where it is.
//!
//! Three things on screen are somebody else's to decide — the marker another
//! player stands as, a hull nobody here is steering, a beast — and all three
//! are drawn the same way because all three *arrive* the same way. A telling
//! is a point and sometimes a bearing, a few a second; what stands between two
//! of them is this machine's own invention, and inventing it three times was
//! three rates, three spellings of a yaw, and a fourth waiting for the next
//! thing a server learns to say.
//!
//! What is deliberately not here is the height. A marker stands a capsule's
//! half-length above the surface, a hull sits on it, a shark rides some way
//! under it, and those are three questions rather than one — see
//! [`crate::sea::SeaConditions::surface_over`], which is the part of it the
//! first two do share.

use bevy::prelude::*;

use crate::eased;

/// Where the world last said a thing is, and how quickly it is drawn closing
/// on that.
///
/// Held by everything the wire moves and written by nothing else: a telling
/// overwrites this whole, so the component is the last word rather than a
/// running total. What a *client* steers — this player's own hull — carries no
/// `Told` at all, its place being its own to invent.
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct Told {
    /// Where it is, on the ground plane.
    pub at: Vec2,
    /// Which way it is pointed, as a yaw about the vertical. `None` leaves
    /// the bearing alone, which is what a thing with no bearing to draw
    /// wants — a capsule marker — and also what a moving thing that happens
    /// to be stopped wants, a velocity of nothing naming no direction.
    pub facing: Option<f32>,
    /// How quickly the place closes, in e-foldings per second — see
    /// [`eased`].
    pub closing: f32,
    /// How quickly the bearing swings, likewise. Its own number because a
    /// body that turns as fast as it slides reads as a compass needle rather
    /// than as something with a keel.
    pub swinging: f32,
}

/// Eases `transform` a frame's worth onto `told`, and says where on the
/// ground plane that left it — which is the point every caller then asks its
/// own question about the height at.
///
/// The height is not touched. A caller that has nothing to say about it
/// leaves whatever was there, which is what keeps a thing over ground that
/// has not arrived at the height it had rather than dropping it to the
/// waterline and climbing back out.
pub fn eased_onto(transform: &mut Transform, told: &Told, dt: f32) -> Vec2 {
    let at = Vec2::new(transform.translation.x, transform.translation.z)
        .lerp(told.at, eased(told.closing, dt));
    transform.translation.x = at.x;
    transform.translation.z = at.y;
    if let Some(facing) = told.facing {
        let onto = Quat::from_rotation_y(facing);
        transform.rotation = transform.rotation.slerp(onto, eased(told.swinging, dt));
    }
    at
}

/// The yaw that points along a velocity, or `None` for one going nowhere.
///
/// Here rather than at the one caller that works a bearing out of movement,
/// because it is the same `-z is forward` convention [`eased_onto`] reads
/// `facing` in, and the two would be a pair of sign conventions to keep in
/// step if they were written apart.
pub fn heading_of(velocity: Vec2) -> Option<f32> {
    (velocity != Vec2::ZERO).then(|| f32::atan2(-velocity.x, -velocity.y))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn told(at: Vec2, facing: Option<f32>) -> Told {
        Told {
            at,
            facing,
            closing: 8.0,
            swinging: 8.0,
        }
    }

    #[test]
    fn a_telling_is_closed_on_rather_than_snapped_to() {
        // The whole point of the easing: a few tellings a second become
        // movement, so one frame of it must land between where the thing was
        // and where it was told to be, and never at either end.
        let mut transform = Transform::from_xyz(0.0, 3.0, 0.0);
        let at = eased_onto(
            &mut transform,
            &told(Vec2::new(10.0, 0.0), None),
            1.0 / 60.0,
        );
        assert!(at.x > 0.0 && at.x < 10.0, "one frame went to {at:?}");
        assert_eq!(transform.translation.xz(), at);
        // And the height is the caller's, untouched by any of it.
        assert_eq!(transform.translation.y, 3.0);
    }

    #[test]
    fn enough_frames_arrive() {
        let mut transform = Transform::default();
        let mut at = Vec2::ZERO;
        for _ in 0..600 {
            at = eased_onto(
                &mut transform,
                &told(Vec2::new(-4.0, 7.0), None),
                1.0 / 60.0,
            );
        }
        assert!(
            at.distance(Vec2::new(-4.0, 7.0)) < 1e-3,
            "ten seconds left it at {at:?}"
        );
    }

    #[test]
    fn a_bearing_of_none_leaves_the_thing_pointed_where_it_was() {
        // What a capsule marker wants: it has no bearing on the wire at all,
        // and a `facing` of zero would be the world spinning it north.
        let pointed = Quat::from_rotation_y(1.2);
        let mut transform = Transform::from_rotation(pointed);
        eased_onto(&mut transform, &told(Vec2::new(1.0, 1.0), None), 1.0 / 60.0);
        assert_eq!(transform.rotation, pointed);
    }

    #[test]
    fn a_bearing_is_swung_onto_and_not_snapped_either() {
        let mut transform = Transform::default();
        for _ in 0..600 {
            eased_onto(&mut transform, &told(Vec2::ZERO, Some(1.0)), 1.0 / 60.0);
        }
        let (yaw, _, _) = transform.rotation.to_euler(EulerRot::YXZ);
        assert!(
            (yaw - 1.0).abs() < 1e-3,
            "ten seconds of swinging reached {yaw}"
        );
    }

    #[test]
    fn a_velocity_names_the_bearing_a_told_facing_means() {
        // The two conventions have to agree or a beast swims backwards: a
        // thing moving along -z is pointed the way `Transform::forward` looks.
        let facing = heading_of(Vec2::new(0.0, -1.0)).expect("a bearing");
        let pointed = Quat::from_rotation_y(facing) * Vec3::NEG_Z;
        assert!(
            (pointed - Vec3::NEG_Z).length() < 1e-6,
            "pointed {pointed:?}"
        );
        assert_eq!(
            heading_of(Vec2::ZERO),
            None,
            "a standstill names no bearing"
        );
    }
}
