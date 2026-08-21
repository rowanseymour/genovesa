//! Boats: the hulls the player gets about in, and the keys that steer the one
//! they are aboard.
//!
//! The player themself is not here — they are a person, see [`crate::player`],
//! riding this boat as a child of it. What *kind* of boat an entity is is the
//! wire's [`BoatKind`], and each kind has a [`Hull`]: the dimensions and
//! manners the rules below are written against, so a new kind of boat is a
//! new `Hull` and a new model rather than a new module. A hull's kind is
//! settled the moment it is spawned and never changes — the ship is entered
//! at, and the rowboat is lowered from it to go ashore, which is
//! `player::embark_or_land`'s story.
//!
//! A hull is modelled rather than drawn here: [`MODEL`] is a glTF file built
//! from a Blender master under `assets-src/`, and this module spawns its meshes
//! and steers what they hang off. The few dimensions a `Hull` names are the
//! ones the rules read, and are not the model's to change quietly —
//! `the_model_is_the_hull_the_keel_is_probed_along` holds the file to them.
//!
//! A boat faces down its own -Z, so [`Transform::forward`] is the way it is
//! pointing and steering can leave the axis convention alone. Its origin is on
//! the waterline rather than at the keel or the deck, which is what lets
//! [`float`] put it down by simply setting the height of the surface it is on.

use bevy::animation::graph::{AnimationGraph, AnimationGraphHandle, AnimationNodeIndex};
use bevy::animation::{AnimationClip, AnimationPlayer};
use bevy::asset::RenderAssetUsages;
use bevy::gltf::GltfAssetLabel;
use bevy::math::Vec3Swizzles;
use bevy::mesh::PrimitiveTopology;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use protocol::{BoatId, BoatKind, PlayerId};

use crate::bindings::{Action, KeyBindings};
use crate::camera::View;
use crate::models::above;
use crate::player::Player;
use crate::sea;
use crate::terrain::Ground;
use crate::{eased, matte, model_mesh, AppState, Helm};

/// The ship, as a file. Built from `assets-src/models/boat/boat.blend` by
/// `assets-src/models/export.sh`, which is also where the export settings the
/// look depends on are written down.
const MODEL: &str = "models/boat.glb";

/// The rowing boat, as a file: `assets-src/models/rowboat/`. Rigged, unlike the
/// ship, so it cannot be pulled apart mesh by mesh the way [`MODEL`] is — a
/// skinned mesh has to arrive as a whole scene or it is a shape with no
/// skeleton behind it.
const ROWBOAT_MODEL: &str = "models/rowboat.glb";

/// The rowboat's clips, by their position in the file — held to their names
/// by `the_rowboat_ships_its_oars_and_pulls_them`. Two states and not a dial:
/// oars in, and one full turn of the stroke — the master's NOTES say why
/// nothing should stand halfway between them.
const STOWED: usize = 0;
const STROKE: usize = 1;

/// Water covered by one turn of the stroke cycle in still air, in metres — a
/// little more than the hull's own length, which is what a steady pull moves a
/// dinghy. In a wind it is more or less, the ground a pull makes good going
/// with the drive while the rower holds their [`CADENCE`].
///
/// The figure's stride constant, worn by a boat, and a number here for the
/// same reason: it is not *in* the file. A clip knows how long it lasts in
/// seconds, not how far the boat it is drawn in would travel. Re-key a
/// different stroke and this wants matching, or the blades will slip through
/// the water.
const PULL: f32 = 3.5;

/// How fast a rower works, in turns of the stroke a second: [`ROWBOAT`]'s
/// still-air speed read as a rate rather than as a distance.
///
/// A rate is the whole of the wind's bargain with the oars. The air moves the
/// water a pull makes good and leaves the pace of the arms alone, so a rower
/// fighting a headwind keeps their stroke and gets less for each one. Turning
/// the stroke by the water covered instead put the weather straight into the
/// rower's arms, and a gale read as somebody rowing lazily.
const CADENCE: f32 = ROWBOAT.speed / PULL;

/// How fast the oars come out and go in again, in e-foldings per second —
/// see [`eased`]. Brisk on purpose: crossing between the two clips sweeps
/// the looms through the gunwale, which is what shipping the oars looks like
/// and is fine taken quickly, but nothing to be left standing halfway in.
const SHIPPING: f32 = 8.0;

/// Within this of fully out or fully in, the oars *are* — the tail-closing
/// [`settled`] gives every ease here, in the blend's own unitless terms
/// rather than [`HEEL_SETTLED`]'s degrees. A hundredth of the pose, far
/// below noticing, and what lets a boat at rest hold one pose frame after
/// frame instead of forever approaching it.
const SHIPPED: f32 = 0.01;

/// How far the hull has to move in a frame, in metres, before it is making
/// way rather than lying still.
///
/// A hull under this client's helm stops dead — [`steer`] snaps the way to
/// zero and then writes nothing — but a moored one is eased towards where it
/// was last told, and an ease never quite arrives. Without a floor under it
/// every dinghy at anchor would row gently forever.
const STIRRING: f32 = 0.001;

/// How far a hull can move in one frame, in metres, and still have rowed
/// there. A hull's place is not always made good: taking the helm of a told
/// hull puts it down where the server says outright, and without this the
/// jump would read as water covered and spin the blades through a dozen
/// strokes.
///
/// A distance rather than a speed, for the reason the figure's own teleport
/// guard is one — judging it as a speed means dividing by the frame's clock,
/// so a frame that ran long reads as a jump and a stutter blanks the stroke.
/// Half a pull is far more water than any frame of rowing covers and far
/// less than any jump worth the name.
const TELEPORT: f32 = PULL / 2.0;

/// Which mesh in [`MODEL`] is which. glTF numbers its meshes rather than naming
/// them in a way the loader can ask for, so these are positions in the file —
/// which means reordering the objects in Blender would silently swap the hull
/// for the spar. `the_model_is_a_hull_and_a_spar_fit_to_draw` is what
/// stops that being found by looking at it.
const HULL_MESH: usize = 0;
const SPAR_MESH: usize = 1;

/// The dimensions and manners a kind of boat is driven by — the wire's
/// [`BoatKind`] resolved to this side's constants.
fn hull_of(kind: BoatKind) -> &'static Hull {
    match kind {
        BoatKind::Sloop => &SHIP,
        BoatKind::Rowboat => &ROWBOAT,
    }
}

/// A mast, as the fittings need it: the masthead the pennant is tied at, in
/// metres above the waterline, and its station on the same axis as the
/// keel's. On [`Hull`] as an `Option` because a mast is the one piece of a
/// boat a kind can simply not have — the rowboat is an open boat with
/// nothing standing in it — and everything hung off one (the pennant, the
/// sail, the wind as throttle) goes with it.
#[derive(Clone, Copy)]
struct Mast {
    head: f32,
    station: f32,
}

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
    /// Keel depth below the waterline. What makes it the game's business
    /// rather than the model's is [`Hull::grounding_draft`], measured from it.
    draft: f32,
    /// Where somebody aboard stands: metres above the waterline, and the
    /// station on the keel's axis. A player is put down here rather than at
    /// the hull's origin, which is the waterline and so is knee-deep in the
    /// bilges. The model's numbers rather than the game's to choose, like the
    /// draft, and held to the file by
    /// `the_model_is_the_hull_the_keel_is_probed_along`.
    helm_deck: f32,
    helm_station: f32,
    /// Where the keel begins and ends, in metres from amidships — negative
    /// forward, the same axis the hull is modelled on. [`grounding`] probes
    /// along these, so what runs aground is the line that is drawn.
    forefoot_station: f32,
    heel_station: f32,
    /// The masthead, where there is a mast: the pennant is tied on at its head,
    /// so like the deck and the draft its numbers are the model's — a mast
    /// re-cut in Blender and not re-measured here would fly its pennant in mid
    /// air, which `the_model_flies_a_pennant_from_its_masthead` catches.
    /// `None` is an unsparred boat: no pennant, no sail, and the wind no
    /// longer the throttle — though it still has its say, a rowed hull
    /// taking the air as a force on top of the oars rather than as a drive.
    /// See [`steer`] and [`row_drive`].
    mast: Option<Mast>,
    /// The waterline footprint the sea is cut away inside, for a hull that
    /// is *open* — looked into from above, with its sole below the water
    /// outside. `None` is a closed hull whose deck hides its insides — the
    /// ship — which needs no hole and gets none. See [`OpenHull`] for the
    /// shape and [`cut_the_water`] for what is done with it.
    open_footprint: Option<OpenHull>,
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
    /// two of the waterline; further out it is off a shelf too thin to float
    /// one, which the palette has been painting as shallows for a while — so a
    /// boat held out is held out of water it can be seen to be held out of.
    fn grounding_draft(&self) -> f32 {
        self.draft - KEEL_BITE
    }

    /// Where somebody aboard stands, in the hull's own frame — see
    /// [`Hull::helm_deck`].
    fn helm(&self) -> Vec3 {
        Vec3::new(0.0, self.helm_deck, self.helm_station)
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
    // The quarterdeck's step up aft and the spot on it just forward of the
    // tiller's grip — the model's numbers. The station keeps the helmsman
    // clear of the boom, which sweeps the main deck and nothing abaft the
    // step.
    helm_deck: 1.2,
    helm_station: 2.6,
    // The forefoot stops short of the bow, which is what gives the stem its
    // rake; the heel runs right aft to the transom.
    forefoot_station: -7.0 * 0.5 * 0.7,
    heel_station: 7.0 * 0.5,
    // Six metres of mast, stepped forward of amidships — the model's, not a
    // choice made here.
    mast: Some(Mast {
        head: 6.9,
        station: -1.05,
    }),
    // A closed hull: the deck hides the inside, so the sea needs no hole.
    open_footprint: None,
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

/// The rowing boat: the small end of the fleet, and [`ROWBOAT_MODEL`]'s
/// subject. The ship's boat, lowered alongside to put somebody ashore and
/// rowed in from there — how every landing happens, see
/// `player::embark_or_land`. The dimensions are the model's, and
/// `the_rowboat_model_is_the_dinghy_the_game_floats` holds the file to them
/// the way the ship's tests hold its.
const ROWBOAT: Hull = Hull {
    // A dinghy rather than a skiff: the model was recut smaller and
    // shallower the day the sea learned to cut a hole around an open hull —
    // its NOTES carry the story.
    length: 3.2,
    beam: 1.3,
    draft: 0.25,
    // Standing on the sole, abaft the rowing thwart — an open boat is stood
    // in wherever the thwarts are not, and this keeps the figure clear of
    // the middle one until somebody is seated at it. The sole is *below*
    // the waterline, the way a real one's is, which is the whole of what
    // the sea's hole buys.
    helm_deck: -0.1,
    helm_station: 0.65,
    // The keel is rockered: deepest a little abaft amidships, rising to the
    // forefoot forward and carried aft to the transom's skeg. The probes
    // read the full draft along all of it, which errs a few centimetres
    // shy at the rockered ends — the right side to miss on.
    forefoot_station: -1.1,
    heel_station: 1.6,
    // An open boat: nothing stands in it, so nothing flies from it and no
    // sail drives it. The wind is not done with it for that — see
    // [`row_drive`] — it simply pushes the hull instead of driving it.
    mast: None,
    // And being open, the sea is cut away inside it. The numbers are the
    // model's waterline outline read a few centimetres above the water —
    // the swell stands that much higher against the planking — and the
    // couple of centimetres they overreach the hull hide under the
    // freeboard from every angle this camera has.
    open_footprint: Some(OpenHull {
        semi_bow: 1.9,
        semi_stern: 1.6,
        semi_beam: 0.6,
        abaft: 0.29,
        bow_fullness: 1.7,
        stern_fullness: 2.2,
        transom: 1.31,
    }),
    // Rowed: a strong steady pull, and about what a real skiff makes — the
    // pace the world is *not* crossed at, the ship being how anyone goes
    // anywhere far.
    speed: 3.0,
    // Backing water is half a pull.
    astern_speed: 1.5,
    // A few hundred kilos gathers way in a stroke or two and glides a
    // length; the ship's numbers, scaled by feel rather than by physics.
    way_response: 0.8,
    // Nimble: a rowboat spins nearly in its own length.
    turn_rate: 2.5,
    // Barely: no keel gripping the water means little to lean against, and
    // a dinghy is turned flat.
    heel_at_full_turn: 0.06,
    heel_response: 0.25,
    // Light enough to follow the chop closer than the ship does.
    sway_response: 0.5,
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
/// That is less than "nothing gets past". A crest only one lattice line wide is
/// *not* seen — the field is linear between its corners, so two probes either
/// side read its flanks and the hull sails through a rock at the waterline.
/// Coasts are safe by being coasts: the bottom shelves, so the ground under the
/// keel is near enough monotone. What is exposed is the isolated skerry, and
/// seeing one reliably would mean probing at a fraction of a metre on every
/// frame at both poses, to buy a rock in open water. Sailing through a skerry
/// is the smaller wrong, and can be paid off from the other end by giving the
/// skerries some width.
///
/// The sides are not probed: a hull here is a shallow V, drawing its full draft
/// on the centreline and nothing at the beam. That is a standing condition on
/// the model — a hull remodelled with a flat bottom carried out to the beam
/// would need probes out there too.
const KEEL_PROBES: usize = 4;

/// Way below this, with no drive asked for, is stopped, and [`steer`] snaps
/// it to exactly zero. The ease only ever halves the remainder — left alone
/// the boat would creep forever, never quite done stopping — and a hull at
/// rest should be *at rest*: the same spot every frame, nothing moving.
const WAY_STOPPED: f32 = 0.02;

/// Way below this, the gunwale key treats the hull as stopped: the way is
/// snapped to zero and the crossing goes ahead. [`WAY_STOPPED`] is where the
/// glide's own tail snaps, and it is deliberately tiny — but tiny means the
/// tail takes seconds to reach it, and a furled boat that *reads* as stopped
/// refused a crossing for way nobody can see is a key that just does nothing.
/// That was found the hard way: "F is broken" reported at a helm whose glide
/// had a few imperceptible centimetres a second left to run. Half a metre a
/// second is a stroll — nobody stepping across a gunwale at it is stepping
/// off a moving deck.
const CROSSING_WAY: f32 = 0.5;

/// Within this of the heel the turn is asking for, the hull snaps to it
/// exactly — the same tail-closing that [`WAY_STOPPED`] does for the way,
/// and for the same reason: the ease only ever halves the remainder, and a
/// hull done straightening should be *level*, holding one rotation frame
/// after frame rather than forever creeping towards it. A third of a degree,
/// invisible at any zoom.
const HEEL_SETTLED: f32 = 0.005;

/// The no-go zone: within this of head-to-wind, set sails carry nothing, in
/// radians. The one piece of sailing realism kept for its own sake, because it
/// *is* the game — upwind is tacked for, downwind is free — and 45 degrees is
/// wide enough that pinching reads as a mistake while a full-helm tack still
/// crosses the zone inside a second.
///
/// The game's sailing rules rather than any hull's manners, which is why they
/// are module constants and not [`Hull`] fields.
const NO_GO: f32 = std::f32::consts::FRAC_PI_4;

/// Where the sails reach their full drive: a beam reach, a quarter turn off the
/// wind. From the edge of the no-go zone to here the drive ramps linearly, and
/// from here through a dead run it is full — no downwind taper, on purpose. A
/// real polar sags a little dead downwind, which only makes the fastest point
/// of sail one the player is never quite on.
const FULL_DRIVE: f32 = std::f32::consts::FRAC_PI_2;

/// The band the wind's strength drives the hull across, as fractions of
/// [`Hull::speed`]: the foot of the ramp and the fraction made once the wind
/// saturates. The foot is where the line would meet a wind of nothing rather
/// than a speed anything on the water makes — the lightest air the sky blows
/// stands a little above it, and under [`sea::WIND_NAMED`] the drive is
/// tapered away entirely (see [`sail_drive`]). What it buys is that the
/// light-air spells the weather calls calms are a slow passage rather than a
/// crawl — the world is crossed by boat and the sky holds its spells for
/// minutes — and the ceiling is a modest reward for sailing a blow rather
/// than a new top gear. The angle to the wind is the game; the strength is
/// flavour inside this band.
const DRIVE_BAND: (f32, f32) = (0.6, 1.1);

/// The wind at which the sky's hold on a hull saturates, in metres per second
/// — a strong breeze, short of the near-gale the weather tops out at. Above it
/// more wind is more sea — the swell's business — but no more speed. Both
/// hulls answer to it: it is where the sails' drive tops out and where the
/// wind's push on a rowed hull does, there being one sky over them.
const WIND_SATURATES: f32 = 12.0;

/// What a wind dead astern adds to a rowed hull's speed and what one dead
/// ahead takes off it, as fractions of [`Hull::speed`] once the wind has
/// saturated. Nothing here is a polar: the oars are the engine and the air is
/// a force laid on top of them, so what the wind does is its component along
/// the heading and a rowboat is given no no-go zone. It gets one anyway,
/// without anybody writing it down — the component falls away with the
/// cosine, so a rower who can make no ground straight into a blow still makes
/// some at an angle to it, and works to windward by zig-zagging rather than
/// by being told that tacking exists.
///
/// Losing more than it gains is the head sea. Pulling into it the chop comes
/// over the bow as well as the air; running before it there is only the shove.
/// The loss is set so a hull dead into the wind stops making ground a little
/// over eleven metres a second, and is carried slowly astern by the near-gale
/// the sky tops out at. That is deliberately up among the weather's
/// occasional excursions rather than in its everyday run — sampled across
/// seeds the sky blows that hard about one moment in fifty, against a median
/// nearer three and a half, where the same rule costs a rower a tenth of their
/// pace and no more. The gain is deliberately the smaller number: a shove down
/// the run ashore, not a new top gear, the same judgement [`DRIVE_BAND`]'s
/// ceiling makes for the sails.
const ROWING_WINDAGE: (f32, f32) = (0.35, 1.15);

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

/// The lean a turn asks of the hull, in radians of roll: the full-turn heel, by
/// how hard the helm is over, by the way's share of the hull's speed — clamped
/// at one, a blow driving past hull speed while the full-turn heel is a ceiling
/// of its own. Port helm is a positive turn and an outward lean is to
/// starboard, which about the forward axis is a negative roll.
fn heel_for(hull: &Hull, helm: f32, way: f32) -> f32 {
    -hull.heel_at_full_turn * helm * (way / hull.speed).clamp(-1.0, 1.0)
}

/// The speed set sails ask for, as a factor on [`Hull::speed`]: the polar at
/// this heading times the wind's strength. The *true* wind, not the apparent,
/// so the speed a heading earns holds still while the boat gathers way towards
/// it. The pennant flies the apparent wind and will disagree, which is real
/// sailing rather than a bug to reconcile.
///
/// The no-go zone always stands: there is no wind this can be sailed straight
/// into. Below [`sea::WIND_NAMED`] — where the screen names no direction —
/// the drive tapers to nothing on the same ramp the compass fades its arm's
/// ink on, so the engine and the instrument go out together. The boom is not
/// on that ramp: it comes amidships at the bar and stays there, having a side
/// to pick rather than an amount to give up — see [`sail_trim`].
///
/// It used to go the other way: below the bar the band's floor drove on *any*
/// heading, so a calm could not park anybody — and a boat could sail dead
/// into a faintly-drawn wind. The stranding worry is answered at the source
/// now, the weather never blowing below a light air (see
/// [`protocol::ToClient`]'s `Weather`), which leaves the bar catching only a
/// console-ordered calm and the drawn wind fading into one — honestly airs
/// that drive nothing.
fn sail_drive(bow: Vec2, wind: Vec2) -> f32 {
    let blowing = wind.length();
    if blowing == 0.0 {
        // No air at all drives nothing — said outright because a zero vector
        // has no angle to take, and the taper below would only turn the NaN
        // into another NaN.
        return 0.0;
    }
    let named = (blowing / sea::WIND_NAMED).clamp(0.0, 1.0);
    // The angle off the eye of the wind — the bow against where the air is
    // coming *from*, `wind` being where it is going.
    strength(blowing) * polar(bow.angle_to(-wind).abs()) * named
}

/// The speed a rowed hull makes good, as a factor on [`Hull::speed`]: the pull
/// the oars are worth, gained on or eaten into by the wind's component along
/// the heading — see [`ROWING_WINDAGE`] for which way and how much.
///
/// Squared, and that is the whole of why an ordinary landing is unchanged by
/// the weather. The rowboat is how everybody gets ashore, so a term straight in
/// the wind would tax every trip in every breeze; squared, an everyday six
/// metres a second costs a quarter of the effect and is barely felt, while a
/// real blow is the whole of it. It is the honest shape as well — windage and a
/// head sea both grow faster than the wind that raises them.
///
/// No bar underneath it like [`sail_drive`]'s: the square carries its own way
/// to nothing, so a wind too slack for the screen to name is already a wind
/// this does nothing with, and there is no calm here to strand anybody in. The
/// oars do not wait on the weather's permission.
fn row_drive(bow: Vec2, wind: Vec2) -> f32 {
    // Normalised, unlike [`sail_drive`]'s, which only ever takes an angle from
    // it: a hull heeling or riding a sea tips its bow out of the horizontal
    // without pointing anywhere new, and a component read off the tipped
    // vector would have the weather flicker with the swell.
    let astern = wind.dot(bow.normalize_or_zero());
    let reach = (astern / WIND_SATURATES).clamp(-1.0, 1.0);
    let (with, against) = ROWING_WINDAGE;
    let bite = if astern >= 0.0 { with } else { -against };
    1.0 + bite * reach * reach
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

/// The sail, as corners in the frame of an entity stood at the mast's foot on
/// the waterline, so everything [`trim_the_sails`] does is a rotation about the
/// mast. Tack and head up the luff, clew aft along the boom, in metres.
const SAIL_TACK: Vec3 = Vec3::new(0.0, 1.3, 0.0);
const SAIL_HEAD: Vec3 = Vec3::new(0.0, 6.5, 0.0);
const SAIL_CLEW: Vec3 = Vec3::new(0.0, 1.3, 3.0);
/// The belly: pushed out to one side for exactly the pennant's reason — see
/// [`pennant_mesh`] — and proportionally deeper, canvas drawing harder than
/// a flag.
const SAIL_BELLY: Vec3 = Vec3::new(0.4, 3.4, 1.1);

/// How far the boom lies off the centreline, in radians: close-hauled at the
/// edge of the no-go zone, eased out to nearly square on a dead run. Visual
/// only — the drive is [`sail_drive`]'s business — but a sail sheeted the way
/// the wind asks is what the eye reads as "the wind is doing this".
const TRIM_BAND: (f32, f32) = (0.26, 1.35);

/// The apparent wind that flies the pennant out, in metres per second, and
/// how far it still sags at that wind, in radians. Both are the drawing's to
/// pick rather than the weather's: a flag that only lifted in the gales would
/// be a limp rag through most of a day, and one that ever came out perfectly
/// straight would read as a signboard rather than as cloth.
const PENNANT_FLIES: (f32, f32) = (5.0, 0.14);

/// How far the pennant's tie stands off the mast's axis, in metres: the spar's
/// half-width and a little air. The flag is tied to the spar's *surface* on the
/// side it is flying, the way a ring on a mast would slide — a tie on the axis
/// swings every sag of the cloth down through the spar itself, and hung a
/// becalmed flag entirely inside the masthead.
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
/// `kind` and the `hull` its dimensions are copied from are only ever set
/// together, by [`Boat::of`] — and always to the kind the entity was rigged
/// as at spawn, hulls never changing kind under their fittings.
///
/// The rest is sailing state. `way` is the speed the hull is making along its
/// heading, ahead positive — the keys name a speed and [`steer`] brings `way`
/// towards it. `heel` is the roll shown *for the turn*, positive with the
/// masthead to port; `pitch` and `roll` are the tilt the water is showing,
/// [`float`] easing the deck towards the sea's own slope. All are kept here
/// rather than read back off the transform, which holds heading, pitch and
/// both rolls multiplied together.
#[derive(Component)]
pub struct Boat {
    kind: BoatKind,
    hull: Hull,
    way: f32,
    heel: f32,
    pitch: f32,
    roll: f32,
    /// Whether the boat is being driven. On the ship this is the sails: set,
    /// the wind is the throttle — see [`sail_drive`]; furled, the target way
    /// is zero, the hull glides to a stop and holds station, and that
    /// holding is the whole of "anchored" here — the crew drops the hook,
    /// nothing simulates it. On the rowboat the same flag is the oars being
    /// pulled, on the same keys.
    sails_set: bool,
}

impl Boat {
    /// A boat of a kind, at rest — sails furled, oars shipped, the state a
    /// world is entered in.
    pub fn of(kind: BoatKind) -> Self {
        Self {
            kind,
            hull: *hull_of(kind),
            way: 0.0,
            heel: 0.0,
            pitch: 0.0,
            roll: 0.0,
            sails_set: false,
        }
    }

    /// What kind of boat this is — which the gunwale key reads, a ship's
    /// helm lowering the tender where a rowboat's lands or boards.
    pub fn kind(&self) -> BoatKind {
        self.kind
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
    /// this. Crossings ask the forgiving form, [`reads_as_stopped`].
    ///
    /// [`reads_as_stopped`]: Boat::reads_as_stopped
    pub fn at_rest(&self) -> bool {
        self.way == 0.0
    }

    /// Whether the hull reads as stopped at a gunwale: within
    /// [`CROSSING_WAY`] of rest. The strict gate refused crossings for way
    /// nobody could see, for the several seconds the glide's tail takes to
    /// reach [`WAY_STOPPED`] on its own — which read as the key being
    /// broken, not as a boat being underway.
    ///
    /// A pure question, deliberately: a gate that answers it may still
    /// refuse the crossing for its own reasons — no footing, nothing in
    /// reach — and a refusal must leave the glide it read exactly as it
    /// found it. The crossing that is granted stops the hull with
    /// [`comes_to_rest`], and the night's offer reads this through a query
    /// that could not write if it wanted to.
    ///
    /// [`comes_to_rest`]: Boat::comes_to_rest
    pub fn reads_as_stopped(&self) -> bool {
        self.way.abs() < CROSSING_WAY
    }

    /// Stops the hull where it lies — the commit half of
    /// [`reads_as_stopped`], called by a crossing that was actually granted,
    /// so the deck being stepped off is exactly as still as the gate read
    /// it. The same tail-closing every ease here gets — see [`settled`].
    ///
    /// [`reads_as_stopped`]: Boat::reads_as_stopped
    pub fn comes_to_rest(&mut self) {
        self.way = settled(self.way, 0.0, CROSSING_WAY);
    }

    /// Where somebody aboard stands, in the hull's own frame — see
    /// [`Hull::helm_deck`]. What a player boarding is put down at, so that
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

/// The rowboat scene hung under a hull — what [`conduct_the_oars`] looks for
/// above an arriving animation player, so the oars claim only their own: the
/// figure walking the same hull's deck arrives through the very same query.
#[derive(Component)]
struct Oared;

/// The rowboat's two clips, mixed and ready to play: the graph both hang in,
/// the node each occupies, and the stroke's own handle, which [`row`] needs
/// to ask how long one turn of the cycle lasts. One graph for every dinghy
/// that will ever be rowed — a graph describes the clips, not any boat
/// rowing to them.
#[derive(Resource)]
struct Rowing {
    graph: Handle<AnimationGraph>,
    stowed: AnimationNodeIndex,
    stroke: AnimationNodeIndex,
    cycle: Handle<AnimationClip>,
}

/// An animation player that is a rowboat's, and whose hull it is pulling —
/// the figure's dancer arrangement (see [`crate::figure`]), worn by a boat.
/// The state lives here rather than on the hull because a hull outlives its
/// sailing systems: ashore the [`Boat`] comes off, and the oars settle
/// stowed on a boat with no sailing state at all.
#[derive(Component)]
struct Rower {
    hull: Entity,
    /// Where in the stroke cycle the oars are, as a fraction of one turn —
    /// advanced by the water the hull covers rather than by time, so the
    /// blades bite at whatever speed the boat is making and backing water
    /// pulls the cycle backwards.
    phase: f32,
    /// How much of the stroke shows against the stowed pose, eased between
    /// 0 and 1 on [`SHIPPING`] and snapped at both ends by [`SHIPPED`].
    out: f32,
    /// Where the hull was, being the only way to learn what it covered —
    /// the oars ask nobody what is moving the boat, exactly as the figure's
    /// legs ask nobody what is moving the walker.
    last: Vec3,
}

/// What a hull is — the kind it was rigged as at spawn, written by [`rig`]
/// and never again.
///
/// The kind lives on the entity rather than only on [`Boat`] because a hull
/// outlives the sailing systems: stepping ashore takes the `Boat` off and
/// leaves the fittings standing, and the gunwale key still has to know a
/// ship lying alongside from a rowboat lying beached.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub struct Rigged(pub BoatKind);

/// A hull the sea is cut away inside — the shape of the hole, which is the
/// hull's waterline outline in its own frame: two superellipse halves sharing
/// their beam at the widest station, cut square at the transom. Two halves
/// because one ellipse cannot be a boat — fat enough for the transom's corners
/// it wraps metres of clear water round the bow, fine enough for the bow it
/// pinches at the quarters.
///
/// Sized so the edge lands within the planking's thickness at the waterline: a
/// hair too wide shows a dark sliver of missing sea against the topsides, a
/// hair too narrow a sliver of sea inside the bilges, and the freeboard hides
/// the first.
///
/// On the boat entity rather than among its fittings, so a moored open hull
/// keeps its hole after its `Boat` comes off with the crew.
#[derive(Component, Clone, Copy)]
struct OpenHull {
    /// Metres from the widest station forward to where the outline closes
    /// at the stem, and aft to where the stern half *would* close — the
    /// transom cuts it off first, which is what keeps the stern half wide
    /// through the quarters without the hole trailing off astern.
    semi_bow: f32,
    semi_stern: f32,
    /// Half the footprint's width at the widest station.
    semi_beam: f32,
    /// How far abaft amidships the widest station sits, in metres — a
    /// hull's widest section rarely being amidships exactly.
    abaft: f32,
    /// How full-bodied each end's outline is: superellipse exponents.
    /// `2.0` is a true ellipse; lower pinches towards a lens, which is a
    /// fine bow, and higher squares out towards the corners, which is a
    /// stern with a transom coming.
    bow_fullness: f32,
    stern_fullness: f32,
    /// Metres from the widest station aft to the transom, where the
    /// outline is cut square whatever the stern half is doing.
    transom: f32,
}

/// The name a hull answers to on the wire — see [`protocol::BoatId`]. Only
/// the hulls a server told us about carry one: a world with no server behind
/// it (the headless tests') launches a nameless boat, and the absence is how
/// [`crate::player::embark_or_land`] tells the two apart.
#[derive(Component, Clone, Copy)]
pub struct HullId(pub BoatId);

/// Any hull at all — ours under sail, another's under way, anyone's at
/// anchor. What the boarding key sweeps for, the components that say *whose*
/// a hull is coming and going with the helm.
#[derive(Component)]
pub struct Vessel;

/// Every hull in the world, as the fittings hung off one read it: the
/// transform it rides at, and its sailing state where there is one — which
/// is only ever this player's own, a told hull's way and canvas being no
/// part of what crosses the wire. `F` keeps the query clear of whichever
/// fitting is doing the reading, a child's transform being a transform.
type Hulls<'w, 's, F> =
    Query<'w, 's, (Option<&'static Boat>, &'static Transform), (With<Vessel>, F)>;

/// Where the server last put a hull nobody here is steering, and how it was
/// pointed — eased towards, like a marker; [`moor`] is what does the easing
/// and keeps the hull riding the swell meanwhile. Present on every hull but
/// the one this player has the helm of, whose transform belongs to the
/// sailing systems.
#[derive(Component, Clone, Copy)]
struct ToldHull {
    position: Vec2,
    heading: f32,
}

/// The boats of the world, as this client was told them: the wire's ids to
/// this side's entities, who is at each helm, and — the fact everything
/// else hangs off — which hull is *ours*.
///
/// Cleared on leaving the world; the entities clear themselves, being
/// `DespawnOnExit`.
#[derive(Resource, Default)]
pub struct Fleet {
    hulls: HashMap<BoatId, Entity>,
    crews: HashMap<BoatId, PlayerId>,
    /// The boat this player holds the helm of, by the server's telling.
    /// What decides which hull the sailing systems steer, and whether the
    /// session reports [`protocol::ToServer::Helm`] or `Move`.
    pub helmed: Option<BoatId>,
}

impl Fleet {
    /// Whether a player is at some boat's helm — whose marker is then the
    /// boat itself, the capsule adding nothing but clutter over the deck.
    pub fn crewed(&self, player: PlayerId) -> bool {
        self.crews.values().any(|aboard| *aboard == player)
    }

    /// Whether anyone holds this boat's helm, as of the last telling — what
    /// keeps the boarding key from asking after a helm that is visibly
    /// somebody's. The server rules either way; this only spares the wire
    /// an ask whose answer is already on screen.
    pub fn manned(&self, boat: BoatId) -> bool {
        self.crews.contains_key(&boat)
    }

    /// Gives our hull back to its moorings — what stepping ashore does. The
    /// sailing systems come off, the hull holds station where it lies until
    /// a telling moves it, and the fleet stops calling any helm ours.
    ///
    /// `pose` is where the hull is standing, which is where it is moored: the
    /// caller reads it off the scene, and a hull spawned earlier in this same
    /// batch of tellings has none there yet. Optional for exactly that
    /// reason — the *bookkeeping* must happen either way, or the fleet goes
    /// on calling a helm ours after the telling took it away. Left unmoored,
    /// the hull is moored by the next telling about it, which is on the wire
    /// already: nothing takes a helm from us without saying where the hull
    /// it left us is.
    pub fn hand_back(&mut self, commands: &mut Commands, hull: Entity, pose: Option<&Transform>) {
        if let Some(boat) = self.helmed.take() {
            self.crews.remove(&boat);
        }
        let mut hull = commands.entity(hull);
        hull.remove::<Boat>();
        if let Some(pose) = pose {
            let forward = pose.forward();
            hull.insert(ToldHull {
                position: pose.translation.xz(),
                heading: f32::atan2(-forward.x, -forward.z),
            });
        }
    }

    /// A word about a boat: the first spawns its hull, every later one
    /// re-moors it or changes whose hands are on the helm. Called by the
    /// session's [`crate::net::receive`], which is where everything a server
    /// says lands.
    ///
    /// The one word that changes this client's own life is `occupant`
    /// becoming — or no longer being — *us*: boarding is asked of the server
    /// and only believed when the telling comes back, so this is where our
    /// hull gains the sailing systems and the player steps onto its deck.
    /// The player may arrive on that deck from anywhere — their own feet, out
    /// of nowhere at all (entry aboard being a player who begins on a deck),
    /// or another helm entirely: a lower grant seats them in the tender while
    /// they still stand on the ship, and re-parenting is the whole crossing.
    /// Whatever helm we held before is given back to its moorings first.
    #[allow(clippy::too_many_arguments)]
    pub fn told(
        &mut self,
        commands: &mut Commands,
        kit: &mut HullKit,
        players: &crate::player::Players,
        poses: &Query<&Transform, With<Vessel>>,
        me: PlayerId,
        id: BoatId,
        kind: BoatKind,
        position: Vec2,
        heading: f32,
        occupant: Option<PlayerId>,
    ) {
        let hull = *self.hulls.entry(id).or_insert_with(|| {
            spawn_hull(
                commands,
                kit,
                kind,
                Transform::from_xyz(position.x, 0.0, position.y)
                    .with_rotation(Quat::from_rotation_y(heading)),
                Some(id),
            )
        });
        if self.helmed == Some(id) && occupant != Some(me) {
            // Ours until this telling said otherwise. Ordinarily our own
            // disembark or lowering already gave it back — see
            // [`crate::player::embark_or_land`] and the grant below — and
            // this is the defence for the word arriving out of that order:
            // believed, because the server is the authority on whose hands
            // are on a helm. Before the crew bookkeeping, which the telling's
            // own word should have the last say on. Handed back unmoored, and
            // deliberately: this branch runs only when the telling is not
            // about us, which is exactly when the bottom of this function
            // moors the same hull on the telling's own word — a pose read off
            // the scene here would be overwritten in the same command queue,
            // the same frame, by the better of the two moorings.
            self.hand_back(commands, hull, None);
        }

        match occupant {
            Some(player) => self.crews.insert(id, player),
            None => self.crews.remove(&id),
        };

        if occupant == Some(me) {
            if self.helmed != Some(id) {
                // Ours, as of this telling: the helm the server granted — at
                // entry, or a boarding or a lowering confirmed. Any helm we
                // held until now goes back to its moorings where it lies; its
                // own telling follows on the wire, the grant being sent first
                // exactly so that no client holds two helms between them.
                if let Some(former) = self.hull() {
                    let pose = poses.get(former).ok().copied();
                    self.hand_back(commands, former, pose.as_ref());
                }
                // The hull comes off its moorings and under the sailing
                // systems, snapped to where the server says it lies, and the
                // player steps aboard.
                self.helmed = Some(id);
                let boat = Boat::of(kind);
                let helm = Transform::from_translation(boat.helm());
                commands.entity(hull).remove::<ToldHull>().insert((
                    boat,
                    Transform::from_xyz(position.x, 0.0, position.y)
                        .with_rotation(Quat::from_rotation_y(heading)),
                ));
                if let Ok((player, _)) = players.single() {
                    commands
                        .entity(player)
                        .remove::<DespawnOnExit<AppState>>()
                        .remove::<crate::player::Unsettled>()
                        .insert((ChildOf(hull), helm));
                } else {
                    commands.entity(hull).with_child((
                        Name::new("Player"),
                        Player,
                        helm,
                        Visibility::default(),
                    ));
                }
            }
            // Later tellings about our own boat are our own reports echoed
            // — the Board grant broadcast reaches the asker too — and this
            // machine's simulation is the authority on its own hull.
            return;
        }

        // Somebody else's, or nobody's: moored to wherever the server said,
        // which [`moor`] eases it towards.
        commands.entity(hull).insert(ToldHull { position, heading });
    }

    /// The entity of the hull we hold the helm of, if any — see
    /// [`Fleet::helmed`].
    ///
    /// The book's answer to what carries the player, which is not the same
    /// question as the scene graph's and must not be mistaken for it. This
    /// one is written the instant a telling is read; the parentage that goes
    /// with it is a queued command, so within one drain of the wire this is
    /// the fresher of the two. It is also the one that can go quiet while the
    /// player is still standing on a deck — see the out-of-order defence in
    /// [`Fleet::told`] — so it is a fallback rather than an authority.
    /// [`crate::player::put_down`] is where the two are put in their order.
    pub fn hull(&self) -> Option<Entity> {
        self.helmed.and_then(|held| self.hulls.get(&held)).copied()
    }

    /// A boat is out of the world — a tender hoisted back aboard a ship, or
    /// one its keeper left behind and the world took back. The hull
    /// despawns; a player the fleet still believed aboard it — a seating
    /// telling lost or out of order — is stood on the water where the hull
    /// was and left [`crate::player::Unsettled`] to find the ground, exactly
    /// as `net::enter_afoot` puts down a player who enters on their own
    /// feet. Their pose was the hull's to hold, so leaving them with it
    /// would drop them at the world origin. Ordinarily nobody is: the wire
    /// says who is seated where before it says a hull is gone.
    ///
    /// Judged by the fleet's own book rather than the scene graph, whose
    /// parentage from a grant earlier this same frame is still a queued
    /// command.
    pub fn gone(
        &mut self,
        commands: &mut Commands,
        players: &crate::player::Players,
        poses: &Query<&Transform, With<Vessel>>,
        id: BoatId,
    ) {
        let Some(hull) = self.hulls.remove(&id) else {
            return;
        };
        self.crews.remove(&id);
        if self.helmed.take_if(|held| *held == id).is_some() {
            if let Ok((player, _)) = players.single() {
                let mut player = commands.entity(player);
                player
                    .remove::<ChildOf>()
                    .insert(DespawnOnExit(AppState::InWorld));
                if let Ok(pose) = poses.get(hull) {
                    // The hull's place, and the way it was pointing — its
                    // yaw alone, a heeling boat being no reason to stand
                    // somebody at an angle. Sea level to start with, like
                    // any other entry afoot: the ground is what settles it.
                    let at = pose.translation;
                    let forward = pose.forward();
                    player.insert((
                        Transform::from_xyz(at.x, 0.0, at.z).with_rotation(Quat::from_rotation_y(
                            f32::atan2(-forward.x, -forward.z),
                        )),
                        crate::player::Unsettled,
                    ));
                }
            }
        }
        commands.entity(hull).despawn();
    }
}

/// What spawning a hull needs in hand — bundled because the telling arrives
/// inside [`crate::net::receive`], which is already juggling the markers'
/// own assets.
#[derive(bevy::ecs::system::SystemParam)]
pub struct HullKit<'w, 's> {
    pub(crate) meshes: ResMut<'w, Assets<Mesh>>,
    pub(crate) materials: ResMut<'w, Assets<StandardMaterial>>,
    pub(crate) assets: Res<'w, AssetServer>,
    /// The procedural pieces every hull shares, made once per system that
    /// spawns hulls and cloned per boat: a harbour of thirty hulls is one
    /// pennant mesh, not thirty.
    fittings: Local<'s, Option<Fittings>>,
}

/// The handles [`spawn_hull`] deals from — see [`HullKit::fittings`].
#[derive(Clone)]
struct Fittings {
    painted: Handle<StandardMaterial>,
    pennant_material: Handle<StandardMaterial>,
    sail_material: Handle<StandardMaterial>,
    pennant_mesh: Handle<Mesh>,
    sail_mesh: Handle<Mesh>,
}

pub struct BoatPlugin;

impl Plugin for BoatPlugin {
    fn build(&self, app: &mut App) {
        // Steering before floating, so ground gained or lost by this frame's
        // movement is under the hull the same frame rather than the next.
        // The conditions the hull floats on, here as well as in the terrain
        // plugin: resources are global and initialising one twice is free,
        // and the boat's own tests run without any terrain at all.
        app.init_resource::<sea::SeaConditions>()
            .init_resource::<Fleet>()
            // Cleared with the world it described: the next world's hulls
            // are new tellings, and a fleet carried over would pin their
            // ids to entities that no longer exist.
            .add_systems(OnExit(AppState::InWorld), scuttle)
            .add_systems(Startup, mix_the_oars)
            // Only for a world with no server behind it — see [`launch`].
            // A served world's boats arrive as tellings instead.
            .add_systems(
                OnEnter(AppState::InWorld),
                launch.run_if(not(resource_exists::<crate::net::Online>)),
            )
            .add_systems(
                Update,
                // Only the steering stops when the game is paused. Floating is
                // not motion — it sets the hull to the height of the ground
                // under it — so leaving it running means a chunk arriving
                // under the pause menu is settled on before the player looks
                // again. The pennant and the sail come after, and outside the
                // pause for the same reason: both are drawn off what the
                // hull's steering wrote, and reading it before this frame's
                // steering wrote it leaves the cloth a frame behind its mast.
                (
                    steer.run_if(in_state(Helm::Sailing)),
                    float,
                    fly_the_pennant,
                    trim_the_sails,
                    // The hulls nobody here is steering, ridden after the
                    // one that is: same water, same frame.
                    moor,
                    // The oars after every hull has been moved, ours and
                    // the moored alike, because they are turned by the water
                    // the hull *covered* — measured frame against frame
                    // rather than read off anything. That is also what lets
                    // them run outside the pause on different terms from the
                    // cloth: the pennant and the sail are re-derived each
                    // frame and merely freeze, while the stroke is integrated
                    // and would otherwise outrun the boat. A hull nothing
                    // moved covered no water, so the stroke stands where it
                    // stood behind the chart on its own.
                    //
                    // Conducted before rowed, so a scene that arrived this
                    // frame rows this frame.
                    conduct_the_oars,
                    row,
                    // And the hole in the sea last, once every hull — sailed
                    // or moored — is where this frame leaves it.
                    cut_the_water,
                )
                    .chain()
                    .run_if(in_state(AppState::InWorld)),
            );
    }
}

/// Forgets the fleet with the world it belonged to.
pub(crate) fn scuttle(mut fleet: ResMut<Fleet>) {
    *fleet = Fleet::default();
}

/// Puts a nameless ship in the world at the point it is entered, pointing
/// the way the opening view looks — with the player aboard.
///
/// The *offline* entry, and only that: a served world's hulls arrive as
/// tellings and go through [`Fleet::told`]. What is left for this is a world
/// with no server behind it at all — the headless tests' — and it keeps the old
/// ceremony whole: boat under the view, player at the helm.
fn launch(mut commands: Commands, mut kit: HullKit, view: Res<View>) {
    let boat = spawn_hull(
        &mut commands,
        &mut kit,
        BoatKind::Sloop,
        // A rotation of `yaw` about the vertical takes -Z to the camera's
        // own forward, so the boat starts pointing away from the viewer.
        Transform::from_xyz(view.focus.x, 0.0, view.focus.z)
            .with_rotation(Quat::from_rotation_y(view.yaw)),
        None,
    );
    commands
        .entity(boat)
        .insert(Boat::of(BoatKind::Sloop))
        .with_child((
            Name::new("Player"),
            Player,
            Transform::from_translation(SHIP.helm()),
            Visibility::default(),
        ));

    // Said out loud for the same reason a run without a seed says which world
    // it picked: a placeholder nobody can find is indistinguishable from one
    // that never spawned, and the socket's `focus` takes exactly these two
    // numbers.
    info!("boat launched at {}, {}", view.focus.x, view.focus.z);
}

/// One hull, meshes and all, at a pose — everything a boat is *before*
/// anyone is aboard: no [`Boat`], because the sailing systems belong to
/// whoever holds the helm, and no [`ToldHull`], because who moves it is the
/// caller's decision. `named` is its wire id, for the hulls a server told
/// us about.
///
/// The meshes hang off the hull as children. Hull and spar stay two meshes not
/// for their colours — both carry their own — but because the game measures
/// them separately, the keel probed along one and the pennant tied to the
/// other. Their geometry is already in the boat's frame, so the children sit at
/// the identity.
pub(crate) fn spawn_hull(
    commands: &mut Commands,
    kit: &mut HullKit,
    kind: BoatKind,
    pose: Transform,
    named: Option<BoatId>,
) -> Entity {
    let hull = commands
        .spawn((
            Name::new("Boat"),
            Vessel,
            DespawnOnExit(AppState::InWorld),
            pose,
            // Carried by the parent because the children inherit it: without
            // one here there is nothing for their own visibility to be
            // computed against, and a boat whose meshes are on entities of
            // their own would never be drawn.
            Visibility::default(),
        ))
        .id();
    rig(commands, kit, hull, kind);
    if let Some(id) = named {
        commands.entity(hull).insert(HullId(id));
    }
    hull
}

/// Hangs a kind of boat's pieces under a bare hull entity — everything that
/// makes the entity *look* like a boat. Once, at spawn: a hull never changes
/// kind, so nothing ever takes them off again short of the hull going.
fn rig(commands: &mut Commands, kit: &mut HullKit, hull: Entity, kind: BoatKind) {
    commands.entity(hull).insert(Rigged(kind));

    // The sea's hole is the kind's, off its [`Hull`]: an open boat is cut
    // for, a closed hull's deck hides its insides and it gets none.
    if let Some(open) = hull_of(kind).open_footprint {
        commands.entity(hull).insert(open);
    }

    // The rowboat is rigged and so has to arrive as a whole scene — see
    // [`ROWBOAT_MODEL`]. The file's own materials come along with it, and
    // `models::paint` dresses the meshes in the shared white matte as they
    // turn up, the same way the figure's and the sharks' are.
    if kind == BoatKind::Rowboat {
        commands.entity(hull).with_child((
            Name::new("Rowboat"),
            Oared,
            WorldAssetRoot(
                kit.assets
                    .load(GltfAssetLabel::Scene(0).from_asset(ROWBOAT_MODEL)),
            ),
        ));
        return;
    }

    // The ship's model carries its own colours on its facets — see the
    // master's NOTES — so the timber is drawn with one white matte that lets
    // them through. The file's PBR materials are ignored: lit as the file
    // asked, the hull would be the one surface in the world with a highlight.
    // The cloth is drawn from both faces, a single-sided pennant winking out
    // every time the wind put its back to the camera.
    let fittings = kit
        .fittings
        .get_or_insert_with(|| Fittings {
            painted: kit.materials.add(matte(Color::WHITE)),
            pennant_material: kit.materials.add(StandardMaterial {
                double_sided: true,
                cull_mode: None,
                ..matte(PENNANT_COLOR)
            }),
            sail_material: kit.materials.add(StandardMaterial {
                double_sided: true,
                cull_mode: None,
                ..matte(SAIL_COLOR)
            }),
            pennant_mesh: kit.meshes.add(pennant_mesh(PENNANT.0, PENNANT.1)),
            sail_mesh: kit.meshes.add(sail_mesh()),
        })
        .clone();
    let mast = SHIP.mast.expect("the ship is masted");

    commands.entity(hull).with_children(|children| {
        children.spawn((
            Name::new("Hull"),
            Mesh3d(kit.assets.load(model_mesh(MODEL, HULL_MESH))),
            MeshMaterial3d(fittings.painted.clone()),
        ));
        children.spawn((
            Name::new("Spar"),
            Mesh3d(kit.assets.load(model_mesh(MODEL, SPAR_MESH))),
            MeshMaterial3d(fittings.painted),
        ));
        // Tied to the masthead and pointed by [`fly_the_pennant`]. The one
        // piece of the boat that is not in the file: a flag is a shape that
        // has to be *aimed*, and aiming it means knowing where its tie is,
        // which a mesh out of Blender does not say.
        children.spawn((
            Name::new("Pennant"),
            Pennant {
                // Astern until the first frame says otherwise, which is
                // where a flag on a boat at rest in still air would lie
                // anyway.
                bearing: 0.0,
            },
            Mesh3d(fittings.pennant_mesh),
            MeshMaterial3d(fittings.pennant_material),
            Transform::from_xyz(0.0, mast.head, mast.station),
        ));
        // The sail, at the mast's foot so its rotation is a turn about the
        // mast, and hidden because a world is entered at anchor —
        // [`trim_the_sails`] shows it while the sails are set and lays the
        // boom where the wind asks.
        children.spawn((
            Name::new("Sail"),
            Sail,
            Mesh3d(fittings.sail_mesh),
            MeshMaterial3d(fittings.sail_material),
            Transform::from_xyz(0.0, 0.0, mast.station),
            Visibility::Hidden,
        ));
    });
}

/// Tells the sea where not to be: the waterline footprint of any open hull,
/// written into the sea's material, whose fragment shader discards the water
/// inside it. The sea is one sheet drawn straight through everything, so
/// without the hole it stands in the bilges of any boat looked into.
///
/// Written through the same read-compare-write two-step as the wake — see
/// [`crate::wake::lay_the_wake`] — so a hull lying still re-uploads nothing.
///
/// The material carries one hole, and a world can hold several open hulls —
/// tenders lowered, beached, abandoned. The player's own boat is cut for
/// first, being the one whose bilges the camera actually looks into; any
/// other open hull in view keeps the sea in its bottom, which at this
/// camera's distance is a puddle and not worth a second uniform yet.
fn cut_the_water(
    hulls: Query<(Entity, &Transform, &OpenHull)>,
    players: Query<&ChildOf, With<Player>>,
    window: Option<Res<sea::DepthWindow>>,
    materials: Option<ResMut<Assets<sea::SeaMaterial>>>,
) {
    let (Some(window), Some(mut materials)) = (window, materials) else {
        return;
    };
    let carrier = players.single().ok().map(ChildOf::parent);
    let chosen = hulls
        .iter()
        .find(|(hull, ..)| Some(*hull) == carrier)
        .or_else(|| hulls.iter().next());
    let (hole, axes, shape) = match chosen {
        Some((_, transform, open)) => {
            let ahead = transform.forward().xz().normalize_or(Vec2::NEG_Y);
            // The footprint's own centre — the widest station — rather than
            // the hull's origin, so the shader tests each half from where
            // the two meet.
            let centre = transform.translation.xz() - ahead * open.abaft;
            (
                Vec4::new(centre.x, centre.y, ahead.x, ahead.y),
                Vec4::new(open.semi_bow, open.semi_stern, open.semi_beam, 1.0),
                Vec4::new(open.bow_fullness, open.stern_fullness, open.transom, 0.0),
            )
        }
        None => (Vec4::ZERO, Vec4::ZERO, Vec4::ZERO),
    };
    let stale = materials.get(window.material()).is_some_and(|material| {
        material.extension.hole != hole
            || material.extension.hole_axes != axes
            || material.extension.hole_shape != shape
    });
    if stale {
        if let Some(mut material) = materials.get_mut(window.material()) {
            material.extension.hole = hole;
            material.extension.hole_axes = axes;
            material.extension.hole_shape = shape;
        }
    }
}

/// Rides the moored hulls: eases each towards where the server last put it,
/// turns it onto its told heading, and keeps it on the water meanwhile —
/// level, the pitching and heeling being the sailing systems' and a boat at
/// anchor or under somebody else's hand having no business doing either
/// this side of the wire. The easing pace is the markers' own, tellings
/// arriving on the same reporting beat.
fn moor(
    ground: Option<Res<Ground>>,
    time: Res<Time>,
    sea: Res<sea::SeaConditions>,
    mut hulls: Query<(&ToldHull, &mut Transform), Without<Boat>>,
) {
    let elapsed = time.elapsed_secs_wrapped();
    let t = eased(8.0, time.delta_secs());

    for (told, mut transform) in &mut hulls {
        let at = transform.translation.xz();
        let eased_to = at.lerp(told.position, t);
        transform.translation.x = eased_to.x;
        transform.translation.z = eased_to.y;
        if let Some(height) = ground
            .as_ref()
            .and_then(|g| g.height(eased_to.x, eased_to.y))
        {
            let water = sea.water_over(ground.as_deref(), eased_to, elapsed);
            transform.translation.y = height.max(water);
        }
        transform.rotation = transform
            .rotation
            .slerp(Quat::from_rotation_y(told.heading), t);
    }
}

/// A flown cloth, as a shape: a burgee `length` metres long and `hoist` deep,
/// tied at the origin, so that everything [`fly_the_pennant`] does is a
/// rotation about the point the flag is actually made fast at.
///
/// It flies along -Z at rest, the way the boat itself faces, so a bearing
/// becomes a rotation about the vertical with no axis convention of its own.
/// The hoist hangs *below* the tie rather than straddling it, because a flag
/// is tied at its top corner and swings from there.
///
/// Its two dimensions are the caller's although only one cloth is cut from it
/// now: a cairn flew the second off the same arithmetic and is a pillar of
/// stone with nothing on it — see [`crate::cairn`]. A size written down at the
/// call is what let there be two at all.
///
/// The one thing it is not is flat, which is the whole reason it is three
/// triangles. A flat pennant vanishes whenever the wind lines up with the
/// camera — several times an hour at a fixed bearing, and indistinguishable
/// from the flag having been deleted. Pushing one interior point out to a side
/// puts a shallow belly in the cloth, which keeps some part of it facing the
/// viewer from any direction.
///
/// Rigid cloth is still a lie in a calm, and [`PENNANT`]'s narrow hoist is what
/// makes the lie cheap: at three tenths of a metre the missing fold is a hand's
/// width, watched from forty metres up.
fn pennant_mesh(length: f32, hoist: f32) -> Mesh {
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
    if wind.length() < sea::WIND_NAMED {
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
///
/// A hull with no [`Boat`] on it is nobody's here, and its canvas is furled
/// without asking: whether those hulls have sails set is not on the wire, and
/// reading the flag off a component just taken away would leave a beached hull
/// under full sail.
fn trim_the_sails(
    conditions: Res<sea::SeaConditions>,
    boats: Hulls<Without<Sail>>,
    mut sails: Query<(&ChildOf, &mut Transform, &mut Visibility), With<Sail>>,
) {
    for (of, mut transform, mut visibility) in &mut sails {
        let Ok((boat, hull)) = boats.get(of.parent()) else {
            continue;
        };
        let set = boat.is_some_and(Boat::sails_set);
        // Written only on change, so an idle boat's sail is as unwritten as
        // the rest of it.
        let shown = if set {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *visibility != shown {
            *visibility = shown;
        }
        if set {
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
/// The bearing is where the air is *going*: a flag is blown, and the eye reads
/// it as blown. The droop is the whole of the strength reading, so a player who
/// never looks at the corner of the screen still knows what the wind is doing.
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
/// purpose. An instrument wants the true wind, a bearing that changed as the
/// player accelerated being useless to steer by; a flag has no such duty, so a
/// boat driving into a light air blows its own pennant astern. The two
/// disagreeing is what sailors get from a burgee and a masthead instrument.
///
/// The boat's rotation is taken back out of the flag's, so heel, pitch and
/// heading move where the pennant *is* without touching where it points.
fn fly_the_pennant(
    time: Res<Time>,
    conditions: Res<sea::SeaConditions>,
    boats: Hulls<Without<Pennant>>,
    mut pennants: Query<(&mut Pennant, &ChildOf, &mut Transform)>,
) {
    for (mut pennant, of, mut transform) in &mut pennants {
        let Ok((boat, hull)) = boats.get(of.parent()) else {
            continue;
        };
        // Every flag in the world flies on the same wind. Our own hull adds
        // the wind of its way; a told hull's way is not on the wire, so its
        // flag reads the true wind alone — moored hulls have no way anyway.
        let way = boat.map_or(0.0, |boat| boat.way);
        // A pennant is only ever spawned on a masted rig — the rowboat flies
        // nothing — so a flag on a hull with no [`Boat`] is a moored ship's,
        // and the ship's own mast is the one to read it at.
        let Some(mast) = boat.map_or(SHIP.mast, |boat| boat.hull.mast) else {
            continue;
        };
        let masthead = Vec3::new(0.0, mast.head, mast.station);
        let apparent = conditions.wind() - hull.forward().xz() * way;
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
        transform.translation = masthead + hull.rotation.inverse() * tie_off;
    }
}

/// Mixes the rowboat's two clips together once for the whole run, exactly as
/// the figure's gaits are. Both hang at full weight — a node's weight
/// *multiplies* what the player asks for rather than being a starting value
/// it overrides — and the mixing is done entirely by what [`row`] sets on
/// the playing animations.
fn mix_the_oars(
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
) {
    let mut graph = AnimationGraph::new();
    let root = graph.root;
    let blend = graph.add_blend(1.0, root);
    let cycle: Handle<AnimationClip> =
        assets.load(GltfAssetLabel::Animation(STROKE).from_asset(ROWBOAT_MODEL));

    let stowed = graph.add_clip(
        assets.load(GltfAssetLabel::Animation(STOWED).from_asset(ROWBOAT_MODEL)),
        1.0,
        blend,
    );
    let stroke = graph.add_clip(cycle.clone(), 1.0, blend);

    commands.insert_resource(Rowing {
        graph: graphs.add(graph),
        stowed,
        stroke,
        cycle,
    });
}

/// Readies a rowboat's oars as its scene finishes arriving: the loader puts
/// an [`AnimationPlayer`] on the root of whatever it found animated, and
/// this hands the rowboat's the graph and starts both clips — the stroke
/// *paused*, because it is played to be seeked; see [`row`].
fn conduct_the_oars(
    mut commands: Commands,
    rowing: Res<Rowing>,
    hierarchy: Query<&ChildOf>,
    oared: Query<(), With<Oared>>,
    hulls: Query<(), With<Vessel>>,
    poses: Query<&Transform, With<Vessel>>,
    mut arrivals: Query<(Entity, &mut AnimationPlayer), Added<AnimationPlayer>>,
) {
    for (entity, mut player) in &mut arrivals {
        // Somebody else's model: the figure standing on this very deck
        // arrives through the same query, and its player is the gait's.
        if above(&hierarchy, &oared, entity).is_none() {
            continue;
        }
        // And whose hull the oars pull, so the stroke is turned by that
        // hull's own movement.
        let Some(hull) = above(&hierarchy, &hulls, entity) else {
            continue;
        };
        // Starting from where the hull stands, so the first frame reads the
        // water it covered and not the whole way from the origin.
        let Ok(place) = poses.get(hull) else {
            continue;
        };

        commands.entity(entity).insert((
            AnimationGraphHandle(rowing.graph.clone()),
            Rower {
                hull,
                phase: 0.0,
                out: 0.0,
                last: place.translation,
            },
        ));
        player.play(rowing.stowed).repeat();
        player.play(rowing.stroke).repeat().pause();
    }
}

/// Rows the oars: the stroke seeked round at the rower's own [`CADENCE`] while
/// a pull is asked for, and by the water going past the blades when it is not
/// — and shown only while there is a pull asked for or the boat is still going,
/// at rest the oars lying stowed.
///
/// Whether it turns at all is *measured*, not taken on trust: the hull's own
/// place, this frame against last.
/// Reading [`Boat::way`] is shorter and was what this did — [`steer`] advances
/// the hull by exactly `way * dt` and zeroes the way it grounds on, so the two
/// numbers agree on every frame `steer` runs. They part on the frames it does
/// not, and `steer` is the one system here the pause stops: with the chart or
/// the menu up the way stands frozen at whatever it last was while the hull
/// sits perfectly still, and a stroke integrating that number rows forever on
/// a dead boat. Water covered cannot lie that way — a hull nothing moved
/// covered none — which is the whole reason the figure measures too.
///
/// Measuring also buys the hulls this client never steers: a dinghy under
/// somebody else's oars carries no [`Boat`] to ask what it is doing and is
/// walked across the water by [`moor`]. Moving, it is taken to be rowed, at
/// the same cadence and whichever way round its own movement says — that
/// client has already put its weather into the place it reports. One left at
/// anchor covers nothing and lies with its oars in.
///
/// Height is thrown away, [`float`] setting the hull to the water's every
/// frame and a swell being no part of a stroke; a jump is not a stroke either,
/// which is [`TELEPORT`].
///
/// The drive is even *within* a stroke — see [`steer`] — so the hull glides at
/// one speed while the blades circle; giving the surge to the stroke is the
/// step not yet taken.
///
/// Nothing happens until the clip has loaded, which is a frame or two after
/// the rig — the oars hold the file's rest pose until then.
fn row(
    time: Res<Time>,
    rowing: Res<Rowing>,
    clips: Res<Assets<AnimationClip>>,
    hulls: Query<(&Transform, Option<&Boat>), With<Vessel>>,
    mut rowers: Query<(&mut Rower, &mut AnimationPlayer)>,
) {
    let Some(cycle) = clips.get(&rowing.cycle).map(AnimationClip::duration) else {
        return;
    };
    let dt = time.delta_secs();

    for (mut rower, mut player) in &mut rowers {
        let Ok((place, boat)) = hulls.get(rower.hull) else {
            continue;
        };
        let step = place.translation.xz() - rower.last.xz();
        rower.last = place.translation;

        // A jump is not a stroke — see [`TELEPORT`]. The water a hull was set
        // down across is water nobody rowed, so it reads as having covered
        // none of it and the stroke is left exactly where it stood.
        let moved = step.length();
        let covered = if moved > TELEPORT { 0.0 } else { moved };
        // Backwards through the same cycle when the water goes the other way
        // past the blades, which is what backing water is and needs no second
        // clip.
        let going = if place.forward().xz().dot(step) < 0.0 {
            -1.0
        } else {
            1.0
        };

        // The cadence, where there is any way on at all. That gate is the
        // whole of what the measuring above is for: behind the chart [`steer`]
        // stops and the hull stands still with its last way still written on
        // it, and a hull pinned against a beach covers nothing either. Both
        // leave the blades where they stand.
        let pulled = if covered > STIRRING {
            CADENCE * dt
        } else {
            0.0
        };
        let turned = match boat {
            // Under oars, and flat: whatever the wind is doing it is doing to
            // the ground the pull makes good, never to the arms. Forwards,
            // too, so a hull carried astern by a gale reads as rowing into it
            // rather than as backing away from it.
            Some(boat) if boat.sails_set => pulled,
            // Off the pull — gliding on after a furl, or backing water. There
            // is no cadence to keep, and the water going past the blades is
            // the whole of what turns them.
            Some(_) => going * covered / PULL,
            // And a hull this client does not steer, with no [`Boat`] to ask.
            None => going * pulled,
        };
        rower.phase = (rower.phase + turned).rem_euclid(1.0);

        // Out while a pull is asked for or the boat is still going: the glide
        // after the last pull still moves water past the blades, and oars
        // shipped mid-glide would be snatched in mid-stroke. Pulling against a
        // beach holds them out too, frozen where the way died, which is what
        // leaning on stopped oars looks like.
        //
        // This one asks the hull rather than the water, where there is a hull
        // to ask, and that is the whole difference between the two decisions
        // made here. The phase must be measured — a remembered way rows a
        // boat the chart has stopped. But measurement cannot tell a hull that
        // has *stopped* from one held still mid-glide, and the pose wants
        // that distinction: judged on water covered, opening the chart during
        // the seconds of glide after a furl ships the oars and closing it
        // runs them straight back out, which is the same overlay animating a
        // boat that has not changed. [`Boat::at_rest`] can tell, [`steer`]
        // snapping the tail of every glide to exactly zero. Only a hull with
        // no [`Boat`] at all — moored, walked across the water by [`moor`] —
        // has nothing to ask, and falls back to the water it covered.
        let target = match boat {
            Some(boat) => f32::from(boat.sails_set || !boat.at_rest()),
            None => f32::from(covered > STIRRING),
        };
        rower.out = settled(
            rower.out + (target - rower.out) * eased(SHIPPING, dt),
            target,
            SHIPPED,
        );

        if let Some(stroke) = player.animation_mut(rowing.stroke) {
            stroke.set_weight(rower.out);
            stroke.seek_to(rower.phase * cycle);
        }
        if let Some(stowed) = player.animation_mut(rowing.stowed) {
            stowed.set_weight(1.0 - rower.out);
        }
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
/// half-buried in the hillside; that is what aground looks like. No easing on
/// the water's motion either: the swell is gentle, and a hull seven metres long
/// simply is where the water is.
///
/// Afloat, the hull also wears the water's *slope*: the swell is sampled off
/// the bow, the stern and either beam, and the deck eases towards the plane
/// those four heights describe, on the hull's own sway response. The height is
/// not eased and the tilt is, deliberately — the hull *is* where the water is,
/// but turning a shape that long takes time the height does not need. A beached
/// hull eases level instead, the ground holding it.
///
/// The tilt goes on and comes off as a factor of its own: the rotation holds
/// heading, then the water's pitch, then a single roll factor the wave roll
/// shares with the turn's heel — so this strips the tilt it applied last frame
/// from the right and hangs the new one on, and [`steer`] keeps reaching the
/// roll factor it always has.
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

        let pitch = settled(
            boat.pitch + (target_pitch - boat.pitch) * t,
            target_pitch,
            HEEL_SETTLED,
        );
        let roll = settled(
            boat.roll + (target_roll - boat.roll) * t,
            target_roll,
            HEEL_SETTLED,
        );
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

/// The tail-closing every ease here gets: `within` of its target, the value
/// *is* the target, so a hull done settling holds one rotation frame after
/// frame rather than creeping towards it forever. [`eased`] only ever closes
/// a fraction of what is left, so nothing arrives without this.
///
/// How close is the caller's, because the things eased are not measured in
/// the same units: the hull's angles want [`HEEL_SETTLED`], a third of a
/// degree, and the oars' blend wants [`SHIPPED`], a hundredth of a pose.
fn settled(eased: f32, target: f32, within: f32) -> f32 {
    if (target - eased).abs() < within {
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
/// This is the whole of collision. The ground is a height field on a two-metre
/// lattice and the boat is a keel line above it, so "is there water enough
/// here" is a handful of lookups rather than triangle intersection —
/// [`Ground::height`] answers on exactly the facets the mesh was built from,
/// which is what makes the ground the boat is stopped by the ground the player
/// can see.
///
/// A probe over a chunk that has not arrived says nothing rather than
/// objecting, the same choice [`float`] makes. If land does turn up under the
/// hull, backing off still works — see [`steer`].
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

/// Where the ship's boat goes in the water, and how it points: a berth a
/// clear oar's width abeam — outside both hulls' planking — on the shoreward
/// side when the ground within reach says which side that is, and to port
/// when nothing does. The heading is the ship's own: a tender is lowered
/// over the side, not aimed, and the first stroke is the rower's to steer.
///
/// Here rather than in `player` because the widths it clears are the hulls',
/// which nothing outside this module knows.
pub(crate) fn tender_berth(ship: &Transform, ground: Option<&Ground>) -> (Vec2, f32) {
    // Outside both half-beams with clear water between the planking, so the
    // dinghy goes in beside the hull rather than through it.
    let abeam = (SHIP.beam + ROWBOAT.beam) / 2.0 + 0.6;
    let right = ship.right().xz().normalize_or(Vec2::X) * abeam;
    let at = ship.translation.xz();
    let (starboard, port) = (at + right, at - right);
    // Shoreward is the side the ground stands higher under. Ground that has
    // not arrived names no side, and port is the habit sailors would expect.
    let height = |spot: Vec2| ground.and_then(|g| g.height(spot.x, spot.y));
    let spot = match (height(starboard), height(port)) {
        (Some(toward), Some(away)) if toward > away => starboard,
        _ => port,
    };
    let forward = ship.forward();
    (spot, f32::atan2(-forward.x, -forward.z))
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
/// With the sails set the wind is the throttle — the target speed is the hull's
/// times [`sail_drive`], and the player's whole control of it is the helm. An
/// unsparred hull is rowed instead, which is the same keys and a different
/// bargain: the oars make their own speed and the wind only leans on it, by
/// [`row_drive`], so there is no heading a rowboat cannot be pointed at — only
/// a wind hard enough that pointing at it makes no ground.
/// Furling takes the target to zero; the way runs off on the glide and the hull
/// holds station where it dies, which is all "anchored" means here. Backing is
/// the one drive the wind has no part in, because backing off a beach is how a
/// grounding is undone and an escape that waited on a favourable wind would be
/// no escape. The sail keys are taps, and a frame carrying both furls first and
/// hoists second.
///
/// The way is eased rather than instant, stepped exactly for however long the
/// frame was so the ramp is the same shape at any frame rate, with
/// [`WAY_STOPPED`] closing the tail the exponential would never finish. The
/// glide is also what makes tacking work at all: the target dies crossing the
/// no-go zone, but the way carried into the turn brings the bow through the eye
/// and out the other side still moving.
///
/// The helm answers with no way on, and aground: a turn refused alongside an
/// advance is a hull wedged bow-first against a shore with nothing left that
/// would free it, so the bow may always come round even where the hull may not
/// go.
///
/// Land is what the hull may not go through, and the frame's advance is offered
/// to [`grounding`] before it is taken. It is allowed if the pose it would
/// reach floats, or failing that if it is aground no *deeper* than the pose
/// already held — the only rule that both frees a stranded hull and cannot be
/// played. It is narrower than it reads: a floating hull can only ever be
/// allowed a pose that floats, since `here` at or under zero makes the second
/// clause imply the first, so a boat under way halts still afloat with at most
/// [`KEEL_BITE`] in the mud. What the second clause is for is the pose the boat
/// did not sail into — a `focus` inland, or ground arriving under a hull
/// already sitting there — out of which every way down to the sea is downhill
/// and every way further in is refused like any other climb. Which is why the
/// comparison carries no tolerance: a hair a frame is a metre a second up a
/// hillside.
///
/// Only the pose at the end of the advance is judged; the path swept getting
/// there is covered by the probes of the frame before, which holds while a
/// frame's advance stays under the probe spacing — seventeen centimetres at
/// sixty frames a second, and two and a half metres at the quarter second Bevy
/// clamps a stalled frame to.
///
/// Turning at speed also heels the hull, on the target of [`heel_for`] and the
/// hull's heel-response curve, which is both the roll into the turn and the
/// straightening out of it. The roll is applied about the boat's own forward
/// axis, and the keel lies along that axis, so heeling moves nothing
/// [`grounding`] probes along — it is wholly a thing the eye gets.
///
/// Both poses are judged with the rotation the helm has just applied, so a turn
/// only ever changes where the advance goes, never whether it is allowed. What
/// the hull is stopped *by* is the keel line and nothing else — the stem rakes
/// out over the forefoot, so a bow can overhang a cliff face by half a metre
/// before anything objects.
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
        match hull.mast {
            Some(_) => hull.speed * sail_drive(transform.forward().xz(), conditions.wind()),
            // Rowed: the oars pull whatever the wind is doing, and what the
            // wind does is help or hinder them — see [`row_drive`]. Even
            // within a stroke still: the hull glides at one speed while the
            // blades circle, and giving the surge to the stroke is the step
            // not yet taken.
            None => hull.speed * row_drive(transform.forward().xz(), conditions.wind()),
        }
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

    // Heel last, against the way this frame settled on, so running aground
    // starts the straightening the same frame it takes the way off. The helm
    // above multiplies heading on from the left, so rolling on from the right
    // reaches the roll factor alone — see [`float`].
    let target_heel = heel_for(&hull, helm, boat.way);
    if boat.heel != target_heel {
        let heel_t = eased(1.0 / hull.heel_response, time.delta_secs());
        let heel = settled(
            boat.heel + (target_heel - boat.heel) * heel_t,
            target_heel,
            HEEL_SETTLED,
        );
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
        assert_model_draws, assert_model_is_painted, assert_rigid_skin, clip_names, elapsed, hold,
        rebind, run_frames, set_wind, test_ground, triangles, world_app, TEST_ISLAND_REACH,
    };

    /// Frames enough for the ease to be indistinguishable from settled —
    /// over eight time constants, a remainder of a few parts in ten thousand.
    const SETTLED: usize = 800;

    /// A headless app with the boat systems running, already in a match —
    /// the shared [`world_app`], under this module's older name.
    fn test_app() -> App {
        world_app()
    }

    /// The hull the player is aboard — which is the only hull at all in most
    /// of these tests, and the one they mean in the rest: once the ship's
    /// boat is in the water there are two, and every helper below is asking
    /// about the boat being sailed rather than the one left at anchor.
    fn helmed_hull(app: &mut App) -> Entity {
        app.world_mut()
            .query_filtered::<&ChildOf, With<Player>>()
            .single(app.world())
            .expect("a match should have a player aboard something")
            .parent()
    }

    fn boat(app: &mut App) -> Transform {
        let hull = helmed_hull(app);
        *app.world()
            .entity(hull)
            .get::<Transform>()
            .expect("a hull has a transform")
    }

    /// The way the hull is making, straight off the component — what the
    /// tack test watches frame by frame, an end position being unable to say
    /// whether the way ever died along the road to it.
    fn way_on(app: &mut App) -> f32 {
        let hull = helmed_hull(app);
        app.world()
            .entity(hull)
            .get::<Boat>()
            .expect("the helmed hull has sailing state")
            .way
    }

    /// Whether the boat's sails are set.
    fn sails_are_set(app: &mut App) -> bool {
        let hull = helmed_hull(app);
        app.world()
            .entity(hull)
            .get::<Boat>()
            .expect("the helmed hull has sailing state")
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

    /// And dead on the bow — the wind the rowing tests want, a rowed hull's
    /// hardest going being the one heading a sailed hull may not even take.
    fn wind_ahead(app: &mut App, speed: f32) {
        let bow = boat(app).forward().xz().normalize();
        set_wind(app, -bow * speed);
    }

    /// And square across it, which is no wind at all to a rowed hull: the
    /// component along the heading is exactly zero, so the oars pull as they
    /// do in still air with a sea running to prove the weather is on.
    fn wind_abeam(app: &mut App, speed: f32) {
        let bow = boat(app).forward().xz().normalize();
        set_wind(app, Vec2::new(-bow.y, bow.x) * speed);
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
            .filter(|c| (c.y - SHIP.helm_deck).abs() < 1e-4)
            .collect();
        let fore = plane.iter().map(|c| c.z).fold(f32::MAX, f32::min);
        let aft = plane.iter().map(|c| c.z).fold(f32::MIN, f32::max);
        assert!(
            (fore..=aft).contains(&SHIP.helm_station),
            "no quarterdeck at {} under the helm at {}",
            SHIP.helm_deck,
            SHIP.helm_station
        );

        // And the boom's sweep: the sail's foot turns about the mast at the
        // tack's height, out to the clew, so whatever the hull raises inside
        // that circle has to stay under it or the canvas drags through the
        // deck furniture. The companionway lives with this rule; the
        // quarterdeck, the tiller and the stem head stand outside the circle
        // or under the cloth instead.
        let mast = SHIP.mast.expect("the ship is masted");
        for corner in &corners {
            let reach = Vec2::new(corner.x, corner.z - mast.station).length();
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
        let mast = SHIP.mast.expect("the ship is masted");
        let top = corners.iter().map(|c| c.y).fold(f32::MIN, f32::max);
        assert!(
            (top - mast.head).abs() < 1e-4,
            "the model's masthead is {top} above the waterline, not {}",
            mast.head
        );

        let (forward, aft) = corners
            .iter()
            .fold((f32::MAX, f32::MIN), |(f, a), c| (f.min(c.z), a.max(c.z)));
        let stepped = (forward + aft) * 0.5;
        assert!(
            (stepped - mast.station).abs() < 1e-4,
            "the mast stands at {stepped}, not {}",
            mast.station
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
    fn the_rowboat_is_a_boat_fit_to_draw() {
        // One mesh carrying its own colours, like the figure — the oars move
        // under the boat's own skin rather than being meshes of their own, so
        // there is one shape here and not three.
        assert_model_draws(ROWBOAT_MODEL, &[(0, "rowboat")]);
        assert_model_is_painted(ROWBOAT_MODEL, 0);
        // And rigged, so the same rule the figure lives by applies: a vertex
        // shared between bones bends its facet as the oars swing, and a
        // gradient across a facet is the one thing this look cannot have.
        assert_rigid_skin(ROWBOAT_MODEL);
    }

    #[test]
    fn the_rowboat_model_is_the_dinghy_the_game_floats() {
        // What `grounding`, `float` and the sea's hole assume of a shape
        // they never look at, the way the ship's own test pins its. The one
        // mesh holds the oars too, but nothing of an oar reaches the
        // extremes measured here: the blades hang shy of the keel's depth,
        // and lie athwartships shy of the stem and transom.
        let corners: Vec<Vec3> = triangles(ROWBOAT_MODEL, 0, "POSITION")
            .into_iter()
            .flatten()
            .collect();

        let lowest = corners.iter().map(|c| c.y).fold(f32::MAX, f32::min);
        assert!(
            (lowest + ROWBOAT.draft).abs() < 1e-3,
            "the model's keel is {lowest} below the waterline, not {}",
            -ROWBOAT.draft
        );

        let half = ROWBOAT.length * 0.5;
        let (bow, transom) = corners
            .iter()
            .fold((f32::MAX, f32::MIN), |(f, a), c| (f.min(c.z), a.max(c.z)));
        assert!(
            (bow + half).abs() < 1e-3 && (transom - half).abs() < 1e-3,
            "the model runs {bow}..{transom}, not a {}m hull about amidships",
            ROWBOAT.length
        );

        // The sole is where anybody aboard stands: a plane of corners at
        // exactly the helm deck's height, spanning the helm's station.
        let plane: Vec<&Vec3> = corners
            .iter()
            .filter(|c| (c.y - ROWBOAT.helm_deck).abs() < 1e-3)
            .collect();
        let fore = plane.iter().map(|c| c.z).fold(f32::MAX, f32::min);
        let aft = plane.iter().map(|c| c.z).fold(f32::MIN, f32::max);
        assert!(
            (fore..=aft).contains(&ROWBOAT.helm_station),
            "no sole at {} under the helm at {}",
            ROWBOAT.helm_deck,
            ROWBOAT.helm_station
        );
        // And that plane is under water, which is the whole of what the
        // sea's hole buys: a master re-lofted with its floor standing above
        // the waterline is a boat that no longer needs cutting for, and
        // would be drawn dry by a sea that was never cut.
        let sole = plane.iter().map(|c| c.y).fold(f32::MIN, f32::max);
        assert!(
            sole < -0.05,
            "the sole is {sole}, back above the waterline — is the sea's hole still earning its keep?"
        );

        // The footprint the sea is cut away inside, against the hull it was
        // read off. The master's NOTES say it plainly: re-loft the hull and
        // these numbers are stale — too narrow and the sea leaks back into
        // the bilges, too wide and a moat of missing water shows round the
        // bow — and stale is exactly what nothing else here would notice.
        // The footprint is measured from its own centre, `abaft` aft of
        // amidships, so the model's stem and transom have to be carried back
        // to that centre before they can be compared.
        let open = ROWBOAT.open_footprint.expect("the rowboat is an open boat");
        // The couple of centimetres the outline is allowed to stand proud of
        // the planking, being read a little above the water: enough to hide
        // under the freeboard, not enough to show as clear water.
        let proud = 0.05;
        let stem = open.abaft - bow;
        assert!(
            (stem..stem + proud).contains(&open.semi_bow),
            "the hole reaches {} forward of its centre, not the {stem} the stem stands at",
            open.semi_bow
        );
        assert!(
            (open.transom - (transom - open.abaft)).abs() < proud,
            "the hole is cut square {} abaft its centre, not at the transom at {}",
            open.transom,
            transom - open.abaft
        );
    }

    /// The one hull in a headless match, and what it is currently dressed
    /// as, sailed as, and standing anybody aboard at.
    fn hull(app: &mut App) -> Entity {
        app.world_mut()
            .query_filtered::<Entity, With<Vessel>>()
            .single(app.world())
            .expect("a match should have a hull in it")
    }

    fn rigged(app: &mut App) -> BoatKind {
        let hull = hull(app);
        app.world()
            .entity(hull)
            .get::<Rigged>()
            .expect("a hull is dressed as something")
            .0
    }

    fn sailed(app: &mut App) -> BoatKind {
        let hull = hull(app);
        app.world()
            .entity(hull)
            .get::<Boat>()
            .expect("a hull somebody has the helm of")
            .kind
    }

    fn cut_for(app: &mut App, hull: Entity) -> bool {
        app.world().entity(hull).contains::<OpenHull>()
    }

    /// The one hull of a kind in the match — once the boat is down there are
    /// two, and a test usually means one of them by name.
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

    /// One press of the gunwale key, released again afterwards — which at a
    /// ship's helm is the whole of putting the boat in the water, this app
    /// having no server to ask.
    fn press_board(app: &mut App) {
        hold(app, KeyCode::KeyF);
        run_frames(app, 1);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::KeyF);
        run_frames(app, 1);
    }

    fn stood_at(app: &mut App) -> f32 {
        app.world_mut()
            .query_filtered::<&Transform, With<Player>>()
            .single(app.world())
            .expect("a player aboard")
            .translation
            .y
    }

    /// A hull's kind is settled the moment it is rigged and never changes:
    /// what it is dressed as, sailed as, and stood upon are one answer.
    #[test]
    fn a_hull_is_rigged_and_sailed_as_the_kind_it_was_spawned() {
        let mut app = test_app();
        let ship = hull(&mut app);
        assert_eq!(rigged(&mut app), BoatKind::Sloop);
        assert_eq!(sailed(&mut app), BoatKind::Sloop);
        assert!(!cut_for(&mut app, ship), "a closed hull needs no hole");
        assert_eq!(stood_at(&mut app), SHIP.helm_deck);
    }

    /// And the rowboat's half of the same answer, reached the only way one
    /// enters a world: lowered from the ship by the gunwale key. The hole is
    /// the half nothing else would notice going missing — an open boat that
    /// was never cut for is drawn with the sea standing in its bilges, which
    /// is a thing to see rather than a thing to fail on.
    #[test]
    fn a_lowered_boat_is_rigged_as_a_rowboat_and_the_sea_is_cut_for_it() {
        let mut app = test_app();
        press_board(&mut app);

        let tender = hull_rigged(&mut app, BoatKind::Rowboat);
        assert_eq!(
            app.world().entity(tender).get::<Boat>().map(Boat::kind),
            Some(BoatKind::Rowboat),
            "the boat that went in the water is not sailed as one"
        );
        assert!(cut_for(&mut app, tender), "an open boat wants its hole");
        assert_eq!(stood_at(&mut app), ROWBOAT.helm_deck);
        // And the ship it came off keeps its own kind and its own deck: a
        // hull is rigged once, at spawn, and nothing about the lowering
        // reaches back to it.
        let ship = hull_rigged(&mut app, BoatKind::Sloop);
        assert!(!cut_for(&mut app, ship), "a closed hull needs no hole");
    }

    /// The sea carries one hole and a world can hold several open hulls, so
    /// which one gets it is a choice — and the one the player is aboard is
    /// the only one whose bilges the camera is actually looking into.
    #[test]
    fn the_hole_in_the_sea_follows_the_boat_the_player_is_aboard() {
        let mut app = test_app();
        // A sea to write to. The real one is dressed by the terrain plugin,
        // which needs a window to draw in; this is that setup with nothing
        // but the material and its depth window, which is all the hole is
        // written through.
        app.init_asset::<Image>().init_asset::<sea::SeaMaterial>();
        let depth = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(sea::depth_image());
        let material = app
            .world_mut()
            .resource_mut::<Assets<sea::SeaMaterial>>()
            .add(sea::SeaMaterial {
                base: StandardMaterial::default(),
                extension: sea::SeaExtension::new(depth.clone(), Vec2::ZERO),
            });
        app.insert_resource(sea::DepthWindow::new(depth, material.clone(), Vec2::ZERO));

        // Two open boats a long way apart — the bare hulls the system reads,
        // no models needed — and the player put aboard the further one.
        let open = ROWBOAT.open_footprint.expect("the rowboat is an open boat");
        let (first, carrier) = (Vec2::new(-300.0, 0.0), Vec2::new(300.0, 0.0));
        for at in [first, carrier] {
            app.world_mut()
                .spawn((Vessel, open, Transform::from_xyz(at.x, 0.0, at.y)));
        }
        let aboard = app
            .world_mut()
            .query_filtered::<(Entity, &Transform), With<OpenHull>>()
            .iter(app.world())
            .find(|(_, pose)| pose.translation.xz() == carrier)
            .map(|(hull, _)| hull)
            .expect("the boats were just spawned");
        let player = app
            .world_mut()
            .query_filtered::<Entity, With<Player>>()
            .single(app.world())
            .expect("a match should have a player in it");
        app.world_mut().entity_mut(player).insert(ChildOf(aboard));
        run_frames(&mut app, 1);

        // The hole's centre is the footprint's, a fraction of a metre abaft
        // the hull's own origin — so which boat it is cut for is not in
        // question at six hundred metres.
        let hole = app
            .world()
            .resource::<Assets<sea::SeaMaterial>>()
            .get(&material)
            .expect("the sea's material")
            .extension
            .hole;
        let cut_at = Vec2::new(hole.x, hole.y);
        assert!(
            cut_at.distance(carrier) < open.abaft + 0.1,
            "the sea was cut at {cut_at}, not under the boat the player is in"
        );
    }

    /// Which side the ship's boat goes in on: the shoreward one, when the
    /// ground under either beam says which that is.
    #[test]
    fn the_tender_goes_in_on_the_shoreward_side() {
        let ground = test_ground();
        // Lying off the island's own slope, near enough in that the two
        // beams stand over ground of visibly different heights. Facing
        // *away* from the island, so that the shoreward side is starboard
        // and an answer of port would be the fallback rather than the rule.
        let at = Vec2::new(TEST_ISLAND_REACH - 10.0, 0.0);
        let ship = Transform::from_xyz(at.x, 0.0, at.y)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::PI));
        let (berth, heading) = tender_berth(&ship, Some(&ground));
        assert!(
            berth.x < at.x,
            "the boat went in at {berth}, on the seaward side of a ship at {at}"
        );
        // Clear of both hulls' planking, and pointed the way the ship lies:
        // a tender is lowered over the side, not aimed.
        let abeam = berth.distance(at);
        assert!(
            abeam > (SHIP.beam + ROWBOAT.beam) / 2.0,
            "the boat went in {abeam} m abeam, inside the ship's own planking"
        );
        // Half a turn is half a turn whichever way it is written down.
        assert!(
            (heading.abs() - std::f32::consts::PI).abs() < 1e-5,
            "the boat was laid at {heading}, not along a ship lying at PI"
        );
    }

    #[test]
    fn the_tender_goes_in_to_port_when_no_ground_says_otherwise() {
        // Open water, or ground that has not arrived: no side is shoreward,
        // and port is the habit sailors would expect.
        let ship = Transform::IDENTITY;
        let (berth, _heading) = tender_berth(&ship, None);
        assert!(
            berth.x < 0.0,
            "with nothing to go on the boat went in to starboard, at {berth}"
        );
        assert_eq!(berth.y, 0.0, "the boat went in fore or aft of abeam");
    }

    #[test]
    fn the_rowboat_ships_its_oars_and_pulls_them() {
        // The two states a boat with oars in it has, and the position the
        // game asks for each at. Renaming or reordering an action in Blender
        // is a keystroke, and the boat that came back would row with its
        // oars lying in the bilges.
        let clips = clip_names(ROWBOAT_MODEL);
        assert_eq!(clips.get(STOWED).map(String::as_str), Some("stowed"));
        assert_eq!(clips.get(STROKE).map(String::as_str), Some("stroke"));
        assert_eq!(clips.len(), 2);
    }

    /// Stands in for the stroke clip the file would have brought, so the
    /// seeking can be tested without a render app to load a glTF with. One
    /// second long, which makes a seek time a fraction of the cycle read
    /// directly.
    const CYCLE: f32 = 1.0;

    /// What kind of hull the player is aboard, off the hull's own [`Rigged`]
    /// — `None` on their own feet.
    fn rigged_kind_aboard(app: &mut App) -> Option<BoatKind> {
        let aboard = app
            .world_mut()
            .query_filtered::<&ChildOf, With<Player>>()
            .single(app.world())
            .ok()?
            .parent();
        app.world()
            .entity(aboard)
            .get::<Rigged>()
            .map(|kind| kind.0)
    }

    /// A match with the ship's boat in the water and the player at its oars,
    /// a stand-in stroke clip loaded, and an animation player under the scene
    /// where the loader would have put one — returned alongside the app,
    /// ready to be read.
    ///
    /// Lowered with the gunwale key rather than conjured, that being the only
    /// way a dinghy enters a world: a hull spawned behind the game's back
    /// would be a rowboat no player could have got into.
    fn rowing_app() -> (App, Entity) {
        let mut app = test_app();
        tap(&mut app, KeyCode::KeyF);
        run_frames(&mut app, 1);
        assert_eq!(
            rigged_kind_aboard(&mut app),
            Some(BoatKind::Rowboat),
            "the ship's boat never went over the side"
        );

        let (cycle, stroke) = {
            let rowing = app.world().resource::<Rowing>();
            (rowing.cycle.clone(), rowing.stroke)
        };
        let mut clip = AnimationClip::default();
        clip.set_duration(CYCLE);
        app.world_mut()
            .resource_mut::<Assets<AnimationClip>>()
            .insert(&cycle, clip)
            .expect("the stand-in clip goes where the real one would");

        let scene = app
            .world_mut()
            .query_filtered::<Entity, With<Oared>>()
            .single(app.world())
            .expect("the lowering hung no rowboat scene");
        let rower = app
            .world_mut()
            .spawn((AnimationPlayer::default(), ChildOf(scene)))
            .id();
        run_frames(&mut app, 2);
        assert!(
            app.world()
                .entity(rower)
                .get::<AnimationPlayer>()
                .expect("a rower")
                .animation(stroke)
                .is_some(),
            "the oars were never conducted"
        );
        (app, rower)
    }

    /// The stroke's seek time and weight and the stowed pose's weight, as
    /// the oars are playing them.
    fn oars_of(app: &mut App, rower: Entity) -> (f32, f32, f32) {
        let rowing = app.world().resource::<Rowing>();
        let (stroke, stowed) = (rowing.stroke, rowing.stowed);
        let player = app
            .world()
            .entity(rower)
            .get::<AnimationPlayer>()
            .expect("a rower");
        let pulling = player.animation(stroke).expect("the stroke is playing");
        let resting = player
            .animation(stowed)
            .expect("the stowed pose is playing");
        assert!(
            (pulling.weight() + resting.weight() - 1.0).abs() < 1e-4,
            "the blend does not add up: {} and {}",
            pulling.weight(),
            resting.weight()
        );
        (pulling.seek_time(), pulling.weight(), resting.weight())
    }

    /// Frames of holding a pull for the way to be indistinguishable from
    /// settled — over eight of [`ROWBOAT`]'s way response, at the sixteen
    /// milliseconds a frame the headless clock steps.
    const UNDER_WAY: usize = 400;

    #[test]
    fn the_oars_lie_stowed_at_rest_and_a_settled_pull_covers_its_own_water() {
        // The no-slip property, which is why the stroke is tuned against a
        // distance at all: settled, one turn of the cycle is PULL metres of
        // water, so the blades cannot be seen to skate under the hull. It is
        // a steady-state reading and only that — coming up to speed the hull
        // is slower than the pace the rower is already keeping, which is a
        // boat gathering way and not a fault.
        //
        // Abeam, so PULL is the whole of the pull: a wind with any of itself
        // along the heading moves what a turn makes good, which is the next
        // test's business rather than this one's.
        let (mut app, rower) = rowing_app();
        wind_abeam(&mut app, 7.0);

        // At rest, nothing asked: the stowed pose carries all the weight.
        let (_, pulling, stowed) = oars_of(&mut app, rower);
        assert_eq!((pulling, stowed), (0.0, 1.0), "the oars are out at rest");

        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, UNDER_WAY);
        let from = boat(&mut app).translation;
        let turns = turns_pulled(&mut app, rower, 60);
        let covered = boat(&mut app).translation.xz().distance(from.xz());

        assert!(turns > 0.2, "only {turns:.3} of a turn to measure against");
        let (_, pulling, _) = oars_of(&mut app, rower);
        assert!(pulling > 0.9, "a hull under oars is only {pulling} pulling");
        let per_turn = covered / turns;
        assert!(
            (per_turn - PULL).abs() < 0.05,
            "{covered:.2} m in {turns:.3} turns is {per_turn:.2} m to a pull, not {PULL}"
        );
        assert!(
            app.world()
                .entity(rower)
                .get::<AnimationPlayer>()
                .expect("a rower")
                .animation(app.world().resource::<Rowing>().stroke)
                .expect("the stroke is playing")
                .is_paused(),
            "the stroke is running on its own clock"
        );

        // Oars in and the glide run off: the boat settles back to stowed —
        // exactly, both ways being snapped, so a resting boat holds one pose
        // frame after frame.
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::ArrowUp);
        tap(&mut app, KeyCode::ArrowDown);
        run_frames(&mut app, SETTLED);
        assert_eq!(way_on(&mut app), 0.0);
        let (_, pulling, stowed) = oars_of(&mut app, rower);
        assert_eq!((pulling, stowed), (0.0, 1.0), "the oars were left out");
    }

    /// Turns of the stroke pulled over the next `frames`, signed, and counted
    /// frame by frame so a run of more than one turn reads as more than one:
    /// the seek is a place on a single turn of the cycle, so its difference is
    /// only a step if the step is taken the shorter way round.
    fn turns_pulled(app: &mut App, rower: Entity, frames: usize) -> f32 {
        let mut last = oars_of(app, rower).0 / CYCLE;
        let mut turns = 0.0;
        for _ in 0..frames {
            run_frames(app, 1);
            let now = oars_of(app, rower).0 / CYCLE;
            let step = now - last;
            turns += step - step.round();
            last = now;
        }
        turns
    }

    #[test]
    fn a_headwind_shortens_the_pull_and_leaves_the_cadence_alone() {
        // The lie this is here to catch. Turn the stroke by the water covered
        // and a wind that slows the hull slows the blades with it, so a rower
        // fighting a gale reads as one rowing lazily. Holding the cadence flat
        // is what answers it: the same strokes a minute either way, and less
        // ground for each of them.
        //
        // Measured past the ramp, so what the two runs are compared on is the
        // pace held rather than the seconds before either hull had way on.
        let rowed = |wind: Option<f32>| {
            let (mut app, rower) = rowing_app();
            match wind {
                Some(speed) => wind_ahead(&mut app, speed),
                None => wind_abeam(&mut app, 9.0),
            }
            hold(&mut app, KeyCode::ArrowUp);
            run_frames(&mut app, 120);
            let from = boat(&mut app).translation;
            let turns = turns_pulled(&mut app, rower, 120);
            let covered = boat(&mut app).translation.xz().distance(from.xz());
            (covered, turns)
        };

        let (easy, easy_turns) = rowed(None);
        let (hard, hard_turns) = rowed(Some(9.0));
        assert!(easy > 1.0, "only {easy:.2} m rowed with the wind abeam");
        assert!(
            hard < easy * 0.5,
            "a hard headwind cost only {:.2} m of {easy:.2}",
            easy - hard
        );
        assert!(
            (easy_turns - hard_turns).abs() < 0.01,
            "the same rower pulled {easy_turns:.3} turns in the clear and \
             {hard_turns:.3} into the wind"
        );
        // And really did pull them, rather than both readings being a boat
        // that never got going — more than a whole turn, so the counting
        // above is being asked to carry past the top of the cycle as well.
        assert!(easy_turns > 1.0, "only {easy_turns:.3} of a turn rowed");
    }

    #[test]
    fn a_hull_carried_astern_still_pulls_its_stroke_ahead() {
        // The far corner of the same rule: in the hardest wind the sky has,
        // dead on the bow, the oars lose and the hull is carried backwards.
        // The rower is still pulling *ahead*, the stroke coming round the way
        // a stroke comes round rather than backing water.
        let (mut app, rower) = rowing_app();

        // With way already on first, which is the leg that can actually catch
        // the failure. Rowing up into a headwind from rest, the hull never
        // goes anywhere but astern and there is no sign to get wrong; take the
        // wind off a hull already running and the stroke has a whole second of
        // way it has to keep faith with. Turning the phase by the water made
        // good over the drive commanded, those two part company for exactly
        // that second, and the blades span backwards at some multiple of the
        // cadence while the hull was still surging ahead.
        wind_astern(&mut app, 12.0);
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 60);
        assert!(way_on(&mut app) > 2.0, "the boat never got under oars");

        wind_ahead(&mut app, 16.0);
        let mut surging = 0;
        for _ in 0..120 {
            let turned = turns_pulled(&mut app, rower, 1);
            let way = way_on(&mut app);
            if way > 0.0 {
                surging += 1;
                assert!(
                    turned >= 0.0,
                    "the stroke ran {turned:.4} turns backwards on a hull \
                     still making {way:.2} m/s ahead"
                );
            }
        }
        assert!(surging > 30, "the way died too fast to prove anything");

        // And settled into being beaten: the hull slides astern and the oars
        // go on pulling ahead at the pace they always keep.
        let from = boat(&mut app).translation;
        let turns = turns_pulled(&mut app, rower, 60);
        let bow = boat(&mut app).forward().xz().normalize();
        let made = bow.dot(boat(&mut app).translation.xz() - from.xz());
        assert!(made < -0.05, "the wind gave up {made:.2} m of ground");
        assert!(
            turns > 0.2,
            "the blades turned {turns:.3} — or barely at all"
        );
    }

    #[test]
    fn backing_water_pulls_the_stroke_backwards() {
        // The same cycle run the other way, without a second clip existing —
        // the phase wraps below zero and comes round from the top of the
        // turn, so the seek is the forward test's reading subtracted from a
        // whole one rather than merely being somewhere past the half.
        let (mut app, rower) = rowing_app();
        let from = boat(&mut app).translation;
        hold(&mut app, KeyCode::ArrowDown);
        run_frames(&mut app, 30);

        let backed = boat(&mut app).translation.xz().distance(from.xz());
        assert!(backed > 0.1, "not enough water backed over to tell");
        let (seek, pulling, _) = oars_of(&mut app, rower);
        assert!(pulling > 0.0, "backing water never got the oars out");
        let expected = (-backed / PULL).rem_euclid(1.0) * CYCLE;
        assert!(
            (seek - expected).abs() < 0.01,
            "{backed:.2} m backed over left the stroke at {seek:.3} s, not {expected:.3}"
        );
    }

    #[test]
    fn a_boat_the_helm_has_left_turns_no_stroke() {
        // The chart and the pause menu take the helm and leave the world
        // running behind them — see [`Helm`] — so [`steer`] stops while the
        // hull keeps its last way written on it. Measuring the water covered
        // is what makes that harmless: the hull sits still, so the stroke
        // does. Reading [`Boat::way`] instead rowed on for as long as the
        // paper was up, which is what this is here to catch.
        let (mut app, rower) = rowing_app();
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 30);
        assert!(way_on(&mut app) > 0.5, "the boat never got under oars");

        set_helm(&mut app, Helm::Chart);
        let held = boat(&mut app).translation;
        let (seek, out, _) = oars_of(&mut app, rower);
        run_frames(&mut app, 60);

        assert_eq!(
            boat(&mut app).translation.xz(),
            held.xz(),
            "the hull sailed on behind the chart"
        );
        assert!(
            way_on(&mut app) > 0.5,
            "the boat lost its way, so this proves nothing"
        );
        let (still, pulling, _) = oars_of(&mut app, rower);
        assert_eq!(still, seek, "the oars pulled a boat that covered no water");
        // And are not snatched in either: the hull covers no water but the
        // pull is still asked for, so the oars stay out — held mid-stroke
        // like everything else behind the paper.
        assert!(
            pulling >= out,
            "the oars shipped themselves behind the chart: {out} to {pulling}"
        );

        // The harder half of the same rule: a boat furled but still gliding
        // is *not* at rest, and the paper must not decide otherwise. Judged
        // on the water covered rather than on the hull's own way, this
        // shipped the oars completely — the glide is what would have carried
        // them, and behind the chart there is no glide to read.
        set_helm(&mut app, Helm::Sailing);
        tap(&mut app, KeyCode::ArrowDown);
        run_frames(&mut app, 2);
        let gliding = way_on(&mut app);
        assert!(
            gliding.abs() > 0.5 && !sails_are_set(&mut app),
            "the boat is not furled and gliding, so this proves nothing"
        );
        let (_, out, _) = oars_of(&mut app, rower);

        set_helm(&mut app, Helm::Chart);
        run_frames(&mut app, 60);
        let (_, pulling, _) = oars_of(&mut app, rower);
        assert_eq!(
            way_on(&mut app),
            gliding,
            "the glide ran off behind the chart"
        );
        assert!(
            pulling >= out,
            "the oars were shipped on a boat still carrying way: {out} to {pulling}"
        );
    }

    #[test]
    fn the_figure_aboard_is_not_taken_for_oars() {
        // The loader puts an animation player on anything animated it finds,
        // and a figure standing in the rowboat is under the same hull — only
        // the players under the rowboat's own scene are the oars'.
        let (mut app, _) = rowing_app();
        let figure = app
            .world_mut()
            .query_filtered::<Entity, With<Player>>()
            .single(app.world())
            .expect("a match should have a player in it");
        let stranger = app
            .world_mut()
            .spawn((AnimationPlayer::default(), ChildOf(figure)))
            .id();
        run_frames(&mut app, 2);

        assert!(
            app.world().entity(stranger).get::<Rower>().is_none(),
            "the walker's own player was taken for oars"
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
        // The floor is where the ramp would meet a wind of nothing — no air
        // on the water actually makes it — the ceiling is what a blow earns,
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
    fn the_oars_gain_downwind_and_lose_more_into_the_wind() {
        // A beam wind is no wind at all to a rowed hull, there being no polar
        // here and nothing but the component along the heading; the gain is
        // the smaller of the two, a head sea costing more than a following one
        // pays; and above the saturation the sky is only making more sea.
        let bow = Vec2::NEG_Y;
        let (with, against) = ROWING_WINDAGE;
        assert_eq!(row_drive(bow, Vec2::ZERO), 1.0);
        assert_eq!(row_drive(bow, Vec2::new(WIND_SATURATES, 0.0)), 1.0);
        assert_eq!(row_drive(bow, bow * WIND_SATURATES), 1.0 + with);
        assert_eq!(row_drive(bow, -bow * WIND_SATURATES), 1.0 - against);
        assert_eq!(row_drive(bow, bow * 40.0), 1.0 + with);
        assert_eq!(row_drive(bow, -bow * 40.0), 1.0 - against);

        // And the shape of the curve between, which is a calibration rather
        // than an incidental: squared, an everyday breeze takes off a quarter
        // of what a strong one does, so the run ashore is not taxed a little
        // on every trip. This restates the square rather than deriving
        // anything from it — what it is for is that trading the square for a
        // gentler curve cannot happen quietly.
        let everyday = 1.0 - row_drive(bow, -bow * (WIND_SATURATES * 0.5));
        let blow = 1.0 - row_drive(bow, -bow * WIND_SATURATES);
        assert!(
            (everyday - blow * 0.25).abs() < 1e-6,
            "half the wind costs {everyday:.3} against the blow's {blow:.3}"
        );
    }

    #[test]
    fn there_is_a_wind_no_rower_can_pull_into() {
        // What the whole shape is for, and worth pinning where it falls: the
        // oars stop making ground a little over eleven metres a second, which
        // is a strong breeze — up among the weather's occasional excursions
        // and well clear of the everyday, so it is a spell somebody waits out
        // or works around rather than a tax on getting ashore.
        let bow = Vec2::NEG_Y;
        assert!(row_drive(bow, -bow * 11.0) > 0.0, "beaten by a fresh wind");
        assert!(
            row_drive(bow, -bow * 11.5) < 0.0,
            "pulling into a near gale"
        );

        // And the way round it is to stop pointing straight at it. No no-go
        // zone is written down anywhere; the cosine leaves one anyway, and
        // leaves a rower who cannot pull into the sky's hardest wind still
        // working to windward across it. Sixty degrees off the eye of that
        // same wind halves what reaches the bow, and the hull makes way
        // again — a slow way, and the only way upwind there is.
        let sixty_off = Vec2::new(f32::sqrt(3.0) / 2.0, 0.5) * 16.0;
        assert!(
            (bow.dot(sixty_off) + 8.0).abs() < 1e-4,
            "the wind is not sixty degrees off the bow"
        );
        assert!(
            row_drive(bow, sixty_off) * ROWBOAT.speed > 1.0,
            "angling off the eye of a near gale wins nothing"
        );
    }

    #[test]
    fn no_wind_can_be_sailed_into() {
        // The rule the whole game hangs off: dead into the eye of any named
        // wind, set sails carry nothing — there is no wind on screen a boat
        // can sail straight at.
        for blowing in [sea::WIND_NAMED, 2.0, 7.0, WIND_SATURATES] {
            let wind = Vec2::new(0.0, -blowing);
            assert_eq!(sail_drive(-wind.normalize(), wind), 0.0);
        }
    }

    #[test]
    fn air_too_faint_to_name_drives_nothing_much() {
        // Below the named bar the screen shows no direction, so the sails may
        // not act on one either: no drive at all in a dead calm — the console
        // can still order one — and below the bar only the fading remnant the
        // compass's ink ramp leaves, on the best point of sail or any other.
        // The weather itself never blows this softly; see [`sail_drive`].
        for bow in [Vec2::X, Vec2::NEG_X, Vec2::Y, Vec2::new(0.7, -0.7)] {
            assert_eq!(sail_drive(bow, Vec2::ZERO), 0.0);
        }
        let breath = Vec2::new(0.0, 0.3);
        let running = breath.normalize();
        assert_eq!(
            sail_drive(running, breath),
            strength(0.3) * (0.3 / sea::WIND_NAMED)
        );
        assert!(sail_drive(running, breath) < sail_drive(running, Vec2::new(0.0, 0.6)));
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

        // And a hull that stops being anybody's here furls whatever it was
        // carrying: the sails go up again, then the [`Boat`] comes off, as
        // stepping ashore takes it off — see [`Fleet::hand_back`]. Nothing
        // is left to say the canvas is set, so it is not drawn set, and the
        // hulls the wire tells us about are all in this state.
        tap(&mut app, KeyCode::ArrowUp);
        assert_eq!(sail_shown(&mut app), Visibility::Inherited);
        let hull = app
            .world_mut()
            .query_filtered::<Entity, With<Boat>>()
            .single(app.world())
            .expect("a match should have a boat in it");
        app.world_mut().entity_mut(hull).remove::<Boat>();
        run_frames(&mut app, 1);
        assert_eq!(
            sail_shown(&mut app),
            Visibility::Hidden,
            "an abandoned hull was left riding under canvas"
        );
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

    /// Puts the boat down at a spot, pointing a way — a `focus` in little.
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
        // What a `focus` on an island leaves behind, and the case that says
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
