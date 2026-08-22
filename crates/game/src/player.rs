//! The player as a person, distinct from whatever is carrying them.
//!
//! The boat used to *be* the player: one entity, driven by the movement keys,
//! followed by the camera, reported to the server. That held only while there
//! was one way to exist in the world, and now there are two — aboard a boat,
//! and ashore on their own feet.
//!
//! Being aboard is [`ChildOf`]: the player rides the scene graph, and stepping
//! ashore is leaving the hierarchy — [`embark_or_land`] is the one threshold,
//! crossed both ways by the same key. Ashore, [`walk`] drives them with the
//! keys the helm answers to afloat, and which of the two systems is listening
//! is decided entirely by whether the player has a parent, so no mode flag can
//! fall out of step with the scene graph. What is *drawn* is
//! [`crate::figure`]'s business, and it reads the transform rather than
//! anything said here.
//!
//! Everything that wants "where the player is" — the camera, the position
//! reports, the wildlife deciding whether to mind them — asks [`PlayerPlace`],
//! which resolves through the *carrier*: the vehicle the player is aboard, or
//! the player themself on their own feet. Those systems neither know nor care
//! which it is, and that is the point: the rowboat changes what the player
//! boards and nothing about what follows them.
//!
//! Moving the player is not among them. That is the world's to do — the
//! server's `goto`, arriving as [`protocol::ToClient::PutDown`] and landing
//! in [`put_down`] — and this machine has no business putting itself
//! anywhere: a hull set down here rather than there could be on a hillside,
//! which the game itself never does.
//!
//! Reading and writing are two params rather than one because Bevy will not
//! let a system hold `&Transform` and `&mut Transform` at once, and the sweep
//! is the only thing that moves a player it did not spawn.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use protocol::ground::CELL_METRES;
use protocol::BoatKind;

use crate::bindings::{Action, KeyBindings};
use crate::boat::{spawn_hull, tender_berth, Boat, Fleet, HullId, HullKit, Rigged, Vessel};
use crate::cairn::{Cairn, BERTH};
use crate::chart::Chart;
use crate::figure::FigurePlugin;
use crate::net::Online;
use crate::sea::SeaConditions;
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
/// standing on: past this their feet leave the ground and they swim — see
/// [`walk`]. Chest deep on a figure not two metres tall, which is about where
/// a body honestly stops walking and starts floating. Comfortably more than
/// the rowboat's draft, so the boat rowed in until its keel takes the sand is
/// standing in water its crew can step out into.
///
/// Measured against the flat waterline, not the ground alone, so it is the
/// *sea* a walker wades and swims in. A lake is neither yet: the client keeps
/// no lake levels once the mesh is built, so a walker crosses a lakebed as if
/// it were dry — visibly wrong in deep lakes, and the honest fix is the
/// `Ground` resource learning lake levels, not a guess here.
const WADE_DEPTH: f32 = 1.0;

/// Metres per second swimming — half the walking pace: enough to cross a bay
/// or reach a ship at anchor, and slow enough that the rowboat is still worth
/// lowering. Backing up in the water is halved again, like the walk's.
const SWIM_SPEED: f32 = 1.5;

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
///
/// That flood was run against the 2 m field. The metre grid resolves the
/// fine bands it smoothed over, so the walled-off share in rugged country
/// runs higher now — a few percent, not two — and the reachability half has
/// not been re-measured. Worth re-flooding if walkers start fetching up
/// against ground the eye reads as ordinary.
const WALKABLE_RISE: f32 = 0.7;

/// The ring the landing probe searches, in metres from the rowboat's origin:
/// from just short of its bow — anything nearer is bilges — out to a few
/// strides past it. The far edge doubles as [`BOARD_REACH`] so that wherever
/// a player can step off, they can step straight back aboard from.
const LANDING_NEAR: f32 = 1.5;
const LANDING_REACH: f32 = 6.0;
/// Spacing of the probe's samples: half the facet the heights are drawn on,
/// derived so a strip of walkable ground one facet wide is never stepped
/// over, whatever the facet becomes.
const LANDING_STEP: f32 = CELL_METRES / 2.0;
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

/// A walker past their depth: sea deeper than [`WADE_DEPTH`] under them, feet
/// off the ground, riding at the surface. Kept current by [`walk`], and put on
/// by [`find_footing`] for somebody put down over deep water; every boarding
/// takes it off, a deck being dry however deep the sea beneath it. Drawing a
/// swimmer prone is [`crate::figure`]'s reading of this.
#[derive(Component)]
pub struct Swimming;

/// The walkers still waiting for ground to stand on, as a query — see
/// [`Unsettled`]. Not aboard anything: a player on a deck stands on the
/// deck, and the hull's own transform is not theirs to write.
type Waiting<'w, 's> = Query<
    'w,
    's,
    (Entity, &'static mut Transform),
    (With<Player>, With<Unsettled>, Without<ChildOf>),
>;

/// Settles an [`Unsettled`] walker onto the ground once it has arrived — or,
/// put down past wading depth, afloat at the surface instead. Height only:
/// where they stand is the server's word, and which way they face was entry's
/// guess to make.
fn find_footing(mut commands: Commands, ground: Option<Res<Ground>>, mut walkers: Waiting) {
    for (walker, mut place) in &mut walkers {
        let standing = ground
            .as_ref()
            .and_then(|g| g.height(place.translation.x, place.translation.z));
        if let Some(height) = standing {
            if -height > WADE_DEPTH {
                // The flat waterline serves here — [`walk`] rides the swell
                // from the next frame on.
                place.translation.y = 0.0;
                commands.entity(walker).insert(Swimming);
            } else {
                place.translation.y = height;
            }
            commands.entity(walker).remove::<Unsettled>();
        }
    }
}

/// The player and whatever they are aboard, as a query. The one shape every
/// system that has to answer "what is carrying the player" reads it in —
/// [`crate::net::receive`] included, which passes it on to the fleet.
pub type Players<'w, 's> = Query<'w, 's, (Entity, Option<&'static ChildOf>), With<Player>>;

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

/// Puts the player down where the world says they are, hull and all — the
/// client's half of [`protocol::ToClient::PutDown`], which is the one word
/// that overrules this machine about its own carrier.
///
/// The carrier is [`carrier_of`]'s, which is the whole point: the eye follows
/// that entity and the position reports are taken off it, so anything else
/// moved here would be a jump the camera never made and the server never
/// heard about. The scene graph is asked first for that reason and no other —
/// it is what every system resolving a carrier reads — and the fleet's book
/// only where the scene graph has no answer yet: the telling that seated this
/// player at a helm may have arrived in the same drain as this one, and the
/// parentage it asked for is still a queued command, the same case
/// [`Fleet::gone`] reads its own book for.
///
/// One window is left open on purpose: a telling that takes our helm away
/// takes the player off the deck in the same breath, but that is a queued
/// command where the book's line through it is written the instant the
/// telling is read — so until the next sync point the scene graph still says
/// the player is a hull's child, and a put down landing inside that window
/// moves a hull the wire has just stopped calling ours. Closing it means
/// changing which of the two is asked first, an argument about carriers in
/// general that wants its own reasons and its own test. Named rather than
/// denied, a window nobody has named being one the next change walks into.
///
/// Moved by a command rather than a transform for the queued-parentage case
/// again: the queue guarantees order, so a seating's transform is written
/// first and this one is the last word about where the hull is.
///
/// A hull is put down at rest. You were taken there; you did not sail there,
/// and a jump that left the sails drawing would deliver a ship to an
/// anchorage already making for the beach it was brought to look at. At rest
/// is also the state a world is entered in — see [`Boat::of`] — so this is
/// arriving, and arriving has never come with way on.
///
/// A walker is put down at sea level and left [`Unsettled`], exactly as
/// [`crate::net::enter_afoot`] puts down somebody entering on their own feet:
/// the ground at the far end of a jump has not arrived yet, and their old
/// island's height is no better a guess than the waterline.
pub fn put_down(
    commands: &mut Commands,
    fleet: &Fleet,
    players: &Players,
    at: Vec2,
    facing: Option<f32>,
) {
    let seated = players.single().ok();
    let hull = seated
        .and_then(|(_, aboard)| aboard.map(ChildOf::parent))
        .or_else(|| fleet.hull());
    let Some(carrier) = hull.or(seated.map(|(player, _)| player)) else {
        return;
    };
    let afoot = hull.is_none();
    commands
        .entity(carrier)
        .entry::<Transform>()
        .and_modify(move |mut place| {
            place.translation.x = at.x;
            place.translation.z = at.y;
            if afoot {
                place.translation.y = 0.0;
            }
            if let Some(facing) = facing {
                place.rotation = Quat::from_rotation_y(facing);
            }
        });
    if afoot {
        commands.entity(carrier).insert(Unsettled);
    } else {
        // Way off, sails furled, heel and pitch back to nothing. Written as
        // a whole boat rather than as a furl and a stop because that is what
        // "at rest" already is here, and the two would drift apart the day
        // something else is added to a hull's motion. Through the entry for
        // the reason the transform goes that way: a hull seated in this same
        // drain has no `Boat` yet to modify, and the entry lands after the
        // seating's own insert.
        commands
            .entity(carrier)
            .entry::<Boat>()
            .and_modify(|mut boat| *boat = Boat::of(boat.kind()));
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
    let reach = CELL_METRES / 2.0;
    let across = |step: Vec2| {
        let (behind, ahead) = (at - step, at + step);
        Some((ground.height(ahead.x, ahead.y)? - ground.height(behind.x, behind.y)?) / CELL_METRES)
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
/// forgiveness [`swims`] shows, and for the same reason: ground the client has
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

/// Whether the sea over a map point is past a walker's depth — deeper than
/// [`WADE_DEPTH`], where feet leave the ground and [`walk`] swims them.
/// Ground that has not arrived is not water to float in: a walker overtaken
/// by a slow chunk keeps walking, the same benefit of the doubt [`climb`]
/// gives, where [`footing`] — a landing choosing a spot — refuses instead.
fn swims(ground: Option<&Ground>, at: Vec2) -> bool {
    ground
        .and_then(|ground| ground.height(at.x, at.y))
        .is_some_and(|height| -height > WADE_DEPTH)
}

/// The walker as [`walk`] reads them: who they are, where they stand, whether
/// anything carries them, and whether the water does.
type Walker = (Entity, &'static mut Transform, Has<ChildOf>, Has<Swimming>);

/// Every cairn this client has been told of, as a query — the only solid thing
/// in the world. `Without<Player>` because Bevy cannot see that a cairn is
/// never the walker, and the walker's own transform is held mutably.
type Stones<'w, 's> = Query<'w, 's, &'static Transform, (With<Cairn>, Without<Player>)>;

/// Whether a step would walk into a pillar of stone.
///
/// A rule about the step rather than a shape to intersect — the same choice the
/// ground is made solid by, where a cliff is a limit on the climb and not a
/// wall. A cairn is a stack of rock about a metre across, so "is there a cairn
/// where I am putting my foot" is a handful of distances against the few a
/// client has been told of.
///
/// Refused only when the step goes *further in*. A claim raises a cairn where
/// the claimant is standing, so that is the one place somebody is certain to be
/// inside the berth; every step that lengthens the distance is allowed, so they
/// walk out of it — a rule that can trap somebody is a bug however rarely it
/// fires. A step *along* the berth is allowed too, so a walker turned back
/// rounds the stones rather than sticking on them — the climb rule's contour
/// clause in another shape.
///
/// [`BERTH`] is the cairn's own: how much room a pillar takes up is a fact
/// about the pillar.
fn barged(cairns: &Stones, from: Vec2, to: Vec2) -> bool {
    cairns.iter().any(|stones| {
        let stones = stones.translation.xz();
        to.distance(stones) < BERTH && to.distance(stones) < from.distance(stones)
    })
}

/// Every hull the gunwale key might mean, as a query: where each lies, what
/// kind it was rigged as, the sailing state of the one this player steers —
/// the others have none — and the name it answers to on the wire, which is
/// how a served world's helm is told apart from a local one's.
type Vessels<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Transform,
        Option<&'static mut Boat>,
        Option<&'static HullId>,
        &'static Rigged,
    ),
    With<Vessel>,
>;

/// Crosses the gunwale, whichever way the player is facing it. One key for
/// every crossing, because they are one threshold, and where the player
/// stands names the only thing the key could mean:
///
/// At the *ship's* helm, at rest, it lowers the ship's boat and steps down
/// into it — the shore is reached by rowing, never over the ship's own rail.
/// The berth is [`crate::boat::tender_berth`]'s: alongside, on the shoreward
/// side when the ground says which that is. Lowering also furls the sails,
/// so the ship is never left riding at anchor under canvas.
///
/// In the *rowboat*, at rest, a ship laid alongside outranks the shore: a
/// boat pulled deliberately against a hull is asking aboard, and the tender
/// goes back aboard with them (see [`crate::boat::Fleet::gone`]). Failing
/// that the key steps ashore, and the spot must offer [`footing`] — the probe
/// walks rings outward from just short of the bow to a few strides past it,
/// bow direction first, nearest winning, so the player steps to the shore the
/// bow is nosed against rather than teleporting down the beach. And there
/// must *be* such a spot: against a cliff coast the probe finds nothing and
/// the key does nothing, which is what makes beaches landings and cliffs
/// scenery without either being named anywhere. The rowboat lies where they
/// left it, anyone's.
///
/// Ashore — or swimming, a gunwale being the other way out of the water —
/// the key boards the nearest boat in reach: back into the hierarchy at the
/// helm, the marker comes off, and the keys answer again — with the ship's
/// sails as the player left them, making sail being a deliberate act rather
/// than a side effect of stepping aboard.
///
/// Every crossing asks the hull the player is leaving to be at rest first —
/// nobody steps off a deck making way — though "at rest" is read by
/// [`Boat::reads_as_stopped`], which forgives a glide too slow to matter
/// rather than refusing in silence (see the constant it reads for the line
/// it draws), and the crossing that is granted stops the hull as it goes.
/// A crossing that is *refused* — no footing, nothing in reach — leaves the
/// glide untouched: the key that does nothing must do nothing.
#[allow(clippy::too_many_arguments)]
fn embark_or_land(
    keys: Res<ButtonInput<KeyCode>>,
    bindings: Res<KeyBindings>,
    mut commands: Commands,
    mut kit: HullKit,
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
            let hull_entity = aboard.parent();
            let Ok((_, hull_place, sailing, _, _)) = vessels.get_mut(hull_entity) else {
                return;
            };
            let hull_place = *hull_place;
            let Some(mut hull) = sailing else {
                return;
            };
            if !hull.reads_as_stopped() {
                return;
            }

            if hull.kind() == BoatKind::Sloop {
                // The crew furls as the skipper steps down into the boat —
                // whichever end of the exchange settles the seat, the ship
                // is left at anchor with its canvas in — and the last of the
                // glide is taken off with it, so the ship lies where the
                // gate read it as lying.
                hull.comes_to_rest();
                hull.furl();
                let (berth, heading) = tender_berth(&hull_place, ground);
                match &online {
                    // A served world's tender is asked for, never assumed:
                    // the player steps down when the telling grants it — see
                    // [`crate::boat::Fleet::told`] — which over the loopback
                    // is the next frame, and across a real sea is a blink.
                    Some(online) => online.connection.lower(berth, heading),
                    // Offline the whole exchange is local: the rowboat goes
                    // in the water and the player crosses to its thwarts,
                    // the ship keeping its own state where it lies.
                    None => {
                        let tender = spawn_hull(
                            &mut commands,
                            &mut kit,
                            BoatKind::Rowboat,
                            Transform::from_xyz(berth.x, 0.0, berth.y)
                                .with_rotation(Quat::from_rotation_y(heading)),
                            None,
                        );
                        let boat = Boat::of(BoatKind::Rowboat);
                        let helm = boat.helm();
                        commands.entity(tender).insert(boat);
                        commands
                            .entity(player)
                            .insert((ChildOf(tender), Transform::from_translation(helm)));
                    }
                }
                return;
            }

            // The rowboat. A ship laid alongside first — the nearest one
            // whose helm is not visibly somebody's, exactly as boarding from
            // the beach offers them.
            let at = hull_place.translation.xz();
            let alongside = vessels
                .iter()
                .filter(|(entity, ..)| *entity != hull_entity)
                .filter(|(_, place, ..)| place.translation.xz().distance(at) <= BOARD_REACH)
                .filter(|(.., named, rigged)| {
                    rigged.0 == BoatKind::Sloop && named.is_none_or(|named| !fleet.manned(named.0))
                })
                .min_by(|(_, a, ..), (_, b, ..)| {
                    a.translation
                        .xz()
                        .distance(at)
                        .total_cmp(&b.translation.xz().distance(at))
                });
            if let Some((ship, _, sailing, named, _)) = alongside {
                match (&online, named) {
                    (Some(online), Some(named)) => online.connection.board(named.0),
                    // Offline: cross the decks and hoist the tender in, the
                    // local mirror of what the server does for a grant.
                    _ => {
                        let Some(helm) = sailing.map(Boat::helm) else {
                            return;
                        };
                        commands
                            .entity(player)
                            .insert((ChildOf(ship), Transform::from_translation(helm)));
                        commands.entity(hull_entity).despawn();
                    }
                }
                return;
            }

            // No ship in reach: the shore, where the probe finds any.
            let Some((spot, height)) = landing(ground, &hull_place) else {
                return;
            };
            // The boat is left as an anchorage leaves a ship: stopped, oars
            // in. Stopping is the commit the gate's reading deferred, and
            // shipping the oars keeps [`Boat::furl`]'s promise that nothing
            // is ever left sailing unmanned — a served world hands the hull
            // back and forgets its trim, but a lone one keeps this very
            // component for whoever boards it next. Fetched afresh because
            // the ship search above needed the query back.
            if let Ok((.., Some(mut left), _, _)) = vessels.get_mut(hull_entity) {
                left.comes_to_rest();
                left.furl();
            }
            let stepped = (spot - hull_place.translation.xz()).normalize_or_zero();
            commands.entity(player).remove::<ChildOf>().insert((
                Transform::from_xyz(spot.x, height, spot.y)
                    .with_rotation(Quat::from_rotation_y(f32::atan2(-stepped.x, -stepped.y))),
                DespawnOnExit(AppState::InWorld),
            ));
            // The step is the client's, having judged the footing; what the
            // wire is owed is the fact of it. The server frees the thwarts
            // for anyone, and this side gives the hull back to its moorings.
            // A world with no server behind it — the headless tests' — keeps
            // the whole exchange local.
            if let Some(online) = online {
                online.connection.disembark(spot);
                fleet.hand_back(&mut commands, hull_entity, Some(&hull_place));
            }
        }
        None => {
            let at = place.translation.xz();
            let Some((boat, _, sailing, named, _)) = vessels
                .iter()
                .filter(|(_, transform, ..)| transform.translation.xz().distance(at) <= BOARD_REACH)
                // A helm that is visibly somebody's is not offered — the
                // server would refuse the ask anyway, and this spares it.
                .filter(|(.., named, _)| named.is_none_or(|named| !fleet.manned(named.0)))
                .min_by(|(_, a, ..), (_, b, ..)| {
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
                        .remove::<Swimming>()
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
/// Past [`WADE_DEPTH`] of sea the walker swims: slower — [`SWIM_SPEED`] —
/// and riding just under the drawn surface at [`SWIM_HEIGHT`], swell and
/// all like a hull, instead of standing on the ground. [`Swimming`] says so
/// to whoever asks — the figure drawn prone most of all — and comes off the
/// moment their feet find the ground again, or at a gunwale, boarding being
/// the other way out of the water.
///
/// What can refuse a step is decided by the water at the far end of it, and
/// nothing else:
///
/// - A step **into the water** is always taken, at any depth and off any
///   edge. There is no fall to refuse when a body floats at the bottom of
///   it, and the alternative was worse than untidy: judged by [`climb`], a
///   shore that shelves steeply — which is most of them — walls a walker out
///   of the sea entirely, held at the water's edge by a seabed they could
///   swim over. Undoing a dive back up the same face is not offered, and
///   need not be: the sea is no trap, ending as it does at every beach.
///
/// - A step **onto dry ground** is [`climb`]'s: ground rising or falling
///   faster than [`WALKABLE_RISE`] is not walked over, judged in strides of
///   at most half a facet so no stride hides a whole facet of ground however
///   long the frame was. It is a limit on the step and not on the spot, so it
///   turns a walker back from a cliff without pinning them against it — the
///   face of a bluff can be crossed along its contour. Read from the seabed
///   for a swimmer, it is also what tells a beach to wade out of from a wall
///   to be swum along.
///
/// - And everywhere, [`barged`]'s: a cairn is the one built thing in this
///   world with any substance to it, and a step into one is not taken.
#[allow(clippy::too_many_arguments)]
fn walk(
    keys: Res<ButtonInput<KeyCode>>,
    bindings: Res<KeyBindings>,
    time: Res<Time>,
    ground: Option<Res<Ground>>,
    sea: Res<SeaConditions>,
    cairns: Stones,
    mut commands: Commands,
    mut players: Query<Walker, With<Player>>,
) {
    let Ok((walker, mut transform, aboard, was_afloat)) = players.single_mut() else {
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
        let pace = if swims(ground, transform.translation.xz()) {
            SWIM_SPEED
        } else {
            WALK_SPEED
        };
        let speed = if drive > 0.0 { pace } else { pace * 0.5 };
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
        let strides = (advance.length() / (CELL_METRES / 2.0)).ceil().max(1.0);
        let stride = advance / strides;
        for _ in 0..strides as usize {
            let (from, to) = (
                transform.translation.xz(),
                (transform.translation + stride).xz(),
            );
            let taken = match ground.and_then(|g| g.height(to.x, to.y)) {
                // Into the water, wading or swimming — see above.
                Some(height) if height <= 0.0 => true,
                Some(_) => climb(ground, from, to) <= WALKABLE_RISE,
                // Ground the client has not been sent is no reason to pin a
                // walker where they stand — [`climb`]'s forgiveness again.
                None => true,
            };
            if !taken || barged(&cairns, from, to) {
                break;
            }
            transform.translation += stride;
        }
    }

    // Feet on the ground out to wading depth; past it, riding the surface the
    // sea is actually drawn wearing rather than the flat waterline — a body
    // pinned to the waterline under a visible swell is sunk half the time.
    // The origin goes exactly on the water; how far a *body* lies through it
    // is the figure's own business — see [`crate::figure`].
    let at = transform.translation.xz();
    if let Some(height) = ground.and_then(|g| g.height(at.x, at.y)) {
        let afloat = -height > WADE_DEPTH;
        let level = if afloat {
            sea.water_over(ground, at, time.elapsed_secs_wrapped())
        } else {
            height
        };
        if transform.translation.y != level {
            transform.translation.y = level;
        }
        if afloat != was_afloat {
            if afloat {
                commands.entity(walker).insert(Swimming);
            } else {
                commands.entity(walker).remove::<Swimming>();
            }
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
        elapsed, hold, plunging_shore, run_frames, run_until, set_wind, test_ground, test_shore,
        world_app, SHORE_BLUFF_FOOT, SHORE_PEAK, SHORE_TOP, SHORE_WATERLINE, TEST_ISLAND_REACH,
    };

    /// How far off the waterline the ship is anchored in a [`shore_app`], in
    /// metres: a safe distance, the way the game means it now — well afloat,
    /// and far enough out that the rowboat rowed in to the beach leaves the
    /// ship behind [`BOARD_REACH`], so the second press of the key means the
    /// shore and not the ship it just left.
    const ANCHORAGE: f32 = 12.0;

    /// A match off the shore island's coast: the ship at anchor a rowable
    /// distance out, bow at the island, player aboard. The island is the one
    /// with a coast on it — an apron up through the waterline and a bluff
    /// behind — because a landing needs ground a walker could stand on,
    /// which the cliff-rimmed one has nowhere.
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

    /// What kind of hull the player is aboard — `None` on their own feet.
    fn aboard_kind(app: &mut App) -> Option<BoatKind> {
        let aboard = aboard(app)?;
        Some(
            app.world()
                .entity(aboard)
                .get::<Boat>()
                .expect("aboard a hull with sailing state")
                .kind(),
        )
    }

    /// The way the one hull of a kind is making, in metres a second — what
    /// the crossing gate reads, read the same way.
    fn way_of(app: &mut App, kind: BoatKind) -> f32 {
        let hull = hull_rigged(app, kind);
        app.world()
            .get::<Boat>(hull)
            .expect("a rigged hull sails")
            .way()
    }

    /// Runs the world until the hull's glide dips inside the crossing gate's
    /// forgiveness while still visibly moving — the window the strict gate
    /// refused — and fails the test if the glide dies before it is caught.
    fn run_into_the_glides_tail(app: &mut App, kind: BoatKind) {
        for _ in 0..2_000 {
            let way = way_of(app, kind);
            let hull = hull_rigged(app, kind);
            let read = app
                .world()
                .get::<Boat>(hull)
                .expect("a rigged hull sails")
                .reads_as_stopped();
            if read && way != 0.0 {
                return;
            }
            assert!(way != 0.0, "the glide ran out before the gate's window");
            run_frames(app, 1);
        }
        panic!("the glide never entered the gate's window");
    }

    /// The one hull of a kind in the match — most tests hold a ship and, once
    /// the boat is down, a rowboat, and mean one of them by name.
    fn hull_rigged(app: &mut App, kind: BoatKind) -> Entity {
        let hulls: Vec<Entity> = app
            .world_mut()
            .query_filtered::<(Entity, &Rigged), With<Vessel>>()
            .iter(app.world())
            .filter(|(_, rigged)| rigged.0 == kind)
            .map(|(hull, _)| hull)
            .collect();
        assert_eq!(hulls.len(), 1, "expected exactly one {kind:?} in the world");
        hulls[0]
    }

    /// How many hulls are in the water.
    fn hulls_afloat(app: &mut App) -> usize {
        app.world_mut()
            .query_filtered::<(), With<Vessel>>()
            .iter(app.world())
            .count()
    }

    fn transform_of(app: &mut App, hull: Entity) -> Transform {
        *app.world()
            .entity(hull)
            .get::<Transform>()
            .expect("a hull has a transform")
    }

    fn boat_transform(app: &mut App) -> Transform {
        let ship = hull_rigged(app, BoatKind::Sloop);
        transform_of(app, ship)
    }

    /// One tap of a key — down for a frame, then released — which is the
    /// whole gesture the oar keys read.
    fn tap(app: &mut App, key: KeyCode) {
        hold(app, key);
        run_frames(app, 1);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(key);
        run_frames(app, 1);
    }

    /// Pulls the rowboat the player is aboard towards the island at the
    /// origin until it takes the ground, then ships the oars: ends at rest
    /// with the bow at the beach and the ship left out of reach astern.
    fn row_in(app: &mut App) {
        tap(app, KeyCode::ArrowUp);
        run_frames(app, 500);
        tap(app, KeyCode::ArrowDown);
        run_frames(app, 30);
    }

    /// The whole way ashore, as the game means it now: lower the ship's
    /// boat, row it in, step off onto the beach.
    fn go_ashore(app: &mut App) {
        press_board(app);
        assert_eq!(
            aboard_kind(app),
            Some(BoatKind::Rowboat),
            "the boat was never lowered"
        );
        row_in(app);
        press_board(app);
        assert_eq!(aboard(app), None, "the landing never happened");
    }

    fn ground_height(app: &App, at: Vec2) -> f32 {
        app.world()
            .resource::<Ground>()
            .height(at.x, at.y)
            .expect("the test ground has arrived")
    }

    #[test]
    fn going_ashore_is_by_rowboat_and_steps_onto_walkable_ground() {
        let mut app = shore_app();
        assert!(aboard(&mut app).is_some(), "the player entered ashore");

        // The first press lowers the ship's boat and steps down into it —
        // the ship's own rail is never the way ashore.
        press_board(&mut app);
        assert_eq!(aboard_kind(&mut app), Some(BoatKind::Rowboat));
        let ship = hull_rigged(&mut app, BoatKind::Sloop);
        let anchorage = transform_of(&mut app, ship).translation.xz();
        assert!(
            anchorage.distance(Vec2::new(SHORE_WATERLINE + ANCHORAGE, 0.0)) < 0.5,
            "lowering the boat moved the ship to {anchorage}"
        );

        // Rowed in until the keel takes the sand, the second press lands.
        row_in(&mut app);
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

        // The ship lies at anchor exactly where it was left, and the rowboat
        // within a step of the walker: landing is the player leaving, not a
        // hull going anywhere.
        let held = transform_of(&mut app, ship).translation.xz();
        assert!(
            held.distance(anchorage) < 0.5,
            "going ashore moved the ship to {held}"
        );
        let beached = hull_rigged(&mut app, BoatKind::Rowboat);
        let beached = transform_of(&mut app, beached).translation.xz();
        assert!(
            spot.distance(beached) <= LANDING_REACH + 1e-3,
            "the player landed {} m from the rowboat, past the probe's reach",
            spot.distance(beached)
        );
    }

    #[test]
    fn going_ashore_is_refused_over_deep_water() {
        // At anchor in open ocean the boat still goes in the water — the
        // crew will lower it anywhere — but rowed off across deep water the
        // key finds neither footing nor a hull in reach, and does nothing.
        let mut app = world_app();
        app.insert_resource(test_shore());
        place_boat(&mut app, Vec2::new(TEST_ISLAND_REACH * 2.0, 0.0));

        press_board(&mut app);
        assert_eq!(aboard_kind(&mut app), Some(BoatKind::Rowboat));
        tap(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 250);
        tap(&mut app, KeyCode::ArrowDown);
        run_frames(&mut app, 60);

        press_board(&mut app);
        assert_eq!(
            aboard_kind(&mut app),
            Some(BoatKind::Rowboat),
            "the player was put over the side in open ocean"
        );
    }

    #[test]
    fn going_ashore_is_refused_against_a_cliff() {
        // The cliff-rimmed island, rowed right up to: there is dry ground a
        // stride from the bow and the key still does nothing, because ground
        // standing on end is not ground anybody could walk off onto. This is
        // what makes a cliff coast scenery — a landing has to find footing, and
        // footing is more than shallow water.
        let mut app = world_app();
        app.insert_resource(test_ground());
        place_boat(&mut app, Vec2::new(TEST_ISLAND_REACH + 8.0, 0.0));

        // The probe's own reach does hold dry land, so what refuses the landing
        // below is the steepness of it and not the distance.
        let dry = app
            .world()
            .resource::<Ground>()
            .height(TEST_ISLAND_REACH - 1.0, 0.0)
            .expect("the cliff island's rim has arrived");
        assert!(dry > 0.0, "the rim is under water, not a cliff");

        press_board(&mut app);
        assert_eq!(aboard_kind(&mut app), Some(BoatKind::Rowboat));
        row_in(&mut app);
        press_board(&mut app);
        assert_eq!(
            aboard_kind(&mut app),
            Some(BoatKind::Rowboat),
            "the player stepped off onto a cliff face"
        );
    }

    #[test]
    fn the_boat_is_not_lowered_from_a_deck_making_way() {
        // The same anchorage that lowers fine at rest refuses while the hull
        // is making way — and serves again once the sails are furled and the
        // way has run off.
        let mut app = shore_app();
        // Onshore — dead astern of a bow pointed at the island — so making
        // sail below actually makes way.
        set_wind(&mut app, Vec2::new(-7.0, 0.0));
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 6);
        // The fixture has to mean what the name says: the deck is making way
        // by the gate's own reading, not by a margin that a tweak to the
        // wind or the frame count could quietly erase.
        assert!(
            way_of(&mut app, BoatKind::Sloop) > 0.0,
            "six frames of canvas made no way"
        );
        let ship = hull_rigged(&mut app, BoatKind::Sloop);
        assert!(
            !app.world()
                .get::<Boat>(ship)
                .expect("a rigged hull sails")
                .reads_as_stopped(),
            "the fixture's way fell inside the gate's forgiveness — it no \
             longer tests a deck making way"
        );
        press_board(&mut app);
        assert_eq!(
            aboard_kind(&mut app),
            Some(BoatKind::Sloop),
            "the boat went over the side of a deck making way"
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
            aboard_kind(&mut app),
            Some(BoatKind::Rowboat),
            "the boat never went in once the ship had stopped"
        );
    }

    #[test]
    fn a_dying_glide_is_let_across_and_stopped_by_the_crossing() {
        // The forgiving half of the gate: furled and gliding at way nobody
        // watching could see, the key works — refusing here read as the key
        // being broken — and the crossing itself is what stops the ship, so
        // the deck stepped off is as still as the gate read it.
        let mut app = shore_app();
        set_wind(&mut app, Vec2::new(-7.0, 0.0));
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 30);
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.release(KeyCode::ArrowUp);
        keys.press(KeyCode::ArrowDown);
        run_frames(&mut app, 1);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::ArrowDown);

        run_into_the_glides_tail(&mut app, BoatKind::Sloop);
        press_board(&mut app);
        assert_eq!(
            aboard_kind(&mut app),
            Some(BoatKind::Rowboat),
            "a glide too slow to see refused the crossing"
        );
        assert_eq!(
            way_of(&mut app, BoatKind::Sloop),
            0.0,
            "the crossing left the abandoned ship gliding"
        );
    }

    #[test]
    fn a_refused_crossing_leaves_the_glide_alone() {
        // The other half of the same bargain: over deep water the key finds
        // nothing and does nothing — including to the way. A gate that
        // stopped the boat while refusing the crossing would be a brake
        // nobody asked for, wearing a key that claims to have done nothing.
        let mut app = world_app();
        app.insert_resource(test_shore());
        place_boat(&mut app, Vec2::new(TEST_ISLAND_REACH * 2.0, 0.0));

        press_board(&mut app);
        assert_eq!(aboard_kind(&mut app), Some(BoatKind::Rowboat));
        tap(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 250);
        tap(&mut app, KeyCode::ArrowDown);

        run_into_the_glides_tail(&mut app, BoatKind::Rowboat);
        press_board(&mut app);
        assert_eq!(
            aboard_kind(&mut app),
            Some(BoatKind::Rowboat),
            "the player was put over the side in open ocean"
        );
        assert!(
            way_of(&mut app, BoatKind::Rowboat) != 0.0,
            "a refused crossing stopped the boat dead"
        );
    }

    #[test]
    fn lowering_the_boat_furls_the_ships_sails() {
        // An anchorage never shows an unattended ship under canvas: stepping
        // down into the boat furls. The wind blows *offshore* here, so the
        // sails go up in irons — set, but the hull at rest at its anchorage,
        // which is what lets the lowering happen while there is still canvas
        // to take in.
        let mut app = shore_app();
        set_wind(&mut app, Vec2::new(7.0, 0.0));
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 2);

        fn sails(app: &mut App) -> bool {
            let ship = hull_rigged(app, BoatKind::Sloop);
            app.world()
                .entity(ship)
                .get::<Boat>()
                .expect("the ship keeps its sailing state")
                .sails_set()
        }
        assert!(sails(&mut app), "the sails never went up in irons");

        press_board(&mut app);
        assert_eq!(
            aboard_kind(&mut app),
            Some(BoatKind::Rowboat),
            "the lowering was refused"
        );
        assert!(
            !sails(&mut app),
            "the ship was left riding at anchor under canvas"
        );
    }

    #[test]
    fn the_walker_walks_the_way_they_face_and_stands_on_the_ground() {
        let mut app = shore_app();
        go_ashore(&mut app);
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
        go_ashore(&mut app);
        let before = player_transform(&mut app);

        hold(&mut app, KeyCode::ArrowLeft);
        run_frames(&mut app, 20);
        let after = player_transform(&mut app);

        assert_eq!(after.translation, before.translation, "turning moved them");
        assert_ne!(after.rotation, before.rotation, "they never turned");
    }

    /// Whether the player is swimming, by their own marker.
    fn swimming(app: &mut App) -> bool {
        app.world_mut()
            .query_filtered::<Has<Swimming>, With<Player>>()
            .single(app.world())
            .expect("a match should have a player in it")
    }

    /// Puts the player on their own feet at a spot, the way several swimming
    /// tests start: off whatever they were aboard, standing (or floating)
    /// there on the next frame's word.
    fn put_afoot(app: &mut App, at: Vec2) {
        let player = app
            .world_mut()
            .query_filtered::<Entity, With<Player>>()
            .single(app.world())
            .expect("a match should have a player in it");
        app.world_mut()
            .entity_mut(player)
            .remove::<ChildOf>()
            .insert(Transform::from_xyz(at.x, 0.0, at.y));
        app.update();
    }

    #[test]
    fn past_wading_depth_the_walker_swims_instead_of_stopping() {
        // Ashore, turned round, and marched at the sea for a long time: the
        // wade gives way to a swim at [`WADE_DEPTH`] instead of the water
        // refusing the step, and the walker ends well out over water past
        // their depth, riding at the surface rather than strolling the
        // seabed.
        let mut app = shore_app();
        go_ashore(&mut app);
        // Out to sea, the island being at the origin.
        face(&mut app, Vec2::X);

        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 600);

        let at = player_transform(&mut app).translation;
        let depth = -ground_height(&app, at.xz());
        assert!(
            depth > WADE_DEPTH,
            "the sea still stops the walker, held in {depth} m of water"
        );
        assert!(swimming(&mut app), "out of their depth with no swim on");
        assert!(
            (-1.0..1.0).contains(&at.y),
            "the swimmer rides at {} m, nowhere near the surface",
            at.y
        );
    }

    #[test]
    fn swimming_is_at_swimming_pace() {
        let mut app = shore_app();
        put_afoot(&mut app, Vec2::new(TEST_ISLAND_REACH * 2.0, 0.0));
        assert!(swimming(&mut app), "open ocean is past anybody's depth");
        face(&mut app, Vec2::X);
        let before = player_transform(&mut app).translation.xz();
        let start = elapsed(&app);

        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 60);

        let seconds = elapsed(&app) - start;
        let made = player_transform(&mut app).translation.xz().distance(before);
        assert!(
            (made - SWIM_SPEED * seconds).abs() < SWIM_SPEED * 0.05,
            "{made} m in {seconds} s is not swimming pace"
        );
    }

    #[test]
    fn a_swimmer_finds_their_feet_on_a_shelving_shore() {
        // The way back in: swum at the island, the feet go down where the
        // apron rises to wading depth and the walk simply carries on ashore.
        let mut app = shore_app();
        put_afoot(&mut app, Vec2::new(SHORE_WATERLINE + 10.0, 0.0));
        assert!(
            swimming(&mut app),
            "ten metres off this shore is deep water"
        );
        face(&mut app, Vec2::NEG_X);

        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 600);

        let at = player_transform(&mut app).translation;
        assert!(!swimming(&mut app), "still swimming, never came ashore");
        assert!(
            at.y > 0.0,
            "the swim ended at {} m instead of walking on inland",
            at.y
        );
        assert_eq!(
            at.y,
            ground_height(&app, at.xz()),
            "ashore but not standing on the ground"
        );
    }

    #[test]
    fn a_cliff_face_is_not_hauled_out_onto_from_the_water() {
        // The swimming twin of what makes a cliff coast scenery: against the
        // cliff island the sea ends at a wall, and a swimmer marched at it
        // for a long time is still in the water. What refuses them is the
        // same [`WALKABLE_RISE`] that refuses a walker — read from the
        // seabed — which is why a shelving beach a stride away would let
        // them wade out and this does not.
        let mut app = world_app();
        app.insert_resource(test_ground());
        put_afoot(&mut app, Vec2::new(TEST_ISLAND_REACH + 6.0, 0.0));
        assert!(swimming(&mut app), "off the rim is deep water");
        face(&mut app, Vec2::NEG_X);

        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 600);

        let at = player_transform(&mut app).translation;
        assert!(
            ground_height(&app, at.xz()) <= 0.0,
            "the swimmer came out of the sea onto the cliff"
        );
    }

    #[test]
    fn a_steeply_shelving_shore_is_still_walked_into() {
        // What the climb rule did to the water before the sea was let
        // receive anybody: a seabed dropping faster than [`WALKABLE_RISE`]
        // walled the walker out of a sea they could perfectly well swim in,
        // held at the water's edge with dry feet. The shore island's apron
        // is gentle, so the wall is built here — a shore that plunges — and
        // the walker marched at it has to end up swimming.
        let mut app = world_app();
        app.insert_resource(plunging_shore());
        put_afoot(&mut app, Vec2::new(-3.0, 0.0));
        assert!(!swimming(&mut app), "the walker started in the water");
        face(&mut app, Vec2::X);

        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 300);

        assert!(
            swimming(&mut app),
            "the walker was walled out of the sea by the bank under it, \
             stopped at {}",
            player_transform(&mut app).translation.x
        );
    }

    #[test]
    fn boarding_from_the_water_ends_the_swim() {
        // Swum out to the ship at anchor, the gunwale key works from the
        // water exactly as from the beach — and the deck is dry: the swim
        // comes off with the boarding.
        let mut app = shore_app();
        go_ashore(&mut app);
        face(&mut app, Vec2::X);
        let ship = hull_rigged(&mut app, BoatKind::Sloop);
        let anchorage = transform_of(&mut app, ship).translation.xz();

        hold(&mut app, KeyCode::ArrowUp);
        run_until(&mut app, "the swimmer never reached the ship", |app| {
            swimming(app)
                && player_transform(app).translation.xz().distance(anchorage) <= BOARD_REACH - 1.0
        });
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::ArrowUp);

        press_board(&mut app);
        assert_eq!(
            aboard_kind(&mut app),
            Some(BoatKind::Sloop),
            "the swimmer never climbed aboard"
        );
        assert!(!swimming(&mut app), "aboard and still swimming");
    }

    #[test]
    fn a_walker_put_down_over_deep_water_settles_afloat() {
        // Entry — or a `goto` — can land a walker over open ocean before or
        // after any ground question is answerable. Settling puts them at the
        // surface, swimming, never on the seabed.
        let mut app = world_app();
        app.insert_resource(test_shore());
        let player = app
            .world_mut()
            .query_filtered::<Entity, With<Player>>()
            .single(app.world())
            .expect("a match should have a player in it");
        app.world_mut()
            .entity_mut(player)
            .remove::<ChildOf>()
            .insert((
                Unsettled,
                Transform::from_xyz(TEST_ISLAND_REACH * 2.0, 0.0, 0.0),
            ));
        run_frames(&mut app, 2);

        let at = player_transform(&mut app).translation;
        assert!(
            swimming(&mut app),
            "put down over the ocean with no swim on"
        );
        assert!(
            (-1.0..1.0).contains(&at.y),
            "settled at {} m rather than afloat at the surface",
            at.y
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
        go_ashore(&mut app);
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

    /// Stands a cairn somewhere, as the session's drain would — the component
    /// and a transform being all [`barged`] reads, and the stones themselves
    /// being `cairn`'s business rather than this module's.
    fn raise_cairn(app: &mut App, at: Vec2) {
        let height = ground_height(app, at);
        app.world_mut().spawn((
            Cairn {
                island: IVec2::ZERO,
            },
            Transform::from_xyz(at.x, height, at.y),
        ));
    }

    /// How far the walker is standing from a spot on the map.
    fn away_from(app: &mut App, spot: Vec2) -> f32 {
        player_transform(app).translation.xz().distance(spot)
    }

    #[test]
    fn a_cairn_stops_the_walker_at_arms_length() {
        // The only solid thing in the world. Marched straight at one, the
        // walker fetches up against it rather than strolling through the
        // stones — and fetches up *at* it, not turned back somewhere short of
        // it by a rule that reaches further than the pillar does.
        let mut app = shore_app();
        go_ashore(&mut app);
        face(&mut app, Vec2::NEG_X);
        let stood = player_transform(&mut app).translation.xz();
        let stones = stood + Vec2::new(-6.0, 0.0);
        raise_cairn(&mut app, stones);

        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 300);

        let off = away_from(&mut app, stones);
        assert!(
            off >= BERTH - 1e-3,
            "the walker ended {off} m from the middle of a cairn, inside its {BERTH} m berth"
        );
        assert!(
            off < BERTH + 0.5,
            "the walker stopped {off} m off a cairn they were marched at, well short of its {BERTH} m berth"
        );
    }

    #[test]
    fn a_cairn_raised_where_the_player_stands_lets_them_walk_out_of_it() {
        // A claim raises a cairn where the claimant is standing, so the one
        // place a walker is certain to be inside the berth is the moment their
        // own claim is granted. Every step that lengthens the distance is
        // allowed, so they walk out of it — a rule that can shut somebody
        // inside a metre of stone is a bug however rarely it fires.
        let mut app = shore_app();
        go_ashore(&mut app);
        face(&mut app, Vec2::NEG_X);
        let stones = player_transform(&mut app).translation.xz();
        raise_cairn(&mut app, stones);
        assert!(
            away_from(&mut app, stones) < BERTH,
            "the walker was not inside the berth to begin with"
        );

        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 300);

        assert!(
            away_from(&mut app, stones) > BERTH,
            "the walker is walled in by their own claim"
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
        go_ashore(&mut app);
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
        go_ashore(&mut app);
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
        go_ashore(&mut app);
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
    fn ashore_the_keys_are_the_walkers_and_the_hulls_hold_station() {
        let mut app = shore_app();
        go_ashore(&mut app);

        // The movement keys are the walker's now: both hulls hold their spot
        // on the map while they are held. (Only the map spot — the swell
        // still bobs a floating hull, which is exactly the point of it.) A
        // short walk, because the second half of the test is boarding again
        // and the walker steps ashore most of [`BOARD_REACH`] from the
        // rowboat already.
        let ship = hull_rigged(&mut app, BoatKind::Sloop);
        let tender = hull_rigged(&mut app, BoatKind::Rowboat);
        let anchorage = transform_of(&mut app, ship).translation.xz();
        let beached = transform_of(&mut app, tender).translation.xz();
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 20);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::ArrowUp);
        assert_eq!(
            transform_of(&mut app, ship).translation.xz(),
            anchorage,
            "the empty ship sailed off with the walker's keys"
        );
        assert_eq!(
            transform_of(&mut app, tender).translation.xz(),
            beached,
            "the empty rowboat rowed off with the walker's keys"
        );

        // Back at the oars: the rowboat is the boat in reach, so the key
        // re-parents the player onto its thwarts. Said out loud first,
        // because the walk above only just leaves them in reach — if the
        // shape of the shore or the landing ever moves the hull further off,
        // this is the assertion that should fail rather than the boarding
        // below.
        let off = player_transform(&mut app)
            .translation
            .xz()
            .distance(beached);
        assert!(
            off <= BOARD_REACH,
            "the walk left the walker {off} m from the rowboat, past boarding reach"
        );
        press_board(&mut app);
        assert_eq!(
            aboard(&mut app),
            Some(tender),
            "the player never got back aboard the rowboat"
        );
        // Aboard standing at the rowboat's own helm.
        let helm = app
            .world()
            .entity(tender)
            .get::<Boat>()
            .expect("a boat")
            .helm();
        assert_eq!(
            player_transform(&mut app),
            Transform::from_translation(helm)
        );

        // Laid alongside the ship, the same key crosses the decks — and the
        // rowboat goes back aboard with them, out of the world.
        let alongside = anchorage + Vec2::new(2.0, 0.0);
        app.world_mut()
            .entity_mut(tender)
            .get_mut::<Transform>()
            .expect("a hull has a transform")
            .translation = Vec3::new(alongside.x, 0.0, alongside.y);
        app.update();
        press_board(&mut app);
        assert_eq!(
            aboard(&mut app),
            Some(ship),
            "the player never crossed back to the ship"
        );
        assert_eq!(
            hulls_afloat(&mut app),
            1,
            "the rowboat was left in the water after being hoisted in"
        );

        // Back at the helm: making sail moves the ship again. The wind is
        // set onshore — dead astern of a bow still pointed at the island —
        // because lowering the boat furled the sails and boarding left them
        // so.
        set_wind(&mut app, Vec2::new(-7.0, 0.0));
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 30);
        assert_ne!(
            transform_of(&mut app, ship).translation.xz(),
            anchorage,
            "the helm never came back with the player"
        );
    }

    #[test]
    fn boarding_needs_the_boat_in_reach() {
        let mut app = shore_app();
        go_ashore(&mut app);

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
        go_ashore(&mut app);
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
        use protocol::ground::{CHUNK_METRES, CORNERS};
        use protocol::survey::survey;

        // Comfortably past [`protocol::survey::LEAST_ISLAND`], so the ring is
        // an island rather than a skerry, and comfortably inside one chunk, so
        // the block of water round it closes the ring.
        const REACH: f32 = 60.0;

        let heights = |chunk: IVec2| -> Vec<f32> {
            let base = chunk.as_vec2() * CHUNK_METRES;
            (0..CORNERS * CORNERS)
                .map(|i| {
                    let local = Vec2::new((i % CORNERS) as f32, (i / CORNERS) as f32) * CELL_METRES;
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
        // underfoot, which is the only thing this side has to offer. Put
        // there by hand rather than by the gunwale key: a served world's
        // crossings wait on tellings this fake server will never send, and
        // none of that is what the claim key is about.
        let player = app
            .world_mut()
            .query_filtered::<Entity, With<Player>>()
            .single(app.world())
            .expect("a match should have a player in it");
        app.world_mut()
            .entity_mut(player)
            .remove::<ChildOf>()
            .insert(Transform::from_xyz(afloat.x, 0.0, afloat.y));
        app.update();
        assert_eq!(aboard(&mut app), None, "the player is still aboard");
        let standing = player_transform(&mut app).translation.xz();
        let island = app
            .world()
            .resource::<Chart>()
            .island_under(standing)
            .expect("the walker was stood inside the test island's ring");
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
}
