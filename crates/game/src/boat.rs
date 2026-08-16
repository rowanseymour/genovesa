//! Boats: the hulls the player gets about in, and the keys that steer the one
//! they are aboard.
//!
//! The player themself is not here. They are a person — see [`crate::player`]
//! — riding this boat as a child of it, and the boat is one of the vehicles
//! they will get about in rather than the player's own shape. What *kind* of
//! boat an entity is lives in its [`Hull`]: the dimensions and manners the
//! rules below are written against, ship-sized today ([`SHIP`]) and
//! rowboat-sized next, so a new kind of boat is a new `Hull` and a new model,
//! not a new module.
//!
//! A hull is modelled rather than drawn here: [`MODEL`] is a glTF file built
//! from a Blender master under `assets-src/`, and this module spawns its
//! meshes and steers what they hang off. The few dimensions a `Hull` names
//! are the ones the rules read — where the keel is, and how deep. Those are
//! not the model's to change quietly, so
//! `the_model_is_the_hull_the_keel_is_probed_along` holds the file to them;
//! everything else about the shape is the modeller's, and this file has no
//! opinion on it.
//!
//! A boat faces down its own -Z, so [`Transform::forward`] is the way it is
//! pointing and steering can leave the axis convention alone. Its origin is on
//! the waterline rather than at the keel or the deck, which is what lets
//! [`float`] put it down by simply setting the height of the surface it is on.

use bevy::asset::RenderAssetUsages;
use bevy::math::Vec3Swizzles;
use bevy::mesh::PrimitiveTopology;
use bevy::prelude::*;

use crate::bindings::{Action, KeyBindings};
use crate::camera::View;
use crate::player::Player;
use crate::sea;
use crate::terrain::Ground;
use crate::{eased, matte, model_mesh, AppState, Helm};

/// The ship, as a file. Built from `assets-src/models/boat/boat.blend` by
/// `assets-src/models/export.sh`, which is also where the export settings the
/// look depends on are written down.
const MODEL: &str = "models/boat.glb";

/// Which mesh in [`MODEL`] is which. glTF numbers its meshes rather than naming
/// them in a way the loader can ask for, so these are positions in the file —
/// which means reordering the objects in Blender would silently swap the hull
/// for the spar. `the_model_is_a_hull_and_a_spar_fit_to_draw` is what
/// stops that being found by looking at it.
const HULL_MESH: usize = 0;
const SPAR_MESH: usize = 1;

/// The dimensions and manners of one kind of boat — everything [`float`],
/// [`steer`] and [`grounding`] need to know to drive one. This is a boat's
/// *character*; what a particular boat is doing lives on [`Boat`]. The values
/// carry their reasoning where they are picked, on [`SHIP`].
#[derive(Clone, Copy)]
struct Hull {
    /// Length overall, in metres.
    length: f32,
    /// Beam, in metres — how far apart the water is sampled athwartships to
    /// read the roll the waves ask of the hull. Close to the model's planking
    /// but not held to it the way `length` is: the samples are reading the
    /// surface's slope, and a slope read a few centimetres wide of the hull
    /// is the same slope.
    beam: f32,
    /// Keel depth below the waterline. The sea is translucent, so this much
    /// of the hull shows through the water as a darker shape under the deck —
    /// but what makes it the game's business rather than the model's is
    /// [`Hull::grounding_draft`], which is measured from it.
    draft: f32,
    /// The quarterdeck: the raised deck aft, in metres above the waterline,
    /// and the helm's station on it — where somebody aboard stands, beside
    /// the tiller. A player is put down here rather than at the hull's
    /// origin, which is the waterline and so is knee-deep in the bilges.
    /// The model's numbers rather than the game's to choose, like the draft:
    /// held to the file by `the_model_is_the_hull_the_keel_is_probed_along`.
    /// The main deck's own height is *not* here — nothing below the sail's
    /// corners reads it, so it belongs to the model alone.
    quarterdeck: f32,
    helm_station: f32,
    /// Where the keel begins and ends, in metres from amidships — negative
    /// forward, the same axis the hull is modelled on. [`grounding`] probes
    /// along these, so what runs aground is the line that is drawn.
    forefoot_station: f32,
    heel_station: f32,
    /// The masthead: metres above the waterline, and metres from amidships on
    /// the same axis as the keel's stations. The pennant is tied on here, so
    /// like the deck and the draft these are the model's numbers rather than
    /// the game's to choose — a mast re-cut in Blender and not re-measured
    /// here would fly its pennant in mid-air beside the spar, which is what
    /// `the_model_flies_a_pennant_from_its_masthead` is for.
    masthead: f32,
    masthead_station: f32,
    /// Metres per second under way.
    speed: f32,
    /// Metres per second going astern.
    astern_speed: f32,
    /// Seconds of lag between the speed the keys ask for and the speed the
    /// hull makes — the time constant of an exponential ease, so most of any
    /// change arrives within this long and it is all but done in three times
    /// it. Named as a duration rather than as the rate [`eased`] takes, a
    /// hull having a weight that is easier to think about in seconds. The
    /// ease is what gives a hull that weight: it gathers way over seconds
    /// instead of leaping to `speed` on the frame the key goes down, and
    /// carries a glide when the key comes up.
    way_response: f32,
    /// How fast the helm brings the bow round, in radians per second.
    /// Together with `speed` this fixes the turning circle.
    turn_rate: f32,
    /// How far the hull heels in a full-helm turn at full speed, in radians.
    /// It heels *outwards*, the way a keeled hull does: the water grips the
    /// keel below the waterline while the turn flings the mass above it, so
    /// the boat leans out of the corner, not into it like a bicycle.
    heel_at_full_turn: f32,
    /// Seconds of lag between the heel a turn asks for and the heel the hull
    /// shows, the same exponential shape as `way_response` and much quicker:
    /// rolling is the lightest thing a hull does. Quick enough that the lean
    /// arrives while the turn is still news, slow enough that the hull rolls
    /// rather than snaps — and the same curve is the straightening, run back
    /// down to level when the helm comes off.
    heel_response: f32,
    /// Seconds of lag between the tilt the water asks for and the tilt the
    /// hull shows — the same exponential family as `heel_response`, and a
    /// good deal slower: heeling is the hull rolling on its own keel, this is
    /// the whole hull being *lifted* by one end. The lag is also what keeps
    /// the chop out of the deck. Under way the short seas pass beneath the
    /// hull every couple of seconds, and a deck that chased each one
    /// faithfully would wag; on this curve the hull rides the long swell and
    /// lets the chop go by underneath.
    sway_response: f32,
}

impl Hull {
    /// How little water the hull is held in: ground standing higher than this
    /// far below the waterline stops it.
    ///
    /// On the coasts the generator draws this puts the hull within a metre or
    /// two of the waterline; where it holds a boat further off, it is off a
    /// shelf too thin to float one, and the shallows are painted as shallows
    /// long before they are this thin — so a boat held out is held out of
    /// water it can be seen to be held out of.
    fn grounding_draft(&self) -> f32 {
        self.draft - KEEL_BITE
    }

    /// Where somebody aboard stands, in the hull's own frame: at the helm,
    /// on the quarterdeck, forward of the tiller.
    fn helm(&self) -> Vec3 {
        Vec3::new(0.0, self.quarterdeck, self.helm_station)
    }

    /// Where the hull meets the water forward, in its own frame: the stem, on
    /// the waterline. Not the forefoot — that is where the *keel* begins, a
    /// good deal aft of the bow because the stem is raked — and the water is
    /// parted at the bow.
    fn stem(&self) -> Vec3 {
        Vec3::new(0.0, 0.0, -self.length / 2.0)
    }
}

/// The ship: the boat a world is entered aboard, and [`MODEL`]'s subject.
const SHIP: Hull = Hull {
    // A small sailing boat: at the default zoom the visible ground is some
    // tens of metres across, so this length reads as a boat rather than as a
    // speck, and at the far end of the zoom range it is still a mark on the
    // water rather than gone.
    length: 7.0,
    beam: 2.4,
    draft: 0.8,
    // The step up aft and the spot on it just forward of the tiller's grip —
    // the model's numbers. The station keeps the helmsman clear of the boom,
    // which sweeps the main deck and nothing abaft the step.
    quarterdeck: 1.2,
    helm_station: 2.6,
    // The forefoot stops short of the bow, which is what gives the stem its
    // rake; the heel runs right aft to the transom.
    forefoot_station: -7.0 * 0.5 * 0.7,
    heel_station: 7.0 * 0.5,
    // Six metres of mast, stepped forward of amidships — the model's, not a
    // choice made here.
    masthead: 6.9,
    masthead_station: -1.05,
    // Brisk beyond honesty for a seven-metre hull, but the ship is how the
    // world is crossed: at this speed the ground in view at the default zoom
    // slides by in a few seconds, and the next island is minutes away rather
    // than tens of minutes.
    speed: 10.0,
    // Enough to back off a beach or out of a cove, and slow enough that
    // nobody crosses an ocean in reverse.
    astern_speed: 4.0,
    // The hull gathers way over a few seconds and carries it for a couple of
    // lengths' glide — seven metres of timber, felt.
    way_response: 1.5,
    // With the speed above, a turning circle of about five metres — tight
    // enough to feel answerable from a camera forty metres up, wide enough
    // that coming about reads as a turn rather than a spin.
    turn_rate: 2.0,
    // Enough to swing the masthead more than a metre, which is what makes a
    // turn visible from forty metres up, and shy of anything that reads as
    // capsizing.
    heel_at_full_turn: 0.22,
    // Quick, rolling being the lightest thing seven metres of timber does.
    heel_response: 0.4,
    // And the lift of the whole hull much slower than its roll.
    sway_response: 0.9,
};

/// How much of the keel the ground is allowed to take before a hull is
/// stopped. Stopping a boat the instant the ground rises to meet the keel is
/// an invisible wall a boat's length offshore, whereas a fifth of a metre of
/// bite is a boat *beaching*: the keel is seen to touch, and then it stops.
/// Well clear of the two centimetres the heights are quantised to, so the
/// threshold cannot chatter. One constant for every hull — what it answers to
/// is the quantisation, not the boat.
const KEEL_BITE: f32 = 0.2;

/// How many points along the keel are asked about the bottom. Spread from the
/// forefoot to the heel inclusive, so the gap between them comes out under
/// the two metres the ground is sampled at — an invariant each hull's keel
/// length has to keep, and `the_keel_is_probed_as_closely_as_the_ground_is_
/// sampled` pins: no facet of the height field can lie wholly between two
/// probes, so ground that rises across a facet is read on the way up rather
/// than stepped over.
///
/// That is what the spacing buys, and it is worth being plain that it is less
/// than "nothing gets past". A crest only one lattice line wide is *not* seen:
/// the field is linear between its corners, so two probes either side of such a
/// crest read its flanks, and the hull sails through a rock standing at the
/// waterline. Coasts are safe from it by being coasts — the bottom shelves, so
/// the ground under the keel is near enough monotone, and the innermost probe
/// is reading the shallowest water and reading it honestly. What is exposed is
/// the isolated skerry, which the generator draws on purpose and draws about a
/// facet across. Seeing one reliably would mean probing at a fraction of a
/// metre rather than at two, on every frame and at both poses, to buy a rock in
/// open water — while the coasts, which are what a boat is actually stopped by,
/// need none of it. Sailing through a skerry is the smaller wrong, and the one
/// that can be paid off from the other end, by giving the skerries some width.
///
/// The sides are not probed: a hull here is a shallow V, drawing its full
/// draft on the centreline and nothing at all at the beam, so a probe out
/// there would
/// have to carry a draught of its own to say anything the keel has not said.
/// That is a standing condition on the model rather than an observation about
/// one — a hull remodelled with a flat bottom carried out to the beam would
/// need probes out there too.
const KEEL_PROBES: usize = 4;

/// Way below this, with no drive asked for, is stopped, and [`steer`] snaps
/// it to exactly zero. The ease only ever halves the remainder — left alone
/// the boat would creep forever, never quite done stopping — and a hull at
/// rest should be *at rest*: the same spot every frame, nothing moving.
const WAY_STOPPED: f32 = 0.02;

/// Within this of the heel the turn is asking for, the hull snaps to it
/// exactly — the same tail-closing that [`WAY_STOPPED`] does for the way,
/// and for the same reason: the ease only ever halves the remainder, and a
/// hull done straightening should be *level*, holding one rotation frame
/// after frame rather than forever creeping towards it. A third of a degree,
/// invisible at any zoom.
const HEEL_SETTLED: f32 = 0.005;

/// The no-go zone: within this of head-to-wind, set sails carry nothing, in
/// radians. This is the one piece of sailing realism kept for its own sake,
/// because it *is* the game — upwind is tacked for, downwind is free — and
/// 45 degrees is wide enough that pinching reads as a mistake while a
/// full-helm tack still crosses the whole zone inside a second (see
/// `a_tack_carries_way_through_the_eye_of_the_wind` for the arithmetic,
/// pinned).
///
/// These are the game's sailing rules rather than any hull's manners, which
/// is why they are module constants and not [`Hull`] fields, the same
/// standing [`KEEL_BITE`] has: the ship sails by them today, and the rowboat
/// to come is rowed, not sailed.
const NO_GO: f32 = std::f32::consts::FRAC_PI_4;

/// Where the sails reach their full drive: a beam reach, a quarter turn off
/// the wind. From the edge of the no-go zone to here the drive ramps
/// linearly, and from here through a dead run it is full — no downwind
/// taper, on purpose. A real hull's polar sags a little dead downwind, but
/// modelling that only makes the fastest point of sail one the player is
/// never quite on, which is realism spent making the game worse.
const FULL_DRIVE: f32 = std::f32::consts::FRAC_PI_2;

/// The band the wind's strength drives the hull across: the fraction of
/// [`Hull::speed`] made in a flat calm, and the fraction made once the wind
/// saturates. The floor is what keeps a calm from stranding anybody — the
/// world is crossed by boat and the weather holds its spells for minutes at
/// a time, so no sky may take the boat away — and the ceiling is a modest
/// reward for sailing a blow rather than a new top gear. The angle to the
/// wind is the game; the strength is flavour inside this band. Under the
/// assumed 7 m/s breeze the factor comes out near 0.9.
const DRIVE_BAND: (f32, f32) = (0.6, 1.1);

/// The wind at which the drive saturates, in metres per second — a strong
/// breeze, short of the near-gale the weather tops out at. Above it more
/// wind is more sea — the swell's business — but no more speed.
const WIND_SATURATES: f32 = 12.0;

/// Below this, in metres per second, the wind names no direction — the same
/// bar the compass's arm and the sea's wave trains hold themselves to — so
/// the no-go zone stands aside and the calm's floor drives on any heading.
/// Without it a dying air would still park a boat pointed the wrong way,
/// with nothing on screen left to say which way "wrong" was.
const WIND_NAMED: f32 = 0.5;

/// How much of the hull's speed set sails can draw at an angle off the wind,
/// 0..=1. `off_wind` is the unsigned angle between the bow and the eye of
/// the wind: zero head-to-wind, a straight angle on a dead run.
fn polar(off_wind: f32) -> f32 {
    ((off_wind - NO_GO) / (FULL_DRIVE - NO_GO)).clamp(0.0, 1.0)
}

/// How hard the wind drives, as a factor on [`Hull::speed`]: [`DRIVE_BAND`]
/// walked linearly, saturating at [`WIND_SATURATES`].
fn strength(wind_speed: f32) -> f32 {
    let (floor, ceiling) = DRIVE_BAND;
    floor + (ceiling - floor) * (wind_speed / WIND_SATURATES).clamp(0.0, 1.0)
}

/// The lean a turn asks of the hull, in radians of roll: the full-turn heel,
/// by how hard the helm is over, by the way's share of the hull's speed —
/// that share clamped at one, because a blow drives past hull speed (see
/// [`DRIVE_BAND`]'s ceiling) and the full-turn heel is a ceiling of its own,
/// not a proportion to be outgrown. Port helm is a positive turn and an
/// outward lean is to starboard, which about the forward axis is a negative
/// roll — hence the sign.
fn heel_for(hull: &Hull, helm: f32, way: f32) -> f32 {
    -hull.heel_at_full_turn * helm * (way / hull.speed).clamp(-1.0, 1.0)
}

/// The speed set sails ask for, as a factor on [`Hull::speed`]: the polar at
/// this heading times the wind's strength. `bow` is the hull's forward in
/// the map's terms; `wind` is [`sea::SeaConditions::wind`] — the *true*
/// wind, not the apparent, so the speed a heading earns holds still while
/// the boat gathers way towards it. The pennant at the masthead flies the
/// apparent wind and will disagree; that disagreement is real sailing, not
/// a bug to reconcile.
fn sail_drive(bow: Vec2, wind: Vec2) -> f32 {
    let blowing = wind.length();
    if blowing < WIND_NAMED {
        return strength(blowing);
    }
    // The angle off the eye of the wind — the bow against where the air is
    // coming *from*, `wind` being where it is going.
    strength(blowing) * polar(bow.angle_to(-wind).abs())
}

/// The pennant. Hotter and lighter than the hull's timber, which is the only
/// other warm thing in a world of greens and blues: at the far end of the zoom
/// the boat is a mark on the water and this is the mark on the mark.
const PENNANT_COLOR: Color = Color::srgb(0.87, 0.35, 0.18);

/// The pennant, as a shape: how far it flies from the mast and how deep it is
/// at the hoist, in metres. A long thin burgee rather than a square flag —
/// the length is what carries the bearing at the distance the boat is watched
/// from, and the narrow hoist is what lets a triangle stand in for a hanging
/// flag when the wind drops (see [`pennant_mesh`]).
const PENNANT: (f32, f32) = (1.2, 0.3);

/// Canvas. Near the spar's cream and a shade warmer, so a set sail reads
/// against sky and sea without adding a new colour to a palette this small.
const SAIL_COLOR: Color = Color::srgb(0.93, 0.89, 0.79);

/// The sail, as corners in the frame of an entity stood at the mast's foot
/// on the waterline — so everything [`trim_the_sails`] does is a rotation
/// about the mast, which is what trimming is. Tack and head up the luff,
/// clew aft along the boom, in metres. The head stops short of the masthead
/// so the pennant flies clear of the cloth, and the clew ends inboard of the
/// transom.
const SAIL_TACK: Vec3 = Vec3::new(0.0, 1.3, 0.0);
const SAIL_HEAD: Vec3 = Vec3::new(0.0, 6.5, 0.0);
const SAIL_CLEW: Vec3 = Vec3::new(0.0, 1.3, 3.0);
/// The belly: pushed out to one side for exactly the pennant's reason — see
/// [`pennant_mesh`] — and proportionally deeper, canvas drawing harder than
/// a flag.
const SAIL_BELLY: Vec3 = Vec3::new(0.4, 3.4, 1.1);

/// How far the boom lies off the centreline, in radians: close-hauled at the
/// edge of the no-go zone, eased out to nearly square on a dead run. Visual
/// only — the drive is [`sail_drive`]'s business — but a sail sheeted the
/// way the wind asks is what the eye reads as "the wind is doing this",
/// which is the whole point of drawing one.
const TRIM_BAND: (f32, f32) = (0.26, 1.35);

/// The apparent wind that flies the pennant out, in metres per second, and
/// how far it still sags at that wind, in radians. Both are the drawing's to
/// pick rather than the weather's: a flag that only lifted in the gales would
/// be a limp rag through most of a day, and one that ever came out perfectly
/// straight would read as a signboard rather than as cloth.
const PENNANT_FLIES: (f32, f32) = (5.0, 0.14);

/// How far the pennant's tie stands off the mast's axis, in metres: the
/// spar's half-width and a little air. The flag is tied to the spar's
/// *surface* on the side it is flying — sliding round the timber with the
/// wind, the way a ring on a mast would — because a tie on the axis swings
/// every sag of the cloth down through the spar itself, and a becalmed flag
/// hung entirely inside the masthead. Held to the model's actual girth by
/// `the_model_flies_a_pennant_from_its_masthead`.
const PENNANT_TIE_OFF: f32 = 0.1;

/// Below this apparent wind, in metres per second, the pennant keeps the
/// bearing it had and simply hangs. The same problem the compass's arm has —
/// a dying wind's direction is noise — and the same answer, except that here
/// the hanging *is* the reading: a flag straight down is how a calm looks.
const PENNANT_CALM: f32 = 0.3;

/// The flutter: how far the pennant swings either side of its bearing at full
/// wind, in radians, and how fast, in radians per second. It is scaled by the
/// wind like everything else about the flag, so a calm hangs dead still and a
/// blow snaps — cloth being the one thing in view that says how hard it is
/// blowing without being asked.
const FLUTTER: (f32, f32) = (0.16, 7.0);

/// A boat in the world: what kind it is, and what it is doing.
///
/// `hull` is the kind — the ship for now, and the rest is the sailing state.
/// `way` is the speed the hull is actually making along its heading, in
/// metres per second, ahead positive — the state the eased throttle lives
/// in. The keys name a speed; [`steer`] brings `way` towards it.
///
/// `heel` is the roll the hull is showing *for the turn*, in radians about
/// its own forward, positive with the masthead to port. `pitch` and `roll`
/// are the tilt the water is showing on it — [`float`] easing the deck
/// towards the sea's own slope under the hull, pitch about the athwart axis
/// with the bow up positive, roll the same sign and the same rotation factor
/// as the heel. All three are kept here rather than read back off the
/// transform because the transform holds heading, pitch and the two rolls
/// multiplied together, and unpicking a quaternion every frame to learn
/// numbers these systems wrote themselves is work for nothing.
#[derive(Component)]
pub struct Boat {
    hull: Hull,
    way: f32,
    heel: f32,
    pitch: f32,
    roll: f32,
    /// Whether the sails are set. Set, the wind is the throttle — see
    /// [`sail_drive`]; furled, the target way is zero, the hull glides to a
    /// stop and holds station, and that holding is the whole of "anchored"
    /// here — the crew drops the hook, nothing simulates it.
    sails_set: bool,
}

impl Boat {
    /// The ship a world is entered aboard, at rest — sails furled, a world
    /// being entered at anchor.
    pub fn ship() -> Self {
        Self {
            hull: SHIP,
            way: 0.0,
            heel: 0.0,
            pitch: 0.0,
            roll: 0.0,
            sails_set: false,
        }
    }

    /// Sets the sails: the wind has the hull until [`furl`] takes it back.
    ///
    /// [`furl`]: Boat::furl
    pub fn hoist(&mut self) {
        self.sails_set = true;
    }

    /// Furls the sails. The way runs off on the hull's own glide and the
    /// boat holds station where it dies — which is what going ashore does to
    /// a boat on the way off it, so nothing is ever left sailing unmanned.
    pub fn furl(&mut self) {
        self.sails_set = false;
    }

    /// Whether the sails are set.
    pub fn sails_set(&self) -> bool {
        self.sails_set
    }

    /// Whether the hull has no way on at all. Exact equality is meaningful
    /// here because [`steer`] snaps the tail of every glide to precisely
    /// zero — see [`WAY_STOPPED`] — so a hull is either making way or it is
    /// this. What going ashore asks before it lets anybody step off a moving
    /// deck.
    pub fn at_rest(&self) -> bool {
        self.way == 0.0
    }

    /// Where somebody aboard stands, in the hull's own frame — see
    /// [`Hull::quarterdeck`]. What a player boarding is put down at, so that
    /// they stand at the helm rather than in the bilges.
    pub fn helm(&self) -> Vec3 {
        self.hull.helm()
    }

    /// Where the hull parts the water, in its own frame — see [`Hull::stem`].
    /// The wake is laid from here rather than from the origin amidships, so
    /// that the white water the hull is standing in is water its own bow
    /// turned over a moment ago.
    pub fn stem(&self) -> Vec3 {
        self.hull.stem()
    }

    /// How wide a stretch of water the hull pushes aside, in metres — its
    /// beam. What the wake is scaled off, a bigger hull leaving a broader
    /// one; see [`crate::wake`].
    pub fn beam(&self) -> f32 {
        self.hull.beam
    }

    /// The way the hull is making, in metres a second — negative going
    /// astern. The hull's own number rather than anything measured off its
    /// transform, which is the point: a transform moves for reasons that are
    /// not sailing, and [`crate::wake`] wants the speed the water is being
    /// stirred at.
    pub fn way(&self) -> f32 {
        self.way
    }
}

/// The pennant flying at the masthead, and the bearing it is streaming on —
/// its own, kept here rather than read back off the transform because the
/// transform holds the boat's rotation taken out again, and because a calm
/// has to leave the bearing where it was rather than invent a new one.
#[derive(Component)]
struct Pennant {
    bearing: f32,
}

/// The sail hung from the mast — a marker, unlike [`Pennant`], because trim
/// carries no memory: the boom lies where [`sail_trim`] puts it this frame,
/// and a wind too slack to name a side leaves it on the centreline, hidden
/// under a furl nobody is watching for long anyway.
#[derive(Component)]
struct Sail;

pub struct BoatPlugin;

impl Plugin for BoatPlugin {
    fn build(&self, app: &mut App) {
        // Steering before floating, so ground gained or lost by this frame's
        // movement is under the hull the same frame rather than the next.
        // The conditions the hull floats on, here as well as in the terrain
        // plugin: resources are global and initialising one twice is free,
        // and the boat's own tests run without any terrain at all.
        app.init_resource::<sea::SeaConditions>()
            .add_systems(OnEnter(AppState::InWorld), launch)
            .add_systems(
                Update,
                // Only the steering stops when the game is paused. Floating is
                // not motion — it sets the hull to the height of the ground
                // under it — so leaving it running means a chunk arriving
                // while the pause menu is up is settled on before the player
                // looks again, rather than snapping under them on resume.
                // The pennant and the sail last, and outside the pause like
                // the floating: both are drawn off the hull's rotation and
                // the wind, so reading either before this frame's steering
                // had written it would leave the cloth a frame behind the
                // mast it hangs on.
                (
                    steer.run_if(in_state(Helm::Sailing)),
                    float,
                    fly_the_pennant,
                    trim_the_sails,
                )
                    .chain()
                    .run_if(in_state(AppState::InWorld)),
            );
    }
}

/// Puts the ship in the world at the point the world is entered, pointing the
/// way the opening view looks — with the player aboard, entering a world
/// being something done afloat.
///
/// The view names where the player enters the world, so the boat goes there
/// rather than anywhere of its own choosing. Entry is the world's spawn
/// point — open water the layout keeps just off the first island's coast —
/// so the boat starts afloat with land dead ahead; a `--focus` can still put
/// it down inland, aground until the movement keys drive it back to the sea.
/// The meshes hang off the boat as children rather than on it: the hull and
/// the spar stay two meshes not for their colours — both carry their own now —
/// but because the game measures them separately, the keel probed along one
/// and the pennant tied to the other. Their geometry is already in the boat's
/// own frame — the modeller places the mast on the deck, not the game — so the
/// children sit at the identity and the only transform anything writes is the
/// boat's own. The player is one more child: aboard *is* being in the
/// hierarchy — see [`crate::player`] — so they stand wherever the hull carries
/// them and go down with the ship when the world is left.
fn launch(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    assets: Res<AssetServer>,
    view: Res<View>,
) {
    // The model carries its own colours on its facets — see the master's
    // NOTES — so the timber is drawn with one white matte that does nothing
    // but let them through, the same way every painted model here is. The
    // file's PBR materials are still ignored: lit the way the file asked for,
    // the hull would be the one surface in the world with a highlight on it.
    let painted = materials.add(matte(Color::WHITE));
    // Cloth is the one thing aboard with no inside, so it is the one material
    // here that is drawn from both faces. Left single-sided the pennant would
    // wink out every time the wind put its back to the camera, which happens
    // several times a minute at a flutter.
    let pennant_material = materials.add(StandardMaterial {
        double_sided: true,
        cull_mode: None,
        ..matte(PENNANT_COLOR)
    });
    // Cloth again, so drawn from both faces for the pennant's reason.
    let sail_material = materials.add(StandardMaterial {
        double_sided: true,
        cull_mode: None,
        ..matte(SAIL_COLOR)
    });

    commands.spawn((
        Name::new("Boat"),
        Boat::ship(),
        DespawnOnExit(AppState::InWorld),
        // A rotation of `yaw` about the vertical takes -Z to the camera's own
        // forward, so the boat starts pointing away from the viewer.
        Transform::from_xyz(view.focus.x, 0.0, view.focus.z)
            .with_rotation(Quat::from_rotation_y(view.yaw)),
        // Carried by the parent because the children inherit it: without one
        // here there is nothing for their own visibility to be computed
        // against, and a boat whose meshes are on entities of their own would
        // never be drawn.
        Visibility::default(),
        children![
            (
                Name::new("Hull"),
                Mesh3d(assets.load(model_mesh(MODEL, HULL_MESH))),
                MeshMaterial3d(painted.clone()),
            ),
            (
                Name::new("Spar"),
                Mesh3d(assets.load(model_mesh(MODEL, SPAR_MESH))),
                MeshMaterial3d(painted),
            ),
            // Tied to the masthead and pointed by [`fly_the_pennant`]. The
            // one piece of the boat that is not in the file: a flag is a
            // shape that has to be *aimed*, and aiming it means knowing where
            // its tie is, which a mesh out of Blender does not say.
            (
                Name::new("Pennant"),
                Pennant {
                    // Astern until the first frame says otherwise, which is
                    // where a flag on a boat at rest in still air would lie
                    // anyway.
                    bearing: 0.0,
                },
                Mesh3d(meshes.add(pennant_mesh())),
                MeshMaterial3d(pennant_material),
                Transform::from_xyz(0.0, SHIP.masthead, SHIP.masthead_station),
            ),
            // The sail, at the mast's foot so its rotation is a turn about
            // the mast, and hidden because a world is entered at anchor —
            // [`trim_the_sails`] shows it while the sails are set and lays
            // the boom where the wind asks.
            (
                Name::new("Sail"),
                Sail,
                Mesh3d(meshes.add(sail_mesh())),
                MeshMaterial3d(sail_material),
                Transform::from_xyz(0.0, 0.0, SHIP.masthead_station),
                Visibility::Hidden,
            ),
            // The figure itself is hung under this by `figure::dress`, which
            // is the player's own business rather than the boat's; what the
            // boat says is where a person aboard stands, which is at the
            // helm. The visibility is so that figure inherits cleanly from
            // its siblings.
            (
                Name::new("Player"),
                Player,
                Transform::from_translation(SHIP.helm()),
                Visibility::default(),
            )
        ],
    ));

    // Said out loud for the same reason a run without a seed says which world
    // it picked: a placeholder nobody can find is indistinguishable from one
    // that never spawned, and `--focus` takes exactly these two numbers.
    info!("boat launched at {}, {}", view.focus.x, view.focus.z);
}

/// The pennant, as a shape: a burgee tied at the origin, so that everything
/// [`fly_the_pennant`] does is a rotation about the point the flag is actually
/// made fast at.
///
/// It flies along -Z at rest, the way the boat itself faces, so a bearing
/// becomes a rotation about the vertical with no axis convention of its own.
/// The hoist hangs *below* the tie rather than straddling it, because a flag
/// is tied at its top corner and swings from there.
///
/// The one thing it is not is flat, and that is the whole reason it is three
/// triangles instead of one. A flat pennant vanishes whenever the wind lines
/// up with the camera — which at a fixed camera bearing is several times an
/// hour, and looks exactly like the flag having been deleted. Pushing a
/// single interior point out to one side puts a shallow belly in the cloth,
/// which is both what a real flag does and enough to keep some part of it
/// facing the viewer from any direction. It costs two triangles.
///
/// Rigid cloth is still a lie in a calm — real canvas folds down the mast
/// rather than swinging round like a boom — and [`PENNANT`]'s narrow hoist is
/// what makes the lie cheap: at three tenths of a metre the missing fold is a
/// hand's width, watched from forty metres up, while the length that carries
/// the reading is the part that behaves.
fn pennant_mesh() -> Mesh {
    let (length, hoist) = PENNANT;
    let tie = Vec3::ZERO;
    let foot = Vec3::new(0.0, -hoist, 0.0);
    let fly = Vec3::new(0.0, -hoist * 0.5, -length);
    // The belly: inside the outline, nearer the hoist than the fly, and out
    // to starboard by a tenth of the flag's length. Deep enough to catch the
    // light differently from its neighbours, shallow enough that the pennant
    // still reads as one shape rather than as a paper aeroplane.
    let belly = Vec3::new(length * 0.1, -hoist * 0.5, -length * 0.4);

    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    // Unindexed, so that each facet can carry its own normal — the flat
    // shading the whole world is drawn in, and the reason the belly is worth
    // having at all.
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![tie, foot, belly, foot, fly, belly, fly, tie, belly],
    )
    .with_computed_flat_normals()
}

/// The sail, as a shape: three triangles fanned round a belly, exactly
/// [`pennant_mesh`]'s construction and for its reasons — unindexed so each
/// facet carries its own flat normal, and bellied so no wind direction ever
/// turns the cloth edge-on to the camera and deletes it. The luff runs up
/// the mast from tack to head, the foot aft to the clew, and the whole shape
/// is drawn sheeted amidships; where the boom actually lies is a rotation,
/// [`trim_the_sails`]'s to make.
fn sail_mesh() -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![
            SAIL_TACK, SAIL_HEAD, SAIL_BELLY, SAIL_HEAD, SAIL_CLEW, SAIL_BELLY, SAIL_CLEW,
            SAIL_TACK, SAIL_BELLY,
        ],
    )
    .with_computed_flat_normals()
}

/// Where the boom lies for a wind, in radians about the mast: swung to
/// leeward — the side the wind is not on — close-hauled at the band's floor
/// against the edge of the no-go zone, nearly square before a dead run. In
/// irons it stays close-hauled, which is also roughly where luffing canvas
/// hangs, and a wind too slack to name a side leaves the boom amidships.
/// The *true* wind, like the drive and unlike the pennant: the boom is
/// trimmed to the same wind the speed is earned from, so what the eye reads
/// off it agrees with what the hull does.
fn sail_trim(bow: Vec2, wind: Vec2) -> f32 {
    if wind.length() < WIND_NAMED {
        return 0.0;
    }
    let off = bow.angle_to(-wind);
    let (close, square) = TRIM_BAND;
    let out = close
        + (square - close) * ((off.abs() - NO_GO) / (std::f32::consts::PI - NO_GO)).clamp(0.0, 1.0);
    // The sign: `off` is positive with the eye of the wind to starboard —
    // the map's turn runs against the yaw's — and leeward is then to port,
    // which about the mast's own vertical is a negative turn for a boom
    // hung aft. Pinned by `the_boom_swings_to_leeward` rather than by this
    // sentence.
    -off.signum() * out
}

/// Shows the sail while it is set, hides it furled, and lays the boom on the
/// wind — after [`steer`] for the pennant's reason: both read the heading
/// and the sail state this frame's steering wrote. The rotation is about the
/// sail's own local vertical, which *is* the mast however the hull heels and
/// pitches, the sail being a child of it.
fn trim_the_sails(
    conditions: Res<sea::SeaConditions>,
    boats: Query<(&Boat, &Transform), Without<Sail>>,
    mut sails: Query<(&ChildOf, &mut Transform, &mut Visibility), With<Sail>>,
) {
    for (of, mut transform, mut visibility) in &mut sails {
        let Ok((boat, hull)) = boats.get(of.parent()) else {
            continue;
        };
        // Written only on change, so an idle boat's sail is as unwritten as
        // the rest of it.
        let shown = if boat.sails_set() {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *visibility != shown {
            *visibility = shown;
        }
        if boat.sails_set() {
            let trimmed = Quat::from_rotation_y(sail_trim(hull.forward().xz(), conditions.wind()));
            if transform.rotation != trimmed {
                transform.rotation = trimmed;
            }
        }
    }
}

/// How the pennant lies under an apparent wind: the bearing it streams on and
/// how far it hangs off the horizontal, in radians. `flying` is the bearing it
/// is on now, which a wind too slack to have a direction leaves alone.
///
/// The bearing is where the air is *going*, which needs no defending here the
/// way it does on the compass: a flag is blown, and the eye reads it as blown.
/// The droop is the whole of the strength reading — flat out in a blow, dead
/// down in a calm, and everything between — so a player who never looks at
/// the corner of the screen still knows what the wind is doing.
fn pennant_pose(apparent: Vec2, flying: f32) -> (f32, f32) {
    let (full, sag) = PENNANT_FLIES;
    let hard = (apparent.length() / full).clamp(0.0, 1.0);
    let bearing = if apparent.length() > PENNANT_CALM {
        // The map's x and y are the world's x and z, and the flag is drawn
        // down -Z: the same turn a boat's own heading is read as.
        f32::atan2(-apparent.x, -apparent.y)
    } else {
        flying
    };
    (
        bearing,
        sag + (std::f32::consts::FRAC_PI_2 - sag) * (1.0 - hard),
    )
}

/// Flies the pennant on the wind the masthead feels.
///
/// Which is the *apparent* wind — the world's wind less the boat's own way
/// through it — and that is the difference between this and the compass, on
/// purpose. An instrument wants the true wind, because a bearing that changed
/// as the player accelerated would be useless for steering by; a flag has no
/// such duty and every reason to be honest, so a boat driving into a light
/// air blows its own pennant astern, and one running before a breeze at
/// nearly the speed of it flies limp. The two disagreeing is not a fault to
/// be reconciled — it is the same thing sailors get from a burgee and a
/// masthead instrument, and a player who notices has learned something true
/// about sailing.
///
/// The boat's rotation is taken back out of the flag's, so heel, pitch and
/// heading move where the pennant *is* without touching where it points: the
/// masthead swings through a turn and the cloth stays on the wind.
fn fly_the_pennant(
    time: Res<Time>,
    conditions: Res<sea::SeaConditions>,
    boats: Query<(&Boat, &Transform), Without<Pennant>>,
    mut pennants: Query<(&mut Pennant, &ChildOf, &mut Transform)>,
) {
    for (mut pennant, of, mut transform) in &mut pennants {
        let Ok((boat, hull)) = boats.get(of.parent()) else {
            continue;
        };
        let apparent = conditions.wind() - hull.forward().xz() * boat.way;
        let (bearing, droop) = pennant_pose(apparent, pennant.bearing);
        pennant.bearing = bearing;

        // The flutter rides on top of the bearing rather than replacing it,
        // and is scaled by the same wind that lifted the flag: cloth that
        // snapped as hard in an air as in a blow would be a flag with a motor
        // in it.
        let (throw, rate) = FLUTTER;
        let hard = (apparent.length() / PENNANT_FLIES.0).clamp(0.0, 1.0);
        let flutter = throw * hard * (time.elapsed_secs_wrapped() * rate).sin();

        let flying = Quat::from_rotation_y(bearing + flutter) * Quat::from_rotation_x(-droop);
        transform.rotation = hull.rotation.inverse() * flying;

        // The tie rides the spar's surface on the side the flag is flying —
        // see [`PENNANT_TIE_OFF`] — so the pivot for all of the above sits
        // clear of the timber, and a hanging flag hangs beside the masthead
        // rather than inside it. Steered by the bearing without the flutter:
        // the tie is a fixed point on a swinging flag, not a swinging one.
        let tie_off = Quat::from_rotation_y(bearing) * (Vec3::NEG_Z * PENNANT_TIE_OFF);
        transform.translation = Vec3::new(0.0, boat.hull.masthead, boat.hull.masthead_station)
            + hull.rotation.inverse() * tie_off;
    }
}

/// Keeps the boat on the surface it is over: the ground where the ground
/// stands proud of the water, and the swell where it does not — the same rule
/// the other players' markers ride. [`Ground::surface`] is this with a flat
/// sea; the swell is what the sea is actually drawn wearing, and taking the
/// higher of ground and water is what makes the changeover continuous — a
/// hull nosing onto a beach settles from bobbing to beached with no step,
/// because at the shoreline the two heights meet.
///
/// The waterline is the hull's origin, so a boat that has run aground is
/// half-buried in the hillside; that is what aground looks like, and steering
/// is what will keep it off. No easing on the water's motion either: the
/// swell is gentle, and a hull seven metres long simply is where the water
/// is.
///
/// Afloat, the hull also wears the water's *slope*: the swell is sampled off
/// the bow and the stern and out at either beam, and the deck eases towards
/// the plane those four heights describe — pitching as seas pass under it
/// fore and aft, rolling as they pass across, on the hull's own sway
/// response, which is where its weight lives. The height above is not eased
/// and the tilt is, deliberately: the hull *is* where the water is, but it is
/// metres long, and turning to a shape that long takes it time the height
/// does not need. A beached hull eases level instead — the ground is holding
/// it, and a deck still working to water sliding past a held keel would give
/// the trick away.
///
/// The tilt goes on and comes off as a factor of its own. The rotation holds
/// heading, then the water's pitch, then a single roll factor that the wave
/// roll shares with the turn's heel — so this system strips the tilt it
/// applied last frame from the right and hangs the new one on, and [`steer`],
/// multiplying its heel delta on from the right, keeps reaching the roll
/// factor it always has.
pub(crate) fn float(
    ground: Option<Res<Ground>>,
    time: Res<Time>,
    sea: Res<sea::SeaConditions>,
    mut boats: Query<(&mut Transform, &mut Boat)>,
) {
    let elapsed = time.elapsed_secs_wrapped();

    for (mut transform, mut boat) in &mut boats {
        let hull = boat.hull;
        let t = eased(1.0 / hull.sway_response, time.delta_secs());
        let at = transform.translation;
        let height = ground.as_ref().and_then(|g| g.height(at.x, at.z));
        let mut afloat = false;
        if let Some(height) = height {
            // The water under the hull decides whether the swell here is the
            // open sea's or the shore's — the ground is asked exactly, where
            // the shader reads its windowed picture of the same heights; they
            // differ by at most a texel of interpolation, in water where the
            // swell is smallest.
            let water = sea.water_over(ground.as_deref(), at.xz(), elapsed);
            transform.translation.y = height.max(water);
            afloat = water >= height;
        }

        let (target_pitch, target_roll) = if afloat {
            // The same swell the hull's own height rides, at a point of the
            // hull rather than its middle. Ground not yet sent counts as
            // deep — the benefit of the doubt the depth window gives the
            // shader — and ground standing dry puts the shore wave's last
            // breath there, which is next to no water and next to no tilt.
            let water_at = |offset: Vec3| {
                let point = transform.transform_point(offset);
                sea.water_over(ground.as_deref(), point.xz(), elapsed)
            };
            (
                f32::atan2(
                    water_at(Vec3::new(0.0, 0.0, -hull.length / 2.0))
                        - water_at(Vec3::new(0.0, 0.0, hull.length / 2.0)),
                    hull.length,
                ),
                f32::atan2(
                    water_at(Vec3::new(hull.beam / 2.0, 0.0, 0.0))
                        - water_at(Vec3::new(-hull.beam / 2.0, 0.0, 0.0)),
                    hull.beam,
                ),
            )
        } else {
            (0.0, 0.0)
        };

        let pitch = settled(boat.pitch + (target_pitch - boat.pitch) * t, target_pitch);
        let roll = settled(boat.roll + (target_roll - boat.roll) * t, target_roll);
        if pitch == boat.pitch && roll == boat.roll {
            // Nothing to change — which is every frame for a boat with no
            // water under it, whose rotation must stay unwritten the way an
            // idle boat's does in [`steer`].
            continue;
        }
        transform.rotation = (transform.rotation
            * Quat::from_rotation_z(-(boat.roll + boat.heel))
            * Quat::from_rotation_x(pitch - boat.pitch)
            * Quat::from_rotation_z(roll + boat.heel))
        .normalize();
        boat.pitch = pitch;
        boat.roll = roll;
    }
}

/// The tail-closing every eased angle here gets, in [`HEEL_SETTLED`]'s terms:
/// within a third of a degree of its target the angle *is* the target, so a
/// hull done settling holds one rotation frame after frame rather than
/// creeping towards it forever.
fn settled(eased: f32, target: f32) -> f32 {
    if (target - eased).abs() < HEEL_SETTLED {
        target
    } else {
        eased
    }
}

/// How far the bottom stands above the depth the hull is held at, in metres,
/// taken at the worst-placed point of the keel — negative for as long as there
/// is water enough under all of it, zero where the hull is about to be stopped.
/// Not the keel's own penetration, which is this plus [`KEEL_BITE`] — the gap
/// between the hull's draft and its [`Hull::grounding_draft`]: the rule wants
/// one number that rises as the ground does, and nothing ever reads it but
/// its sign and its ordering against itself, both of which the offset leaves
/// alone.
///
/// This is the whole of collision. The ground the client has is a height field
/// on a two-metre lattice, and the boat is a keel line above it, so "is there
/// water enough here" is a handful of lookups rather than anything to do with
/// intersecting the hull's triangles: [`Ground::height`] answers on exactly the
/// facets the mesh was built from, which is what makes the ground the boat is
/// stopped by the ground the player can see.
///
/// A probe over a chunk that has not arrived says nothing rather than
/// objecting, the same choice [`float`] makes: ground the client has not been
/// sent is not ground it may invent. Outrunning the stream would take a stalled
/// server, and if land does turn up under the hull, backing off still works —
/// see [`steer`] for why.
fn grounding(hull: &Hull, ground: Option<&Ground>, transform: &Transform) -> f32 {
    let Some(ground) = ground else {
        return f32::NEG_INFINITY;
    };

    let keel = hull.heel_station - hull.forefoot_station;
    (0..KEEL_PROBES)
        .filter_map(|i| {
            let station = hull.forefoot_station + keel * i as f32 / (KEEL_PROBES - 1) as f32;
            let at = transform.transform_point(Vec3::new(0.0, 0.0, station));
            Some(ground.height(at.x, at.z)? + hull.grounding_draft())
        })
        .fold(f32::NEG_INFINITY, f32::max)
}

/// Sails the boat the player is at the helm of, in its own frame, the way a
/// boat is sailed: one key makes sail and hands the hull to the wind, one
/// furls, and the steering keys are the helm, bringing the bow round for as
/// long as they're held. The view plays no part — turning the camera changes
/// what the keys look like on screen, never what they do — which is what
/// makes a long sail a held course rather than a chase between the camera's
/// yaw and the boat's.
///
/// Only the boat the player is *aboard* answers, which is what being at the
/// helm means here. Ashore, the same keys are the walker's — see
/// `player::walk` — and a hull left at anchor holds station rather than
/// sailing off with its absent owner's keystrokes.
///
/// With the sails set the wind is the throttle: the target speed is the
/// hull's times [`sail_drive`] — the polar at this heading, the strength of
/// the blow — and the player's whole control of it is the helm, the game
/// trimming the sails itself. Furling takes the target to zero; the way runs
/// off on the glide and the hull holds station where it dies, which is all
/// "anchored" means here. Backing is the one drive the wind has no part in —
/// held astern with the sails furled, at the hull's own astern speed —
/// because backing off a beach is how a grounding is undone, and an escape
/// that waited on a favourable wind would be no escape. The sail keys are
/// taps rather than holds, and a frame that carries both taps furls first
/// and hoists second — a fixed order rather than a race, erring towards
/// sailing.
///
/// The way is eased rather than instant: whatever names the target speed,
/// the hull's way relaxes towards it on its own way-response curve, stepped
/// exactly for however long the frame was, so the ramp is the same shape at
/// any frame rate. That covers both ends of a sail — way gathered over
/// seconds when the sails go up, and carried into a glide when they come
/// down — from one constant, with [`WAY_STOPPED`] closing the tail the
/// exponential would otherwise never finish. The glide is also what makes
/// tacking work at all: the target dies crossing the no-go zone, but the way
/// carried into the turn is enough to bring the bow through the eye and out
/// the other side still moving.
///
/// The helm answers even with no way on, which no rudder would; a boat that
/// can't point where it's told while stationary is annoying before it is
/// realistic. It answers aground as well, and for a better reason than that:
/// a turn refused alongside an advance is a hull wedged bow-first against a
/// shore with nothing left that would free it, so the bow may always come
/// round even where the hull may not go.
///
/// Land is what the hull may not go through, and the frame's advance is
/// offered to [`grounding`] before it is taken. It is allowed if the pose it
/// would reach floats — or, failing that, if it is aground no *deeper* than
/// the pose already held. That second half is not an escape hatch for a boat
/// that has got itself ashore; it is the only rule that both frees one and
/// can't be played. It is also narrower than it reads. A hull that is floating
/// can only ever be allowed a pose that floats — `here` at or under zero makes
/// the second clause imply the first — so a boat under way never reaches dry
/// ground at all: it halts still afloat, with at most the fifth of a metre
/// of [`KEEL_BITE`] in the mud, and backing off from
/// there is the *first* clause doing the work, the water astern being water.
/// What the second clause is for is the pose the boat did not sail into — a
/// `--focus` that puts it inland, and ground arriving under a hull already
/// sitting there. Out of those every way down to the sea is downhill, so it
/// goes; and every way further in is uphill, so nosing the bow over a beach to
/// unlock the island — which "aground already, let it through" would hand a
/// player on the first frame — is refused like any other climb. Which is also
/// why the comparison carries no tolerance: a hair of slack is a hair of climb
/// every frame, and a hair a frame is a metre a second up a hillside.
///
/// Only the pose at the end of the advance is judged; the path swept getting
/// there is covered by the probes of the frame before, which holds for as long
/// as a frame's advance stays under the probe spacing. At the ship's speed that is
/// seventeen centimetres at sixty frames a second, and two and a half metres
/// at the quarter second Bevy clamps a stalled frame to — so the sweep is only
/// ever missed on a frame that was already a visible break in the picture.
///
/// Turning at speed also heels the hull: the target lean is helm times way —
/// sharpness times speed, so a hard turn at full way carries the whole of
/// the hull's full-turn heel, a gentle one at half way a quarter of it, and a bow
/// swung round at rest none at all — and the shown heel relaxes towards it on
/// the hull's heel-response curve, which is both the roll into the turn and the
/// straightening out of it. The way's share is clamped at the hull's own
/// speed, because a strong blow drives past it — see [`DRIVE_BAND`]'s
/// ceiling — and the full-turn heel is a ceiling of its own, not a
/// proportion to be outgrown: the biggest lean the hull ever shows, however
/// hard the day. Signed way keeps the geometry honest going
/// astern: the same helm turns about a centre on the other side, so the heel
/// flips with it. The roll is applied about the boat's own forward axis, and
/// the keel lies along that axis, so heeling moves nothing [`grounding`]
/// probes or [`steer`] advances along — it is wholly a thing the eye gets.
///
/// Both poses are judged with the rotation the helm has just applied, so a
/// turn only ever changes where the advance goes, never whether it is allowed.
/// Being stopped takes the way off, which is what running aground does; the
/// ease then makes coming off again the few seconds it should be. What the
/// hull is stopped *by* is the keel line and nothing else — the stem rakes out
/// over the forefoot, so a bow can overhang a cliff face by half a metre
/// before anything objects, which from a camera forty metres up is nothing.
fn steer(
    keys: Res<ButtonInput<KeyCode>>,
    bindings: Res<KeyBindings>,
    time: Res<Time>,
    ground: Option<Res<Ground>>,
    conditions: Res<sea::SeaConditions>,
    players: Query<&ChildOf, With<Player>>,
    mut boats: Query<(&mut Transform, &mut Boat)>,
) {
    // A player ashore is in no boat's query, and that is the whole of how
    // the helm goes dead when they step off. It is also how a typed `w`
    // never hoists sail: this system runs only while the player has the
    // helm, so the console and the menus keep the keys to themselves.
    let Some((mut transform, mut boat)) = players
        .single()
        .ok()
        .and_then(|aboard| boats.get_mut(aboard.parent()).ok())
    else {
        return;
    };

    // The sail keys — see the doc above for the furl-then-hoist order.
    if bindings.tapped(&keys, Action::MoveBack, KeyCode::ArrowDown) {
        boat.furl();
    }
    if bindings.tapped(&keys, Action::MoveForward, KeyCode::ArrowUp) {
        boat.hoist();
    }

    let (_, helm) = bindings.driving(&keys);

    let ground = ground.as_deref();

    let hull = boat.hull;
    // A response named in seconds is a rate of its reciprocal.
    let t = eased(1.0 / hull.way_response, time.delta_secs());

    if helm != 0.0 {
        transform.rotate_y(helm * hull.turn_rate * time.delta_secs());
    }

    // The target after the helm, so the drive is read off the heading this
    // frame settled on — the same rule the grounding poses live by.
    let astern = !boat.sails_set && bindings.held(&keys, Action::MoveBack, KeyCode::ArrowDown);
    let target = if boat.sails_set {
        hull.speed * sail_drive(transform.forward().xz(), conditions.wind())
    } else if astern {
        -hull.astern_speed
    } else {
        0.0
    };
    // Written only while something is happening, so an idle boat holds
    // still without being marked changed every frame.
    if target != 0.0 || boat.way != 0.0 {
        let way = boat.way + (target - boat.way) * t;
        let way = if target == 0.0 && way.abs() < WAY_STOPPED {
            0.0
        } else {
            way
        };

        let advance = transform.forward() * way * time.delta_secs();
        let here = grounding(&hull, ground, &transform);
        let there = grounding(
            &hull,
            ground,
            &Transform {
                translation: transform.translation + advance,
                ..*transform
            },
        );

        if there <= 0.0 || there <= here {
            boat.way = way;
            transform.translation += advance;
        } else {
            boat.way = 0.0;
        }
    }

    // Heel last, against the way this frame settled on, so running
    // aground starts the straightening the same frame it takes the way
    // off. The transform holds heading, then the water's pitch,
    // then one roll factor the heel shares with the wave roll — see
    // [`float`] — and the helm above multiplies heading on from the
    // left, so rolling on from the right reaches that roll factor alone
    // and the guard keeps an idle boat's rotation unwritten.
    let target_heel = heel_for(&hull, helm, boat.way);
    if boat.heel != target_heel {
        let heel_t = eased(1.0 / hull.heel_response, time.delta_secs());
        let heel = settled(boat.heel + (target_heel - boat.heel) * heel_t, target_heel);
        transform.rotation *= Quat::from_rotation_z(heel - boat.heel);
        boat.heel = heel;
    }
}

#[cfg(test)]
mod tests {
    use protocol::ground::FACET_METRES;

    use super::*;
    use crate::bindings::Action;
    use crate::testing::{
        assert_model_draws, assert_model_is_painted, elapsed, hold, rebind, run_frames, set_wind,
        test_ground, triangles, world_app, TEST_ISLAND_REACH,
    };

    /// Frames enough for the ease to be indistinguishable from settled —
    /// over eight time constants, a remainder of a few parts in ten thousand.
    const SETTLED: usize = 800;

    /// A headless app with the boat systems running, already in a match —
    /// the shared [`world_app`], under this module's older name.
    fn test_app() -> App {
        world_app()
    }

    fn boat(app: &mut App) -> Transform {
        *app.world_mut()
            .query_filtered::<&Transform, With<Boat>>()
            .single(app.world())
            .expect("a match should have a boat in it")
    }

    /// The way the hull is making, straight off the component — what the
    /// tack test watches frame by frame, an end position being unable to say
    /// whether the way ever died along the road to it.
    fn way_on(app: &mut App) -> f32 {
        app.world_mut()
            .query::<&Boat>()
            .single(app.world())
            .expect("a match should have a boat in it")
            .way
    }

    /// Whether the boat's sails are set.
    fn sails_are_set(app: &mut App) -> bool {
        app.world_mut()
            .query::<&Boat>()
            .single(app.world())
            .expect("a match should have a boat in it")
            .sails_set()
    }

    /// Taps a key — down for one frame, then released. What the sail keys
    /// are: [`steer`] reads their edges, so a tap is the whole gesture.
    fn tap(app: &mut App, key: KeyCode) {
        hold(app, key);
        run_frames(app, 1);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(key);
    }

    /// Puts the wind dead astern of the boat as it lies, blowing `speed`
    /// metres a second — the run every driving test wants, because the
    /// default heading under the assumed wind is *in irons* and a test that
    /// forgets the weather is testing a boat that never moves.
    fn wind_astern(app: &mut App, speed: f32) {
        let forward = boat(app).forward();
        set_wind(app, forward.xz() * speed);
    }

    /// The bow's bearing, in the same terms as a camera yaw.
    fn heading_yaw(app: &mut App) -> f32 {
        let forward = boat(app).forward();
        f32::atan2(-forward.x, -forward.z)
    }

    /// Which way the pennant is flying, in the world and in the same terms as
    /// a heading — the flag's own rotation carried back out through the hull's,
    /// which is the reverse of what [`fly_the_pennant`] does to get it.
    fn pennant_bearing(app: &mut App) -> f32 {
        let hull = boat(app).rotation;
        let flag = *app
            .world_mut()
            .query_filtered::<&Transform, With<Pennant>>()
            .single(app.world())
            .expect("a boat should be flying a pennant");
        let flying = hull * flag.rotation * Vec3::NEG_Z;
        f32::atan2(-flying.x, -flying.z)
    }

    /// Radians per second the bow comes round at while `key` is held. A rate
    /// rather than an angle — proportionality to how long the key was held is
    /// what makes a turn the same on any machine.
    fn turn_rate(key: KeyCode) -> f32 {
        let mut app = test_app();
        let start_yaw = heading_yaw(&mut app);
        let before = elapsed(&app);
        hold(&mut app, key);
        run_frames(&mut app, 20);
        let seconds = elapsed(&app) - before;
        assert!(seconds > 0.0, "no time passed while the key was held");
        (heading_yaw(&mut app) - start_yaw) / seconds
    }

    #[test]
    fn the_model_is_a_hull_and_a_spar_fit_to_draw() {
        // The things about the file the game cannot see for itself — see
        // `assert_model_draws`. The order is the sharp one here: a Blender
        // afternoon that left the spar first would paint the hull in
        // bare-spar cream and stand a seven-metre plank of timber where the
        // mast should be.
        assert_model_draws(MODEL, &[(HULL_MESH, "hull"), (SPAR_MESH, "spar")]);
        // And both carry their own colours — drawn with a white material, a
        // mesh that lost them would arrive as a white boat, not a broken one.
        assert_model_is_painted(MODEL, HULL_MESH);
        assert_model_is_painted(MODEL, SPAR_MESH);
    }

    #[test]
    fn the_model_is_the_hull_the_keel_is_probed_along() {
        // What `grounding` assumes about a shape it never looks at: the keel
        // runs at the ship's draft below the waterline, from the forefoot aft
        // to the heel, and the hull is the ship's length. Remodel the boat
        // deeper and every probe would be reading the water above its own keel
        // — the hull would sail through the shallows it should be stopped by,
        // and nothing but this would notice.
        let corners: Vec<Vec3> = triangles(MODEL, HULL_MESH, "POSITION")
            .into_iter()
            .flatten()
            .collect();
        let lowest = corners.iter().map(|c| c.y).fold(f32::MAX, f32::min);
        let (bow, transom) = corners
            .iter()
            .fold((f32::MAX, f32::MIN), |(f, a), c| (f.min(c.z), a.max(c.z)));

        assert!(
            (lowest + SHIP.draft).abs() < 1e-4,
            "the model's keel is {lowest} below the waterline, not {}",
            -SHIP.draft
        );
        let half = SHIP.length * 0.5;
        assert!(
            (bow + half).abs() < 1e-4 && (transom - half).abs() < 1e-4,
            "the model runs {bow}..{transom}, not a {}m hull about amidships",
            SHIP.length
        );

        // The keel itself, not just the depth: the probes are spread between
        // these two stations, and each one has to be over hull rather than over
        // the water ahead of a forefoot that has crept aft.
        let keel: Vec<&Vec3> = corners
            .iter()
            .filter(|c| (c.y + SHIP.draft).abs() < 1e-4)
            .collect();
        let forefoot = keel.iter().map(|c| c.z).fold(f32::MAX, f32::min);
        let heel = keel.iter().map(|c| c.z).fold(f32::MIN, f32::max);
        assert!(
            (forefoot - SHIP.forefoot_station).abs() < 1e-4
                && (heel - SHIP.heel_station).abs() < 1e-4,
            "the keel runs {forefoot}..{heel}, not {}..{}",
            SHIP.forefoot_station,
            SHIP.heel_station
        );

        // The quarterdeck, the same way as the keel: a plane of corners at
        // exactly its height, spanning the helm's station — where a player
        // aboard is stood. Remodel it without re-measuring and they are
        // shin-deep in timber, or walking on air.
        let plane: Vec<&Vec3> = corners
            .iter()
            .filter(|c| (c.y - SHIP.quarterdeck).abs() < 1e-4)
            .collect();
        let fore = plane.iter().map(|c| c.z).fold(f32::MAX, f32::min);
        let aft = plane.iter().map(|c| c.z).fold(f32::MIN, f32::max);
        assert!(
            (fore..=aft).contains(&SHIP.helm_station),
            "no quarterdeck at {} under the helm at {}",
            SHIP.quarterdeck,
            SHIP.helm_station
        );

        // And the boom's sweep: the sail's foot turns about the mast at the
        // tack's height, out to the clew, so whatever the hull raises inside
        // that circle has to stay under it or the canvas drags through the
        // deck furniture. The companionway lives with this rule; the
        // quarterdeck, the tiller and the stem head stand outside the circle
        // or under the cloth instead.
        for corner in &corners {
            let reach = Vec2::new(corner.x, corner.z - SHIP.masthead_station).length();
            assert!(
                reach >= SAIL_CLEW.z || corner.y < SAIL_TACK.y,
                "{corner} stands into the boom's sweep, {reach} m from the mast"
            );
        }
    }

    #[test]
    fn the_model_flies_a_pennant_from_its_masthead() {
        // The spar's own top, which is where the flag is tied. A mast re-cut
        // in Blender and not re-measured here would leave the pennant flying
        // in mid-air beside it, and nothing else in the game looks at the
        // spar at all.
        let corners: Vec<Vec3> = triangles(MODEL, SPAR_MESH, "POSITION")
            .into_iter()
            .flatten()
            .collect();
        let top = corners.iter().map(|c| c.y).fold(f32::MIN, f32::max);
        assert!(
            (top - SHIP.masthead).abs() < 1e-4,
            "the model's masthead is {top} above the waterline, not {}",
            SHIP.masthead
        );

        let (forward, aft) = corners
            .iter()
            .fold((f32::MAX, f32::MIN), |(f, a), c| (f.min(c.z), a.max(c.z)));
        let stepped = (forward + aft) * 0.5;
        assert!(
            (stepped - SHIP.masthead_station).abs() < 1e-4,
            "the mast stands at {stepped}, not {}",
            SHIP.masthead_station
        );

        // And the tie stands off the axis by more than the spar's own half
        // width — see [`PENNANT_TIE_OFF`]. Re-cut a fatter mast without
        // re-measuring and the flag is back inside the timber.
        let girth = corners
            .iter()
            .map(|c| c.x.abs().max((c.z - stepped).abs()))
            .fold(0.0f32, f32::max);
        assert!(
            girth < PENNANT_TIE_OFF,
            "the spar reaches {girth} m from its axis, past the pennant's {PENNANT_TIE_OFF} m tie"
        );
    }

    #[test]
    fn the_pennant_blows_downwind_and_hangs_in_a_calm() {
        // Where the air is going, not where it came from, and the whole of
        // the strength reading in the droop.
        let (flat, sag) = PENNANT_FLIES;
        // A wind blowing due north — towards -Z — lays the flag along -Z,
        // which is the boat's own zero.
        let (bearing, droop) = pennant_pose(Vec2::new(0.0, -flat), 0.0);
        assert!(
            bearing.abs() < 1e-5,
            "a northward wind flew it to {bearing}"
        );
        assert!(
            (droop - sag).abs() < 1e-5,
            "a full wind left it {droop} down"
        );

        // And one blowing east swings it a quarter turn — the sign the game's
        // own headings use, so port helm and a veering wind agree.
        let (east, _) = pennant_pose(Vec2::new(flat, 0.0), 0.0);
        assert!(
            (east + std::f32::consts::FRAC_PI_2).abs() < 1e-5,
            "an eastward wind flew it to {east}"
        );

        // A calm hangs it straight down and leaves the bearing it had, rather
        // than snapping to whatever direction the last breath of air took.
        let (held, hanging) = pennant_pose(Vec2::new(0.05, -0.05), 1.234);
        assert_eq!(held, 1.234);
        // Within a couple of degrees of straight down: the breath of air
        // still in the numbers is allowed to lift it by that much and no
        // more.
        assert!(hanging > std::f32::consts::FRAC_PI_2 - 0.03);

        // Half the wind is most of the way down: the flag lifts through the
        // airs and only the last of it is spent on the blows, which is what
        // makes a light day readable at all.
        let (_, half) = pennant_pose(Vec2::new(0.0, -flat * 0.5), 0.0);
        assert!(half > droop && half < std::f32::consts::FRAC_PI_2);
    }

    #[test]
    fn the_pennant_stays_on_the_wind_through_a_turn() {
        // The reading is the world's, not the boat's: putting the helm over
        // swings the masthead through a right angle and must leave the cloth
        // pointing where it was. Only the helm — with no way on, the apparent
        // wind is the true one throughout, so anything that moved here moved
        // for the wrong reason.
        let mut app = test_app();
        run_frames(&mut app, 2);
        let before = pennant_bearing(&mut app);
        let bow = heading_yaw(&mut app);

        hold(&mut app, KeyCode::ArrowLeft);
        run_frames(&mut app, 40);

        let turned = (heading_yaw(&mut app) - bow).abs();
        assert!(turned > 1.0, "the bow only came round {turned} radians");

        let swung = (pennant_bearing(&mut app) - before).abs();
        assert!(
            swung < 2.0 * FLUTTER.0,
            "the pennant followed the bow round by {swung} radians"
        );
    }

    #[test]
    fn the_polar_is_dead_in_irons_and_full_from_a_beam_reach() {
        // Nothing to windward of the zone's edge, everything from a beam
        // reach round to a dead run — the no-downwind-taper choice, pinned.
        assert_eq!(polar(0.0), 0.0);
        assert_eq!(polar(NO_GO), 0.0);
        let mid = (NO_GO + FULL_DRIVE) * 0.5;
        assert!(
            (polar(mid) - 0.5).abs() < 1e-6,
            "halfway up the ramp draws {}, not half",
            polar(mid)
        );
        assert_eq!(polar(FULL_DRIVE), 1.0);
        assert_eq!(polar(std::f32::consts::PI), 1.0);
    }

    #[test]
    fn the_wind_drives_within_its_band() {
        // The floor is what a calm leaves, the ceiling what a blow earns,
        // and between them more wind is never less speed.
        assert_eq!(strength(0.0), DRIVE_BAND.0);
        assert_eq!(strength(WIND_SATURATES), DRIVE_BAND.1);
        assert_eq!(strength(WIND_SATURATES * 2.0), DRIVE_BAND.1);
        let mut last = 0.0;
        for tenth in 0..=120 {
            let s = strength(tenth as f32 * 0.1);
            assert!(s >= last, "the band dips at {} m/s", tenth as f32 * 0.1);
            last = s;
        }
    }

    #[test]
    fn a_calm_refuses_no_heading() {
        // A wind too slack to have a direction has no eye to be caught in:
        // the floor of the band drives the boat wherever it points, dead
        // "upwind" of the last breath of air included.
        for bow in [Vec2::X, Vec2::NEG_X, Vec2::Y, Vec2::new(0.7, -0.7)] {
            assert_eq!(sail_drive(bow, Vec2::ZERO), DRIVE_BAND.0);
        }
        let breath = Vec2::new(0.0, 0.3);
        assert_eq!(sail_drive(-breath.normalize(), breath), strength(0.3));
    }

    #[test]
    fn the_boat_points_its_bow_the_way_it_faces() {
        // What steering will be written against: -Z is the bow, so a boat
        // turned to a heading moves along its own forward.
        let mut app = test_app();
        let heading = boat(&mut app).forward();
        let yaw = app.world().resource::<View>().yaw;

        let expected = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
        assert!(
            (*heading - expected).length() < 1e-5,
            "a boat at yaw {yaw} faces {heading:?}, not {expected:?}"
        );
    }

    /// A paused boat is a stopped boat, and the world it is in is still there
    /// to be sailed on afterwards.
    #[test]
    fn pausing_takes_the_helm_away_and_resuming_gives_it_back() {
        let mut app = test_app();
        wind_astern(&mut app, 7.0);
        set_helm(&mut app, Helm::Paused);

        let before = boat(&mut app).translation;
        tap(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 20);
        assert_eq!(
            boat(&mut app).translation,
            before,
            "the boat sailed on with the pause menu up"
        );
        // Not merely unmoved: the tap itself fell on a dead helm, so the
        // sails never went up — a paused press does nothing later either.
        assert!(
            !sails_are_set(&mut app),
            "a keypress under the pause menu set the sails"
        );

        set_helm(&mut app, Helm::Sailing);
        tap(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 20);
        assert_ne!(
            boat(&mut app).translation,
            before,
            "the helm never came back"
        );
    }

    /// Floating is not steering — see the run conditions in [`BoatPlugin`].
    /// Ground arriving while the game is paused is settled on there and then,
    /// rather than snapping the hull the frame the player resumes.
    #[test]
    fn a_paused_boat_still_rides_ground_that_arrives_under_it() {
        let mut app = test_app();
        set_helm(&mut app, Helm::Paused);
        assert_eq!(boat(&mut app).translation.y, 0.0);

        app.insert_resource(test_ground());
        run_frames(&mut app, 2);
        assert!(
            boat(&mut app).translation.y > 0.0,
            "the hull is still at sea level with an island under it"
        );
    }

    fn set_helm(app: &mut App, helm: Helm) {
        app.world_mut().resource_mut::<NextState<Helm>>().set(helm);
        app.update();
    }

    #[test]
    fn making_sail_drives_the_boat_the_way_the_bow_points() {
        let mut app = test_app();
        wind_astern(&mut app, 7.0);
        let before = boat(&mut app);
        tap(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 20);
        let moved = boat(&mut app).translation - before.translation;

        // The wind may drive the hull only along its own forward — no helm
        // held, so the whole of the movement is dead ahead.
        assert!(moved.length() > 0.0, "the boat never moved");
        assert!(
            moved.normalize().dot(*before.forward()) > 0.999,
            "the boat went {moved:?} rather than along its heading"
        );
        // And along the surface, not through it — height is `float`'s alone.
        assert_eq!(moved.y, 0.0);
    }

    /// Metres per second the boat settles to from here, signed by whether it
    /// went ahead or astern — measured after the way is gathered, so it is
    /// the speed made good and not some point on the ramp.
    fn speed_settled(app: &mut App) -> f32 {
        run_frames(app, SETTLED);

        let before = boat(app);
        let start = elapsed(app);
        run_frames(app, 60);
        let seconds = elapsed(app) - start;
        assert!(seconds > 0.0, "no time passed while the boat settled");

        let moved = boat(app).translation - before.translation;
        moved.dot(*before.forward()) / seconds
    }

    #[test]
    fn sails_make_the_speed_the_wind_gives() {
        // Dead downwind under the reference breeze: the polar's full drive,
        // scaled by the band — the expectation computed from the same
        // functions the helm reads, so this pins the *wiring*, the curves
        // having tests of their own.
        let mut app = test_app();
        wind_astern(&mut app, 7.0);
        tap(&mut app, KeyCode::ArrowUp);
        let expected = SHIP.speed * strength(7.0);
        let ahead = speed_settled(&mut app);
        assert!(
            (ahead - expected).abs() < expected * 0.01,
            "the boat made {ahead} m/s on a run, not {expected}"
        );
    }

    #[test]
    fn astern_makes_its_own_speed_whatever_the_wind() {
        // Backing off a beach is the whole use of astern, so it is slower,
        // it is backwards — along the heading reversed, not a turn — and it
        // owes the wind nothing: the default heading is in irons, and the
        // boat backs at exactly its astern speed anyway.
        let mut app = test_app();
        hold(&mut app, KeyCode::ArrowDown);
        let astern = speed_settled(&mut app);
        assert!(
            (astern + SHIP.astern_speed).abs() < SHIP.astern_speed * 0.01,
            "the boat made {astern} m/s astern, not -{}",
            SHIP.astern_speed
        );
    }

    #[test]
    fn the_boat_gathers_way_rather_than_leaping_to_speed() {
        // The first half second after making sail: under way at once, but
        // nowhere near the wind's speed — the ramp is the point of the ease.
        let mut app = test_app();
        wind_astern(&mut app, 7.0);
        let before = boat(&mut app).translation;
        let start = elapsed(&app);
        tap(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 29);
        let seconds = elapsed(&app) - start;

        let made = (boat(&mut app).translation - before).length() / seconds;
        assert!(made > 0.0, "the boat never began to move");
        assert!(
            made < SHIP.speed * strength(7.0) * 0.5,
            "{made} m/s inside the first half second is a leap, not gathered way"
        );
    }

    #[test]
    fn furling_carries_the_way_into_a_glide_and_then_holds_station() {
        // What "anchored" is: the sails come down, the way runs off over a
        // few metres of glide, and the hull ends *held* — the same spot
        // exactly, frame after frame.
        let mut app = test_app();
        wind_astern(&mut app, 7.0);
        tap(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, SETTLED);

        // Furled at full speed: a glide of a few metres, not a dead stop.
        tap(&mut app, KeyCode::ArrowDown);
        assert!(!sails_are_set(&mut app), "the sails never came down");
        let going = boat(&mut app).translation;
        run_frames(&mut app, 30);
        let glide = (boat(&mut app).translation - going).length();
        assert!(
            glide > 1.0,
            "the boat stopped dead the moment the sails came down"
        );

        // And the glide ends: the way snaps to stopped, and a boat at rest
        // holds the same spot exactly, frame after frame.
        run_frames(&mut app, SETTLED);
        let at_rest = boat(&mut app).translation;
        run_frames(&mut app, 10);
        assert_eq!(
            boat(&mut app).translation,
            at_rest,
            "the boat is still creeping"
        );
    }

    #[test]
    fn the_helm_brings_the_bow_round_at_the_turn_rate() {
        // Port is a positive turn about the vertical, the same way round as
        // the camera's own Q; starboard its mirror.
        let port = turn_rate(KeyCode::ArrowLeft);
        let starboard = turn_rate(KeyCode::ArrowRight);

        let tolerance = SHIP.turn_rate * 0.01;
        assert!(
            (port - SHIP.turn_rate).abs() < tolerance,
            "the bow came round at {port} rad/s to port, not {}",
            SHIP.turn_rate
        );
        assert!(
            (starboard + SHIP.turn_rate).abs() < tolerance,
            "the bow came round at {starboard} rad/s to starboard, not -{}",
            SHIP.turn_rate
        );
    }

    #[test]
    fn the_helm_answers_with_no_way_on() {
        // Turning without moving: the bow swings, the hull stays put.
        let mut app = test_app();
        let before = boat(&mut app);
        hold(&mut app, KeyCode::ArrowLeft);
        run_frames(&mut app, 20);
        let after = boat(&mut app);

        assert_eq!(after.translation, before.translation);
        assert_ne!(after.rotation, before.rotation, "the bow never swung");
    }

    #[test]
    fn the_sail_shows_when_set_and_hides_furled() {
        // The sail is the sailing state, drawn: hidden at launch — a world
        // is entered at anchor — shown the frame the sails go up, hidden
        // again the frame they come down.
        fn sail_shown(app: &mut App) -> Visibility {
            *app.world_mut()
                .query_filtered::<&Visibility, With<Sail>>()
                .single(app.world())
                .expect("a boat should carry a sail")
        }

        let mut app = test_app();
        wind_astern(&mut app, 7.0);
        run_frames(&mut app, 2);
        assert_eq!(sail_shown(&mut app), Visibility::Hidden);

        tap(&mut app, KeyCode::ArrowUp);
        assert_eq!(sail_shown(&mut app), Visibility::Inherited);

        tap(&mut app, KeyCode::ArrowDown);
        assert_eq!(sail_shown(&mut app), Visibility::Hidden);
    }

    #[test]
    fn the_boom_swings_to_leeward() {
        // A bow due north — along -Z, the map's (0, -1) — with the wind
        // blowing toward -X is a wind out of the east: from starboard, so
        // the boom belongs to port, which about the mast is a negative turn
        // (see `sail_trim` for the axis bookkeeping this pins).
        let north = Vec2::new(0.0, -1.0);
        let from_starboard = sail_trim(north, Vec2::new(-7.0, 0.0));
        assert!(
            from_starboard < 0.0,
            "a wind from starboard laid the boom {from_starboard} rad — to windward"
        );
        // A wind from port is the mirror.
        assert_eq!(sail_trim(north, Vec2::new(7.0, 0.0)), -from_starboard);

        // Sheeted harder the closer the course: square before a dead run,
        // hauled to the band's floor close to the wind, and never outside
        // the band.
        let running = sail_trim(north, Vec2::new(0.0, -7.0)).abs();
        assert_eq!(running, TRIM_BAND.1);
        let abeam = from_starboard.abs();
        assert!(
            abeam > TRIM_BAND.0 && abeam < TRIM_BAND.1,
            "a beam reach sheets the boom {abeam} rad, outside the band"
        );

        // In irons the boom sits close-hauled rather than inventing a side.
        let in_irons = sail_trim(north, Vec2::new(0.0, 7.0)).abs();
        assert_eq!(in_irons, TRIM_BAND.0);

        // And a nameless wind leaves it amidships.
        assert_eq!(sail_trim(north, Vec2::new(0.1, -0.1)), 0.0);
    }

    #[test]
    fn a_tack_carries_way_through_the_eye_of_the_wind() {
        // The proof that the constants are compatible: the glide is long
        // enough, against the turn rate and the width of the no-go zone,
        // that a boat tacking from a beam reach is never stopped in the eye.
        // If a constant change ever reds this, the boat has been made
        // un-tackable and the constant is wrong, not the test.
        let mut app = test_app();
        set_wind(&mut app, Vec2::new(7.0, 0.0));
        // A beam reach: wind from the west abeam, bow due north.
        place(&mut app, Vec2::ZERO, Vec2::new(0.0, -1.0));
        tap(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, SETTLED);
        let entered = way_on(&mut app);
        assert!(entered > 0.0, "the boat never settled onto its reach");

        // Helm hard over through the eye to the mirror course — half a turn
        // at the ship's turn rate, watched frame by frame: an end position
        // could not say whether the way died somewhere along the road.
        hold(&mut app, KeyCode::ArrowLeft);
        let frames = (std::f32::consts::PI / SHIP.turn_rate / 0.016) as usize;
        let mut least = f32::MAX;
        for _ in 0..frames {
            run_frames(&mut app, 1);
            least = least.min(way_on(&mut app));
        }
        assert!(
            least > entered * 0.3,
            "the tack fell to {least} m/s from {entered} — becalmed in the eye"
        );

        // And out the other side the wind fills the sails again.
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release_all();
        run_frames(&mut app, SETTLED);
        let out = way_on(&mut app);
        assert!(
            (out - entered).abs() < entered * 0.05,
            "the mirror reach makes {out} m/s where the first made {entered}"
        );
    }

    #[test]
    fn in_irons_the_boat_stops_and_the_helm_frees_it() {
        // Sails set dead into the wind gather nothing — and the way snaps to
        // exactly zero, which is what re-opens going ashore and the night
        // offer to a boat parked head-to-wind. The helm answers with no way
        // on, so being in irons is a state, never a trap.
        let mut app = test_app();
        let bow = boat(&mut app).forward().xz();
        set_wind(&mut app, -bow * 7.0);
        let before = boat(&mut app).translation;
        tap(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 60);
        assert!(sails_are_set(&mut app));
        assert_eq!(way_on(&mut app), 0.0, "the eye of the wind drove the boat");
        assert_eq!(boat(&mut app).translation, before);

        // A second of helm swings the bow out of the zone and the sails
        // fill.
        hold(&mut app, KeyCode::ArrowLeft);
        run_frames(&mut app, 60);
        assert!(
            way_on(&mut app) > 0.0,
            "the bow came round but the sails never filled"
        );
    }

    #[test]
    fn astern_needs_the_sails_furled() {
        // One held key is "stop, then back": its first frame furls — sails
        // set, the same key would otherwise be asking for two drives at once
        // — and every frame after backs the boat on the way the glide is
        // still paying out.
        let mut app = test_app();
        wind_astern(&mut app, 7.0);
        tap(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, SETTLED);

        hold(&mut app, KeyCode::ArrowDown);
        run_frames(&mut app, 1);
        assert!(
            !sails_are_set(&mut app),
            "a held furl key left the sails up"
        );
        run_frames(&mut app, SETTLED);
        let astern = way_on(&mut app);
        assert!(
            (astern + SHIP.astern_speed).abs() < SHIP.astern_speed * 0.01,
            "the boat settled to {astern} m/s, not -{}",
            SHIP.astern_speed
        );
    }

    /// The hull's heel, in radians, positive with the masthead to port — what
    /// is left of the pose once the heading's yaw is taken back off it. Read
    /// from the transform rather than the component, because the lean the
    /// player sees is the one the transform holds.
    fn heel_shown(app: &mut App) -> f32 {
        let transform = boat(app);
        let forward = transform.forward();
        let yaw = f32::atan2(-forward.x, -forward.z);
        let roll = Quat::from_rotation_y(yaw).inverse() * transform.rotation;
        let (axis, angle) = roll.to_axis_angle();
        // `to_axis_angle` answers on [0, 2π), and a heading that has wound
        // past half a turn negates the quaternion — the same rotation,
        // double-covered — so a small heel can read as nearly a full turn
        // the other way. Wrap to the short way round.
        let heel = angle * axis.z;
        if heel > std::f32::consts::PI {
            heel - std::f32::consts::TAU
        } else if heel < -std::f32::consts::PI {
            heel + std::f32::consts::TAU
        } else {
            heel
        }
    }

    /// The biggest lean a spell of turning shows, signed as [`heel_shown`]
    /// is, taken frame by frame — a circle under sail sweeps through the
    /// no-go zone every half turn, so the way breathes and no single settled
    /// heel exists to read.
    fn deepest_heel(app: &mut App, frames: usize) -> f32 {
        let mut deepest = 0.0f32;
        for _ in 0..frames {
            run_frames(app, 1);
            let heel = heel_shown(app);
            if heel.abs() > deepest.abs() {
                deepest = heel;
            }
        }
        deepest
    }

    #[test]
    fn a_turn_at_speed_heels_the_hull_outwards() {
        // A circle at full helm under a saturating wind. Under sail the way
        // breathes as the bow sweeps through the no-go zone, so what is
        // pinned is the *deepest* lean of a full circle: a real lean, to
        // starboard under port helm — a keeled hull leans out of a corner,
        // not into it like a bicycle — and never past the full-turn heel,
        // which is the ceiling [`heel_for`] clamps to however hard the wind
        // drives.
        let mut app = test_app();
        wind_astern(&mut app, WIND_SATURATES);
        tap(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, SETTLED);

        // A full circle at 2 rad/s is about 200 frames; watch two.
        hold(&mut app, KeyCode::ArrowLeft);
        let heel = deepest_heel(&mut app, 400);
        assert!(
            heel < -SHIP.heel_at_full_turn * 0.7,
            "a full-helm circle under sail only heeled {heel} rad"
        );
        assert!(
            heel >= -SHIP.heel_at_full_turn - HEEL_SETTLED,
            "the hull heeled {heel} rad, past the full-turn ceiling of {}",
            SHIP.heel_at_full_turn
        );

        // The lean is the eye's alone: the bow still points along the
        // surface, so nothing the movement is built on has tilted with it.
        assert!(boat(&mut app).forward().y.abs() < 1e-6);

        // Starboard is the mirror.
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::ArrowLeft);
        hold(&mut app, KeyCode::ArrowRight);
        run_frames(&mut app, SETTLED);
        let heel = deepest_heel(&mut app, 400);
        assert!(
            heel > SHIP.heel_at_full_turn * 0.7,
            "a starboard circle only heeled {heel} rad"
        );
        assert!(heel <= SHIP.heel_at_full_turn + HEEL_SETTLED);
    }

    #[test]
    fn the_heel_asked_of_a_turn_is_capped_at_the_full_turns() {
        // The clamp in [`heel_for`], pinned where it lives: a blow drives
        // the way past hull speed — see [`DRIVE_BAND`]'s ceiling — and the
        // full-turn heel must be the most the hull is ever asked for, not a
        // proportion that grows with the day.
        let over = SHIP.speed * DRIVE_BAND.1;
        assert!(over > SHIP.speed, "the band's ceiling no longer overdrives");
        assert_eq!(heel_for(&SHIP, 1.0, over), -SHIP.heel_at_full_turn);
        assert_eq!(heel_for(&SHIP, -1.0, over), SHIP.heel_at_full_turn);
        // Under hull speed it is still a proportion — half way, half heel —
        // and astern the same helm heels the other side.
        assert_eq!(
            heel_for(&SHIP, 1.0, SHIP.speed * 0.5),
            -SHIP.heel_at_full_turn * 0.5
        );
        assert!(heel_for(&SHIP, 1.0, -SHIP.astern_speed) > 0.0);
    }

    #[test]
    fn the_heel_rolls_on_rather_than_snapping() {
        // A few frames into a full-speed turn: leaning already, but nowhere
        // near the whole heel — the roll is eased the way the way is.
        let mut app = test_app();
        wind_astern(&mut app, 7.0);
        tap(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, SETTLED);
        hold(&mut app, KeyCode::ArrowLeft);
        run_frames(&mut app, 6);

        let heel = heel_shown(&mut app).abs();
        assert!(heel > 0.0, "the hull never began to lean");
        assert!(
            heel < SHIP.heel_at_full_turn * 0.5,
            "{heel} rad inside the first tenth of a second is a snap, not a roll"
        );
    }

    #[test]
    fn the_helm_alone_heels_nothing() {
        // The lean is sharpness times speed, and a bow swung round at rest
        // has no speed: the boat pivots bolt upright.
        let mut app = test_app();
        hold(&mut app, KeyCode::ArrowLeft);
        run_frames(&mut app, SETTLED);
        assert_eq!(heel_shown(&mut app), 0.0);
    }

    #[test]
    fn the_hull_comes_level_when_the_turn_ends() {
        let mut app = test_app();
        hold(&mut app, KeyCode::ArrowUp);
        hold(&mut app, KeyCode::ArrowLeft);
        run_frames(&mut app, SETTLED);
        assert_ne!(heel_shown(&mut app), 0.0, "the turn never heeled the hull");

        // Helm amidships, way still on: the heel runs back down its own
        // curve and *ends* — exactly level, not forever creeping towards it.
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::ArrowLeft);
        run_frames(&mut app, SETTLED);
        let level = heel_shown(&mut app);
        assert!(
            level.abs() < 1e-6,
            "the hull is still heeled {level} rad with the helm amidships"
        );
    }

    #[test]
    fn opposed_helm_keys_hold_the_heading() {
        // Port cancels starboard outright, and a boat with furled sails and
        // both helm keys down is a boat doing nothing at all.
        let mut app = test_app();
        let before = boat(&mut app);
        hold(&mut app, KeyCode::ArrowLeft);
        hold(&mut app, KeyCode::ArrowRight);
        run_frames(&mut app, 20);
        let after = boat(&mut app);
        assert_eq!(after.translation, before.translation);
        assert_eq!(after.rotation, before.rotation);
    }

    #[test]
    fn both_sail_keys_at_once_leave_the_sails_set() {
        // One frame carrying both taps furls first and hoists second — a
        // fixed order rather than a race — so the boat comes out sailing,
        // and the held furl key cannot also drag it astern: backing is gated
        // on the sails being down, and they are up.
        let mut app = test_app();
        wind_astern(&mut app, 7.0);
        let before = boat(&mut app).translation;
        hold(&mut app, KeyCode::ArrowUp);
        hold(&mut app, KeyCode::ArrowDown);
        run_frames(&mut app, 30);

        assert!(sails_are_set(&mut app), "opposed taps left the sails down");
        let moved = boat(&mut app).translation - before;
        assert!(
            moved.dot(*boat(&mut app).forward()) > 0.0,
            "the boat went {moved:?} rather than ahead under sail"
        );
    }

    #[test]
    fn a_rebound_key_makes_sail_and_the_key_it_replaced_stops() {
        let mut app = test_app();
        wind_astern(&mut app, 7.0);
        rebind(&mut app, Action::MoveForward, KeyCode::KeyJ);

        let before = boat(&mut app).translation;
        tap(&mut app, KeyCode::KeyJ);
        run_frames(&mut app, 20);
        assert_ne!(
            boat(&mut app).translation,
            before,
            "the newly bound key did not make sail"
        );

        // J was nobody's key, so nothing was traded for it and W is now bound
        // to nothing at all. Tapping it has to leave the sails down and the
        // boat where it lies.
        let mut app = test_app();
        wind_astern(&mut app, 7.0);
        rebind(&mut app, Action::MoveForward, KeyCode::KeyJ);
        let before = boat(&mut app).translation;
        tap(&mut app, KeyCode::KeyW);
        run_frames(&mut app, 20);
        assert!(
            !sails_are_set(&mut app),
            "W still makes sail after being rebound away"
        );
        assert_eq!(boat(&mut app).translation, before);
    }

    #[test]
    fn the_arrow_keys_steer_whatever_the_bindings_say() {
        let mut app = test_app();
        wind_astern(&mut app, 7.0);
        // Hand every movement action to keys nowhere near the arrows.
        rebind(&mut app, Action::MoveForward, KeyCode::KeyI);
        rebind(&mut app, Action::MoveBack, KeyCode::KeyK);
        rebind(&mut app, Action::SteerLeft, KeyCode::KeyJ);
        rebind(&mut app, Action::SteerRight, KeyCode::KeyL);

        let before = boat(&mut app).translation;
        tap(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 20);
        assert_ne!(
            boat(&mut app).translation,
            before,
            "the arrow keys stopped steering once the letters moved"
        );
    }

    #[test]
    fn a_match_launches_the_boat_where_the_world_is_entered() {
        let mut app = test_app();
        app.insert_resource(View {
            focus: Vec3::new(98.0, 0.0, -317.0),
            ..default()
        });
        // Back to the menu and in again: entering is what launches a boat, and
        // the one from the last world went with it.
        for state in [AppState::MainMenu, AppState::InWorld] {
            app.world_mut()
                .resource_mut::<NextState<AppState>>()
                .set(state);
            app.update();
        }

        let at = boat(&mut app).translation;
        assert_eq!(Vec2::new(at.x, at.z), Vec2::new(98.0, -317.0));
    }

    #[test]
    fn the_player_enters_the_world_aboard_the_boat() {
        // Entering a world is done afloat: one player, riding the boat as a
        // child of it — which is what the camera and the position reports
        // resolve through, so a player spawned loose or not at all would
        // leave both staring at nothing.
        let mut app = test_app();
        let boat = app
            .world_mut()
            .query_filtered::<Entity, With<Boat>>()
            .single(app.world())
            .expect("a match should have a boat in it");
        let aboard = app
            .world_mut()
            .query_filtered::<&ChildOf, With<Player>>()
            .single(app.world())
            .expect("a match should have a player in it")
            .parent();
        assert_eq!(aboard, boat, "the player is not aboard the boat");
    }

    #[test]
    fn the_boat_rides_the_surface_it_is_over() {
        let mut app = island_app();

        /// Where the boat comes to rest when it is put down at a spot.
        fn put_down(app: &mut App, spot: Vec2) -> f32 {
            let mut transform = app
                .world_mut()
                .query_filtered::<&mut Transform, With<Boat>>()
                .single_mut(app.world_mut())
                .expect("a match should have a boat in it");
            transform.translation.x = spot.x;
            transform.translation.z = spot.y;
            app.update();
            boat(app).translation.y
        }

        /// What the ground the boat is riding says about a spot.
        fn ground(app: &App, spot: Vec2) -> f32 {
            app.world()
                .resource::<Ground>()
                .height(spot.x, spot.y)
                .expect("the test ground has arrived")
        }

        // Dry land: the highest ground on the island, where the answer is
        // furthest from the waterline.
        let mut peak = (Vec2::ZERO, f32::MIN);
        for iz in 0..24 {
            for ix in 0..24 {
                let spot = Vec2::new(ix as f32 / 23.0 * 2.0 - 1.0, iz as f32 / 23.0 * 2.0 - 1.0)
                    * TEST_ISLAND_REACH;
                let height = ground(&app, spot);
                if height > peak.1 {
                    peak = (spot, height);
                }
            }
        }
        assert!(peak.1 > 0.0, "the island is entirely under water");
        assert_eq!(
            put_down(&mut app, peak.0),
            peak.1,
            "the boat is not sitting on the ground it is over"
        );

        // And open water past the coast: the swell exactly, at the moment the
        // frame settled, however deep the seabed under it. The same function
        // the game floats with, because what is being pinned is the
        // agreement — the hull at the height the water is drawn at.
        let offshore = Vec2::new(TEST_ISLAND_REACH * 1.5, 0.0);
        assert!(
            ground(&app, offshore) < 0.0,
            "the point picked to be open water is dry land"
        );
        let floated = put_down(&mut app, offshore);
        // The default conditions, because no forecast has reached this app —
        // exactly what `float` is riding on.
        let water = crate::sea::SeaConditions::default().swell(
            offshore,
            elapsed(&app),
            -ground(&app, offshore),
        );
        assert_eq!(
            floated, water,
            "the boat floats at {floated} m, the swell there stands at {water} m"
        );
    }

    #[test]
    fn an_anchored_boat_bobs_on_the_swell() {
        // Not sailing, not steering — just afloat, and still never quite
        // still: the water moves, so the hull does. A run of frames has to
        // find it at more than one height, where the flat sea held it at
        // exactly one forever.
        let mut app = island_app();
        place(&mut app, Vec2::new(TEST_ISLAND_REACH * 1.5, 0.0), Vec2::X);

        let mut heights = Vec::new();
        for _ in 0..60 {
            run_frames(&mut app, 1);
            heights.push(boat(&mut app).translation.y);
        }
        assert!(
            heights.iter().any(|h| h != &heights[0]),
            "a second of frames never moved the hull off {} m",
            heights[0]
        );
    }

    #[test]
    fn an_anchored_hull_sways_with_the_swell() {
        // The other half of bobbing: the water's slope moves the deck, not
        // just its height. At anchor over open water the hull is never quite
        // level and never quite still — a degree or two of pitch and roll as
        // the swell passes under it, and nowhere near the lean of a
        // full-helm turn, which must stay the biggest thing the hull does.
        let mut app = island_app();
        place(&mut app, Vec2::new(TEST_ISLAND_REACH * 1.5, 0.0), Vec2::X);
        run_frames(&mut app, 300);

        let mut tilts = Vec::new();
        for _ in 0..400 {
            run_frames(&mut app, 1);
            tilts.push(boat(&mut app).up().as_vec3().angle_between(Vec3::Y));
        }
        let most = tilts.iter().fold(0.0f32, |a, &b| a.max(b));
        assert!(
            most > 0.005,
            "the deck never left level ({most} rad at most)"
        );
        assert!(
            most < SHIP.heel_at_full_turn,
            "the swell alone tilts the hull {most} rad, past a full-helm heel"
        );
        assert!(tilts.iter().any(|t| *t != tilts[0]), "the deck froze");
    }

    #[test]
    fn the_deck_leans_with_the_water_not_against_it() {
        // The sign, pinned end to end: the bow rises where the water under
        // it stands higher than under the stern, and the masthead goes to
        // port when the starboard beam is the lifted side. Correlated over a
        // few swell periods rather than matched frame by frame, because the
        // tilt is eased and trails the water it is following.
        let mut app = island_app();
        place(&mut app, Vec2::new(TEST_ISLAND_REACH * 1.5, 0.0), Vec2::X);
        run_frames(&mut app, 300);

        let (mut fore_aft, mut athwart) = (0.0, 0.0);
        for _ in 0..600 {
            run_frames(&mut app, 1);
            let transform = boat(&mut app);
            let water_at = |offset: Vec3| {
                let point = transform.transform_point(offset);
                let depth = -app
                    .world()
                    .resource::<Ground>()
                    .height(point.x, point.z)
                    .expect("the test ground has arrived");
                app.world().resource::<sea::SeaConditions>().swell(
                    Vec2::new(point.x, point.z),
                    elapsed(&app),
                    depth,
                )
            };
            let asks_pitch = water_at(Vec3::new(0.0, 0.0, -SHIP.length / 2.0))
                - water_at(Vec3::new(0.0, 0.0, SHIP.length / 2.0));
            let asks_roll = water_at(Vec3::new(SHIP.beam / 2.0, 0.0, 0.0))
                - water_at(Vec3::new(-SHIP.beam / 2.0, 0.0, 0.0));
            // The bow's lift is the pitch's sine; the starboard rail's is
            // the roll's, masthead to port as it rises.
            fore_aft += asks_pitch * transform.forward().y;
            athwart += asks_roll * transform.right().y;
        }
        assert!(fore_aft > 0.0, "the bow dips as the water under it rises");
        assert!(athwart > 0.0, "the hull rolls away from the lifted beam");
    }

    #[test]
    fn a_beached_hull_sits_level() {
        // The ground holds what it has taken: put down on the island's high
        // ground, the pose is not the water's to touch however the sea moves
        // — the rotation stays exactly as placed, frame after frame, the
        // same never-written stillness an idle boat holds.
        let mut app = island_app();
        place(&mut app, Vec2::ZERO, Vec2::X);
        app.update();
        let posed = boat(&mut app).rotation;
        run_frames(&mut app, 120);
        assert_eq!(boat(&mut app).rotation, posed);
    }

    /// The same app with a hand of ground already delivered — the island the
    /// collision tests run aground on, and the surface the float test rides.
    /// The tests that take a bare [`test_app`] instead are the other case worth
    /// having: a client that has been sent nothing must still be able to move.
    fn island_app() -> App {
        let mut app = test_app();
        app.insert_resource(test_ground());
        app
    }

    /// Puts the boat down at a spot, pointing a way — a `--focus` in little.
    fn place(app: &mut App, at: Vec2, facing: Vec2) {
        let mut transform = app
            .world_mut()
            .query_filtered::<&mut Transform, With<Boat>>()
            .single_mut(app.world_mut())
            .expect("a match should have a boat in it");
        transform.translation = Vec3::new(at.x, 0.0, at.y);
        transform.rotation = Quat::from_rotation_y(f32::atan2(-facing.x, -facing.y));
    }

    /// How far the boat is from the middle of the test island, which is round:
    /// this is its distance off the coast plus [`TEST_ISLAND_REACH`].
    fn from_the_island(app: &mut App) -> f32 {
        let at = boat(app).translation;
        Vec2::new(at.x, at.z).length()
    }

    /// What [`grounding`] makes of the pose the boat is lying in: how far the
    /// bottom there stands above the depth the hull is held at, so positive is
    /// aground and more positive is further in.
    fn bite(app: &mut App) -> f32 {
        let transform = boat(app);
        grounding(&SHIP, Some(app.world().resource::<Ground>()), &transform)
    }

    #[test]
    fn the_keel_is_probed_as_closely_as_the_ground_is_sampled() {
        // What a handful of points along the keel does buy: no facet of the
        // height field fits between two probes, so ground rising across a facet
        // is read on the way up. Not the same as seeing everything the field can
        // draw — a crest narrower than a facet is read off its flanks and
        // missed, which no spacing at this scale fixes; [`KEEL_PROBES`] carries
        // the argument for wearing that rather than probing the keel to death.
        let spacing = (SHIP.heel_station - SHIP.forefoot_station) / (KEEL_PROBES - 1) as f32;
        assert!(
            spacing <= FACET_METRES,
            "{spacing} m between probes leaves room for a {FACET_METRES} m facet to hide in"
        );
    }

    #[test]
    fn a_boat_driven_at_a_coast_grounds_short_of_it() {
        let mut app = island_app();
        place(
            &mut app,
            Vec2::new(TEST_ISLAND_REACH + 60.0, 0.0),
            Vec2::new(-1.0, 0.0),
        );
        wind_astern(&mut app, 7.0);

        tap(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, SETTLED);

        // Held off the island rather than stopped out in the open: the test
        // island's rim is near vertical, so its coast is where its reach says.
        let reached = from_the_island(&mut app);
        assert!(
            reached > TEST_ISLAND_REACH,
            "the boat is {reached} m out, which is inside a coast at {TEST_ISLAND_REACH} m"
        );
        assert!(
            reached < TEST_ISLAND_REACH + 10.0,
            "the boat stopped {reached} m out, nowhere near the coast it was driven at"
        );

        // And stopped is stopped, not grinding: the ground takes the way off
        // each frame the wind puts a breath of it back on, so the hull holds
        // exactly the same spot with the sails still set and the wind still
        // blowing onshore. The same spot *on the map* — it still bobs, the
        // water under it being water.
        let aground = boat(&mut app).translation;
        run_frames(&mut app, 10);
        let held = boat(&mut app).translation;
        assert_eq!(
            Vec2::new(held.x, held.z),
            Vec2::new(aground.x, aground.z),
            "the boat is still creeping ashore"
        );
    }

    #[test]
    fn astern_backs_a_grounded_boat_off() {
        let mut app = island_app();
        place(
            &mut app,
            Vec2::new(TEST_ISLAND_REACH + 60.0, 0.0),
            Vec2::new(-1.0, 0.0),
        );
        // The wind blows *onshore* for the whole test, so the backing-off
        // half is also the pin on astern owing the wind nothing: dead
        // downwind of the beach, the boat still comes off it.
        wind_astern(&mut app, 7.0);
        tap(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, SETTLED);
        let aground = from_the_island(&mut app);
        // Phase one has to have been *stopped* for phase two to say anything:
        // a run that sailed clean over the island would back off it exactly as
        // far, and the test would pass with no collision in the build at all.
        // So the same check the coast test makes, made again here.
        assert!(
            aground > TEST_ISLAND_REACH && aground < TEST_ISLAND_REACH + 10.0,
            "the boat is {aground} m out, which is not held at a coast at {TEST_ISLAND_REACH} m"
        );

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release_all();
        hold(&mut app, KeyCode::ArrowDown);
        run_frames(&mut app, SETTLED);

        // Backing off a beach is what astern is for, and it needs no rule of
        // its own: away from the ground is the deeper water the hull came in
        // over, and deeper is always allowed. One held key is the whole
        // gesture — its first frame furls the sails, the rest of it backs.
        let backed = from_the_island(&mut app);
        assert!(
            backed > aground + SHIP.length,
            "the boat came off {} m, less than its own length",
            backed - aground
        );
    }

    #[test]
    fn open_water_is_sailed_at_the_speed_the_wind_gives() {
        // Nothing under the keel, nothing in the way: the ground the boat has
        // been sent must cost it no speed at all where there is water enough.
        let mut app = island_app();
        place(
            &mut app,
            Vec2::new(TEST_ISLAND_REACH + 100.0, 0.0),
            Vec2::new(1.0, 0.0),
        );
        wind_astern(&mut app, 7.0);
        tap(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, SETTLED);

        let before = boat(&mut app).translation;
        let start = elapsed(&app);
        run_frames(&mut app, 60);
        let seconds = elapsed(&app) - start;

        let expected = SHIP.speed * strength(7.0);
        let made = (boat(&mut app).translation - before).length() / seconds;
        assert!(
            (made - expected).abs() < expected * 0.01,
            "the boat made {made} m/s over open water, not {expected}"
        );
    }

    #[test]
    fn a_boat_put_down_inland_drives_back_to_the_sea() {
        // What `--focus` on an island leaves behind, and the case that says
        // the ground holds a boat without ever trapping one.
        let mut app = island_app();
        let ashore = Vec2::new(TEST_ISLAND_REACH * 0.5, 0.0);
        place(&mut app, ashore, Vec2::new(1.0, 0.0));
        assert!(
            bite(&mut app) > 0.0,
            "the spot picked to be dry land has water over it"
        );

        // No wind is set here, deliberately: facing +X the assumed day's
        // breeze is already near dead astern, and the assertion below reads
        // the *default* conditions' swell — a custom wind is a different sea.
        tap(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, SETTLED);

        // Back at sea means back on the water: riding the swell exactly,
        // rather than holding any height the hillside gave it.
        let at = boat(&mut app).translation;
        let depth = -app
            .world()
            .resource::<Ground>()
            .height(at.x, at.z)
            .expect("the boat sailed off the ground it was given");
        assert_eq!(
            at.y,
            crate::sea::SeaConditions::default().swell(Vec2::new(at.x, at.z), elapsed(&app), depth),
            "the boat never made it back to the water"
        );
        let afloat = from_the_island(&mut app);
        assert!(
            afloat > TEST_ISLAND_REACH,
            "the boat is still {afloat} m from the middle, inside the coast"
        );
    }

    #[test]
    fn a_grounded_boat_is_never_driven_further_aground() {
        // The pose a rule of "aground already, let it through" would hand the
        // whole island to: bow over the beach, and then whatever the player
        // likes. Driven straight at it, and with no helm — the helm was held
        // here once, on the thought that a hull swept round every heading
        // tries the rule from more angles than one heading does. It tries
        // nothing at all. The turning circle at full way is five metres, so
        // the bow comes round and the boat sails off the shore under any rule
        // whatever, including both of the ones this test exists to catch; what
        // it was watching was a boat that had floated away. A heading held is
        // also what makes the two bites comparable, `grounding` being a
        // reading of the ground under a particular pose and not a property of
        // the spot. Coming round while aground has a test of its own.
        let mut app = island_app();
        place(
            &mut app,
            Vec2::new(TEST_ISLAND_REACH - 1.0, 0.0),
            Vec2::new(-1.0, 0.0),
        );
        let before = bite(&mut app);
        let out = from_the_island(&mut app);
        assert!(before > 0.0, "the boat was meant to start aground");

        // An onshore wind, or the no-go zone would be doing the rule's work.
        wind_astern(&mut app, 7.0);
        tap(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, SETTLED);

        let after = bite(&mut app);
        assert!(
            after <= before,
            "the boat worked its way {} m further into the ground",
            after - before
        );
        // And the same thing said in the terms a player sees it in: bow at the
        // island, key down for eight hundred frames, and not a metre of the
        // island gained.
        let ended = from_the_island(&mut app);
        assert!(
            ended >= out,
            "the boat made {} m towards the middle of the island",
            out - ended
        );
    }

    #[test]
    fn the_helm_answers_while_aground() {
        // A refused turn on top of a refused advance is a hull wedged against
        // a shore for good, so the bow comes round whatever is under it.
        let mut app = island_app();
        place(
            &mut app,
            Vec2::new(TEST_ISLAND_REACH - 1.0, 0.0),
            Vec2::new(-1.0, 0.0),
        );
        let before = boat(&mut app).rotation;

        wind_astern(&mut app, 7.0);
        tap(&mut app, KeyCode::ArrowUp);
        hold(&mut app, KeyCode::ArrowLeft);
        run_frames(&mut app, 20);

        assert_ne!(
            boat(&mut app).rotation,
            before,
            "a boat the ground has stopped cannot come round"
        );
    }
}
