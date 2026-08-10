//! The swell: the waves the sea wears, and everything that has to agree on
//! them.
//!
//! The sea is drawn as low-poly waves — real geometry, displaced in the vertex
//! shader and shaded flat, so a wave is a run of tilting facets like everything
//! else in the world rather than a normal-mapped shimmer. On a surface as
//! matte as this water a normal map would barely read anyway: there is no
//! specular for it to perturb, and what sells the motion instead is facets
//! changing tone as they tilt, and the waterline creeping up and down every
//! beach as the surface rises and falls through the shore.
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
//! over to a single longer shore wave whose *phase is the depth itself*:
//! its crests are the depth contours, which by definition run parallel to
//! whatever shore they approach, so waves wrap into bays and meet every
//! beach face-on without any refraction being computed. That trick is why
//! the shore wave has no heading — steering a directional wave by depth
//! means integrating phase along its path, which has no honest local answer,
//! while a contour is already the shape refraction bends a crest into. The
//! shore wave's height is capped by the water it stands in, the way breaking
//! caps a real one, and the fragment shader paints foam where the cap is
//! biting on a crest.
//!
//! Depth reaches the shader through [`DepthWindow`]: a coarse byte-per-texel
//! picture of the water depth around the camera, refilled a few rows a frame
//! from the ground chunks the server has sent. The client is not generating
//! anything here — it is reading the very heights it was given to draw, the
//! same way it builds meshes from them.
//!
//! Three parties have to agree on where the water stands at a moment: the
//! shader displacing the sea mesh, the boat riding on it, and the markers
//! other players stand as. The parameters live once, in this file, and reach
//! the shader through a uniform so they cannot drift from the Rust side; the
//! *formula* — [`swell`] — is written twice, here and in
//! `assets/shaders/sea.wgsl`, and the two must be kept the same. Time is the
//! other half of the agreement: the shader reads `globals.time`, which Bevy
//! fills from `Time::elapsed_secs_wrapped`, so that is what every Rust
//! caller of [`swell`] must pass. Depth is the last part, and there the
//! agreement is deliberately loose: the shader reads the windowed texture,
//! the boat asks the ground exactly, and the two differ by at most a texel
//! of interpolation in water where the swell is smallest.
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
    /// `x` and `y` are [`FADE`], `z` is [`SHADING_TILT`]; `w` is padding.
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
    /// `xy` is [`FOAM_FEED`]; `zw` is padding.
    #[uniform(100)]
    feed: Vec4,
    /// The depth window's place in the world: `xy` the world coordinates of
    /// its corner texel's corner, `z` `1 / DEPTH_EXTENT`, `w`
    /// [`DEPTH_RANGE`]. Rewritten whenever the window scrolls.
    #[uniform(100)]
    window: Vec4,
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
            fade: Vec4::new(FADE.0, FADE.1, SHADING_TILT, 0.0),
            shore: Vec4::new(
                TAU / CREST_EVERY,
                TAU / SHORE_PERIOD,
                conditions.shore_amplitude(),
                BREAK_SLOPE,
            ),
            surf: Vec4::new(SHOAL.0, SHOAL.1, RUNUP, FOAM_CREST),
            stagger: Vec4::new(stagger_vector().x, stagger_vector().y, FOAM_SLOPE, SPACING),
            feed: Vec4::new(FOAM_FEED.0, FOAM_FEED.1, 0.0, 0.0),
            window: Self::window_uniform(origin),
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
    /// A wind ordered up from the `--debug` keys, outranking the server's
    /// word for as long as it is set — see [`crate::debug`]. Local by
    /// construction: it doctors what this renderer draws and nothing anybody
    /// else sails on. The server's forecast keeps landing in `wind`
    /// underneath, so lifting the order eases straight back to the truth
    /// rather than to wherever the truth was when the order was given.
    pub commanded: Option<Vec2>,
}

impl Forecast {
    /// The wind the sea should be settling towards: the ordered one while a
    /// debug key holds it, otherwise whatever the server last said.
    fn told(&self) -> Option<Vec2> {
        self.commanded.or(self.wind)
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
        let deep: f32 = self
            .components()
            .iter()
            .map(|wave| wave.w * (wave.xy().dot(at) - wave.z * elapsed).sin())
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
        if told.length() > 0.5 {
            let bearing = told.normalize();
            for (i, slot) in conditions.slots.iter_mut().enumerate() {
                slot.heading = Vec2::from_angle(WAVES[i].0).rotate(bearing);
            }
        }
    }

    let follow = crate::eased(1.0 / SEA_RESPONSE, time.delta_secs());
    let fade = crate::eased(1.0 / REAIM.1, time.delta_secs());
    conditions.wind = conditions.wind.lerp(told, follow);

    // A dying wind names no bearing: below half a metre a second the slots
    // hold the heading they had, and the calm is carried by the amplitudes.
    let steady = conditions.wind.length() > 0.5;
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

    /// The window origin that centres the window on a focus, on the lattice.
    fn origin_under(focus: Vec2) -> Vec2 {
        Vec2::new(snap(focus.x), snap(focus.y)) - DEPTH_EXTENT / 2.0
    }
}

/// A depth window with nothing in it yet: every texel deep, so the sea wears
/// the open swell everywhere until the sweep has read the actual ground.
pub fn depth_image() -> Image {
    let mut image = Image::new_fill(
        Extent3d {
            width: DEPTH_TEXELS as u32,
            height: DEPTH_TEXELS as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[u8::MAX],
        TextureFormat::R8Unorm,
        RenderAssetUsages::default(),
    );
    // Bilinear, so a facet between two texels gets water between their
    // depths rather than one or the other's.
    image.sampler = ImageSampler::linear();
    image
}

/// One texel's worth of depth. Ground the client has not been sent reads as
/// the deepest water a byte can say — see [`DEPTH_RANGE`].
fn depth_byte(height: Option<f32>) -> u8 {
    match height {
        None => u8::MAX,
        Some(height) => ((-height).clamp(0.0, DEPTH_RANGE) / DEPTH_RANGE * 255.0).round() as u8,
    }
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
    let mut rows = [0u8; DEPTH_TEXELS * SWEEP_ROWS];
    for r in 0..SWEEP_ROWS {
        let row = window.sweep + r;
        for x in 0..DEPTH_TEXELS {
            let at = window.origin + Vec2::new(x as f32 + 0.5, row as f32 + 0.5) * SPACING;
            rows[r * DEPTH_TEXELS + x] = depth_byte(ground.height(at.x, at.y));
        }
    }
    let start = window.sweep * DEPTH_TEXELS;
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
    for y in 0..n {
        for x in 0..n {
            let from = IVec2::new(x, y) + step;
            data[(y * n + x) as usize] = if (0..n).contains(&from.x) && (0..n).contains(&from.y) {
                old[(from.y * n + from.x) as usize]
            } else {
                u8::MAX
            };
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
        let deep: f32 = sea
            .components()
            .iter()
            .map(|wave| wave.w * (wave.xy().dot(at) - wave.z * elapsed).sin())
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
    fn a_commanded_wind_outranks_the_forecast_and_hands_back() {
        let mut app = settle_app();
        // An order given before any word of weather still snaps the assumed
        // day away: whichever way the first real wind arrives, the sea being
        // watched is the one asked for.
        let ordered = Vec2::new(16.0, 0.0);
        app.world_mut().resource_mut::<Forecast>().commanded = Some(ordered);
        app.update();
        assert_eq!(drawn(&app).0, ordered);

        // A forecast landing under the order changes nothing on screen...
        tell(&mut app, Vec2::new(0.0, 12.0));
        for _ in 0..200 {
            app.update();
        }
        assert_eq!(drawn(&app).0, ordered);

        // ...but is kept, so lifting the order eases back to the server's
        // truth as it stands now — not a snap, this being weather like any
        // other change of it.
        app.world_mut().resource_mut::<Forecast>().commanded = None;
        app.update();
        assert_ne!(drawn(&app).0, ordered, "the sea ignored the lifted order");
        assert!(
            drawn(&app).0.distance(Vec2::new(0.0, 12.0)) > 1.0,
            "the lifted order landed as a snap"
        );
        for _ in 0..4_000 {
            app.update();
        }
        assert!(drawn(&app).0.distance(Vec2::new(0.0, 12.0)) < 0.5);
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
    fn a_depth_survives_its_texel() {
        // A byte holds the whole working range to better than the height
        // quantisation; dry land is zero water, and ground the client has
        // not been sent reads as the deepest water there is.
        assert_eq!(depth_byte(Some(2.0)), 0);
        assert_eq!(depth_byte(None), u8::MAX);
        let depth = 3.7;
        let byte = depth_byte(Some(-depth));
        let decoded = byte as f32 / 255.0 * DEPTH_RANGE;
        assert!(
            (decoded - depth).abs() < 0.03,
            "{depth} m came back {decoded} m"
        );
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
        let mut data: Vec<u8> = (0..n * n).map(|i| (i % 251) as u8).collect();
        let before = data.clone();
        let step = IVec2::new(3, -2);
        scroll(&mut data, step);

        // A texel well inside both windows.
        let (x, y) = (100, 100);
        let from = ((y + step.y) as usize * n) + (x + step.x) as usize;
        assert_eq!(data[y as usize * n + x as usize], before[from]);
        // A texel the scroll exposed.
        assert_eq!(data[n - 1], u8::MAX);
    }
}
