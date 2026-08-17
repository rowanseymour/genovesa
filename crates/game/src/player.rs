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

use protocol::ground::FACET_METRES;

use crate::bindings::{Action, KeyBindings};
use crate::boat::{Boat, Fleet, HullId, Vessel};
use crate::chart::Chart;
use crate::figure::FigurePlugin;
use crate::net::Online;
use crate::terrain::Ground;
use crate::{AppState, Helm};

/// Metres per second on foot. A determined pace rather than a stroll —
/// beaches are tens of metres and summits hundreds away, and the walk is a
/// thirtieth of what the ship makes, so an island is *big* on foot without
/// being a chore. Backing up is half of it: nobody reverses at marching
/// speed.
///
/// The figure drawn walking knows nothing about it. A gait scaled against
/// this pace is what [`crate::figure`]'s `STIRRING` explains the removal of:
/// the cycle advances by ground covered, so a slow walk is the same swing
/// taken slowly, and the walker owes the drawing no number at all.
const WALK_SPEED: f32 = 3.0;

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

/// The steepest ground a walker will cross, as a gradient: metres of height
/// per metre travelled, so this is a slope of about 35°. Up and down are the
/// same number, because steep ground is a wall from either side — which is
/// also what keeps the rule from trapping anybody. Every step is judged
/// against the one that would undo it, so the way back off a slope is as open
/// as the way onto it was, and there is nowhere a walker can reach that they
/// cannot leave.
///
/// The number is set against the ground the generator actually raises rather
/// than picked for the look of it: half of any island's land lies under 15°
/// and a tenth of it over 40°, so a limit here turns back crags, gullies and
/// cliff faces and leaves ordinary hillside alone. What that costs was
/// measured by flooding nine seeds outward from their waterlines under this
/// rule — about two percent of the land walled off, and every one of their
/// summits still reachable by some way round. A walker is stopped by the
/// steep, in other words, without being shut out of anywhere.
const WALKABLE_RISE: f32 = 0.7;

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

/// A walker put down before the ground under them had streamed in — entry
/// on foot, whose height the wire never carries. [`find_footing`] settles
/// them onto the ground the moment there is ground to stand on.
#[derive(Component)]
pub struct Unsettled;

/// The walkers still waiting for ground to stand on, as a query — see
/// [`Unsettled`]. Not aboard anything: a player on a deck stands on the
/// deck, and the hull's own transform is not theirs to write.
type Waiting<'w, 's> = Query<
    'w,
    's,
    (Entity, &'static mut Transform),
    (With<Player>, With<Unsettled>, Without<ChildOf>),
>;

/// Settles an [`Unsettled`] walker onto the ground once it has arrived.
/// Height only: where they stand is the server's word, and which way they
/// face was entry's guess to make.
fn find_footing(mut commands: Commands, ground: Option<Res<Ground>>, mut walkers: Waiting) {
    for (walker, mut place) in &mut walkers {
        let standing = ground
            .as_ref()
            .and_then(|g| g.height(place.translation.x, place.translation.z));
        if let Some(height) = standing {
            place.translation.y = height;
            commands.entity(walker).remove::<Unsettled>();
        }
    }
}

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

    /// Whether the player is aboard something rather than standing on their
    /// own feet. `false` outside a match, there being nobody to be either.
    pub fn aboard(&self) -> bool {
        self.players
            .single()
            .is_ok_and(|(_, aboard)| aboard.is_some())
    }

    /// Which way the carrier is pointing on the map, as a unit vector — the
    /// hull's own bow, or the walker's own face. `None` outside a match, and
    /// `None` for a carrier standing so exactly on end that its forward has no
    /// bearing left, which nothing here can produce but the arithmetic can.
    ///
    /// Flattened rather than taken whole because what asks is drawing in plan:
    /// a hull pitching over a swell is still heading the way it was heading.
    pub fn heading(&self) -> Option<Vec2> {
        let carrier = self.carrier()?;
        let forward = self.carriers.get(carrier).ok()?.forward().xz();
        forward.try_normalize()
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
        app.add_plugins(FigurePlugin)
            .init_resource::<Fleet>()
            .add_systems(
                Update,
                (embark_or_land, claim_the_island, walk)
                    .chain()
                    .in_set(Afoot)
                    .run_if(in_state(Helm::Sailing)),
            )
            // Outside the pause and the helm's own set: a walker waiting
            // for their ground should find it even while the menu is up.
            .add_systems(Update, find_footing.run_if(in_state(AppState::InWorld)));
    }
}

/// Stands a cairn on the island underfoot: the claim, asked for.
///
/// The client asks and the server rules. It could not do otherwise — a claim
/// is settled against the coast the *world* has watched this player sail, and
/// this side's chart is a drawing of what it was told, not evidence. So what
/// this does is ask on the two counts a player can see for themselves: they
/// are on their own feet, and the sheet says they have closed the ring they
/// are standing inside. A refusal is silent, and deserves to be: the two
/// honest ways to earn one are a coast that looked closed here and did not
/// close there, and an island somebody claimed while you were walking up to
/// it — and the second answers itself, since the refusal carries the cairn
/// that beat you to it.
///
/// Asking from a boat is not offered at all. A cairn is built by somebody
/// standing on the ground with stones in their hands, and the key that would
/// have done it from the helm is a key that says the world is a menu.
fn claim_the_island(
    keys: Res<ButtonInput<KeyCode>>,
    bindings: Res<KeyBindings>,
    online: Option<Res<Online>>,
    chart: Option<Res<Chart>>,
    players: Query<(&Transform, Option<&ChildOf>), With<Player>>,
) {
    if !keys.just_pressed(bindings.key(Action::Claim)) {
        return;
    }
    let (Some(online), Some(chart)) = (online, chart) else {
        return;
    };
    // Afoot: a player aboard is a child of their hull.
    let Ok((place, None)) = players.single() else {
        return;
    };
    let standing = Vec2::new(place.translation.x, place.translation.z);
    // Asked of the sheet's own survey, which is the same arithmetic the
    // server will use on the same question — see `protocol::survey`. That is
    // what makes this an ask worth making rather than a guess: where the two
    // disagree it is because the world has seen more coast than this client
    // has been told of yet, and a moment later it will have been.
    if let Some(island) = chart.island_under(standing) {
        online.connection.claim(island);
    }
}

/// The ground under a map point when it offers footing, in metres — and
/// nothing when it does not: sea past [`WADE_DEPTH`] deep, ground steeper than
/// [`WALKABLE_RISE`], or a chunk that has not arrived, which is not ground to
/// be stepped onto however briefly.
///
/// The steepness half is [`WALKABLE_RISE`] asked of a spot rather than of a
/// step, and it is here so that the two agree: a player put down on a cliff face
/// would be standing where their own legs say they cannot be. It reads the
/// ground more bluntly than a step is judged — see [`tilt`] — and errs towards
/// refusing, which for a probe with a whole ring of spots to try is the right
/// way round.
fn footing(ground: Option<&Ground>, at: Vec2) -> Option<f32> {
    let ground = ground?;
    let height = ground.height(at.x, at.y)?;
    (-height <= WADE_DEPTH && tilt(ground, at)? <= WALKABLE_RISE).then_some(height)
}

/// How steeply the ground tilts at a map point, as a gradient — the limit
/// [`WALKABLE_RISE`] sets, read of a spot instead of a step. A central
/// difference a facet wide, which is as fine as the height field says anything:
/// the ground between two corners is one flat facet, so there is no finer slope
/// there to read. `None` where any of the four samples is over a chunk that has
/// not arrived.
///
/// That makes it a blunter instrument than [`climb`] rather than the same
/// measurement, and near a break in the ground the two disagree: a facet-wide
/// difference smears the break over the facet either side of it, so flat sand a
/// stride from the foot of a bluff reads as steep, and a rise narrower than a
/// facet can read as level from the top of it. Neither is worth sharpening. The
/// first costs a landing spot where the probe has a whole ring of others to try,
/// and past the landing it is [`climb`] that says where a walker may go.
fn tilt(ground: &Ground, at: Vec2) -> Option<f32> {
    // Half a facet either side of the spot, so each difference spans one.
    let reach = FACET_METRES / 2.0;
    let across = |step: Vec2| {
        let (behind, ahead) = (at - step, at + step);
        Some((ground.height(ahead.x, ahead.y)? - ground.height(behind.x, behind.y)?) / FACET_METRES)
    };
    Some(Vec2::new(across(Vec2::X * reach)?, across(Vec2::Y * reach)?).length())
}

/// How steeply the ground climbs across a step, as a gradient: metres of
/// height per metre travelled, unsigned, up and down being one rule — see
/// [`WALKABLE_RISE`].
///
/// The two ends of the step and nothing in between, which makes this a *mean*
/// gradient and so only as honest as the step is short: one long enough to
/// straddle a wall averages it away against the flat ground either side. Keeping
/// steps short enough for that not to matter is [`walk`]'s business — and it
/// does it by cutting the frame's advance up rather than by probing a fixed
/// distance ahead, because a fixed probe is what a long frame steps clean over,
/// putting the walker up the wall it was watching for.
///
/// Zero where either end is over a chunk that has not arrived — the same
/// forgiveness [`wading`] shows, and for the same reason: ground the client has
/// not been sent is no reason to pin a walker where they stand.
fn climb(ground: Option<&Ground>, from: Vec2, to: Vec2) -> f32 {
    let along = from.distance(to);
    let (Some(ground), true) = (ground, along > 0.0) else {
        return 0.0;
    };
    let (Some(here), Some(there)) = (ground.height(from.x, from.y), ground.height(to.x, to.y))
    else {
        return 0.0;
    };
    (there - here).abs() / along
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

/// Every hull the gunwale key might mean, as a query: where each lies, the
/// sailing state of the one this player steers — the others have none — and
/// the name it answers to on the wire, which is how a served world's helm is
/// told apart from a local one's.
type Vessels<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Transform,
        Option<&'static mut Boat>,
        Option<&'static HullId>,
    ),
    With<Vessel>,
>;

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
/// under the boat's. Stepping off also furls the sails, so a boat is never
/// left riding at anchor under canvas. Boarding is the mirror: back into the
/// hierarchy at the identity, the marker comes off, and the helm answers
/// again — with the sails as the player left them, making sail being a
/// deliberate act rather than a side effect of stepping aboard.
#[allow(clippy::too_many_arguments)]
fn embark_or_land(
    keys: Res<ButtonInput<KeyCode>>,
    bindings: Res<KeyBindings>,
    mut commands: Commands,
    ground: Option<Res<Ground>>,
    online: Option<Res<Online>>,
    mut fleet: ResMut<Fleet>,
    players: Query<(Entity, &Transform, Option<&ChildOf>), With<Player>>,
    mut vessels: Vessels,
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
            let Ok((hull_entity, boat, sailing, _)) = vessels.get_mut(aboard.parent()) else {
                return;
            };
            let Some(mut hull) = sailing else {
                return;
            };
            if !hull.at_rest() {
                return;
            }
            let Some((spot, height)) = landing(ground, boat) else {
                return;
            };
            // The crew furls as the skipper steps off. Belt and braces —
            // `boat::steer` drives no boat nobody is aboard — but a beach
            // must not show an unattended hull under canvas either.
            hull.furl();
            let stepped = (spot - boat.translation.xz()).normalize_or_zero();
            commands.entity(player).remove::<ChildOf>().insert((
                Transform::from_xyz(spot.x, height, spot.y)
                    .with_rotation(Quat::from_rotation_y(f32::atan2(-stepped.x, -stepped.y))),
                DespawnOnExit(AppState::InWorld),
            ));
            // The step is the client's, having judged the footing; what the
            // wire is owed is the fact of it. The server frees the helm for
            // anyone, and this side gives the hull back to its moorings. A
            // world with no server behind it — the headless tests' — keeps
            // the whole exchange local, exactly as it always was.
            if let Some(online) = online {
                online.connection.disembark(spot);
                fleet.hand_back(&mut commands, hull_entity, boat);
            }
        }
        None => {
            let at = place.translation.xz();
            let Some((boat, _, sailing, named)) = vessels
                .iter()
                .filter(|(_, transform, _, _)| {
                    transform.translation.xz().distance(at) <= BOARD_REACH
                })
                // A helm that is visibly somebody's is not offered — the
                // server would refuse the ask anyway, and this spares it.
                .filter(|(_, _, _, named)| named.is_none_or(|named| !fleet.manned(named.0)))
                .min_by(|(_, a, _, _), (_, b, _, _)| {
                    a.translation
                        .xz()
                        .distance(at)
                        .total_cmp(&b.translation.xz().distance(at))
                })
            else {
                return;
            };
            match (&online, named) {
                // A served world's helm is asked for, never taken: the
                // player steps aboard when the telling grants it — see
                // [`crate::boat::Fleet::told`] — which over the loopback is
                // the next frame, and across a real sea is still a blink.
                (Some(online), Some(named)) => online.connection.board(named.0),
                // Offline, boarding is immediate, as it always was. Standing
                // at the helm, not at the hull's origin: that origin is the
                // waterline, which is most of a metre down inside the boat.
                _ => {
                    let Some(hull) = sailing else {
                        return;
                    };
                    commands
                        .entity(player)
                        .remove::<DespawnOnExit<AppState>>()
                        .insert((ChildOf(boat), Transform::from_translation(hull.helm())));
                }
            }
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
/// Two things can refuse a step, and a step has to satisfy both.
///
/// The water is [`wading`]'s answer, judged like the keel's: the step is
/// allowed into walkable ground, or anywhere no *deeper* than where they
/// already stand — so a player somehow past their depth is herded shoreward by
/// the same clause that frees a beached hull, and the sea edge can never be
/// inched past because every step further in is deeper.
///
/// The land is [`climb`]'s: ground rising or falling faster than
/// [`WALKABLE_RISE`] across the step is not walked over. A frame's advance is
/// taken in strides of at most half a facet and each of them judged in turn, so
/// that however long the frame was, no stride has a whole facet of ground hidden
/// inside it — what the limit is held to is the height field's own resolution
/// rather than the frame rate. A walker turned back mid-advance keeps the
/// strides they had already made and stops there.
///
/// That is a limit on the step and not on the spot, so it turns a walker back
/// from a cliff without pinning them against it — the face of a bluff can be
/// crossed along its contour, where the ground being climbed is level, exactly
/// as a person picks their way across a steep hillside rather than straight up
/// it. The two rules are ANDed, and between them the climb has the last word:
/// the shoreward clause above frees a walker who is merely out of their depth,
/// not one standing under a drop-off, who has nowhere to go until the water
/// falls. Letting them climb the drop instead would be the worse answer.
///
/// The walker then stands on the ground wherever the frame left them —
/// knee-deep in the shallows, on the sand above the waterline — and keeps their
/// last height over a chunk that has not arrived, exactly as everything riding
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

    // The very keys the helm answers to — see [`KeyBindings::driving`].
    let (drive, turn) = bindings.driving(&keys);

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
        // Half a facet at a time, so that no stride can have a facet of ground
        // hidden inside it — that is the shortest distance over which the height
        // field says anything, and a stride judged by its two ends is only as
        // sharp as it is short.
        //
        // In practice this is always one stride: the engine clamps a long frame
        // before anything sees it, so the worst step is well under half a facet
        // and this arithmetic rounds to nothing. Written as strides anyway
        // because then the limit owes nothing to that clamp being where it is,
        // or to WALK_SPEED being what it is: raise either and the rule still
        // holds rather than quietly starting to slip.
        let strides = (advance.length() / (FACET_METRES / 2.0)).ceil().max(1.0);
        let stride = advance / strides;
        for _ in 0..strides as usize {
            let (from, to) = (
                transform.translation.xz(),
                (transform.translation + stride).xz(),
            );
            let (here, there) = (wading(ground, from), wading(ground, to));
            let wadeable = there <= 0.0 || there <= here;
            if !wadeable || climb(ground, from, to) > WALKABLE_RISE {
                break;
            }
            transform.translation += stride;
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
    use std::time::Duration;

    use bevy::ecs::system::SystemState;
    use bevy::time::{TimeUpdateStrategy, Virtual};

    use super::*;
    use crate::testing::{
        elapsed, hold, run_frames, set_wind, test_ground, test_shore, world_app, SHORE_BLUFF_FOOT,
        SHORE_PEAK, SHORE_TOP, SHORE_WATERLINE, TEST_ISLAND_REACH,
    };

    /// How far off the waterline the boat is anchored in a [`shore_app`], in
    /// metres: far enough out that the hull is still floating, near enough that
    /// the landing probe reaches wadeable ground.
    const ANCHORAGE: f32 = 4.5;

    /// A match off the shore island's coast: the boat close enough in for a
    /// landing to have somewhere to go, bow at the island, player aboard. The
    /// island is the one with a coast on it — an apron up through the waterline
    /// and a bluff behind — because a landing needs ground a walker could
    /// stand on, which the cliff-rimmed one has nowhere.
    fn shore_app() -> App {
        let mut app = world_app();
        app.insert_resource(test_shore());
        place_boat(&mut app, Vec2::new(SHORE_WATERLINE + ANCHORAGE, 0.0));
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
            boat.distance(Vec2::new(SHORE_WATERLINE + ANCHORAGE, 0.0)) < 0.5,
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
        app.insert_resource(test_shore());
        place_boat(&mut app, Vec2::new(TEST_ISLAND_REACH * 2.0, 0.0));

        press_board(&mut app);
        assert!(
            aboard(&mut app).is_some(),
            "the player was put over the side in open ocean"
        );
    }

    #[test]
    fn going_ashore_is_refused_against_a_cliff() {
        // The cliff-rimmed island, nosed right up to: there is dry ground a
        // stride from the bow and the key still does nothing, because ground
        // standing on end is not ground anybody could walk off onto. This is
        // what makes a cliff coast scenery — a landing has to find footing, and
        // footing is more than shallow water.
        let mut app = world_app();
        app.insert_resource(test_ground());
        place_boat(&mut app, Vec2::new(TEST_ISLAND_REACH + 2.0, 0.0));

        // The probe's own reach does hold dry land, so what refuses the landing
        // below is the steepness of it and not the distance.
        let dry = app
            .world()
            .resource::<Ground>()
            .height(TEST_ISLAND_REACH - 1.0, 0.0)
            .expect("the cliff island's rim has arrived");
        assert!(dry > 0.0, "the rim is under water, not a cliff");

        press_board(&mut app);
        assert!(
            aboard(&mut app).is_some(),
            "the player stepped off onto a cliff face"
        );
    }

    #[test]
    fn going_ashore_is_refused_under_way() {
        // The same shore that lands fine at rest refuses while the hull is
        // making way — and lands again once the sails are furled and the way
        // has run off.
        let mut app = shore_app();
        // Onshore — dead astern of a bow pointed at the island — so making
        // sail below actually makes way.
        set_wind(&mut app, Vec2::new(-7.0, 0.0));
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 6);
        press_board(&mut app);
        assert!(
            aboard(&mut app).is_some(),
            "the player stepped off a deck making way"
        );

        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.release(KeyCode::ArrowUp);
        keys.press(KeyCode::ArrowDown);
        run_frames(&mut app, 1);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::ArrowDown);
        run_frames(&mut app, 800);
        press_board(&mut app);
        assert_eq!(
            aboard(&mut app),
            None,
            "the landing never worked again once the boat had stopped"
        );
    }

    #[test]
    fn going_ashore_furls_the_sails() {
        // A beach never shows an unattended hull under canvas: stepping off
        // furls. The wind blows *offshore* here, so the sails go up in irons
        // — set, but the hull at rest at its anchorage, which is what lets
        // the landing happen while there is still canvas to take in.
        let mut app = shore_app();
        set_wind(&mut app, Vec2::new(7.0, 0.0));
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 2);

        fn sails(app: &mut App) -> bool {
            app.world_mut()
                .query::<&Boat>()
                .single(app.world())
                .expect("a match should have a boat in it")
                .sails_set()
        }
        assert!(sails(&mut app), "the sails never went up in irons");

        press_board(&mut app);
        assert_eq!(aboard(&mut app), None, "the landing was refused");
        assert!(
            !sails(&mut app),
            "the boat was left riding at anchor under canvas"
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
        // Out to sea, the island being at the origin.
        face(&mut app, Vec2::X);

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

    /// Points the walker a way, as a test that cares where they are headed
    /// rather than how long they took to come round.
    ///
    /// Runs no frame of its own, deliberately: [`walk`] reads the transform this
    /// writes, so the turn is in force next frame either way, and a frame here
    /// would be a frame of walking that the test's own clock and distance
    /// measurements around the call knew nothing about.
    fn face(app: &mut App, along: Vec2) {
        let mut players = app
            .world_mut()
            .query_filtered::<&mut Transform, With<Player>>();
        players
            .single_mut(app.world_mut())
            .expect("a match should have a player in it")
            .rotation = Quat::from_rotation_y(f32::atan2(-along.x, -along.y));
    }

    #[test]
    fn ground_too_steep_turns_the_walker_back() {
        // Ashore and marched inland for a quarter of a minute: the apron is
        // crossed and the bluff at the back of it is not, so the walk ends at
        // its foot rather than up its face or on the top.
        let mut app = shore_app();
        press_board(&mut app);
        face(&mut app, Vec2::NEG_X);

        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 900);

        let at = player_transform(&mut app).translation;
        assert!(
            at.y <= SHORE_BLUFF_FOOT + 0.5,
            "the walker climbed to {} m, up a bluff starting at {SHORE_BLUFF_FOOT} m",
            at.y
        );
        // And held at the foot of it rather than stuck at the water's edge:
        // getting this high is the whole apron walked, ankle-deep water to
        // bluff, which is the half of the rule that has to keep working.
        assert!(
            at.y > SHORE_BLUFF_FOOT - 1.0,
            "the walker stopped {} m up, nowhere near the bluff they were sent at",
            at.y
        );
    }

    #[test]
    fn a_long_frame_turns_the_walker_back_in_the_same_place() {
        // Where a walker is stopped is the ground's business and not the frame
        // rate's, which is what the strides in `walk` are for. The engine clamps
        // a long frame before anything sees it, so the clamp is lifted here to
        // get one long enough to tell the difference — two seconds of walking
        // arriving as a single step — and the answer has to be the one sixty
        // frames a second gives above: held at the foot of the bluff.
        //
        // Judged end to end instead, that step is a mean taken across ground the
        // walker never sets foot on, and it goes wrong in both directions: this
        // one refuses two and a half metres early, and a step straddling a low
        // ledge averages it away and climbs it.
        let mut app = shore_app();
        press_board(&mut app);
        app.world_mut()
            .resource_mut::<Time<Virtual>>()
            .set_max_delta(Duration::from_secs(10));
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs(2)));
        face(&mut app, Vec2::NEG_X);

        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 40);

        let at = player_transform(&mut app).translation;
        assert!(
            (at.y - SHORE_BLUFF_FOOT).abs() < 0.5,
            "a two-second frame left the walker {} m up, not at the bluff's \
             {SHORE_BLUFF_FOOT} m foot",
            at.y
        );
    }

    #[test]
    fn a_bluff_may_be_crossed_along_its_contour() {
        // The rule is about the step and not about the spot, which is what
        // stops it pinning anybody: turned side-on at the foot of the bluff
        // that just refused them, the same walker walks off along it.
        let mut app = shore_app();
        press_board(&mut app);
        face(&mut app, Vec2::NEG_X);
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 900);
        let held = player_transform(&mut app).translation;

        // A quarter turn: across the slope rather than up it, the island being
        // round and the walker standing due east of its middle.
        face(&mut app, Vec2::NEG_Y);
        let start = elapsed(&app);
        run_frames(&mut app, 120);
        let seconds = elapsed(&app) - start;

        let at = player_transform(&mut app).translation;
        let along = at.xz().distance(held.xz());
        assert!(
            (along - WALK_SPEED * seconds).abs() < WALK_SPEED * 0.1,
            "the walker made {along} m along the contour, not the {} m they were walking",
            WALK_SPEED * seconds
        );
        assert!(
            (at.y - held.y).abs() < 1.0,
            "walking along the contour climbed {} m",
            at.y - held.y
        );
    }

    #[test]
    fn the_walker_will_not_step_off_a_drop() {
        // The same limit from above, which is the half that keeps it honest: a
        // walker who could step off the top of the bluff would be somewhere
        // they could never climb back to, and the drop is refused for exactly
        // the reason the climb was.
        let mut app = shore_app();
        press_board(&mut app);
        // Three metres in from where the top gives out, so there is a little
        // level ground to cross before the drop.
        let brink = Vec3::new(SHORE_TOP - 3.0, 0.0, 0.0);
        let mut players = app
            .world_mut()
            .query_filtered::<&mut Transform, With<Player>>();
        players
            .single_mut(app.world_mut())
            .expect("a match should have a player in it")
            .translation = brink;
        app.update();
        // Standing on the level top, facing out to sea and so at the drop.
        let from = player_transform(&mut app).translation;
        assert!(
            (from.y - SHORE_PEAK).abs() < 1e-3,
            "the walker was put at {} m rather than on the island's top",
            from.y
        );
        face(&mut app, Vec2::X);

        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 300);

        let at = player_transform(&mut app).translation;
        assert!(
            (at.y - SHORE_PEAK).abs() < 1.0,
            "the walker went over the edge and ended {} m down",
            from.y - at.y
        );
        // Held at the brink rather than never having set off: they crossed the
        // last of the level ground and stopped at the edge of it, well short of
        // the fourteen metres three hundred frames of walking would cover.
        let made = at.xz().distance(from.xz());
        assert!(
            (1.0..8.0).contains(&made),
            "the walker made {made} m across the top"
        );
    }

    #[test]
    fn a_walker_sent_no_ground_still_moves() {
        // A client that has been told nothing about the world must not be a
        // client whose player cannot move: with no chunk under them there is
        // no depth to refuse a step and no slope to measure it against, and
        // both rules stand aside rather than guessing.
        let mut app = world_app();
        let (player, _) = app
            .world_mut()
            .query_filtered::<(Entity, &Transform), With<Player>>()
            .single(app.world())
            .expect("a match should have a player in it");
        app.world_mut().entity_mut(player).remove::<ChildOf>();
        run_frames(&mut app, 1);
        let before = player_transform(&mut app).translation;

        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 60);
        assert!(
            player_transform(&mut app).translation.distance(before) > 1.0,
            "the walker was pinned by ground the client never had"
        );
    }

    #[test]
    fn ashore_the_helm_is_dead_and_boarding_brings_it_back() {
        let mut app = shore_app();
        press_board(&mut app);

        // The movement keys are the walker's now: the anchored boat holds
        // its spot on the map while they are held. (Only the map spot — the
        // swell still bobs the hull, which is exactly the point of it.) A short
        // walk, because the second half of the test is boarding again and the
        // walker steps ashore most of [`BOARD_REACH`] from the hull already.
        let before = boat_transform(&mut app).translation.xz();
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 20);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::ArrowUp);
        assert_eq!(
            boat_transform(&mut app).translation.xz(),
            before,
            "the empty boat sailed off with the walker's keys"
        );

        // Back aboard: within reach, so the key re-parents the player, sets
        // them at the identity, and the helm answers again. Said out loud first,
        // because the walk above only just leaves them in reach — if the shape
        // of the shore or the landing ever moves the hull further off, this is
        // the assertion that should fail rather than the boarding below.
        let off = player_transform(&mut app).translation.xz().distance(before);
        assert!(
            off <= BOARD_REACH,
            "the walk left the walker {off} m from the boat, past boarding reach"
        );
        press_board(&mut app);
        let boat = aboard(&mut app).expect("the player never got back aboard");
        assert!(
            app.world().entity(boat).get::<Boat>().is_some(),
            "the player boarded something that is not a boat"
        );
        // Aboard at the boat's own heading, standing at the helm.
        let helm = app
            .world()
            .entity(boat)
            .get::<Boat>()
            .expect("a boat")
            .helm();
        assert_eq!(
            player_transform(&mut app),
            Transform::from_translation(helm)
        );

        // Back at the helm: making sail moves the boat again. The wind is
        // set onshore — dead astern of a bow still pointed at the island —
        // because going ashore furled the sails and boarding left them so.
        set_wind(&mut app, Vec2::new(-7.0, 0.0));
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

    /// A chart with one closed island on it, centred where a test wants one.
    ///
    /// Built rather than sailed: this side's survey arrives over the wire, and
    /// what the claim key reads is the sheet. The block of chunks is what
    /// closes the ring — a cone with no water recorded round it is a coast
    /// that has not been shown to end.
    fn a_chart_with_an_island_at(middle: Vec2) -> Chart {
        use protocol::ground::{CHUNK_METRES, FACET_VERTS};
        use protocol::survey::survey;

        // Comfortably past [`protocol::survey::LEAST_ISLAND`], so the ring is
        // an island rather than a skerry, and comfortably inside one chunk, so
        // the block of water round it closes the ring.
        const REACH: f32 = 60.0;

        let heights = |chunk: IVec2| -> Vec<f32> {
            let base = chunk.as_vec2() * CHUNK_METRES;
            (0..FACET_VERTS * FACET_VERTS)
                .map(|i| {
                    let local = Vec2::new((i % FACET_VERTS) as f32, (i / FACET_VERTS) as f32)
                        * FACET_METRES;
                    REACH - (base + local).distance(middle)
                })
                .collect()
        };
        let mut chart = Chart::default();
        let home = (middle / CHUNK_METRES).floor().as_ivec2();
        for down in -1..=1 {
            for across in -1..=1 {
                let chunk = home + IVec2::new(across, down);
                chart.record(chunk, survey(&heights(chunk)));
            }
        }
        chart
    }

    #[test]
    fn the_claim_key_asks_from_the_beach_and_says_nothing_from_the_helm() {
        // Both halves of what this side judges for itself. A cairn is built by
        // somebody standing on the ground, so the key is dead at the helm —
        // and afoot inside a ring the sheet has closed, it asks, the world
        // being the one that rules on it.
        use crate::net::{fake_server, Online};
        use protocol::ToServer;

        let (addr, socket) = fake_server(Vec2::ZERO, Vec2::ZERO);
        let connection = crate::net::Connection::join(&addr).expect("join");
        let server = socket.recv().expect("the fake server keeps its socket");
        server
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("set timeout");

        let mut app = shore_app();
        let afloat = boat_transform(&mut app).translation.xz();
        app.insert_resource(a_chart_with_an_island_at(afloat));
        app.insert_resource(Online::new(connection));

        // Aboard, standing inside the very ring that would earn it: nothing
        // crosses. The silence is bracketed by a word said after it, a socket
        // that has gone quiet being indistinguishable from one that never
        // spoke.
        press_claim(&mut app);
        app.world()
            .resource::<Online>()
            .connection
            .command("help".to_string());
        assert_eq!(
            next_word(&server),
            ToServer::Command {
                line: "help".to_string()
            },
            "the helm asked for a cairn"
        );

        // Ashore, and the same key asks — for the island the sheet says is
        // underfoot, which is the only thing this side has to offer.
        press_board(&mut app);
        assert_eq!(aboard(&mut app), None, "the player is still aboard");
        let standing = player_transform(&mut app).translation.xz();
        let island = app
            .world()
            .resource::<Chart>()
            .island_under(standing)
            .expect("the walker stepped out inside the test island's ring");
        press_claim(&mut app);
        assert_eq!(next_word(&server), ToServer::Claim { island });
    }

    /// The next thing the client actually *says*, past the traffic every
    /// session carries anyway: where the player is, and the step ashore that
    /// put them there. None of it is what the claim key is about.
    fn next_word(server: &std::net::TcpStream) -> protocol::ToServer {
        use protocol::ToServer;
        loop {
            match ToServer::read(&mut &*server).expect("a word from the client") {
                ToServer::Move { .. } | ToServer::Helm { .. } | ToServer::Disembark { .. } => {
                    continue
                }
                word => return word,
            }
        }
    }

    /// One press of the claim key, released again afterwards.
    fn press_claim(app: &mut App) {
        hold(app, KeyCode::KeyC);
        run_frames(app, 1);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::KeyC);
        run_frames(app, 1);
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
