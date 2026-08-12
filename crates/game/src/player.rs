//! The player as a person, distinct from whatever is carrying them.
//!
//! The boat used to *be* the player: one entity, driven by the movement keys,
//! followed by the camera, reported to the server. That held only as long as
//! there was exactly one way to exist in the world, and now there are two —
//! aboard a boat, and ashore on their own feet — with a rowboat to come
//! between them. All of them are one person getting about by different means.
//!
//! Being aboard is [`ChildOf`]: the player rides the scene graph, standing
//! wherever their vehicle carries them, and stepping ashore is nothing more
//! than leaving the hierarchy — [`embark_or_land`] is the one threshold,
//! crossed both ways by the same key. Ashore, [`walk`] drives them with the
//! keys the helm answers to afloat; which of the two systems is listening is
//! decided entirely by whether the player has a parent, so there is no mode
//! flag anywhere to fall out of step with the scene graph. What is *drawn* is
//! not this module's business at all: [`crate::figure`] hangs a person under
//! the entity, standing on the deck afloat and walking on the sand ashore,
//! and learns which of those is happening from the transform rather than from
//! anything said here.
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

use crate::bindings::{Action, KeyBindings};
use crate::boat::Boat;
use crate::figure::FigurePlugin;
use crate::terrain::Ground;
use crate::{AppState, Helm};

/// Metres per second on foot. A determined pace rather than a stroll —
/// beaches are tens of metres and summits hundreds away, and the walk is a
/// thirtieth of what the ship makes, so an island is *big* on foot without
/// being a chore. Backing up is half of it: nobody reverses at marching
/// speed.
///
/// Public because the figure drawn walking is scaled against it: full stride
/// is a player making this, and anything much faster is a teleport rather
/// than a step.
pub const WALK_SPEED: f32 = 3.0;

/// Radians per second the walker turns — brisker than any hull, because a
/// body pivots and seven metres of timber does not.
const WALK_TURN_RATE: f32 = 3.0;

/// How deep the player will wade, in metres of sea over the ground they are
/// standing on. Past the waist the water is for boats; short of it the
/// shallows are walkable, which is what lets a landing step off into knee
/// water rather than demanding dry sand under the keel. Deliberately less
/// than the ship's grounding draft, so everywhere the ship can float is
/// water the walker refuses — the gap between the two is what the landing
/// probe crosses, and what the rowboat will one day own.
///
/// Measured against the flat waterline, not the ground alone, so it is the
/// *sea* that stops a walker. A lake never does yet: the client keeps no
/// lake levels once the mesh is built, so a walker crosses a lakebed as if
/// it were dry — visibly wrong in deep lakes, and the honest fix is the
/// `Ground` resource learning lake levels, not a guess here.
const WADE_DEPTH: f32 = 0.5;

/// The ring the landing probe searches, in metres from the boat's origin:
/// from just short of the bow — anything nearer is deck — out to a long
/// stride past it. The far edge doubles as [`BOARD_REACH`] so that wherever
/// a player can step off, they can step straight back aboard from.
const LANDING_NEAR: f32 = 3.0;
const LANDING_REACH: f32 = 6.0;
/// Spacing of the probe's samples, well under the two-metre facet the
/// heights are drawn on, so a strip of walkable ground one facet wide is
/// not stepped over.
const LANDING_STEP: f32 = 0.5;
/// How many directions are tried at each radius, bow first — a boat is
/// usually nosed *at* the shore, so the first ray is the likely one and the
/// rest cover a hull lying alongside a beach.
const LANDING_RAYS: usize = 8;

/// How far from a boat's origin the player can board it from — the landing
/// reach exactly, see above.
const BOARD_REACH: f32 = LANDING_REACH;

/// The player's own movement, as something other systems can run after.
///
/// The figure drawn walking reads the transform these systems write, and a
/// reader free to run either side of them sees a frame's step in one frame
/// and nothing in the next. That reads as a walker stopping and starting
/// several times a second, which is exactly what it looked like.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct Afoot;

/// The person playing: one per match. Spawned aboard the ship the world is
/// entered on — by `boat::launch`, entering being done afloat. Aboard they
/// despawn with the boat, the hierarchy going down as one; ashore they carry
/// a `DespawnOnExit` of their own, put on at the gunwale by
/// [`embark_or_land`] and taken off again on boarding.
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

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        // Crossing the threshold before walking, so a player who steps ashore
        // is on their own feet the same frame — the chain is what makes Bevy
        // apply the parentage commands between the two. Both are the player's
        // hands and stop while paused, like the helm they share keys with.
        //
        // The figure comes with the player rather than being added beside
        // them in `main`: it is nothing but how this entity is drawn, and a
        // player spawned without one would be invisible.
        app.add_plugins(FigurePlugin).add_systems(
            Update,
            (embark_or_land, walk)
                .chain()
                .in_set(Afoot)
                .run_if(in_state(Helm::Sailing)),
        );
    }
}

/// The ground under a map point when it offers footing, in metres — and
/// nothing when it does not: sea past [`WADE_DEPTH`] deep, or a chunk that
/// has not arrived, which is not ground to be stepped onto however briefly.
fn footing(ground: Option<&Ground>, at: Vec2) -> Option<f32> {
    let height = ground?.height(at.x, at.y)?;
    (-height <= WADE_DEPTH).then_some(height)
}

/// How far past wadeable the water over a map point stands, in metres —
/// negative or zero where a walker may go. The walking twin of
/// `boat::grounding`, down to its answer for ground that has not arrived:
/// `NEG_INFINITY`, ground the client has not been sent being no reason to
/// pin a walker where they stand. [`footing`] is the strict half — where a
/// player may be *put down* — and this is the forgiving one — where one
/// already walking may go — and the gap between them is deliberate: a
/// landing must never choose unknown ground, but a walker overtaken by a
/// slow chunk must still be able to move.
fn wading(ground: Option<&Ground>, at: Vec2) -> f32 {
    match ground.and_then(|ground| ground.height(at.x, at.y)) {
        Some(height) => -height - WADE_DEPTH,
        None => f32::NEG_INFINITY,
    }
}

/// Crosses the gunwale, whichever way the player is facing it: ashore it
/// boards the nearest boat in reach, aboard it steps off onto the nearest
/// walkable ground. One key for both because they are one threshold, and
/// whichever side of it the player is on names the only thing the key could
/// mean.
///
/// Going ashore asks three things. The boat must be at rest — nobody steps
/// off a deck making way. The spot must offer [`footing`]: the probe walks
/// rings outward from just short of the bow ([`LANDING_NEAR`]) to a stride
/// past it ([`LANDING_REACH`]), bow direction first at each radius, and takes
/// the first walkable point — nearest wins, so the player steps to the shore
/// the bow is nosed against rather than teleporting down the beach. And there
/// must *be* such a spot: off a cliff coast or at anchor in deep water the
/// probe finds nothing and the key does nothing, which is the rule that makes
/// beaches landings and cliffs scenery without either being named anywhere.
///
/// The player steps off facing away from the boat — the direction they
/// stepped — and takes on a `DespawnOnExit` of their own, being no longer
/// under the boat's. Boarding is the mirror: back into the hierarchy at the
/// identity, the marker comes off, and the helm answers again.
fn embark_or_land(
    keys: Res<ButtonInput<KeyCode>>,
    bindings: Res<KeyBindings>,
    mut commands: Commands,
    ground: Option<Res<Ground>>,
    players: Query<(Entity, &Transform, Option<&ChildOf>), With<Player>>,
    boats: Query<(Entity, &Transform, &Boat)>,
) {
    if !keys.just_pressed(bindings.key(Action::Board)) {
        return;
    }
    let Ok((player, place, aboard)) = players.single() else {
        return;
    };
    let ground = ground.as_deref();

    match aboard {
        Some(aboard) => {
            let Ok((_, boat, hull)) = boats.get(aboard.parent()) else {
                return;
            };
            if !hull.at_rest() {
                return;
            }
            let Some((spot, height)) = landing(ground, boat) else {
                return;
            };
            let stepped = (spot - boat.translation.xz()).normalize_or_zero();
            commands.entity(player).remove::<ChildOf>().insert((
                Transform::from_xyz(spot.x, height, spot.y)
                    .with_rotation(Quat::from_rotation_y(f32::atan2(-stepped.x, -stepped.y))),
                DespawnOnExit(AppState::InWorld),
            ));
        }
        None => {
            let at = place.translation.xz();
            let Some((boat, _, hull)) = boats
                .iter()
                .filter(|(_, transform, _)| transform.translation.xz().distance(at) <= BOARD_REACH)
                .min_by(|(_, a, _), (_, b, _)| {
                    a.translation
                        .xz()
                        .distance(at)
                        .total_cmp(&b.translation.xz().distance(at))
                })
            else {
                return;
            };
            // Standing on the deck, not at the hull's origin: that origin is
            // the waterline, which is most of a metre down inside the boat.
            commands
                .entity(player)
                .remove::<DespawnOnExit<AppState>>()
                .insert((ChildOf(boat), Transform::from_xyz(0.0, hull.deck(), 0.0)));
        }
    }
}

/// Where a player stepping off this boat would stand: the nearest point with
/// [`footing`], and the ground height there — see [`embark_or_land`] for how
/// the rings are walked. `None` when no reachable ground offers any, which is
/// the probe saying "not here" about deep water and cliff faces alike.
fn landing(ground: Option<&Ground>, boat: &Transform) -> Option<(Vec2, f32)> {
    let ahead = boat.forward().xz().normalize_or_zero();
    let rings = ((LANDING_REACH - LANDING_NEAR) / LANDING_STEP) as usize + 1;
    for ring in 0..rings {
        let radius = LANDING_NEAR + ring as f32 * LANDING_STEP;
        for ray in 0..LANDING_RAYS {
            let turned = ray as f32 * std::f32::consts::TAU / LANDING_RAYS as f32;
            let spot = boat.translation.xz() + Vec2::from_angle(turned).rotate(ahead) * radius;
            if let Some(height) = footing(ground, spot) {
                return Some((spot, height));
            }
        }
    }
    None
}

/// Walks the player, with the keys the helm answers to afloat — forward and
/// back along their facing, the steering keys turning them — and only while
/// they are on their own feet: aboard anything, these keys are the boat's.
/// The view plays no part, exactly as with the boat, and for the same
/// reason.
///
/// No easing anywhere, deliberately: the eased family here belongs to hulls,
/// which have way to gather and carry, and a body simply walks when told and
/// stands when not. Standing is exact — an idle walker's transform goes
/// unwritten frame after frame, the same stillness an idle boat holds.
///
/// Where a walker may go is [`wading`]'s answer, judged like the keel's: the
/// step is allowed into walkable ground, or anywhere no *deeper* than where
/// they already stand — so a player somehow past their depth is herded
/// shoreward by the same clause that frees a beached hull, and the sea edge
/// can never be inched past because every step further in is deeper. The
/// walker then stands on the ground wherever the frame left them — knee-deep
/// in the shallows, on the sand above the waterline — and keeps their last
/// height over a chunk that has not arrived, exactly as everything riding
/// the world does.
fn walk(
    keys: Res<ButtonInput<KeyCode>>,
    bindings: Res<KeyBindings>,
    time: Res<Time>,
    ground: Option<Res<Ground>>,
    mut players: Query<(&mut Transform, Has<ChildOf>), With<Player>>,
) {
    let Ok((mut transform, aboard)) = players.single_mut() else {
        return;
    };
    if aboard {
        return;
    }

    // Opposed keys cancel outright, exactly as at the helm.
    let mut drive = 0.0;
    if bindings.held(&keys, Action::MoveForward, KeyCode::ArrowUp) {
        drive += 1.0;
    }
    if bindings.held(&keys, Action::MoveBack, KeyCode::ArrowDown) {
        drive -= 1.0;
    }
    let mut turn = 0.0;
    if bindings.held(&keys, Action::SteerLeft, KeyCode::ArrowLeft) {
        turn += 1.0;
    }
    if bindings.held(&keys, Action::SteerRight, KeyCode::ArrowRight) {
        turn -= 1.0;
    }

    if turn != 0.0 {
        transform.rotate_y(turn * WALK_TURN_RATE * time.delta_secs());
    }

    let ground = ground.as_deref();
    if drive != 0.0 {
        let speed = if drive > 0.0 {
            WALK_SPEED
        } else {
            WALK_SPEED * 0.5
        };
        let advance = transform.forward() * drive * speed * time.delta_secs();
        let here = wading(ground, transform.translation.xz());
        let there = wading(ground, (transform.translation + advance).xz());
        if there <= 0.0 || there <= here {
            transform.translation += advance;
        }
    }

    if let Some(height) =
        ground.and_then(|g| g.height(transform.translation.x, transform.translation.z))
    {
        if transform.translation.y != height {
            transform.translation.y = height;
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::SystemState;

    use super::*;
    use crate::testing::{hold, run_frames, test_ground, world_app, TEST_ISLAND_REACH};

    /// A match on the test island's shore: the boat close enough in for a
    /// landing to have somewhere to go, bow at the island, player aboard.
    /// The island is a steep-rimmed dome at the origin reaching
    /// [`TEST_ISLAND_REACH`], so "in close" is a couple of metres past it.
    fn shore_app() -> App {
        let mut app = world_app();
        app.insert_resource(test_ground());
        place_boat(&mut app, Vec2::new(TEST_ISLAND_REACH + 2.0, 0.0));
        app
    }

    /// Puts the boat down at a spot, facing the island at the origin.
    fn place_boat(app: &mut App, at: Vec2) {
        let mut transform = app
            .world_mut()
            .query_filtered::<&mut Transform, With<Boat>>()
            .single_mut(app.world_mut())
            .expect("a match should have a boat in it");
        transform.translation = Vec3::new(at.x, 0.0, at.y);
        let facing = -at.normalize_or_zero();
        transform.rotation = Quat::from_rotation_y(f32::atan2(-facing.x, -facing.y));
        app.update();
    }

    /// One press of the go-ashore/board key, released again afterwards so
    /// the next call is a fresh press rather than a key held down.
    fn press_board(app: &mut App) {
        hold(app, KeyCode::KeyF);
        run_frames(app, 1);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::KeyF);
        run_frames(app, 1);
    }

    fn player_transform(app: &mut App) -> Transform {
        *app.world_mut()
            .query_filtered::<&Transform, With<Player>>()
            .single(app.world())
            .expect("a match should have a player in it")
    }

    /// What the player is aboard, if anything.
    fn aboard(app: &mut App) -> Option<Entity> {
        app.world_mut()
            .query_filtered::<Option<&ChildOf>, With<Player>>()
            .single(app.world())
            .expect("a match should have a player in it")
            .map(ChildOf::parent)
    }

    fn boat_transform(app: &mut App) -> Transform {
        *app.world_mut()
            .query_filtered::<&Transform, With<Boat>>()
            .single(app.world())
            .expect("a match should have a boat in it")
    }

    fn ground_height(app: &App, at: Vec2) -> f32 {
        app.world()
            .resource::<Ground>()
            .height(at.x, at.y)
            .expect("the test ground has arrived")
    }

    #[test]
    fn going_ashore_steps_the_player_onto_walkable_ground() {
        let mut app = shore_app();
        assert!(aboard(&mut app).is_some(), "the player entered ashore");

        press_board(&mut app);

        assert_eq!(aboard(&mut app), None, "the player is still aboard");
        let at = player_transform(&mut app);
        let spot = at.translation.xz();
        let height = ground_height(&app, spot);
        assert!(
            -height <= WADE_DEPTH,
            "the player was put down in {} m of water",
            -height
        );
        assert_eq!(at.translation.y, height, "the player is not on the ground");

        // And the boat stayed where it was left, still a boat: landing is
        // the player leaving, not the vehicle going anywhere.
        let boat = boat_transform(&mut app).translation.xz();
        assert!(
            boat.distance(Vec2::new(TEST_ISLAND_REACH + 2.0, 0.0)) < 0.5,
            "going ashore moved the boat to {boat}"
        );
        assert!(
            spot.distance(boat) <= LANDING_REACH + 1e-3,
            "the player landed {} m from the boat, past the probe's reach",
            spot.distance(boat)
        );
    }

    #[test]
    fn going_ashore_is_refused_over_deep_water() {
        // At anchor in open ocean: nothing within reach offers footing, so
        // the key does nothing and the player stays aboard.
        let mut app = world_app();
        app.insert_resource(test_ground());
        place_boat(&mut app, Vec2::new(TEST_ISLAND_REACH * 2.0, 0.0));

        press_board(&mut app);
        assert!(
            aboard(&mut app).is_some(),
            "the player was put over the side in open ocean"
        );
    }

    #[test]
    fn going_ashore_is_refused_under_way() {
        // The same shore that lands fine at rest refuses while the hull is
        // making way — and lands again once the way has run off.
        let mut app = shore_app();
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 6);
        press_board(&mut app);
        assert!(
            aboard(&mut app).is_some(),
            "the player stepped off a deck making way"
        );

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::ArrowUp);
        run_frames(&mut app, 800);
        press_board(&mut app);
        assert_eq!(
            aboard(&mut app),
            None,
            "the landing never worked again once the boat had stopped"
        );
    }

    #[test]
    fn the_walker_walks_the_way_they_face_and_stands_on_the_ground() {
        let mut app = shore_app();
        press_board(&mut app);
        let before = player_transform(&mut app);

        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 60);
        let after = player_transform(&mut app);

        let moved = after.translation.xz() - before.translation.xz();
        assert!(
            moved.length() > 0.0,
            "the walker never moved with the key down"
        );
        assert!(
            moved.normalize().dot(before.forward().xz().normalize()) > 0.99,
            "the walker went {moved} rather than the way they faced"
        );
        // No easing to wait out: a second of walking is a second at speed.
        assert!(
            (moved.length() - WALK_SPEED * 60.0 * 0.016).abs() < WALK_SPEED * 0.05,
            "{} m in a second is not walking speed",
            moved.length()
        );
        assert_eq!(
            after.translation.y,
            ground_height(&app, after.translation.xz()),
            "the walker is not standing on the ground they walked to"
        );
    }

    #[test]
    fn the_steering_keys_turn_the_walker() {
        let mut app = shore_app();
        press_board(&mut app);
        let before = player_transform(&mut app);

        hold(&mut app, KeyCode::ArrowLeft);
        run_frames(&mut app, 20);
        let after = player_transform(&mut app);

        assert_eq!(after.translation, before.translation, "turning moved them");
        assert_ne!(after.rotation, before.rotation, "they never turned");
    }

    #[test]
    fn the_sea_stops_the_walker_at_wading_depth() {
        // Ashore, turned round, and marched at the sea for a long time: the
        // walker ends held at the water's edge — past the waterline into the
        // shallows, and not a step past wading depth — instead of strolling
        // out along the seabed.
        let mut app = shore_app();
        press_board(&mut app);

        let mut players = app
            .world_mut()
            .query_filtered::<&mut Transform, With<Player>>();
        let mut transform = players
            .single_mut(app.world_mut())
            .expect("a match should have a player in it");
        // Face +X: out to sea, the island being at the origin.
        transform.rotation = Quat::from_rotation_y(f32::atan2(-1.0, 0.0));

        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 600);

        let at = player_transform(&mut app).translation;
        let depth = -ground_height(&app, at.xz());
        assert!(
            depth <= WADE_DEPTH + 1e-3,
            "the walker is out in {depth} m of water"
        );
        assert!(
            depth > 0.0,
            "the walker never even got their feet wet, stopped {} m up",
            -depth
        );
    }

    #[test]
    fn ashore_the_helm_is_dead_and_boarding_brings_it_back() {
        let mut app = shore_app();
        press_board(&mut app);

        // The movement keys are the walker's now: the anchored boat holds
        // its spot on the map while they are held. (Only the map spot — the
        // swell still bobs the hull, which is exactly the point of it.)
        let before = boat_transform(&mut app).translation.xz();
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 30);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::ArrowUp);
        assert_eq!(
            boat_transform(&mut app).translation.xz(),
            before,
            "the empty boat sailed off with the walker's keys"
        );

        // Back aboard: within reach, so the key re-parents the player, sets
        // them at the identity, and the helm answers again.
        press_board(&mut app);
        let boat = aboard(&mut app).expect("the player never got back aboard");
        assert!(
            app.world().entity(boat).get::<Boat>().is_some(),
            "the player boarded something that is not a boat"
        );
        // Aboard at the boat's own heading, standing on its deck.
        let deck = app
            .world()
            .entity(boat)
            .get::<Boat>()
            .expect("a boat")
            .deck();
        assert_eq!(
            player_transform(&mut app),
            Transform::from_xyz(0.0, deck, 0.0)
        );

        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 30);
        assert_ne!(
            boat_transform(&mut app).translation.xz(),
            before,
            "the helm never came back with the player"
        );
    }

    #[test]
    fn boarding_needs_the_boat_in_reach() {
        let mut app = shore_app();
        press_board(&mut app);

        // Carry the walker well inland, far past the probe's reach.
        let mut players = app
            .world_mut()
            .query_filtered::<&mut Transform, With<Player>>();
        players
            .single_mut(app.world_mut())
            .expect("a match should have a player in it")
            .translation = Vec3::new(TEST_ISLAND_REACH * 0.5, 0.0, 0.0);
        app.update();

        press_board(&mut app);
        assert_eq!(
            aboard(&mut app),
            None,
            "the player boarded a boat from halfway up the island"
        );
    }

    #[test]
    fn a_paused_walker_stands_still() {
        let mut app = shore_app();
        press_board(&mut app);
        app.world_mut()
            .resource_mut::<NextState<Helm>>()
            .set(Helm::Paused);
        app.update();

        let before = player_transform(&mut app);
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 20);
        assert_eq!(
            player_transform(&mut app).translation,
            before.translation,
            "the walker walked on with the pause menu up"
        );
    }

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
