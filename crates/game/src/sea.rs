//! The swell: the waves the sea wears, and everything that has to agree on
//! them.
//!
//! The sea is drawn as low-poly waves — real geometry, displaced in the vertex
//! shader and shaded flat, so a wave is a run of tilting facets rather than a
//! normal-mapped shimmer. On water this matte a normal map would barely read:
//! there is no specular for it to perturb, and what sells the motion is facets
//! changing tone as they tilt and the waterline creeping up every beach.
//!
//! The sea belongs to the weather now. The server owns the wind — one
//! authority, so every player in a world is under the same sky — and tells
//! each client as it changes; what arrives is a [`Forecast`], and what is
//! drawn is [`SeaConditions`], which [`settle_conditions`] eases towards it
//! so a quantised update lands as weather rather than as a step. Wind speed
//! scales every amplitude between a breathing calm and a near-gale sea;
//! wind direction is where the wave trains point, through the re-aiming
//! dance [`REAIM`] explains.
//!
//! The swell is two regimes crossfaded by depth. Over open water it is three
//! sines crossing at odd angles — see [`WAVES`]. In the shallows those hand
//! over to a single longer shore wave whose *phase is the depth itself*: its
//! crests are the depth contours, which run parallel to whatever shore they
//! approach, so waves wrap into bays and meet every beach face-on with no
//! refraction computed. That is why the shore wave has no heading — steering a
//! directional wave by depth means integrating phase along its path, which has
//! no honest local answer. Its height is capped by the water it stands in, the
//! way breaking caps a real one, and the fragment shader paints foam where the
//! cap is biting.
//!
//! The open sea breaks too, once it is blowing hard enough: [`WHITECAP`] puts
//! white down the leading faces of the swell wherever the three trains heap
//! high enough together. It is deliberately never told what the wind is doing —
//! the wind is in the amplitudes already, so a height to clear is all it takes
//! for caps to arrive with the weather.
//!
//! Depth reaches the shader through [`DepthWindow`]: a coarse byte-per-texel
//! picture of the water depth around the camera, refilled a few rows a frame
//! from the ground chunks the server has sent.
//!
//! Three parties have to agree on where the water stands at a moment: the
//! shader displacing the sea mesh, the boat riding on it, and the markers other
//! players stand as. The parameters live once here and reach the shader through
//! a uniform; the *formula* — [`swell`] — is written twice, here and in
//! `assets/shaders/sea.wgsl`, and the two must be kept the same. So is time:
//! the shader reads `globals.time`, which Bevy fills from
//! `Time::elapsed_secs_wrapped`, so that is what every Rust caller must pass.
//! Depth is deliberately loose — the shader reads the windowed texture and the
//! boat asks the ground exactly, differing by at most a texel of interpolation
//! in water where the swell is smallest.
//!
//! The camera deliberately does *not* ride the swell. Its focus stays on the
//! flat waterline, so the world bobs around a steady eye rather than the
//! whole picture heaving with the boat.

use std::f32::consts::TAU;

use bevy::asset::RenderAssetUsages;
use bevy::image::{Image, ImageSampler};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, Extent3d, TextureDimension, TextureFormat};

use protocol::ground::LIT_ALL_DAY;

use crate::camera::MapCamera;
use crate::terrain::Ground;

/// Displaces the sea's vertices and shades the result — see the module doc,
/// and the file itself, which carries the other copy of [`swell`].
const SHADER: &str = "shaders/sea.wgsl";

/// The open sea's swell, as components: bearing off the wind in radians,
/// wavelength in metres, amplitude in metres under [`REFERENCE_WIND`]. Three
/// of them, because one sine reads as a marching pattern and two as a grid;
/// three, crossing at odd angles, is the fewest that reads as water. The
/// bearings are deliberately odd and unequal, and all within a right angle
/// of the wind, so the sea visibly belongs to it.
///
/// The wavelengths stay well above twice [`SPACING`], or a wave would fall
/// between the mesh's vertices and alias into shimmer. The amplitudes are
/// what a moderate breeze wears — [`amplitude_scale`] is what takes them
/// down to a calm and up to a blow — and stay gentle because the boat rides
/// this height with no easing: what the water does, the hull does.
const WAVES: [(f32, f32, f32); 3] = [(0.0, 43.0, 0.22), (0.85, 24.0, 0.12), (-1.17, 14.0, 0.065)];

/// The wind the amplitudes in [`WAVES`] are written for, in metres per
/// second: a moderate breeze, and the sea this file drew before it ever
/// heard a forecast.
const REFERENCE_WIND: f32 = 7.0;

/// The wind a client assumes until a server says otherwise: the reference
/// breeze, from a bearing nothing else in the world uses. Every played frame
/// replaces this within the first second of a session — the server sends the
/// weather straight after the welcome — so what it really serves is the
/// moments before that, and the headless tests, which want one deterministic
/// sea and no server at all.
const ASSUMED_WIND: Vec2 = Vec2::new(6.762, 1.813);

/// How the sea's height follows the wind: a factor on every amplitude, `1.0`
/// at [`REFERENCE_WIND`]. Grows faster than linearly, the way seas do, but
/// far short of the physical square-and-more — a real gale's sea would bury
/// this world's islands. Floored just off zero so a flat calm still carries
/// a breathing remnant of swell: a sea gone perfectly still reads as a
/// rendering fault, not as weather.
fn amplitude_scale(wind: f32) -> f32 {
    let relative = (wind / REFERENCE_WIND).max(0.0);
    (0.12 + 0.88 * relative * relative.sqrt()).min(2.4)
}

/// How long the sea takes to follow the wind, in seconds — the time constant
/// of the ease [`settle_conditions`] runs. Real seas lag real wind by far
/// more, but what this buys is not realism: it is the server's occasional
/// quantised updates arriving as weather rather than as steps.
const SEA_RESPONSE: f32 = 12.0;

/// Below this, in metres per second, a wind is too slack to name a
/// direction, and everything drawn off the true wind says so together:
/// the wave trains hold the heading they had (see [`settle_conditions`]),
/// the compass takes its arm off the card, and sails carry nothing — one
/// bar, so the card, the water and the hull never disagree about whether
/// there is a wind. The pennant is the deliberate exception: it flies the
/// *apparent* wind, which is a different wind, and keeps its own lower bar
/// for hanging limp.
///
/// The weather itself never blows this softly — the forecast holds a light
/// air even in its calms, see [`protocol::ToClient::Weather`] — and the
/// drawn wind eases by turning rather than by crossing the middle, see
/// [`veered`], so what the bar actually catches is a console-ordered flat
/// calm and the tail of the drawn wind dying into one.
pub(crate) const WIND_NAMED: f32 = 0.5;

/// How far the wind may veer, in radians, before a wave slot is re-aimed —
/// and the fade that re-aiming hides behind, as a time constant in seconds.
///
/// A drawn wave's direction can never simply turn: its phase is position
/// along its heading, so rotating a live wave rephases the whole ocean at
/// once — every crest in view lurches sideways, further the further from the
/// origin it stands. The only clean move is through zero: a slot that needs
/// to re-aim fades its amplitude out, swings while it is invisible, and
/// fades back. The threshold is hysteresis, so a wandering wind re-aims a
/// slot occasionally rather than continuously; the slots' unequal bearings
/// off the wind mean they cross it at different moments, so the sea never
/// goes flat all at once.
const REAIM: (f32, f32) = (0.45, 4.0);

/// Gravity, for the dispersion relation: deep-water waves travel at
/// `ω = sqrt(g·k)`, so the long swell outruns the short chop and the surface
/// never repeats itself the way a single-speed pattern would.
const GRAVITY: f32 = 9.81;

/// How much more steeply the facets are *lit* than the water actually
/// slopes. The shading and the geometry pull in opposite directions: a swell
/// gentle enough for the boat to ride and the beaches to keep their
/// waterlines tilts its facets a few degrees, which under this sky is almost
/// no tone change at all — honest amplitudes were tried first and the sea
/// read as flat; amplitudes big enough to read lit honestly were a storm,
/// with the hull heaving metres and the waterline marching up the beaches.
/// So the fragment shader exaggerates only the slope the *light* sees, and
/// the surface everything rides stays calm.
const SHADING_TILT: f32 = 4.0;

/// The depths across which the open sea hands over to the shore wave, in
/// metres: the crossfade starts as the water shallows through the first and
/// is complete by the second. The whole ocean floor is only [`OCEAN_DEPTH`]
/// down, so "deep" here is a few metres — what matters is that the handover
/// spans the islands' skirts, where the criss-cross of the open sea starts
/// to look wrong marching over a beach.
///
/// [`OCEAN_DEPTH`]: protocol::ground::OCEAN_DEPTH
const SHOAL: (f32, f32) = (6.5, 2.5);

/// The depths across which the water goes blind, in metres: the surface's
/// alpha climbs from the material's own as the bottom passes the first, and
/// by the second — [`ANCHOR_DEPTH`], on purpose — reaches the foam's own
/// near-opacity. So the open sea's bed is never read at all, the eye can
/// take the anchorage rule off the water (ground showing through is water
/// the anchor holds in), and what *swims* under blind water is a last few
/// per cent of shadow rather than erased outright.
///
/// [`ANCHOR_DEPTH`]: protocol::ground::ANCHOR_DEPTH
const MURK: (f32, f32) = (5.0, protocol::ground::ANCHOR_DEPTH);

/// Metres of *depth* between one shore crest and the next. The shore wave's
/// phase is the depth itself, so this is its wavelength measured down the
/// beach profile rather than across the water — on a typical island skirt it
/// comes out a few tens of metres between crests, tightening where the
/// bottom steepens and stretching where a flat bay hardly deepens at all.
const CREST_EVERY: f32 = 1.6;

/// Seconds from one shore crest to the next — unhurried, and longer than any
/// of the open sea's periods: the shallows collect the deep swell into
/// fewer, larger arrivals, which is precisely what a beach does.
const SHORE_PERIOD: f32 = 7.5;

/// Height of the shore wave over water deep enough not to break it, metres.
/// A little larger than any single deep wave, for the same reason the
/// period is longer: what arrives at a beach is the swell gathered up.
const SHORE_AMPLITUDE: f32 = 0.3;

/// The tallest wave a depth of water can carry, as a slope: a wave breaks
/// where its height outruns `BREAK_SLOPE` times the depth under it, so this
/// is what caps the shore wave as the bottom comes up — and where the cap
/// bites is where the foam is painted.
const BREAK_SLOPE: f32 = 0.45;

/// What survives of a broken wave to run up the sand, in metres: the cap on
/// the shore wave's height never quite closes to zero, so the waterline
/// itself still breathes instead of the swell dying politely offshore.
const RUNUP: f32 = 0.06;

/// How much of a crest wears foam: the threshold on the shore wave's sine
/// above which a breaking crest is painted white. Higher is thinner bands.
const FOAM_CREST: f32 = 0.35;

/// The least the bottom must be rising, as a grade, for a breaking crest to
/// foam. Breaking wants a face to trip over: without this, a tidal flat an
/// inch deep is "breaking" across its whole area at once, and because depth
/// is phase, the whole flat crests in unison — sheets of white flashing
/// over every low spit and lagoon in view. Gating on the depth gradient
/// pins the foam to where the bottom actually comes up — the shelf edge,
/// the beach face — which is where the white line lives in an aerial
/// photograph too.
const FOAM_SLOPE: f32 = 0.02;

/// What must lie behind a breaker for it to be one: this far down the
/// bottom's slope, at least this much water. A breaking wave is deep water
/// arriving somewhere too shallow for it, so a face with only shallows at
/// its back — a ripple in a lagoon floor, the bank of a tidal creek — has
/// nothing to break. The slope gate alone let those through: a lagoon
/// floor undulates past [`FOAM_SLOPE`] in plenty of places, and the lagoon
/// wore slabs of foam no real one would. Metres to look, then metres of
/// water that must be found there.
const FOAM_FEED: (f32, f32) = (24.0, 1.2);

/// How far the open sea's crests wander off straight, in metres of sideways
/// displacement — the amplitude of [`bend`].
///
/// Three sines summed have dead straight parallel crests, because that is
/// what a sum of plane waves is. On the water itself it half passes, the
/// facets being subtle; the moment anything draws a hard line on the surface
/// — a whitecap, a foam edge — the ocean turns into graph paper. Adding a
/// fourth and a fifth train does not help: more plane waves are still plane
/// waves, and all it buys is a busier lattice.
///
/// So the swell is read through a slowly bending picture of the plane instead
/// — a domain warp, the same trick that stops the islands' coastlines coming
/// out as smooth noise blobs. The crests meander, cross each other at angles
/// that change along their length, and no two stretches of the same wave look
/// alike, all without a single extra train being summed.
///
/// The size is bounded by the mesh rather than by taste: the bend adds to
/// every train's phase gradient, which compresses wavelengths, and a train
/// compressed under three vertices to a crest aliases into shimmer. What
/// keeps this well clear of that is bending each train by a fraction of its
/// own wavelength rather than by metres — see [`SeaConditions::swell`] — and
/// `the_bend_leaves_the_waves_sampled` holds the arithmetic to it.
const BEND: f32 = 8.0;

/// The field the whitecaps' bar wanders on: how big its coarsest cell is, in
/// metres, and how fast it drifts downwind, in metres per second.
///
/// Value noise rather than more sines — `cap_bar` in the shader carries that
/// argument. The cell is what sets how big a patch of breaking sea is, and a
/// couple of hundred metres is a few boat-lengths of white and then a stretch
/// of clean water, which is what a blow looks like from above. The drift is
/// slow next to the wind that causes it: a gust patch is a place where the
/// sea is rougher, and places move at nothing like the speed of the air over
/// them.
const BREAKING_FIELD: (f32, f32) = (220.0, 2.5);

/// The bend the swell is read through: a unit-ish vector field over the
/// plane, in metres once [`BEND`] has scaled it.
///
/// Two sines per component, at wavelengths of a few hundred metres — long
/// compared to any wave, so what they do is bend crests rather than add
/// ripples of their own — and at angles unrelated to the trains', so the
/// bending never lines up with what it is bending. Stationary in the world:
/// the waves travel through it, so the crests are always changing shape,
/// while the place where the water is doing that stays put the way a current
/// would.
///
/// This is the other copy of `bend` in `assets/shaders/sea.wgsl` and the two
/// must agree, exactly as [`SeaConditions::swell`] and its twin must.
fn bend(at: Vec2) -> Vec2 {
    let component = |sines: &[(Vec2, f32)]| -> f32 {
        sines
            .iter()
            .map(|(k, weight)| weight * at.dot(*k).sin())
            .sum()
    };
    Vec2::new(component(&BENDS[..2]), component(&BENDS[2..])) * BEND
}

/// The sines [`bend`] is made of: a wave vector in radians per metre, and a
/// weight. The first two are its x, the last two its y — written out here
/// rather than inline so that `the_bend_leaves_the_waves_sampled` can do the
/// arithmetic on the same numbers the field is built from.
const BENDS: [(Vec2, f32); 4] = [
    (Vec2::new(0.01079, -0.00917), 1.0),
    (Vec2::new(-0.02347, 0.01768), 0.5),
    (Vec2::new(0.00774, 0.01209), 1.0),
    (Vec2::new(0.01918, 0.02236), 0.5),
];

/// When the open sea goes white: how far up the leading face of the swell a
/// whitecap starts, as a cosine, and how high the sea must be heaping under
/// it, in metres.
///
/// The first number is the *shape* of a cap and the second only cuts it up,
/// which is the way round that matters. A band down the leading face of the
/// longest wave train is a sliver lying along a crest — the shape water makes
/// falling over — and the heaping breaks that band into caps wherever the
/// three trains stop agreeing. Done the other way about, with the heaping for
/// the shape and the wave to trim it, every cap comes out a round blob a few
/// metres across, because the heap is the smaller of the two: a sea of white
/// spots that read as paint rather than as water.
///
/// The heaping is a height rather than a wind, and that is what makes caps
/// weather. [`WAVES`] are written for [`REFERENCE_WIND`] and
/// [`amplitude_scale`] raises them from there, so a fixed height is a bar the
/// sea clears never in a calm, seldom in a moderate breeze and all over the
/// place in a blow — without the shader being told anything at all about the
/// wind. Above the sum of the three amplitudes nothing can break: at a third
/// of the reference sea's height, the caps arrive at about the wind real ones
/// do, which is a coincidence worth keeping rather than a calculation.
/// The third number is what stops the caps being a lattice: how far the
/// heaping bar wanders about, as a fraction of the sea's own full height.
/// Thresholding three sines at a constant draws the pattern they beat in —
/// rows of identical commas over the whole ocean — and a bar that wanders
/// instead leaves patches breaking and patches bare. A fraction rather than a
/// height because the swing has to be able to climb past what the swell can
/// reach in order to take the caps off a patch at all, and what it can reach
/// is the wind's business; `cap_bar` in the shader carries the rest of it.
const WHITECAP: (f32, f32, f32) = (0.60, 0.23, 0.45);

/// Wavelength, in metres, of a slow drift the shore wave's phase picks up
/// along the coast. Without it the phase at the waterline is `ω·t` alone
/// and every beach in the world breaks in unison, like lights on one
/// switch; a long spatial term staggers the sets from bay to bay while
/// leaving each crest still reading as parallel to its shore.
const SHORE_STAGGER: f32 = 300.0;

/// Metres between the sea mesh's vertices, over the region that waves. The
/// terrain's facets are 2 m; the sea's are coarser because its shapes are
/// longer — at 4 m the shortest wave in [`WAVES`] still gets three facets per
/// crest. A quarter of a million vertices, which sounds like a lot and is a
/// single static buffer the vertex shader walks; halving it was tried first,
/// and took the shortest legible wavelength — and most of the sea's visible
/// slope — with it.
pub const SPACING: f32 = 4.0;

/// Half-width of the region that actually waves, in metres — vertices at
/// [`SPACING`] reach this far out from the mesh's centre, and a single ring
/// of enormous flat cells carries on from there to the horizon. A whole
/// number of spacings, so the fine grid meets the ring exactly.
const REACH: f32 = 1024.0;

/// Where the swell starts fading with distance from the mesh's centre, and
/// where it has fully gone, in metres. The far edge stays inside [`REACH`],
/// so every vertex the flat outer ring shares an edge with has stopped
/// moving and the two regions meet without cracks. All of it is deep inside
/// the haze — the fade exists so the mesh can end, not to be seen.
const FADE: (f32, f32) = (512.0, 960.0);

/// Texels along each side of the depth window. With a texel per [`SPACING`]
/// that is a 2560 m square — comfortably past where the swell has faded,
/// so nothing that moves is ever asking about water outside it.
const DEPTH_TEXELS: usize = 640;

/// Width of the depth window in metres.
const DEPTH_EXTENT: f32 = DEPTH_TEXELS as f32 * SPACING;

/// The deepest water a texel can say, in metres: a byte spans this range in
/// 5 cm steps. Comfortably past [`OCEAN_DEPTH`], and unknown ground — chunks
/// not yet sent — is encoded as the full value, so water the client has not
/// been told about wears the open sea's swell until it learns better.
///
/// [`OCEAN_DEPTH`]: protocol::ground::OCEAN_DEPTH
const DEPTH_RANGE: f32 = 12.75;

/// How far the camera may drift from the depth window's centre before the
/// window is scrolled back under it, in metres. Scrolling shifts the texels
/// by whole steps of [`SPACING`], so everything already known stays pinned
/// to the world points it was read from and only the strip that slid into
/// view is stale — and that strip enters at the window's edge, beyond the
/// fade, where nothing is drawn moving anyway.
const RECENTER: f32 = 128.0;

/// Rows of the depth window refilled from the ground each frame. The sweep
/// simply goes round and round: at ten rows a frame the whole window is
/// re-read about once a second, which is how arriving chunks, and the gaps a
/// scroll exposes, find their way in without anyone tracking what changed.
/// A row is a few hundred height lookups, noise next to a single chunk mesh
/// build.
const SWEEP_ROWS: usize = 10;

/// The sea's water, waves and all: the standard water surface underneath,
/// with the swell displacing its vertices on top.
pub type SeaMaterial = ExtendedMaterial<StandardMaterial, SeaExtension>;

/// What the sea shader needs beyond the standard material: the swell's
/// parameters, packed for the sums the shader runs, and the depth window it
/// reads the shallows from.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct SeaExtension {
    /// One deep wave per row: `xy` is the heading scaled by the wavenumber,
    /// `z` the angular frequency, `w` the amplitude — exactly the terms of
    /// [`swell`], so the shader adds them up rather than deriving anything.
    #[uniform(100)]
    waves: [Vec4; WAVES.len()],
    /// `x` and `y` are [`FADE`], `z` is [`SHADING_TILT`], `w` is [`BEND`].
    #[uniform(100)]
    fade: Vec4,
    /// The shore wave: `x` its wavenumber down the depth, `y` its angular
    /// frequency, `z` [`SHORE_AMPLITUDE`], `w` [`BREAK_SLOPE`].
    #[uniform(100)]
    shore: Vec4,
    /// The shallows: `x` and `y` are [`SHOAL`], `z` is [`RUNUP`], `w` is
    /// [`FOAM_CREST`].
    #[uniform(100)]
    surf: Vec4,
    /// `xy` is the along-shore stagger as a wave vector — [`SHORE_STAGGER`]
    /// along the primary deep heading; `z` is [`FOAM_SLOPE`]; `w` is a
    /// depth texel's width in metres, which is what the shader steps by to
    /// read the bottom's grade.
    #[uniform(100)]
    stagger: Vec4,
    /// `xy` is [`FOAM_FEED`]; `zw` is [`MURK`].
    #[uniform(100)]
    feed: Vec4,
    /// `xyz` is [`WHITECAP`]; `w` is padding.
    #[uniform(100)]
    caps: Vec4,
    /// `xy` is [`BREAKING_FIELD`]; `zw` is padding.
    #[uniform(100)]
    breaking: Vec4,
    /// The depth window's place in the world: `xy` the world coordinates of
    /// its corner texel's corner, `z` `1 / DEPTH_EXTENT`, `w`
    /// [`DEPTH_RANGE`]. Rewritten whenever the window scrolls.
    #[uniform(100)]
    window: Vec4,
    /// The hour to hold the window's lit intervals against, and how wide the
    /// gaining and losing of the sun is drawn: `x` and `y` exactly as
    /// `crate::terrain`'s `Daylight` carries them, written by the same system
    /// so the water and the ground beside it are never at different times of
    /// day. `zw` padding.
    ///
    /// Beside [`SeaExtension::window`] because the two are one answer: that
    /// says where the texels lie, this says what hour to read them at. The
    /// order of these fields *is* the uniform's layout, and the shader's
    /// `SeaParams` has to name them in the same order or every field after
    /// the difference is read as its neighbour.
    #[uniform(100)]
    pub(crate) daylight: Vec4,
    /// The wake's band: `x` the half-width of the water a hull turns over at
    /// its stem, `y` how far the arms open per metre run, `z` how thick an
    /// arm is, `w` how long a wake lasts.
    #[uniform(100)]
    pub(crate) wash: Vec4,
    /// The boil and what wears it away: `x` how fast it widens in metres per
    /// second of age, `y` how many seconds of it there are, `z` the cell of
    /// the field an ageing wake breaks up on, `w` the least way that leaves
    /// any mark at all.
    #[uniform(100)]
    pub(crate) boil: Vec4,
    /// Where the wake could possibly be: `xy` the least corner, `zw` the
    /// greatest. Water outside it rejects the wake in two comparisons rather
    /// than walking the track, and a box with its least corner past its
    /// greatest — which is what this opens as — is water with no wake on it
    /// at all.
    #[uniform(100)]
    pub(crate) wake_bounds: Vec4,
    /// The hull's track, newest first: `xy` where its stem was, `z` how many
    /// seconds ago, `w` the way it was making then. Written by
    /// [`crate::wake`], which owns every number in these last four fields and
    /// is where the reasoning for all of them lives; the sea only carries
    /// them to the shader that paints the foam.
    #[uniform(100)]
    pub(crate) wake: [Vec4; crate::wake::TRAIL],
    /// The holes the open boats cut in the surface: `xy` the centre of one's
    /// waterline footprint on the map — its widest station — and `zw` the
    /// way that hull is pointing, as a unit vector. Written by
    /// `boat::cut_the_water`, which owns the numbers in these last three
    /// fields the way the wake module owns its; the fragment shader discards
    /// the water inside each footprint, which is how a boat that is looked
    /// *into* is not drawn full of sea.
    #[uniform(100)]
    pub(crate) hole: [Vec4; crate::boat::HOLES],
    /// Each footprint's reach from that centre: `x` forward to the stem, `y`
    /// aft to where its stern piece would close, `z` half its width, and
    /// `w` 1.0 in a slot that holds a boat. The slots are filled from the
    /// front, so the first `w` of zero is the end of the list — and all of
    /// them zero is every frame without an open hull afloat.
    #[uniform(100)]
    pub(crate) hole_axes: [Vec4; crate::boat::HOLES],
    /// Their shapes: `xy` the bow and stern pieces' fullness — superellipse
    /// exponents — and `z` metres from the centre aft to the transom, where
    /// the outline is cut square. See `boat::OpenHull`, where each number's
    /// reasoning lives.
    #[uniform(100)]
    pub(crate) hole_shape: [Vec4; crate::boat::HOLES],
    /// The depth window itself — see [`DepthWindow`], which owns the scroll
    /// and the sweep that keep it current.
    #[texture(101)]
    #[sampler(102)]
    depth: Handle<Image>,
}

impl SeaExtension {
    /// `focus` is where the camera enters the world — the window opens
    /// centred there, exactly as [`DepthWindow::new`] will place it. The
    /// waves open under the assumed wind, and [`settle_conditions`] rewrites
    /// them the moment a forecast says otherwise.
    pub fn new(depth: Handle<Image>, focus: Vec2) -> Self {
        let origin = DepthWindow::origin_under(focus);
        let conditions = SeaConditions::default();
        Self {
            waves: conditions.components(),
            fade: Vec4::new(FADE.0, FADE.1, SHADING_TILT, BEND),
            shore: Vec4::new(
                TAU / CREST_EVERY,
                TAU / SHORE_PERIOD,
                conditions.shore_amplitude(),
                BREAK_SLOPE,
            ),
            surf: Vec4::new(SHOAL.0, SHOAL.1, RUNUP, FOAM_CREST),
            stagger: Vec4::new(stagger_vector().x, stagger_vector().y, FOAM_SLOPE, SPACING),
            feed: Vec4::new(FOAM_FEED.0, FOAM_FEED.1, MURK.0, MURK.1),
            caps: Vec4::new(WHITECAP.0, WHITECAP.1, WHITECAP.2, 0.0),
            breaking: Vec4::new(BREAKING_FIELD.0, BREAKING_FIELD.1, 0.0, 0.0),
            window: Self::window_uniform(origin),
            // Noon until the sky says otherwise, as the ground opens too.
            daylight: crate::terrain::DAYLIGHT_AT_NOON,
            // No boat has been anywhere yet. The empty box is what makes the
            // rest of this safe to leave at zero: nothing reads a track it is
            // never allowed to be inside the bounds of.
            wash: Vec4::ZERO,
            boil: Vec4::ZERO,
            wake_bounds: Vec4::new(1.0, 1.0, -1.0, -1.0),
            wake: [Vec4::ZERO; crate::wake::TRAIL],
            hole: [Vec4::ZERO; crate::boat::HOLES],
            hole_axes: [Vec4::ZERO; crate::boat::HOLES],
            hole_shape: [Vec4::ZERO; crate::boat::HOLES],
            depth,
        }
    }

    fn window_uniform(origin: Vec2) -> Vec4 {
        Vec4::new(origin.x, origin.y, 1.0 / DEPTH_EXTENT, DEPTH_RANGE)
    }
}

impl MaterialExtension for SeaExtension {
    fn vertex_shader() -> bevy::shader::ShaderRef {
        SHADER.into()
    }

    fn fragment_shader() -> bevy::shader::ShaderRef {
        SHADER.into()
    }
}

/// The along-shore stagger as a wave vector — see [`SHORE_STAGGER`]. A fixed
/// bearing rather than the wind's: it only desynchronises distant beaches,
/// and re-phasing every shore each time the wind veered would make the surf
/// stutter for nothing.
fn stagger_vector() -> Vec2 {
    Vec2::new(0.966, 0.259) * (TAU / SHORE_STAGGER)
}

/// The weather as last told by the server: the wind over the whole world,
/// as a velocity in metres per second, or `None` before the first word of
/// it arrives. A target, not a picture — the drawn sea is
/// [`SeaConditions`], which [`settle_conditions`] eases towards this
/// whenever a [`protocol::ToClient::Weather`] moves it. The very first
/// forecast is taken as a snap instead: until it lands the client is
/// drawing an assumed day, and easing from a guess to the truth animates a
/// change of weather that never happened.
#[derive(Resource, Default)]
pub struct Forecast {
    pub wind: Option<Vec2>,
}

impl Forecast {
    /// The wind the sea should be settling towards. There used to be a
    /// second, locally commanded wind outranking this one, for the `--debug`
    /// keys; the console's `weather` command replaced it by ordering the
    /// *server's* weather, so every wind now arrives here the same way.
    fn told(&self) -> Option<Vec2> {
        self.wind
    }
}

/// One of the sea's wave trains, as currently drawn: the way it runs, and
/// how faded it is while it re-aims — see [`REAIM`] for why a wave may only
/// change direction while it cannot be seen.
#[derive(Clone, Copy)]
struct Slot {
    heading: Vec2,
    dim: f32,
}

/// The sea as it is being drawn this frame: the smoothed wind, and the state
/// of each wave train under it. The one authority both riders and renderer
/// read — [`SeaConditions::swell`] for the boat and the markers,
/// [`SeaConditions::components`] for the uniform the shader gets — so the
/// water everything sits on is the water on screen, whatever the weather is
/// doing.
#[derive(Resource)]
pub struct SeaConditions {
    wind: Vec2,
    slots: [Slot; WAVES.len()],
    /// Still the assumed day — no forecast has ever landed. What lets the
    /// first one snap rather than ease.
    assumed: bool,
}

impl Default for SeaConditions {
    fn default() -> Self {
        Self {
            wind: ASSUMED_WIND,
            slots: WAVES.map(|(bearing, ..)| Slot {
                heading: Vec2::from_angle(bearing).rotate(ASSUMED_WIND.normalize()),
                dim: 1.0,
            }),
            assumed: true,
        }
    }
}

impl SeaConditions {
    /// A sea already settled under a given wind — no forecast, no easing, the
    /// slots aimed as [`settle_conditions`] would have left them after the
    /// veer was over. What a test that cares which way the wind blows starts
    /// from: `world_app` never runs the settle system, so a sea inserted this
    /// way blows its wind for good.
    #[cfg(test)]
    pub(crate) fn blowing(wind: Vec2) -> Self {
        Self {
            wind,
            slots: WAVES.map(|(bearing, ..)| Slot {
                // A calm names no bearing, so like the settle system the
                // slots fall back to *a* heading rather than a NaN one.
                heading: Vec2::from_angle(bearing).rotate(wind.normalize_or(Vec2::X)),
                dim: 1.0,
            }),
            assumed: true,
        }
    }

    /// The wave trains, worked into the terms both copies of the formula run
    /// on: `xy` heading times wavenumber, `z` angular frequency, `w`
    /// amplitude — the reference amplitude scaled by the wind and by the
    /// slot's own fade.
    fn components(&self) -> [Vec4; WAVES.len()] {
        let scale = amplitude_scale(self.wind.length());
        core::array::from_fn(|i| {
            let (_, wavelength, amplitude) = WAVES[i];
            let slot = self.slots[i];
            let wavenumber = TAU / wavelength;
            let frequency = (GRAVITY * wavenumber).sqrt();
            let direction = slot.heading * wavenumber;
            Vec4::new(
                direction.x,
                direction.y,
                frequency,
                amplitude * scale * slot.dim,
            )
        })
    }

    /// The wind the sea is being drawn under, in metres per second — the
    /// eased wind rather than the forecast, which is what anything *reading*
    /// the weather wants: an instrument settling on a new wind at a different
    /// rate from the water under it would be telling the player about a sea
    /// they cannot see. The compass's arm is drawn off this.
    pub fn wind(&self) -> Vec2 {
        self.wind
    }

    /// How big a sea is running, as a factor on the reference breeze's — the
    /// very number every drawn amplitude is scaled by, so anything reading
    /// this is reading the water actually on screen rather than a second
    /// opinion about the weather. A shade over zero in a flat calm and a good
    /// deal over one in a blow; see [`amplitude_scale`]. What the sound of
    /// the sea is worked out from.
    pub fn liveliness(&self) -> f32 {
        amplitude_scale(self.wind.length())
    }

    /// The shore wave's unbroken height under this wind. The breaking cap is
    /// depth's business, not the weather's — a blow widens the surf simply
    /// by giving the cap more to bite off.
    fn shore_amplitude(&self) -> f32 {
        SHORE_AMPLITUDE * amplitude_scale(self.wind.length())
    }

    /// Height of the swell above the flat waterline at a point, in metres —
    /// negative in a trough. `elapsed` is `Time::elapsed_secs_wrapped`, the
    /// same clock the shader's `globals.time` runs on, and `depth` is how
    /// much water stands under the point — the negative of
    /// [`Ground::height`], with anything unknown counting as deep.
    ///
    /// This is the Rust copy of the formula in `assets/shaders/sea.wgsl`;
    /// the two must agree or the boat stops sitting on the water it is
    /// drawn in.
    pub fn swell(&self, at: Vec2, elapsed: f32, depth: f32) -> f32 {
        let components = self.components();
        // Every train is read through the same bend — see [`bend`] — and each
        // is bent by the same fraction of its own wavelength, which is what
        // the longest train's wavenumber out front does: a train's phase
        // shift is its heading against the bend, in units of the longest
        // train's waves. Bending them all by the same number of *metres*
        // would swing the chop's phase three times as far as the swell's and
        // fold it over itself.
        let bend = bend(at) * components[0].xy().length();
        let deep: f32 = components
            .iter()
            .map(|wave| {
                let heading = wave.xy().normalize();
                let phase = wave.xy().dot(at) + heading.dot(bend) - wave.z * elapsed;
                wave.w * phase.sin()
            })
            .sum();
        let w = shore_weight(depth);
        deep * (1.0 - w) + self.shore(at, elapsed, depth) * w
    }

    /// Where the water stands over a map point, in metres — [`swell`] with
    /// the depth looked up rather than passed in.
    ///
    /// This is what everything riding the sea wants: a hull, a marker, a
    /// dolphin. Ground the client has not been sent counts as
    /// [`OCEAN_DEPTH`] — the same benefit of the doubt the depth window
    /// gives the shader, and the reason this is one function rather than a
    /// lookup written out at each of them. A caller reaching for
    /// [`f32::MAX`] instead would get NaN back: the shore wave's phase *is*
    /// the depth, and an infinite phase has no sine.
    ///
    /// [`swell`]: SeaConditions::swell
    /// [`OCEAN_DEPTH`]: protocol::ground::OCEAN_DEPTH
    pub fn water_over(&self, ground: Option<&Ground>, at: Vec2, elapsed: f32) -> f32 {
        let depth = ground
            .and_then(|ground| ground.height(at.x, at.y))
            .map_or(protocol::ground::OCEAN_DEPTH, |height| -height);
        self.swell(at, elapsed, depth)
    }

    /// What a floating body rides at over a map point, in metres: the ground
    /// where it stands proud of the water, and the water itself everywhere
    /// else — or `None` where the ground has not arrived, which is a caller's
    /// cue to keep the height it had rather than guess.
    ///
    /// One function because two things ask it and mean exactly the same thing
    /// by it — a hull sits on this surface and another player's marker stands
    /// its capsule's half-length above it — and because the `None` is the
    /// half that is easy to get wrong. A body dropped to the waterline
    /// wherever a chunk has streamed out and climbed back when it returns is
    /// a body that flickers through the ground, and it looks exactly like a
    /// physics bug rather than like a chunk that has not been sent.
    ///
    /// Not what a beast asks: an animal rides a depth measured *down* from
    /// the moving surface, which is [`SeaConditions::water_over`] with no
    /// ground in the answer at all.
    pub fn surface_over(&self, ground: Option<&Ground>, at: Vec2, elapsed: f32) -> Option<f32> {
        let standing = ground?.height(at.x, at.y)?;
        Some(standing.max(self.water_over(ground, at, elapsed)))
    }

    /// Where a thing riding a carrier stands on the map, and where the water
    /// is under it — the opening move of anything drawn as a formation.
    ///
    /// The station is read out of the rider's own transform *in the plane*
    /// and carried into the world through the carrier's, so a member keeps
    /// its place in the group however the group is headed, and its own height
    /// is left entirely to whatever is about to set it. Both the flying and
    /// the swimming formations begin here — a line of seabirds over the
    /// shallows and a pod of dolphins under them are the same question asked
    /// of the same swell.
    pub fn under_station(
        &self,
        ground: Option<&Ground>,
        carrier: &Transform,
        station: &Transform,
        elapsed: f32,
    ) -> (Vec3, f32) {
        let at =
            carrier.transform_point(Vec3::new(station.translation.x, 0.0, station.translation.z));
        (at, self.water_over(ground, at.xz(), elapsed))
    }
}

/// Takes the wind the server has told, which the drawn sea then eases onto —
/// see [`settle_conditions`], which is the easing.
///
/// A target rather than an order, and the last word wins: a batch holding two
/// tellings is a client that has been away for a frame, and the older of them
/// is weather that has already happened.
pub(crate) fn take_the_weather(
    mut forecast: ResMut<Forecast>,
    mut told: MessageReader<crate::net::WindChanged>,
) {
    for changed in told.read() {
        forecast.wind = Some(changed.wind);
    }
}

/// Brings the drawn sea to the forecast: the wind eases over, each wave
/// train re-aims when the wind has veered past its hysteresis — through
/// zero, see [`REAIM`] — and whatever changed lands in the sea material's
/// uniform, which is the whole of how the shader hears about weather.
pub(crate) fn settle_conditions(
    time: Res<Time>,
    forecast: Res<Forecast>,
    mut conditions: ResMut<SeaConditions>,
    window: Option<Res<DepthWindow>>,
    mut materials: ResMut<Assets<SeaMaterial>>,
) {
    let told = forecast.told().unwrap_or(ASSUMED_WIND);
    if conditions.assumed && forecast.told().is_some() {
        // The first word of real weather replaces the assumed day outright —
        // see [`Forecast`]. Only the wind and the aims: the slots' fades are
        // already at one, and there is nothing on screen worth easing from.
        conditions.assumed = false;
        conditions.wind = told;
        if told.length() > WIND_NAMED {
            let bearing = told.normalize();
            for (i, slot) in conditions.slots.iter_mut().enumerate() {
                slot.heading = Vec2::from_angle(WAVES[i].0).rotate(bearing);
            }
        }
    }

    let follow = crate::eased(1.0 / SEA_RESPONSE, time.delta_secs());
    let fade = crate::eased(1.0 / REAIM.1, time.delta_secs());
    conditions.wind = veered(conditions.wind, told, follow);

    // A dying wind names no bearing — see [`WIND_NAMED`]: below the bar the
    // slots hold the heading they had, and the lull is carried by the
    // amplitudes.
    let steady = conditions.wind.length() > WIND_NAMED;
    let bearing = if steady {
        conditions.wind.normalize()
    } else {
        Vec2::X
    };
    for (i, slot) in conditions.slots.iter_mut().enumerate() {
        let desired = Vec2::from_angle(WAVES[i].0).rotate(bearing);
        if steady && slot.heading.dot(desired) < REAIM.0.cos() {
            slot.dim += (0.0 - slot.dim) * fade;
            if slot.dim < 0.03 {
                slot.heading = desired;
            }
        } else {
            slot.dim += (1.0 - slot.dim) * fade;
        }
    }

    // Into the uniform — via the same read-compare-write two-step as the
    // depth sweep, so a settled sea marks nothing changed and re-prepares
    // nothing.
    let Some(window) = window else {
        return;
    };
    let waves = conditions.components();
    let shore_amplitude = conditions.shore_amplitude();
    let stale = materials.get(&window.material).is_some_and(|material| {
        (material.extension.shore.z - shore_amplitude).abs() > 1e-4
            || material
                .extension
                .waves
                .iter()
                .zip(&waves)
                .any(|(held, wanted)| held.distance(*wanted) > 1e-4)
    });
    if stale {
        if let Some(mut material) = materials.get_mut(&window.material) {
            material.extension.waves = waves;
            material.extension.shore.z = shore_amplitude;
        }
    }
}

/// One step of the ease that brings the drawn wind to the told one: the
/// bearing swung round the card, the strength eased alongside it, both at
/// `follow` — the same shape a wave slot's re-aim already has.
///
/// It was a plain `lerp` of the two vectors, and that is not the small
/// difference it looks. A straight line between two nearly opposite winds
/// passes close to the origin, so a hard veer took the drawn wind *down
/// through the calm* on its way round: over a simulated hour, spells of
/// seconds under [`WIND_NAMED`] with a true wind that never once dropped
/// below a light air. Everything downstream believed it, because the drawn
/// wind is the only wind they can see — the arm left the card, the wave
/// trains froze their headings, and the sails, which taper to nothing under
/// the bar, stopped the boat dead in the middle of a veer. Turning the
/// bearing keeps the drawn strength between the two winds' own, so the floor
/// the weather promises survives being drawn.
fn veered(drawn: Vec2, told: Vec2, follow: f32) -> Vec2 {
    if drawn == told {
        // Already there, said outright: taking a vector apart into a bearing
        // and a strength and multiplying it back together is not quite the
        // identity in floats, and a sea with nothing to follow — a session
        // still on the assumed day, a wind that has settled — would otherwise
        // shuffle its last bit for as long as it ran.
        return drawn;
    }
    // Neither end is guaranteed to have a bearing to offer. A told calm —
    // only the console can order one — is a wind to die *into* rather than
    // turn towards, so the drawn wind holds the bearing it had and simply
    // goes out; and a drawn wind already out has nothing to swing from, so
    // it takes the told bearing whole and grows along it.
    match (Dir2::new_and_length(drawn), Dir2::new_and_length(told)) {
        (Ok((from, was)), Ok((to, wants))) => {
            from.slerp(to, follow) * (was + (wants - was) * follow)
        }
        (Ok((from, was)), Err(_)) => from * (was * (1.0 - follow)),
        (Err(_), Ok((to, wants))) => to * (wants * follow),
        (Err(_), Err(_)) => Vec2::ZERO,
    }
}

/// How much of the swell at a depth is the shore wave rather than the open
/// sea's: none in the deep, all of it in the shallows — see [`SHOAL`].
fn shore_weight(depth: f32) -> f32 {
    1.0 - smoothstep(SHOAL.1, SHOAL.0, depth)
}

impl SeaConditions {
    /// The shore wave's height at a point, in metres. Phase runs down the
    /// depth itself — see the module doc for why that alone is what bends
    /// crests parallel to every shore — and its height is capped by the
    /// water under it, which is breaking.
    fn shore(&self, at: Vec2, elapsed: f32, depth: f32) -> f32 {
        let amplitude = self
            .shore_amplitude()
            .min(depth.max(0.0) * BREAK_SLOPE + RUNUP);
        let phase =
            depth * (TAU / CREST_EVERY) + stagger_vector().dot(at) + (TAU / SHORE_PERIOD) * elapsed;
        amplitude * phase.sin()
    }
}

/// The same curve as WGSL's `smoothstep`, which the shader's copy of the
/// crossfade uses — the two must be the same function or the boat and the
/// water part company exactly where the regimes blend.
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Snaps a coordinate onto the sea mesh's own lattice — in strides of *two*
/// cells, never one.
///
/// The mesh travels with the camera, and its vertices sample the swell at
/// whatever world points they land on. Moved continuously, every vertex
/// resamples the field every frame and the facets swim against the waves
/// they are drawing; moved in whole steps of [`SPACING`], each vertex sits
/// exactly where one sat before, and the surface holds still while the mesh
/// slides underneath it.
///
/// The vertices are not the whole of the surface, though. The grid's
/// diagonals checker by cell parity — see [`surface_mesh`] — so a
/// single-cell step lands every vertex on a lattice site while flipping
/// every facet's split in world terms, and the flat shading of the entire
/// sea jumps with it: invisibly in a calm, unmissably in a gale, and only
/// while the camera moves, which made it look like anything but geometry.
/// Two cells is the stride that maps the checkerboard onto itself.
pub fn snap(coordinate: f32) -> f32 {
    const STRIDE: f32 = 2.0 * SPACING;
    (coordinate / STRIDE).round() * STRIDE
}

// ---------------------------------------------------------------------------
// The depth window
// ---------------------------------------------------------------------------

/// The picture of the water's depth around the camera that the sea shader
/// reads: a byte per texel, a texel per [`SPACING`], scrolled to follow the
/// camera and perpetually re-read from [`Ground`] by [`refresh_depth`].
///
/// Holds the handles rather than living on an entity because two assets have
/// to change together: scrolling the image only means anything if the
/// material's idea of where the window sits moves in the same frame.
#[derive(Resource)]
pub struct DepthWindow {
    image: Handle<Image>,
    material: Handle<SeaMaterial>,
    /// World coordinates of the corner of texel (0, 0), a multiple of
    /// [`SPACING`] always, so texels stay pinned to world points across
    /// every scroll.
    origin: Vec2,
    /// The row the round-robin sweep refills next.
    sweep: usize,
}

impl DepthWindow {
    pub fn new(image: Handle<Image>, material: Handle<SeaMaterial>, focus: Vec2) -> Self {
        Self {
            image,
            material,
            origin: Self::origin_under(focus),
            sweep: 0,
        }
    }

    /// The sea's own material — the one asset the water is drawn out of, and
    /// so the one place anything with something to tell the water writes it.
    /// The wake has no window of its own to hang a handle on and reaches it
    /// through here; see [`crate::wake`].
    pub(crate) fn material(&self) -> &Handle<SeaMaterial> {
        &self.material
    }

    /// The window origin that centres the window on a focus, on the lattice.
    fn origin_under(focus: Vec2) -> Vec2 {
        Vec2::new(snap(focus.x), snap(focus.y)) - DEPTH_EXTENT / 2.0
    }
}

/// Bytes a texel of the window occupies: the depth, the two ends of the lit
/// interval, and one the format asks for and nothing reads.
const TEXEL_BYTES: usize = 4;

/// What an unread texel says: water too deep to break, lit the whole day.
/// Both are the answer for ground the client has not been told about — it
/// wears the open sea's swell until it learns better, and nothing it has
/// never heard of can be casting a shadow it could see.
const UNREAD: [u8; TEXEL_BYTES] = [u8::MAX, LIT_ALL_DAY[0], LIT_ALL_DAY[1], u8::MAX];

/// A window with nothing in it yet — see [`UNREAD`].
///
/// Four channels rather than one because the sea has two things to learn
/// from the ground under it, and they are learned in the same sweep over the
/// same texels: how deep the water is, which decides how it breaks, and when
/// the ground around it lets the sun through, which decides whether it is in
/// a headland's shadow. A second window would be a second scroll, a second
/// sweep and a second staleness test, all of them in step with this one.
pub fn depth_image() -> Image {
    let mut image = Image::new_fill(
        Extent3d {
            width: DEPTH_TEXELS as u32,
            height: DEPTH_TEXELS as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &UNREAD,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::default(),
    );
    // Bilinear, so a facet between two texels gets water between their
    // depths rather than one or the other's — and a shadow's edge crossing
    // the window arrives as an edge rather than as a staircase of texels.
    image.sampler = ImageSampler::linear();
    image
}

/// One texel's worth of what the sea needs to know about the ground beneath
/// it: how deep the water is — [`DEPTH_RANGE`] — and when that water sees
/// the sun. Ground the client has not been sent reads as [`UNREAD`].
fn texel(height: Option<f32>, lit: Option<[u8; 2]>) -> [u8; TEXEL_BYTES] {
    let depth = match height {
        None => UNREAD[0],
        Some(height) => ((-height).clamp(0.0, DEPTH_RANGE) / DEPTH_RANGE * 255.0).round() as u8,
    };
    let lit = lit.unwrap_or(LIT_ALL_DAY);
    [depth, lit[0], lit[1], UNREAD[3]]
}

/// Keeps the depth window under the camera and its texels agreeing with the
/// ground.
///
/// Two motions, deliberately decoupled. The *scroll* fires when the camera
/// has drifted [`RECENTER`] from the window's centre: texels shift by whole
/// steps of the lattice, so every depth already read stays pinned to the
/// world point it was read from, and the material is told where the window
/// now sits in the same frame. The strip a scroll exposes is stale, and
/// allowed to be — it enters at the window's edge, past the swell's fade.
///
/// The *sweep* refills [`SWEEP_ROWS`] rows a frame, round and round,
/// re-reading the ground whether or not anything changed. That sounds
/// wasteful and is the entire trick: chunks arriving, chunks forgotten and
/// scroll-exposed strips all heal within a second of sweep with nothing
/// keeping track of any of them. The rewrite only touches the image asset
/// when some byte actually changed, so a settled view re-uploads nothing.
pub(crate) fn refresh_depth(
    ground: Res<Ground>,
    cameras: Query<&MapCamera>,
    mut window: ResMut<DepthWindow>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<SeaMaterial>>,
) {
    let Ok(camera) = cameras.single() else {
        return;
    };
    let focus = Vec2::new(camera.focus.x, camera.focus.z);

    // The scroll.
    let drift = focus - (window.origin + DEPTH_EXTENT / 2.0);
    if drift.x.abs() > RECENTER || drift.y.abs() > RECENTER {
        let origin = DepthWindow::origin_under(focus);
        let step = ((origin - window.origin) / SPACING).round().as_ivec2();
        if let Some(mut image) = images.get_mut(&window.image) {
            if let Some(data) = image.data.as_mut() {
                scroll(data, step);
            }
        }
        window.origin = origin;
        if let Some(mut material) = materials.get_mut(&window.material) {
            material.extension.window = SeaExtension::window_uniform(origin);
        }
    }

    // The sweep. Read into a scratch first and compare, so a frame that
    // changed nothing marks nothing changed and re-uploads nothing. The
    // sweep never straddles the wrap — see `the_sweep_divides_the_window`.
    let mut rows = [0u8; DEPTH_TEXELS * SWEEP_ROWS * TEXEL_BYTES];
    for r in 0..SWEEP_ROWS {
        let row = window.sweep + r;
        for x in 0..DEPTH_TEXELS {
            let at = window.origin + Vec2::new(x as f32 + 0.5, row as f32 + 0.5) * SPACING;
            let texel = texel(ground.height(at.x, at.y), ground.lit(at.x, at.y));
            let into = (r * DEPTH_TEXELS + x) * TEXEL_BYTES;
            rows[into..into + TEXEL_BYTES].copy_from_slice(&texel);
        }
    }
    let start = window.sweep * DEPTH_TEXELS * TEXEL_BYTES;
    let stale = |data: &[u8]| data[start..start + rows.len()] != rows[..];
    if images
        .get(&window.image)
        .and_then(|image| image.data.as_deref())
        .is_some_and(stale)
    {
        if let Some(mut image) = images.get_mut(&window.image) {
            if let Some(data) = image.data.as_mut() {
                data[start..start + rows.len()].copy_from_slice(&rows);
            }
        }
    }
    window.sweep = (window.sweep + SWEEP_ROWS) % DEPTH_TEXELS;
}

/// Shifts the window's texels so that texel `(x, y)` afterwards holds what
/// texel `(x, y) + step` held before — the data moves opposite to the
/// window, which is what keeps each surviving texel over the same piece of
/// world. Texels that slide in from beyond the old window are set deep, and
/// left for the sweep.
fn scroll(data: &mut [u8], step: IVec2) {
    let n = DEPTH_TEXELS as i32;
    let old = data.to_vec();
    let texel = |x: i32, y: i32| (y * n + x) as usize * TEXEL_BYTES;
    for y in 0..n {
        for x in 0..n {
            let from = IVec2::new(x, y) + step;
            let into = texel(x, y);
            // The index is only worked out once the texel is known to be on
            // the old window: a coordinate that slid in from beyond it is
            // negative, and negative has no place in an index.
            let carried = ((0..n).contains(&from.x) && (0..n).contains(&from.y))
                .then(|| texel(from.x, from.y));
            data[into..into + TEXEL_BYTES].copy_from_slice(match carried {
                Some(was) => &old[was..was + TEXEL_BYTES],
                None => &UNREAD,
            });
        }
    }
}

/// The sea's mesh: a grid of [`SPACING`] cells out to [`REACH`], with one
/// outer ring of cells carrying on to `extent / 2` — the same tensor grid
/// throughout, so the fine middle and the enormous rim share their border
/// vertices and cannot crack apart. The rim's cells are kilometres across,
/// which is fine, because past [`FADE`] the surface they draw is flat.
///
/// Vertices are shared, unlike the terrain's: the facet look comes from the
/// fragment shader deriving each facet's normal from its own slope, so
/// nothing here needs duplicating per triangle. The quads' diagonals
/// alternate in a checkerboard, or the shared diagonal direction reads as a
/// grain running across the water.
pub fn surface_mesh(extent: f32) -> Mesh {
    let half = extent / 2.0;
    let steps = (2.0 * REACH / SPACING) as usize;

    let mut stations = Vec::with_capacity(steps + 3);
    stations.push(-half);
    stations.extend((0..=steps).map(|i| -REACH + i as f32 * SPACING));
    stations.push(half);

    let across = stations.len();
    let mut positions = Vec::with_capacity(across * across);
    for &z in &stations {
        for &x in &stations {
            positions.push(Vec3::new(x, 0.0, z));
        }
    }
    // Every normal is up. The shader replaces them per fragment; these exist
    // because the standard pipeline expects the attribute.
    let normals = vec![[0.0f32, 1.0, 0.0]; positions.len()];

    let mut indices = Vec::with_capacity((across - 1) * (across - 1) * 6);
    for z in 0..across - 1 {
        for x in 0..across - 1 {
            let a = (z * across + x) as u32;
            let b = a + 1;
            let c = a + across as u32;
            let d = c + 1;
            // Wound counter-clockwise seen from above, the same way round as
            // the terrain's facets. The diagonals checker by cell parity so
            // no one direction ridges the shading — which makes the parity
            // part of the surface: [`snap`] strides two cells at a time to
            // land this checkerboard back on itself.
            if (x + z) % 2 == 0 {
                indices.extend([a, c, d, a, d, b]);
            } else {
                indices.extend([a, c, b, b, c, d]);
            }
        }
    }

    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_indices(Indices::U32(indices))
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
}

#[cfg(test)]
mod tests {
    use super::*;

    use protocol::ground::OCEAN_DEPTH;

    /// Somewhere with the whole ocean's depth under it.
    const DEEP: f32 = 8.0;

    /// The sea as every test here finds it: under the assumed wind, nothing
    /// re-aiming — exactly what a client draws before its first forecast.
    fn assumed() -> SeaConditions {
        SeaConditions::default()
    }

    /// The most the surface can ever stand off the waterline under a given
    /// sea: every deep wave at its crest at once, or the shore wave's whole
    /// height.
    fn ceiling(sea: &SeaConditions) -> f32 {
        let deep: f32 = sea.components().iter().map(|wave| wave.w).sum();
        deep.max(sea.shore_amplitude())
    }

    #[test]
    fn a_test_wind_blows_settled() {
        // What the sailing tests build on: the wind reads back exactly, and
        // the primary train runs downwind at full strength — the sea a real
        // forecast would have left once the easing was over.
        let wind = Vec2::new(-3.0, 4.0);
        let sea = SeaConditions::blowing(wind);
        assert_eq!(sea.wind(), wind);
        let heading = sea.slots[0].heading;
        assert!(
            (heading - wind.normalize()).length() < 1e-6,
            "the primary train runs {heading:?} under a wind toward {:?}",
            wind.normalize()
        );
        assert_eq!(sea.slots[0].dim, 1.0);

        // And a calm is allowed: no direction to aim by must not mean NaN in
        // the headings the swell is summed over.
        let calm = SeaConditions::blowing(Vec2::ZERO);
        assert!(calm.swell(Vec2::new(5.0, 5.0), 1.0, DEEP).is_finite());
    }

    #[test]
    fn water_over_ground_nobody_has_been_sent_is_open_ocean() {
        // Everything riding the sea asks `water_over`, and some of them ask
        // over ground that has not arrived — a bird past the end of its
        // sounded course, a chunk that streamed out behind the boat. The
        // answer is the open sea's, and the point of having one function say
        // so is that the obvious hand-rolled fallback is not: the shore
        // wave's phase *is* the depth, so a caller reaching for `f32::MAX`
        // gets `inf.sin()` and hands NaN to whatever it was placing.
        let sea = assumed();
        let (at, elapsed) = (Vec2::new(37.0, -11.0), 3.0);
        let answer = sea.water_over(None, at, elapsed);
        assert!(answer.is_finite(), "unknown ground answered {answer}");
        assert_eq!(answer, sea.swell(at, elapsed, OCEAN_DEPTH));
    }

    #[test]
    fn the_swell_stays_within_its_amplitudes() {
        // The boat and the shore both live within centimetres of the
        // waterline, so the swell being bounded is not decoration — it is
        // what keeps a calm day calm everywhere and forever, at every depth
        // the crossfade passes through — and in every weather, which is what
        // the storm-force conditions in the second pass are for.
        let mut stormy = assumed();
        stormy.wind = Vec2::new(-9.0, 12.0);
        for sea in [assumed(), stormy] {
            let limit = ceiling(&sea) + 1e-5;
            for i in 0..1000 {
                let at = Vec2::new((i * 37 % 997) as f32 * 3.1, (i * 61 % 991) as f32 * -2.7);
                let depth = (i % 100) as f32 * 0.1;
                let height = sea.swell(at, i as f32 * 0.37, depth);
                assert!(
                    height.abs() <= limit,
                    "the swell reaches {height} m at {at} in {depth} m of water, \
                     past every crest combined ({limit} m)"
                );
            }
        }
    }

    #[test]
    fn the_swell_moves() {
        // Anywhere at all, the surface a few seconds later is a different
        // surface — the whole point of it. In the deep and in the shallows,
        // because the two regimes are different waves.
        let sea = assumed();
        let at = Vec2::new(12.0, -34.0);
        for depth in [DEEP, 1.0] {
            assert_ne!(sea.swell(at, 0.0, depth), sea.swell(at, 2.0, depth));
        }
    }

    #[test]
    fn deep_water_is_all_open_sea() {
        // At the ocean's own depth the crossfade must not have started: the
        // shore wave is a coastal thing, and the open sea's look — and every
        // digest of screenshots anyone has taken of it — stays exactly the
        // sum of the three deep waves.
        let sea = assumed();
        let at = Vec2::new(517.0, -212.0);
        let elapsed = 12.3;
        let components = sea.components();
        let bend = bend(at) * components[0].xy().length();
        let deep: f32 = components
            .iter()
            .map(|wave| {
                let heading = wave.xy().normalize();
                wave.w * (wave.xy().dot(at) + heading.dot(bend) - wave.z * elapsed).sin()
            })
            .sum();
        assert_eq!(sea.swell(at, elapsed, DEEP), deep);
    }

    #[test]
    fn the_shallows_break_the_wave_down_to_the_runup() {
        // Breaking is a cap, not a fade to nothing: in ankle-deep water the
        // shore wave still moves the waterline by the runup, and no more
        // than the water can carry.
        let sea = assumed();
        for i in 0..500 {
            let depth = i as f32 * 0.01;
            let tallest = (0..80)
                .map(|t| sea.shore(Vec2::ZERO, t as f32 * 0.1, depth).abs())
                .fold(0.0, f32::max);
            let cap = sea.shore_amplitude().min(depth * BREAK_SLOPE + RUNUP);
            assert!(
                tallest <= cap + 1e-6,
                "{tallest} m of shore wave in {depth} m of water, over the {cap} m cap"
            );
        }
        // And the waterline itself still breathes.
        let at_the_sand = (0..80)
            .map(|t| sea.shore(Vec2::ZERO, t as f32 * 0.1, 0.0).abs())
            .fold(0.0, f32::max);
        assert!(at_the_sand > RUNUP * 0.5, "the beach has gone still");
    }

    #[test]
    fn the_sea_follows_the_wind() {
        // The whole point of a forecast: more wind is more sea. A calm
        // stands well short of the reference breeze, a gale well past it,
        // and neither end runs away — the calm still breathes and the gale
        // still fits between the islands.
        let calm = amplitude_scale(0.0);
        let breeze = amplitude_scale(REFERENCE_WIND);
        let gale = amplitude_scale(16.0);
        assert!(calm > 0.05, "a dead calm went glassy ({calm})");
        assert!(calm < 0.2, "a dead calm still carries sea ({calm})");
        assert!((breeze - 1.0).abs() < 1e-5, "the reference wind is not 1.0");
        assert!(gale > 1.8 && gale <= 2.4, "a gale scales by {gale}");

        // And the wave trains belong to it: under the assumed wind the
        // primary train runs dead downwind.
        let sea = assumed();
        let downwind = ASSUMED_WIND.normalize();
        assert!(sea.slots[0].heading.dot(downwind) > 0.9999);
    }

    /// A headless app running nothing but the settle system, its clock
    /// stepped a frame at a time — the same harness the boat's tests sail
    /// in.
    fn settle_app() -> App {
        use bevy::time::{TimePlugin, TimeUpdateStrategy};
        let mut app = App::new();
        app.add_plugins(TimePlugin)
            .insert_resource(TimeUpdateStrategy::ManualDuration(
                std::time::Duration::from_millis(16),
            ))
            .init_resource::<Forecast>()
            .init_resource::<SeaConditions>()
            .init_resource::<Assets<SeaMaterial>>()
            .add_systems(Update, settle_conditions);
        app
    }

    fn tell(app: &mut App, wind: Vec2) {
        app.world_mut().resource_mut::<Forecast>().wind = Some(wind);
    }

    fn drawn(app: &App) -> (Vec2, [Slot; WAVES.len()]) {
        let conditions = app.world().resource::<SeaConditions>();
        (conditions.wind, conditions.slots)
    }

    #[test]
    fn the_first_forecast_lands_whole_and_the_rest_ease() {
        let mut app = settle_app();
        // Frames with no forecast leave the assumed day exactly alone.
        for _ in 0..10 {
            app.update();
        }
        assert_eq!(drawn(&app).0, ASSUMED_WIND);

        // The first word of real weather replaces it outright: until it
        // came the client was drawing a guess, and easing from a guess to
        // the truth animates a change of weather that never happened.
        let told = Vec2::new(0.0, 12.0);
        tell(&mut app, told);
        app.update();
        let (wind, slots) = drawn(&app);
        assert_eq!(wind, told, "the first forecast was eased, not snapped");
        assert!(
            slots[0].heading.dot(Vec2::Y) > 0.9999,
            "the primary train did not re-aim with the snap"
        );

        // The second is weather changing, and lands as weather: the drawn
        // wind leaves where it was but takes its time getting there.
        let veered = Vec2::new(12.0, 0.0);
        tell(&mut app, veered);
        app.update();
        let (wind, _) = drawn(&app);
        assert_ne!(wind, told, "the sea ignored the new forecast");
        assert!(
            wind.distance(veered) > 1.0,
            "a later forecast landed as a snap"
        );
    }

    #[test]
    fn a_veering_wind_never_dies_on_the_way_round() {
        // The drawn wind is the only wind anything on screen can see, so the
        // light-air floor the weather holds has to survive the ease. It did
        // not while the ease was a lerp of the two vectors: told to reverse,
        // the drawn wind went round by way of the origin, and for a few
        // seconds in the middle the arm left the card and the sails — which
        // taper under the bar — stopped a boat the weather was still blowing
        // on. Reverse the wind over and over, starting from an exact
        // about-turn, and watch every frame of it.
        let mut app = settle_app();
        let mut told = Vec2::new(0.0, 1.6);
        tell(&mut app, told);
        app.update();
        for turn in 0..8 {
            told = -Vec2::from_angle(turn as f32 * 0.37).rotate(told);
            tell(&mut app, told);
            for _ in 0..400 {
                app.update();
                let wind = drawn(&app).0;
                assert!(
                    wind.length() > WIND_NAMED,
                    "the drawn wind fell to {} m/s crossing a veer",
                    wind.length()
                );
            }
        }
    }

    #[test]
    fn an_ordered_calm_dies_without_spinning() {
        // The console can order a flat calm, and a calm is the one wind with
        // no bearing to turn towards: the drawn wind holds the one it had and
        // simply goes out, which is a wind dropping rather than an instrument
        // spinning — and no NaN heading on the way. Then a wind ordered back
        // up has to grow out of nothing along the new bearing, the drawn wind
        // by then having no bearing of its own worth keeping.
        let mut app = settle_app();
        tell(&mut app, Vec2::new(0.0, 6.0));
        app.update();

        tell(&mut app, Vec2::ZERO);
        for _ in 0..5_000 {
            app.update();
            let wind = drawn(&app).0;
            assert!(wind.is_finite(), "the calm drew {wind}");
            assert_eq!(wind.x, 0.0, "the wind wandered off its bearing dying");
            assert!(wind.y >= 0.0, "the wind died through the other side");
        }
        assert!(
            drawn(&app).0.length() < 0.05,
            "the ordered calm never arrived"
        );

        let back = Vec2::new(-4.0, 0.0);
        tell(&mut app, back);
        for _ in 0..5_000 {
            app.update();
        }
        let wind = drawn(&app).0;
        assert!(wind.distance(back) < 0.05, "the wind came back as {wind}");
    }

    #[test]
    fn a_wave_train_only_turns_while_it_cannot_be_seen() {
        // The invariant the re-aiming dance exists for: a drawn wave's
        // direction never changes while the wave is visible, because
        // rotating a live wave rephases the whole ocean at once. Blow the
        // wind right around and watch every frame of the sea following it:
        // any frame whose heading moved must have been all but invisible
        // the frame before.
        let mut app = settle_app();
        tell(&mut app, Vec2::new(7.0, 0.0));
        app.update();

        tell(&mut app, Vec2::new(0.0, 7.0));
        let (_, mut before) = drawn(&app);
        let mut turned = 0;
        for _ in 0..4_000 {
            app.update();
            let (_, after) = drawn(&app);
            for (was, is) in before.iter().zip(&after) {
                if was.heading != is.heading {
                    turned += 1;
                    // The frame that turns is the frame the fade crossed the
                    // threshold, so the new heading's first rendered frame is
                    // under it — and the last of the old heading showed only
                    // a hair more.
                    assert!(
                        is.dim < 0.03,
                        "a new heading first rendered at {} of its height",
                        is.dim
                    );
                    assert!(
                        was.dim < 0.04,
                        "a train turned straight out of plain sight ({})",
                        was.dim
                    );
                }
            }
            before = after;
        }

        // And the dance actually happened and resolved: every train ended
        // within the hysteresis of its aim off the new wind — a slot that
        // re-aims mid-veer keeps whatever residual drift fits inside
        // [`REAIM`], which is the deal that stops a wandering wind re-aiming
        // it forever — and at full height again.
        assert!(turned >= WAVES.len(), "the sea never followed the wind");
        let (_, slots) = drawn(&app);
        for (i, slot) in slots.iter().enumerate() {
            let desired = Vec2::from_angle(WAVES[i].0).rotate(Vec2::Y);
            assert!(
                slot.heading.dot(desired) > (REAIM.0 + 0.05).cos(),
                "train {i} settled aimed {} off the wind, past its hysteresis",
                slot.heading.angle_to(desired)
            );
            assert!(slot.dim > 0.9, "train {i} never came back up");
        }
    }

    #[test]
    fn the_wavelengths_clear_the_mesh() {
        // A wave shorter than two spacings falls between the vertices and
        // aliases; three per crest is where it stops looking like shimmer.
        for (_, wavelength, _) in WAVES {
            assert!(
                wavelength >= 3.0 * SPACING,
                "a {wavelength} m wave is under-sampled at {SPACING} m spacing"
            );
        }
    }

    #[test]
    fn the_bend_leaves_the_waves_sampled() {
        // The bend buys its meander by varying every train's phase gradient,
        // which is to say by stretching and squeezing wavelengths. Squeezed
        // too far and the shortest train falls under three vertices to a
        // crest and aliases into the shimmer the whole file is written to
        // avoid — so the bend's steepness is not a free parameter, and this
        // is the arithmetic that bounds it.
        //
        // A train's phase shift is the longest train's wavenumber times its
        // heading against the bend, so the worst the gradient can be pushed
        // is that wavenumber times the bend's own steepest slope.
        // Every sine at its steepest at once, and the bend's two components
        // pulling the same way — neither ever happens, which is what makes
        // this a bound rather than a measurement.
        let steepest: f32 = BEND
            * BENDS
                .iter()
                .map(|(k, weight)| weight * k.length())
                .sum::<f32>();
        let longest = TAU / WAVES[0].1;

        for (_, wavelength, _) in WAVES {
            let squeezed = TAU / (TAU / wavelength + longest * steepest);
            assert!(
                squeezed >= 3.0 * SPACING,
                "the bend squeezes a {wavelength} m wave to {squeezed} m, \
                 under the {} m the mesh can draw",
                3.0 * SPACING
            );
        }
    }

    #[test]
    fn the_waves_run_long_to_short() {
        // The shader reads `waves[0]` as *the* wave — the one whose leading
        // face the whitecaps lie along — and that is only the sea's own
        // reading of itself while the longest train is first. Reorder these
        // and the caps would quietly start following the chop instead, which
        // is a change nothing else in either file would notice.
        let lengths: Vec<f32> = WAVES.iter().map(|(_, wavelength, _)| *wavelength).collect();
        assert!(
            lengths.windows(2).all(|pair| pair[0] > pair[1]),
            "the trains run {lengths:?}, not longest first"
        );
    }

    #[test]
    fn a_calm_sea_wears_no_whitecaps() {
        // The caps' bar is a height, so what keeps a calm clean is simply
        // that the whole swell is shorter than it. Nothing anywhere says
        // "no wind, no foam" — this is where that comes from, and it holds
        // for every wind up to the one the bar is written for.
        // The bar wanders, so what has to clear the sea is its *lowest*
        // reach — the floor less the swing, which is itself a fraction of
        // the sea's own height. A light air has to be under that too, or a
        // day nobody would call windy would still be flecked with white.
        for wind in [0.4, 3.0] {
            let light = SeaConditions {
                wind: Vec2::new(wind, 0.0),
                ..assumed()
            };
            let heaps = ceiling(&light);
            assert!(
                heaps < WHITECAP.1 - WHITECAP.2 * heaps,
                "a {wind} m/s sea heaps to {heaps}, over the bar's lowest reach"
            );
        }
        // And a blow has to be able to clear it, or the caps would be
        // scenery that never appears.
        let blow = SeaConditions {
            wind: Vec2::new(14.0, 0.0),
            ..assumed()
        };
        assert!(ceiling(&blow) > WHITECAP.1 * 2.0);
    }

    #[test]
    fn the_fade_ends_inside_the_fine_grid() {
        // The flat rim shares vertices with the fine grid's border; those
        // border vertices must have stopped moving or the two crack apart.
        assert!(FADE.1 < REACH);
    }

    #[test]
    fn the_mesh_seams_cannot_crack() {
        // The reach is a whole number of spacings — the last fine cell ends
        // exactly at REACH, where the rim begins.
        assert_eq!(REACH % SPACING, 0.0);
    }

    #[test]
    fn the_mesh_travels_two_cells_at_a_time() {
        // The travelling mesh may only land where the checkerboard of
        // diagonals maps onto itself: on the lattice, an even number of
        // cells from the origin. An odd landing keeps every vertex on a
        // lattice site — the swell holds perfectly still — while flipping
        // every facet's split, which is the whole sea's shading jumping
        // with each step of the camera: worst in a gale, absent at anchor,
        // and exactly the kind of wrong a vertex-level test cannot see.
        for i in 0..1_000 {
            let snapped = snap(i as f32 * 1.7 - 850.0);
            let cells = (snapped / SPACING).round() as i64;
            assert_eq!(
                snapped,
                cells as f32 * SPACING,
                "{snapped} is off the lattice"
            );
            assert_eq!(cells % 2, 0, "{snapped} lands an odd {cells} cells out");
        }
    }

    #[test]
    fn the_depth_window_outreaches_the_swell() {
        // Everything that moves fades out inside the window, wherever the
        // camera has drifted since the last scroll — so no moving water is
        // ever reading depth from beyond the window's edge.
        assert!(DEPTH_EXTENT / 2.0 > FADE.1 + RECENTER + SPACING);
    }

    #[test]
    fn the_shader_reads_the_uniform_in_the_order_it_is_written() {
        // The order of the fields *is* the uniform's layout, and only one
        // side of it is compiled here. A shader naming them in a different
        // order reads every field after the difference as its neighbour's —
        // which draws a picture that is wrong everywhere and blames nothing:
        // the sea dimmed by a wake bound read as an hour, say.
        //
        // Both lists are taken from the source rather than written out here,
        // so this holds them to each other rather than to a third copy that
        // could go stale on its own.
        let shader = std::fs::read_to_string(format!(
            "{}/../../assets/shaders/sea.wgsl",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("the sea's shader under assets/shaders/");
        let rust = include_str!("sea.rs");

        let fields = |source: &str, from: &str, ends: &str| -> Vec<String> {
            source
                .split_once(from)
                .expect("the struct the uniform is packed from")
                .1
                .split_once(ends)
                .expect("the end of it")
                .0
                .lines()
                .filter_map(|line| {
                    let line = line
                        .trim()
                        .strip_prefix("pub(crate) ")
                        .unwrap_or(line.trim());
                    let (name, rest) = line.split_once(':')?;
                    // A field, not a doc line or an attribute: a bare name
                    // followed by a type.
                    let named = !name.is_empty()
                        && name
                            .chars()
                            .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit());
                    (named && !rest.trim().is_empty()).then(|| name.to_string())
                })
                .collect()
        };

        // The texture and its sampler are bindings of their own rather than
        // part of the packed block, so the Rust side runs one longer.
        let packed = fields(rust, "pub struct SeaExtension {", "\n}");
        let read = fields(&shader, "struct SeaParams {", "\n}");
        assert!(
            !read.is_empty(),
            "no fields found in the shader's SeaParams"
        );
        assert_eq!(
            packed[..read.len()],
            read[..],
            "Rust packs {packed:?} and the shader reads {read:?}"
        );
    }

    #[test]
    fn a_depth_survives_its_texel() {
        // A byte holds the whole working range to better than the height
        // quantisation; dry land is zero water, and ground the client has
        // not been sent reads as the deepest water there is.
        assert_eq!(texel(Some(2.0), None)[0], 0);
        assert_eq!(texel(None, None)[0], u8::MAX);
        let depth = 3.7;
        let byte = texel(Some(-depth), None)[0];
        let decoded = byte as f32 / 255.0 * DEPTH_RANGE;
        assert!(
            (decoded - depth).abs() < 0.03,
            "{depth} m came back {decoded} m"
        );
    }

    #[test]
    fn a_texel_carries_the_light_beside_the_depth() {
        // The two things the sea learns from the ground under it, in one
        // texel. Ground the client has not been sent is lit the whole day:
        // what it has never heard of cannot be shadowing water it can see.
        let shadowed = [80, 150];
        assert_eq!(texel(Some(-3.0), Some(shadowed))[1..3], shadowed);
        assert_eq!(texel(None, None)[1..3], LIT_ALL_DAY);
        assert_eq!(texel(Some(-3.0), None)[1..3], LIT_ALL_DAY);
    }

    #[test]
    fn the_sweep_divides_the_window() {
        // What lets `refresh_depth` treat every sweep as one contiguous
        // block: the rows per frame divide the rows there are, so a sweep
        // never straddles the wrap back to row zero.
        assert_eq!(DEPTH_TEXELS % SWEEP_ROWS, 0);
    }

    #[test]
    fn scrolling_keeps_texels_over_their_ground() {
        // A scroll is the window moving, not the world: after shifting, the
        // texel now over a world point holds the byte the old texel over
        // that point held, and the strip that slid in from beyond is deep.
        let n = DEPTH_TEXELS;
        let mut data: Vec<u8> = (0..n * n * TEXEL_BYTES).map(|i| (i % 251) as u8).collect();
        let before = data.clone();
        let step = IVec2::new(3, -2);
        scroll(&mut data, step);

        // A texel well inside both windows — the whole texel, so a scroll
        // that carried the depth and dropped the light would show.
        let texel = |x: usize, y: usize| (y * n + x) * TEXEL_BYTES;
        let (x, y) = (100usize, 100usize);
        let to = texel(x, y);
        let from = texel((x as i32 + step.x) as usize, (y as i32 + step.y) as usize);
        assert_eq!(data[to..to + TEXEL_BYTES], before[from..from + TEXEL_BYTES]);
        // A texel the scroll exposed, which reads as water nothing is known
        // about: deep, and lit the whole day.
        let exposed = texel(n - 1, 0);
        assert_eq!(data[exposed..exposed + TEXEL_BYTES], UNREAD);
    }
}
