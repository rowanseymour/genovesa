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
//! reports, the wildlife deciding whether to mind them — asks [`PlayerPlace`],
//! and a `--shot`'s teleport asks [`PlayerSweep`]. Both resolve through the
//! *carrier*: the vehicle the player is aboard, or the player themself on
//! their own feet. Those systems neither know nor care which it is, and that
//! is the point: a rowboat, when it comes, changes what the player boards and
//! nothing about what follows them.
//!
//! Reading and writing are two params rather than one because Bevy will not
//! let a system hold `&Transform` and `&mut Transform` at once, and the sweep
//! is the only thing that moves a player it did not spawn.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

/// The person playing: one per match. Spawned aboard the ship the world is
/// entered on — by `boat::launch`, entering being done afloat — and despawned
/// with it, the hierarchy going down as one.
#[derive(Component)]
pub struct Player;

/// The player and whatever they are aboard, as a query.
type Players<'w, 's> = Query<'w, 's, (Entity, Option<&'static ChildOf>), With<Player>>;

/// The entity carrying the player through the world: the vehicle they are
/// aboard, or the player themself on their own feet. This is the entity to
/// follow and the entity to move — a teleport moves the carrier whole,
/// vehicle and rider together, never the rider out of the vehicle. `None`
/// outside a match, there being nobody playing yet.
fn carrier_of(players: &Players) -> Option<Entity> {
    let (player, aboard) = players.single().ok()?;
    Some(aboard.map_or(player, ChildOf::parent))
}

/// Where the player is, resolved through whatever they are aboard.
#[derive(SystemParam)]
pub struct PlayerPlace<'w, 's> {
    players: Players<'w, 's>,
    carriers: Query<'w, 's, &'static Transform>,
}

impl PlayerPlace<'_, '_> {
    /// The carrier's entity — see [`carrier_of`].
    pub fn carrier(&self) -> Option<Entity> {
        carrier_of(&self.players)
    }

    /// Where the carrier stands in the world. `None` outside a match.
    pub fn at(&self) -> Option<Vec3> {
        let carrier = self.carrier()?;
        Some(self.carriers.get(carrier).ok()?.translation)
    }

    /// The same point on the map, which is what most callers are after — the
    /// height a hull is riding being the swell's business rather than
    /// anybody else's.
    pub fn on_the_map(&self) -> Option<Vec2> {
        self.at().map(|at| at.xz())
    }
}

/// The player as a thing a capture sweep can move.
#[derive(SystemParam)]
pub struct PlayerSweep<'w, 's> {
    players: Players<'w, 's>,
    carriers: Query<'w, 's, &'static mut Transform>,
}

impl PlayerSweep<'_, '_> {
    /// Moves whatever carries the player — vehicle and rider whole — to a map
    /// point, leaving the height stale for `float` to settle. Does nothing
    /// with no player in the world, which is every shot of a menu.
    pub fn teleport(&mut self, to: Vec2) {
        let Some(mut place) =
            carrier_of(&self.players).and_then(|carrier| self.carriers.get_mut(carrier).ok())
        else {
            return;
        };
        place.translation.x = to.x;
        place.translation.z = to.y;
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::SystemState;

    use super::*;

    fn place_in<T>(world: &mut World, read: impl FnOnce(PlayerPlace) -> T) -> T {
        read(
            SystemState::<PlayerPlace>::new(world)
                .get(world)
                .expect("a query param is always valid"),
        )
    }

    #[test]
    fn a_player_on_their_own_feet_carries_themself() {
        let mut world = World::new();
        let player = world
            .spawn((Player, Transform::from_xyz(4.0, 1.0, -9.0)))
            .id();
        assert_eq!(place_in(&mut world, |p| p.carrier()), Some(player));
        assert_eq!(
            place_in(&mut world, |p| p.on_the_map()),
            Some(Vec2::new(4.0, -9.0))
        );
    }

    #[test]
    fn a_player_aboard_is_carried_by_the_vehicle() {
        // The vehicle's spot, not the rider's identity transform inside it.
        let mut world = World::new();
        let vehicle = world.spawn(Transform::from_xyz(12.0, 0.5, 7.0)).id();
        world.spawn((Player, Transform::default(), ChildOf(vehicle)));
        assert_eq!(place_in(&mut world, |p| p.carrier()), Some(vehicle));
        assert_eq!(
            place_in(&mut world, |p| p.at()),
            Some(Vec3::new(12.0, 0.5, 7.0))
        );
    }

    #[test]
    fn no_player_names_no_carrier() {
        // The menu, a shot of the menu: nobody is playing, and everything
        // that asks has a nothing-to-do path rather than a panic.
        let mut world = World::new();
        assert_eq!(place_in(&mut world, |p| p.carrier()), None);
        assert_eq!(place_in(&mut world, |p| p.at()), None);
    }

    #[test]
    fn a_sweep_teleports_the_vehicle_and_takes_the_rider_along() {
        let mut world = World::new();
        let vehicle = world.spawn(Transform::from_xyz(1.0, 3.0, 2.0)).id();
        let player = world
            .spawn((Player, Transform::default(), ChildOf(vehicle)))
            .id();

        let mut state = SystemState::<PlayerSweep>::new(&mut world);
        state
            .get_mut(&mut world)
            .expect("a query param is always valid")
            .teleport(Vec2::new(-40.0, 80.0));
        state.apply(&mut world);

        // The carrier moved on the map and kept its stale height, and the
        // rider was not lifted out of the boat to do it.
        let moved = *world.entity(vehicle).get::<Transform>().expect("vehicle");
        assert_eq!(moved.translation, Vec3::new(-40.0, 3.0, 80.0));
        assert_eq!(
            world.entity(player).get::<Transform>().expect("player"),
            &Transform::default()
        );
    }
}
