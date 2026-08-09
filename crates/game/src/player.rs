//! The player as a person, distinct from whatever is carrying them.
//!
//! The boat used to *be* the player: one entity, driven by the movement keys,
//! followed by the camera, reported to the server. That held only as long as
//! there was exactly one way to exist in the world, and there are about to be
//! several — the ship, a rowboat to get ashore in, the player's own feet on an
//! island — all of them one person getting about by different means.
//!
//! So the person is an entity of their own, and being aboard is [`ChildOf`]:
//! the player rides the scene graph, standing wherever their vehicle carries
//! them, and stepping ashore will one day be nothing more than leaving the
//! hierarchy. Nothing draws them yet — from a camera forty metres up there is
//! nothing worth drawing — but the entity is where a figure will hang when
//! there is one, already in the right frame.
//!
//! Everything that wants "where the player is" — the camera, the position
//! reports, a `--shot`'s teleport — asks [`PlayerPlace`] for the *carrier*:
//! the vehicle the player is aboard, or the player themself on their own
//! feet. Those systems neither know nor care which it is, and that is the
//! point: a rowboat, when it comes, changes what the player boards and
//! nothing about what follows them.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

/// The person playing: one per match. Spawned aboard the ship the world is
/// entered on — by `boat::launch`, entering being done afloat — and despawned
/// with it, the hierarchy going down as one.
#[derive(Component)]
pub struct Player;

/// Where the player is, resolved through whatever they are aboard.
#[derive(SystemParam)]
pub struct PlayerPlace<'w, 's> {
    players: Query<'w, 's, (Entity, Option<&'static ChildOf>), With<Player>>,
}

impl PlayerPlace<'_, '_> {
    /// The entity carrying the player through the world: the vehicle they are
    /// aboard, or the player themself on their own feet. This is the entity
    /// to follow and the entity to move — a teleport moves the carrier whole,
    /// vehicle and rider together, never the rider out of the vehicle. `None`
    /// outside a match, there being nobody playing yet.
    pub fn carrier(&self) -> Option<Entity> {
        let (player, aboard) = self.players.single().ok()?;
        Some(aboard.map_or(player, ChildOf::parent))
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::SystemState;

    use super::*;

    fn carrier_in(world: &mut World) -> Option<Entity> {
        SystemState::<PlayerPlace>::new(world)
            .get(world)
            .expect("a query param is always valid")
            .carrier()
    }

    #[test]
    fn a_player_on_their_own_feet_carries_themself() {
        let mut world = World::new();
        let player = world.spawn((Player, Transform::default())).id();
        assert_eq!(carrier_in(&mut world), Some(player));
    }

    #[test]
    fn a_player_aboard_is_carried_by_the_vehicle() {
        let mut world = World::new();
        let vehicle = world.spawn(Transform::default()).id();
        world.spawn((Player, Transform::default(), ChildOf(vehicle)));
        assert_eq!(carrier_in(&mut world), Some(vehicle));
    }

    #[test]
    fn no_player_names_no_carrier() {
        // The menu, a shot of the menu: nobody is playing, and everything
        // that asks has a nothing-to-do path rather than a panic.
        let mut world = World::new();
        assert_eq!(carrier_in(&mut world), None);
    }
}
