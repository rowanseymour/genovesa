//! Procedural terrain: the height field and what grows on it. Sampling that
//! into chunks a client can be sent is the `archipelago` module; turning what
//! arrives into meshes is the game's business and happens nowhere near here.
//!
//! # Scale
//!
//! One world unit is one metre. One terrain tile is one metre square, and a map
//! is a whole number of [`CHUNK_TILES`]-tile chunks along each axis — any
//! number on each, from a single chunk up, square or not.
//!
//! The tile is the map's unit of ground, not the picture's: the height field is
//! continuous, and it is drawn every [`protocol::ground::CELL_METRES`]. See
//! [`TerrainGenerator::material`] for why the palette is what it is — the
//! ground is flat shaded in a fixed set of colours, and both the cells and the
//! colour bands want to be large enough to read as deliberate shapes.
//!
//! The colours themselves are not here. Ground is *named* — see
//! [`protocol::ground::Material`] — because what a square metre is made of has
//! to cross the wire, and a name is a byte where three floats are twelve. This
//! module decides which name; the protocol says what each one looks like.

use glam::{UVec2, Vec2, Vec3};
use protocol::ground::{Material, CELLS, CELL_COUNT, CELL_METRES, CHUNK_METRES, CORNERS};

use crate::noise::{smoothstep, Noise};

/// `x` to the power `y`, the same way on every machine.
///
/// Not `f32::powf`, which is the whole point of this function existing. Add,
/// multiply and square root are pinned by IEEE 754, but powers are not:
/// `f32::powf` is whatever libm the platform linked, and Apple's, glibc's and
/// Microsoft's do not agree to the last bit. A seed is written down, shared,
/// and handed to another machine to host, so if `powf` decided what the ground
/// was, one seed served from a Mac and from a Linux box would be two different
/// oceans wearing one name.
///
/// That was not a theory: the first CI run across three operating systems came
/// back with three different maps for seed 20040112.
///
/// The implementation is MUSL's, ported to plain Rust — no platform dispatch
/// and no hardware instruction to disagree about. Bit-for-bit stability across
/// *versions* of it is not promised either, which is what `--locked` and the
/// digest tests are for.
pub(crate) fn pow(x: f32, y: f32) -> f32 {
    libm::powf(x, y)
}

/// Metres per terrain tile — the map's unit of ground, and the spacing the
/// height field is sampled at for ground queries.
pub const TILE_SIZE: f32 = 1.0;

/// Tiles (so, metres) along the edge of one terrain chunk — a sensible unit of
/// both culling and rebuilding, and the unit ground is asked for in.
///
/// Read from the protocol rather than declared here, the chunk being the
/// *client's* unit before it is the generator's: a client asks for ground by
/// chunk coordinate, so how big one is belongs with the words for asking.
pub const CHUNK_TILES: u32 = (CHUNK_METRES / TILE_SIZE) as u32;

// --- What every map is aiming for ---------------------------------------
//
// Noise does not hand you a landscape with a given amount of anything in it. A
// seed that happens to sit low in its own field gives a drowned map, one that
// sits high gives a featureless plateau, and thresholds picked to suit one give
// nonsense on the other. So the shape of the land comes from noise, and *how
// much* of each kind there is comes from these — fitted per seed by
// [`Calibration`], which samples the raw field and solves for the sea level and
// height curve that hit them.

/// Share of the map that comes out as land. Kept near a third rather than
/// half: land is what there is *less* of, so sea level sits high enough in the
/// field that its shape shows — bays flood, low saddles become sounds between
/// islands, and hollows inland hold water. At 0.42 the map was one solid
/// island with a rim of shore and the falloff's rounded-square outline showed
/// through on small maps; what stopped it going lower was the coastal band
/// being fixed in metres, which [`TerrainGenerator::fit_coast_scale`] now
/// fits per seed.
const LAND_FRACTION: f32 = 0.34;

/// The land share on maps too small for the landmass field to break up. Their
/// land arrives as a single blob — the field barely completes a cycle across
/// the map — and a lone blob holding a third of a square has the same radius
/// as the falloff ring, so it presses into the frame and comes out
/// squircle-shaped however the noise wanders. Shrinking the target is what
/// buys the blob enough clearance for the noise to draw its outline. Larger
/// maps split their land into several lobes with sea between, so each lobe
/// clears the ring on its own and the full share is safe.
const LAND_FRACTION_SMALL: f32 = 0.28;

/// The land share on the smallest maps of all, where even the shrunken blob
/// fills the falloff ring's whole interior. Down there the noise has no
/// wavelength short enough to texture the outline, so a coast anywhere near
/// the ring is *drawn by* the ring and every islet comes out a circle. This
/// pulls the sea-level contour well inside the ring, onto the flat of the
/// falloff, where the noise's short octaves are the steepest thing left and
/// get to draw the outline instead.
const LAND_FRACTION_TINY: f32 = 0.16;

/// How big a map is, for everything that scales with size: the geometric mean
/// of its two extents, in metres. The geometric mean and not the smaller or
/// the average, because what the size tapers below actually ration — lobes of
/// landmass, room for a mountain's climb, cells for the depth fit to read —
/// all go with the map's *area*, so a long thin map earns back along its
/// length what it gave up across its width. (The falloff is the deliberate
/// exception: its bend and reach are frame clearances, and measure the tight
/// axis on its own.)
fn mean_extent(extent: Vec2) -> f32 {
    (extent.x * extent.y).sqrt()
}

/// How far the sea-depth anchor slides from the sea's median towards its
/// shallow side as the map shrinks: 0 leaves the [`Calibration`] knee on the
/// median, 1 moves it to the shallowest sixth or so of the sea.
///
/// The smallest maps need it moved, their sorted field being nearly radial
/// order: over half of any map is falloff ring, and with no noise to interleave
/// the two, every quantile from the median down lands *in* the ring. Anchored
/// there, the whole visible lagoon maps to centimetres of water — the bullseye
/// that made every small map read as a circle. Anchoring on the shallow side
/// puts the knee among the cells the player actually sees. On maps with
/// wavelengths to spare the median anchor is both safe and better (a
/// shallow-side anchor there bent healthy seeds and starved their beaches), so
/// it fades out entirely before the preset sizes.
fn shoal_shift(extent: Vec2) -> f32 {
    let cycles = mean_extent(extent) / CONTINENT_SCALE;
    1.0 - smoothstep(0.35, 1.0, cycles)
}

/// A per-seed draw in `0.0..1.0`, unrelated to any noise field's phase —
/// splitmix64 on the seed and a salt, the same mix [`crate::archipelago::ParcelRng`]
/// uses to keep unrelated draws from one seed unrelated to each other.
///
/// What this is for is [`land_fraction`]'s tiny-map swing: two islands the
/// same handful of chunks across should not be the same amount of island. A
/// spatial noise field cannot supply that — at one chunk across every field
/// here is near its own lowest octave, so the land share barely varies seed to
/// seed and only *where* the cut falls does.
fn seed_draw(seed: u32, salt: u32) -> f32 {
    let mut z = (seed as u64) ^ ((salt as u64) << 32);
    z = z.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 40) as f32 * (1.0 / (1u64 << 24) as f32)
}

/// Salts [`seed_draw`] for [`land_fraction`]'s swing, so it draws unrelated
/// numbers from a seed already spent on the noise fields' own offsets.
const LAND_LUCK_SALT: u32 = 0x6C61_6E64; // "land" in ASCII, no meaning beyond being unlike the other salts

/// How far [`land_fraction`]'s swing may multiply the tiny-map target down
/// or up, at its strongest.
///
/// Asymmetric on purpose. Pulled down to a third of the target, a one-chunk
/// island is mostly bare sand and standing water — the shifting balance real
/// skerries show, which a fixed target could never give. Pushed up it only
/// reaches back towards [`LAND_FRACTION_SMALL`] and no further: the tiny share
/// exists for the falloff ring rather than for variety, and a swing that
/// overshot would print the ring's own outline on the lucky seeds.
const LAND_SWING_LOW: f32 = 0.34;
const LAND_SWING_HIGH: f32 = 1.35;

/// The land share for a given map extent and seed, continuous in the extent
/// so nothing jumps as a size control sweeps through it. Measured in how many
/// times the landmass field repeats across the map, since that is what
/// decides what the land can be: down near a fifth of a repeat the coast has
/// to fit inside the falloff ring and gets [`LAND_FRACTION_TINY`], up to
/// about one repeat the land is a single blob and gets [`LAND_FRACTION_SMALL`],
/// and by one and a half it is lobed enough to carry the full
/// [`LAND_FRACTION`].
///
/// Below one repeat the target is also swung per seed between
/// [`LAND_SWING_LOW`] and [`LAND_SWING_HIGH`] of itself — see [`seed_draw`] —
/// fading out over the same stretch [`LAND_FRACTION_TINY`] does, so a healthy
/// map's tuned share is never touched. Without it every seed at one size came
/// out the same amount of island, which with the shape barely varying either
/// (see [`feature_zoom`]) made every small map the same lozenge of grass.
fn land_fraction(extent: Vec2, seed: u32) -> f32 {
    let cycles = mean_extent(extent) / CONTINENT_SCALE;
    let target = LAND_FRACTION_TINY
        + (LAND_FRACTION_SMALL - LAND_FRACTION_TINY) * smoothstep(0.15, 0.7, cycles)
        + (LAND_FRACTION - LAND_FRACTION_SMALL) * smoothstep(1.2, 1.5, cycles);

    let swing = 1.0 - smoothstep(0.15, 0.7, cycles);
    let luck = seed_draw(seed, LAND_LUCK_SALT);
    let multiplier =
        1.0 + swing * (LAND_SWING_LOW - 1.0 + (LAND_SWING_HIGH - LAND_SWING_LOW) * luck);
    target * multiplier
}

/// Share of that land standing high enough to count as mountain.
const MOUNTAIN_FRACTION: f32 = 0.11;

/// Height at which land counts as mountain, in metres — where the ground goes
/// bare and a range starts reading as a range from across the map.
const MOUNTAIN_HEIGHT: f32 = 45.0;

/// Height at which the trees give out and the ground turns to moor, in metres.
/// Set higher than where the treeline is actually wanted, to pay for the wander
/// below it: there is far more land low down than high up, so an edge swinging
/// [`BAND_WANDER`] either way takes in a good deal more ground on its way down
/// than it gives back on its way up. Without the compensation the moor spreads
/// over most of the island.
const MOOR_HEIGHT: f32 = 37.0;

/// Height of the tallest peaks above sea level, in metres. True by
/// construction, not by hope: [`TerrainGenerator::fit_range_height`] scales
/// every seed's massif until its summit lands here. Left to the noise this is
/// the widest-spread number on the map — the same settings gave one seed a
/// hundred metres and another nearly three hundred — and it is most of what
/// makes one map feel tame and the next absurd.
const HEIGHT_SCALE: f32 = 125.0;

/// The steepest a map may climb from its waterline to its summit, in metres of
/// height per metre of ground. Derived rather than tuned: it is exactly the
/// pitch [`INLAND_REACH`] and [`HEIGHT_SCALE`] already imply between them, so
/// that the rule saying *where* a mountain may stand and the rule saying *how
/// high* agree instead of each having its own idea of how steep land gets.
const PEAK_GRADE: f32 = HEIGHT_SCALE / INLAND_REACH;

/// The tallest peak a map is fitted to as a share of its geometric-mean
/// extent, which caps [`HEIGHT_SCALE`] on small maps. A mountain is mostly
/// climb, and a map has to have room for the climb: fitted to the full height,
/// a map a couple of hundred metres across comes out as one grey cone with a
/// beach — all flank, no country.
///
/// This is what the massif is *aimed* at, not what a map comes out with;
/// [`TerrainGenerator::ceiling`] has the last word, and on most maps it takes
/// something off. The two are not doing the same job. This one keeps the range
/// term a sane share of the field, which has to be settled before there is a
/// map to measure; the ceiling decides what the ground has earned, which
/// cannot be known until there is.
const PEAK_PITCH: f32 = 0.18;

/// What this map's mountains are fitted to, in metres: the [`PEAK_PITCH`]
/// share of its extent, up to the full [`HEIGHT_SCALE`].
fn peak_height(extent: Vec2) -> f32 {
    (PEAK_PITCH * mean_extent(extent)).min(HEIGHT_SCALE)
}

/// What one map is aiming for: the per-map values of the size tapers above,
/// computed once from the extent and read together by every fit. Nothing
/// needs them once the fits are done.
struct Targets {
    /// Share of the map that is land — [`land_fraction`].
    land: f32,
    /// Share of the full mountain height the map is fitted to —
    /// [`peak_height`] over [`HEIGHT_SCALE`], 1.0 on anything but a small map.
    relief: f32,
    /// How far the sea-depth anchor slides to the shallow side —
    /// [`shoal_shift`], 0 on anything but a small map.
    shoal: f32,
}

impl Targets {
    fn for_extent(extent: Vec2, seed: u32) -> Self {
        Self {
            land: land_fraction(extent, seed),
            relief: peak_height(extent) / HEIGHT_SCALE,
            shoal: shoal_shift(extent),
        }
    }
}

/// Share of the map that reaches the deepest the sea bed is allowed to go, so
/// that open water reads as open water rather than as one endless shelf.
const DEEP_FRACTION: f32 = 0.18;

/// What [`DEEP_FRACTION`] grows to on the smallest maps, sliding on the same
/// taper as [`shoal_shift`]. Their sea has to reach the floor *inside* the
/// frame: left at the open-water share, the deepest cells all sit in the
/// falloff ring, the whole visible lagoon stays shallow, and the shelf's
/// outer edge is drawn by the frame — a rounded rectangle pressed against
/// the chunk boundary, on every small island alike. Anchoring far more of
/// the map at full depth pulls the drop-off well inside the frame, where
/// the noise draws its line; the shelf that survives hugs the land instead,
/// and a one-chunk islet that comes out a bare rock with no bank at all has
/// paid the intended price.
const DEEP_FRACTION_SMALL: f32 = 0.45;

/// Depth the middle of the sea is guaranteed to reach, in metres — the mirror
/// of [`LOWLAND_FLOOR`]. A minority of seeds run flat just *below* where the
/// sea lands in the field, which the deep anchor cannot save: it pins one point
/// far down while the whole middle of the distribution sits centimetres under
/// the surface, and the map comes out one endless bright shelf. Set well past
/// the last of the shallow-water colours rather than at their edge, a median
/// pinned on the threshold leaving half the sea painted as shallows.
const SHALLOWS_FLOOR: f32 = 8.0;

/// Height the middle of the land is guaranteed to reach, in metres. A minority
/// of seeds put nearly all their land within a metre or two of sea level — the
/// field happens to run flat just above where the sea lands in it — and the
/// whole map paints as shore and reads as a drowned sandflat. Seeds whose land
/// median would map below this get the low half of their mapping steepened
/// until it lands here; seeds already above it are left exactly linear.
///
/// Kept well below the typical seed's median (16–25 m), so this is a floor for
/// the outliers and not a target every map gets pulled to — flattening the
/// difference between a low island and a high one would cost more variety than
/// the sandflats do.
const LOWLAND_FLOOR: f32 = 10.0;

// --- How far from the sea a mountain is allowed to be -----------------------
//
// Left to the mask field alone, a range sits wherever the noise puts it, and
// half the time that is on the coast: a wall of rock rising out of the water
// with no country behind it. Real land does not do that, and the reason is
// erosion. Ground is only so steep before it comes down, so height has to be
// *earned* over horizontal distance — which means the high ground is inland,
// and a coast is the bottom of a slope that started somewhere.
//
// So how far a point stands from the open sea sets a ceiling on how high it
// may be, and [`TerrainGenerator::ceiling`] holds the finished landform under
// it. Nothing decides where the ranges *go* — that is still the mask field's
// business, and the noise's. What changes is that a range which happens to
// land on a headland no longer gets to be a mountain there, because the ground
// under it has not climbed from anywhere.
//
// Two things this deliberately is not. It is not a term added to the height,
// which would raise the interior rather than lower the coast and drain every
// inland lagoon. And it is not applied to the massif before the height fit:
// that fit exists to guarantee a summit, so it answers any suppression by
// winding the range term up until the least coastal cell spikes — which on a
// ring-shaped island put the full height of rock fifty metres from the water.
// The ceiling has to be the last word, applied to metres, after everything
// else has had its say.

/// How far inland ground has to be, in metres, before it may stand at the full
/// [`HEIGHT_SCALE`]. Absolute rather than a share of the map, for the same
/// reason [`FEATURE_SCALE`] is: a bigger map should mean more landscape, not a
/// stretched copy of the same one.
///
/// This is the dial worth turning: it fixes [`PEAK_GRADE`], and with it how
/// tall a given map's mountains come out. A kilometre-square map's summits land
/// between about fifty-five and ninety metres rather than the full height,
/// falling short of the reach because a summit sits where the mask put the
/// massif and that is rarely the most interior point. Shortening this gives
/// every size taller mountains and steeper country.
///
/// It is a frankly generous number — a climb of about thirty degrees held the
/// whole way, where the steepest real islands manage a seventh of that. That is
/// the price of a map a kilometre across having mountains at all, and the
/// useful part is that height has to be paid for in ground.
const INLAND_REACH: f32 = 220.0;

/// How far out [`water_fraction`] looks when deciding whether a stretch of
/// water is sea or a pond, in metres. A little under half [`INLAND_REACH`], so
/// that the water bodies it counts as sea are the ones big enough to be worth
/// a range's height.
const OPEN_RADIUS: f32 = 80.0;

/// Where the share of water around a point cuts into pond and sea.
///
/// Well below a half on purpose, and that is the whole trick. Any symmetric
/// window straddling a shoreline sees about half water, so the open ocean's
/// own beach reads 0.5 — cut anywhere near there and the coastline itself
/// stops counting as coast, which is the one thing this field exists to
/// measure. Cut low instead and the reading separates by *size*: a shore of
/// open sea holds its half whichever way the window is nudged, while a pond
/// small enough to sit inside the window can never reach the lower edge at
/// all, however its middle is sampled.
const OPEN_POND: f32 = 0.15;
const OPEN_SEA: f32 = 0.35;

/// Share of the *land* a mountain massif covers, counting its flanks. Roughly
/// three times [`MOUNTAIN_FRACTION`], since most of a mountain is the climb
/// rather than the top of it.
///
/// A share of the land and not of the map, because the land is what it has to
/// stay in proportion to. Let it cover too much and the line where the
/// mountains start rides up with the range itself, so pushing the range harder
/// stops raising the summits at all — and a seed whose land is broken into
/// islands, where a footprint measured against the whole map is most of every
/// one of them, comes out with no mountains however hard it is pushed.
const RANGE_FOOTPRINT: f32 = 0.35;

/// Wavelength of the dominant landforms, in metres. Kept independent of the map
/// size so that a bigger map means *more* landscape rather than a stretched
/// version of the same landscape.
const FEATURE_SCALE: f32 = 400.0;

// --- How long the two fields that shape a map are ---------------------------
//
// These two decide the shape of the island and where its mountains sit, and
// what matters about them is not their absolute length but how many times they
// repeat across a map. A field whose wavelength is longer than the map cannot
// draw anything: it is a single smooth gradient by the time it gets there, and
// whatever it was supposed to shape falls to whatever else is in the sum. Both
// of these used to be around 1300 m against a default map of 1024 m, and it
// showed — the island's outline came from the falloff, which is a squircle, so
// every map was a rounded square, and the massif was one broad lobe, so every
// map had exactly one round grey lump for a mountain.
//
// At a bit over half the default map they repeat two or three times across it,
// which is enough to put bays down one coast and headlands along another, and
// to give a map two or three ranges instead of one blob. Much shorter and an
// island stops being an island.
//
// The map size these are judged against is the default 1024 m; a bigger map
// gets proportionally more of both, which is the intent of [`FEATURE_SCALE`].

/// Wavelength of the landmass field — the one that decides where there is land
/// at all, and so what shape the coastline is.
const CONTINENT_SCALE: f32 = 670.0;
/// The same, as the multiplier the field is actually sampled with — its inputs
/// are already in [`FEATURE_SCALE`] units.
const CONTINENT_FREQ: f32 = FEATURE_SCALE / CONTINENT_SCALE;

/// Wavelength of the field that decides where the mountains are. Kept the same
/// as the landmass field, since [`TerrainGenerator::range_seat`] adds a good
/// share of that field to this one and a range wants to be about as big a thing
/// as the land it sits on.
const MASSIF_SCALE: f32 = 670.0;
/// The same, in the units the field is sampled in. See [`CONTINENT_FREQ`].
const MASSIF_FREQ: f32 = FEATURE_SCALE / MASSIF_SCALE;

/// Radius, in metres, that a massif is measured against its neighbours over —
/// the window the local ceiling of the massif field is taken across. A little
/// over half [`MASSIF_SCALE`], so that every range's own summit is inside its
/// window, and the next range's summit — a wavelength away — mostly is not.
const MASSIF_WINDOW: f32 = 400.0;

/// How far [`TerrainGenerator::continent_at`] may sample the landmass field
/// ahead of its tuned wavelength, at its strongest.
///
/// [`FEATURE_SCALE`] and [`CONTINENT_SCALE`] are deliberately fixed lengths —
/// that is what makes a bigger map more landscape rather than a stretched
/// copy of a smaller one. But fixed also means a map under one repeat gets
/// none of the shaping this field exists to do: at a chunk across it is most
/// of the way to its own lowest octave, which is a smooth gradient across the
/// whole map — so the outline noise draws is close to a straight line, and a
/// straight line cut through the falloff's disc is a semicircle whichever way
/// it falls. That is the shape every one-chunk islet shared before this
/// existed, seed after seed.
///
/// Zooming in on the same field buys back cycles the fixed wavelength cannot
/// supply this small, so the outline gets bays and lobes instead of one chord.
/// Not the same fix as a bigger map: that earns *more* landscape spread
/// further apart, where this packs the existing texture tighter, which is the
/// only variety a frame this size can hold.
const FEATURE_ZOOM_MAX: f32 = 3.0;

/// The zoom [`TerrainGenerator::continent_at`] samples the landmass field
/// with — 1.0 once the map holds most of a [`CONTINENT_SCALE`] repeat, rising
/// towards [`FEATURE_ZOOM_MAX`] as the map shrinks below it. Opened over much
/// the stretch [`land_fraction`]'s own tapers cover, a shade wider at the
/// top: the zoom's last few per cent are a gentler change than a land-share
/// step, so it can afford to fade later, and at 512 m it is still worth a
/// couple of per cent of extra coastline cycle.
///
/// Left off the hills, the massif and the warp's own drift and bend
/// deliberately. The warp's guards are tapered for small maps against the
/// wavelength as fixed, and zooming what they read would detune both for
/// nothing. Zooming the hills was tried and cost more than it bought: hills
/// already sums down to a wavelength short enough to pit a small map with
/// hollows, and zoomed it pits more of them a few [`COAST_GRID`] cells wide —
/// which the lake flood then floods, and a lake that small comes out a
/// square-edged pond. The landmass field puts nothing at that scale and is the
/// field a coastline actually is, so it is the one worth zooming alone.
fn feature_zoom(extent: Vec2) -> f32 {
    let cycles = mean_extent(extent) / CONTINENT_SCALE;
    1.0 + (FEATURE_ZOOM_MAX - 1.0) * (1.0 - smoothstep(0.15, 0.8, cycles))
}

/// How much each massif is measured against its own local peak rather than
/// the map's tallest. At 0 the map is scaled by its single highest point, and
/// on a large map with a dozen massifs only that one reaches full height —
/// the rest sit lower by pure luck of the mask field. At 1 every blob that
/// clears the footprint reaches full height, however slight it is, and the
/// map turns into a picket of identical cones. In between, the lesser ranges
/// are lifted most of the way to parity while the luck of the field still
/// shows through. Set high because the massif term is squared: a range
/// measured at nine tenths of its neighbour stands at eight tenths the
/// height, so even near-parity here leaves a visible pecking order.
const MASSIF_EQUALITY: f32 = 0.85;

/// Wavelength of the undulations within a field, in metres.
const DETAIL_SCALE: f32 = 50.0;
/// Wavelength of the surface roughness, in metres. Deliberately *not* a
/// multiple of the metre the mesh is sampled at, for the reason
/// [`GRAIN_SCALE`] spells out — at 12.0 the ridged crag pinned its maximum
/// onto every twelfth mesh corner, a regular grid of peaks in the band that
/// exists to look broken.
const MICRO_SCALE: f32 = 11.4;
/// How far the undulations lift or drop the ground away from the landform they
/// are laid over, in metres.
const DETAIL_RELIEF: f32 = 5.0;
/// The same for the roughness. Sized to hold the power law the undulations
/// set — relief halving with wavelength — rather than capped the way it was
/// at 2 m facets, when this band was only three vertices across and any more
/// amplitude turned into facet-to-facet jitter. At 1 m facets the band spans
/// six to twelve vertices and can carry its full spectral share.
const MICRO_RELIEF: f32 = 1.2;
/// Wavelength of the facet-scale grain, in metres. Added when the mesh went
/// from 2 m to 1 m facets: the roughness above bottoms out at half
/// [`MICRO_SCALE`], so between ~6 m and the facet the field was smooth and the
/// finer mesh only resampled it — the faceting the look leans on washed out
/// instead of sharpening. A few vertices wide, so neighbouring facets tilt
/// against each other instead of shading one slope.
///
/// Not a whole number of metres, and that is load-bearing: Perlin is exactly
/// zero at its own lattice nodes, and a wavelength commensurate with the
/// metre grid the mesh samples pins those zeros to mesh corners — measured
/// at 4.0, a third of the band's amplitude vanished along axis-aligned lines
/// every fourth corner, and one lattice edge in sixteen was perfectly flat.
/// An incommensurate wavelength walks the sample phase through the whole
/// field instead, which is what noise sampled on a grid has to do to read
/// as noise.
const GRAIN_SCALE: f32 = 4.7;
/// How far the grain lifts or drops the ground, in metres. The next step of
/// the same power law [`MICRO_RELIEF`] holds to. It was first tuned alone —
/// up to a full metre — to be visible over a MICRO still capped for the old
/// mesh, and a lone loud octave with quiet neighbours read as crumpled paper
/// rather than ground; the middle band does that work now.
const GRAIN_RELIEF: f32 = 0.5;

// --- Crags: what bare rock does instead of rolling -------------------------
//
// The rugged dial says what kind of *country* this is; the crag pass answers
// a different question — whether anything grows here. Soil smooths ground and
// bare rock breaks it, so wherever the palette strips the ground bare (the
// spray zone and the mountain band) the finished ground is reworked into rock
// forms, and grassy hills keep rolling right beside them. A first attempt
// added a quiet ridged band instead, and it read as a bump map on the same
// smooth hills: relief far below the landform's own can only ever decorate
// it. What says *rock formation* is structure — ridges warped until they
// gnarl, crests sharpened into stacks and pinnacles, clefts cut between them,
// and the whole part-terraced into strata — at amplitudes that compete with
// the hills they stand on.
//
// See [`TerrainGenerator::crags`] for how the parts compose and the
// waterline guarantees that keep the coast the landform's — the lesson
// [`APRON_DRY`] already paid for.

/// Wavelength of the crag backbone, in metres — the ridged field the spires
/// and clefts are both read off. Incommensurate with the metre grid for the
/// reason [`GRAIN_SCALE`] spells out.
const CRAG_SCALE: f32 = 23.3;
/// Wavelength and reach of the warp the backbone is read through, in metres.
/// Unwarped ridged noise is even-tempered — every crest the same width, every
/// gully the same depth — and reads as corrugation; pushed through a warp the
/// crests pinch, fork and wander, which is most of what makes rock look
/// grown rather than stamped.
const CRAG_WARP_SCALE: f32 = 41.0;
/// See [`CRAG_WARP_SCALE`].
const CRAG_WARP: f32 = 11.0;
/// The exponent the backbone is sharpened by. Raised to a power, most of the
/// field lies low and the crests stab — isolated stacks and pinnacles rather
/// than an even swell of bumps.
const CRAG_SHARP: f32 = 2.6;
/// How far a spire may stand above the ground it grew from, in metres, where
/// the ground is wholly bare. Sized against [`DETAIL_RELIEF`] rather than the
/// fine bands: a form shorter than the hills cannot contrast with them.
const CRAG_SPIRE: f32 = 11.0;
/// How deep the clefts between spires cut, in metres — bounded at each point
/// by [`CRAG_HEADROOM`] of the headroom above the shore band, so no cleft
/// ever reaches the water.
const CRAG_CLEFT: f32 = 7.0;
/// The share of that headroom a cleft may spend. The margin the remainder
/// leaves is small and nothing downstream would notice a retune eating it,
/// which is why `crags_never_reach_the_shore_band` pins it.
const CRAG_HEADROOM: f32 = 0.8;
/// The strata: bare rock is pulled part of the way onto terraces this many
/// metres apart. Horizontal bedding is the one regularity real rock wears,
/// and with flat-shaded facets a level tread against a sheer riser is the
/// strongest "rock, not hill" signal the mesh can draw. Not a whole number,
/// so treads and the metre mesh stay out of phase.
const CRAG_STEP: f32 = 2.7;
/// How far toward those terraces the ground is pulled, 0 none to 1 fully.
const CRAG_STRATA: f32 = 0.65;
/// The slice of each strata interval the riser climbs through, as a share of
/// [`CRAG_STEP`]. A hard `floor` was tried and is a true discontinuity, which
/// the one-tile central differences behind [`TerrainGenerator::normal`]
/// cannot see — so paint and plants read "flat" at the very lip of a sheer
/// step. A finite riser leans with the ground underneath it instead: broad
/// and visible where the backbone is gentle, near-sheer where it is already
/// steep and painted rock anyway.
const CRAG_RISER: f32 = 0.3;
/// Height above which the crag pass is fully faded in, in metres. The fade
/// starts at [`SHORE_TOP`], so the waterline and the drawn shore stay the
/// landform's. Long on purpose: at 4.0 the first metres behind a rocky
/// waterline carried enough crag to read as cliff, and the coast-agreement
/// test rightly objected — a rocky shore is the middle ground, and its
/// foreshore has to stay walkable. The forms this pass exists for stand on
/// clifftops and summits, well above the fade.
const CRAG_FULL: f32 = 7.0;

/// The waterline apron: the altitude band, in landform metres, across which
/// the whole detail stack fades in from nothing. Below [`APRON_DRY`] the
/// ground is the bare landform, so the drawn coast is the landform's own
/// smooth contour; by [`APRON_FULL`] the stack is all there.
///
/// Added when the detail grew teeth at the waterline. Grain riding the
/// shoreline redrew every coast into noise-carved bays and lagoons — coasts a
/// circumnavigation could no longer close, because their heads receded past
/// the sight band of a boat sailing by — and re-bumped the nearshore seabed
/// that keels ground on. The apron pins the waterline back to the landform;
/// it also subsumes the old underwater fade, reaching zero before the water
/// does.
const APRON_DRY: f32 = 0.75;
/// See [`APRON_DRY`].
const APRON_FULL: f32 = 2.5;

/// Wavelength of the country's character, in metres: the dial between gentle
/// and rugged ground that the whole detail stack answers to. A uniform stack
/// at any amplitude read as one bump map laid over everything — the fix other
/// generators reach for is a low-frequency control field over the roughness
/// itself (Minecraft's "erosion" parameter; Quilez's derivative-damped fbm
/// self-regulates the same way), so the variety is in where the roughness
/// *is*, not how loud it is. Sized to hold one character across a hillside
/// while still turning over a few times per big island.
const RUGGED_SCALE: f32 = 280.0;
/// Metres of altitude that push the dial one whole step towards rugged, on
/// top of what the field says. High ground trends to broken rock and low
/// ground to parkland, which reads as mountains being mountains rather than
/// as patches of texture landing wherever the noise fell.
const RUGGED_CLIMB: f32 = 150.0;
/// Where the dial's field reads as fully gentle and fully rugged: the
/// smoothstep edges the field-plus-altitude sum is cut at. Asymmetric about
/// zero on purpose — the field's spread is narrow (a 2-octave fbm rarely
/// leaves ±0.5), so a symmetric cut would leave the dial parked mid-range
/// everywhere and neither country would commit. What the pair actually
/// produces is reported per seed by `island_shape` (the rugged share),
/// which is the number to watch when moving either edge.
const RUGGED_GENTLE: f32 = -0.45;
/// See [`RUGGED_GENTLE`].
const RUGGED_FULL: f32 = 0.5;
/// How much of the undulations' relief survives in the gentlest country.
/// Plains still roll — a dead-flat interior reads as unfinished, not calm.
const RUGGED_SWELL_FLOOR: f32 = 0.35;
/// The same for the two fine bands, which nearly vanish there: a meadow is
/// smooth at walking scale, and the contrast against the broken country is
/// what makes both legible.
const RUGGED_FINE_FLOOR: f32 = 0.08;

/// Wavelength of the woodland/meadow patchwork, in metres. Field-sized on
/// purpose: at the default zoom the camera sees ~50 m of ground, so parcels much
/// bigger than this mean the whole screen is one colour.
const PATCH_SCALE: f32 = 30.0;
/// Wavelength of the variation within a parcel, in metres.
const MOTTLE_SCALE: f32 = 18.0;
/// How hard the mottle field pushes on the parcel bucket, as a fraction of
/// [`PATCH_SCALE`]'s own field.
///
/// Judged by eye on nine seeds, because what it changes is the *shape* of the
/// parcels and no count of them moves enough to measure: at 0.5 the parcels
/// stop being shapes at all and the lowland reads as static, and at 0.1 the
/// picture is indistinguishable from leaving the field out. Here it works as
/// a finer grain within a parcel — a wood with lighter clearings in it — which
/// is the job the brightness step used to do before a tone stopped carrying
/// one.
const MOTTLE_WEIGHT: f32 = 0.25;

/// Deepest the sea bed is allowed to go, in metres below sea level.
///
/// The same number a chunk of open water *means* — see
/// [`protocol::ground::OCEAN_DEPTH`], and taken from there so the two cannot
/// drift. An island's bed reaching a different floor from the ocean around it
/// would leave a step at every coast, on a client that has no way to know
/// which of the two was wrong.
pub const MAX_DEPTH: f32 = protocol::ground::OCEAN_DEPTH;

// --- Coast ------------------------------------------------------------------
//
// A coastline is the most varied thing on the map, and the least interesting if
// it is all one thing. What kind of coast a stretch is comes from a single noise
// field, [`TerrainGenerator::shore_character`], and *both* the landform and the
// colour are read off it — so a beach is always flat and sandy, and a cliff is
// always steep and grey, rather than the two disagreeing.

/// Wavelength of the coastal character field, in metres. Much longer than any
/// other layer on purpose: a beach wants to be a whole bay and a cliff a whole
/// headland. At the default zoom the camera sees ~50 m of ground, so a stretch
/// of coast fills the screen several times over before it turns into the next
/// kind.
const SHORE_SCALE: f32 = 260.0;

/// How much the shore field is opened out before it is read. Its raw output is
/// bunched up near zero, and the thresholds below need the whole `-1.0..1.0`
/// range if all three kinds of coast are to get a fair share of the map.
const SHORE_GAIN: f32 = 3.6;

/// Where the shore character field is cut into beach, rocky shore and cliff.
/// Set off the field's measured distribution, not its nominal range — see
/// `shore_mix` in the tests, which reports the split these produce. Re-set
/// when the waterline apron pinned the drawn coast to the landform's own
/// contour: the character field never moved, but the smoother waterline runs
/// a different route through it, and the old cut left one seed's coast
/// under a sixth beach.
const ROCKY_SHORE: f32 = -0.10;
const CLIFF_SHORE: f32 = 0.26;

/// How far above sea level the coastal reshaping reaches, in metres — so also
/// how tall a cliff stands before it gives way to ordinary hillside.
const CLIFF_HEIGHT: f32 = 11.0;

/// How steeply a cliff face climbs, in metres of height per metre of ground
/// away from the water — so a little over 50°. Deliberately not vertical: from
/// a camera pitched down at 50° a sheer face is edge-on, and lands on the
/// screen as a dark line rather than as a cliff.
const CLIFF_RISE: f32 = 1.35;

/// How gently a beach climbs, in metres of height per metre of ground away
/// from the water — about 4°. The mirror of [`CLIFF_RISE`]: a cliff's ground is
/// lifted to meet a steep face, a beach's is held down to a shallow one.
const BEACH_RISE: f32 = 0.075;

/// How quickly the lifted clifftop is let back down towards the natural ground
/// behind it, in metres of height per metre of distance past the top of the
/// face — about 14°, a hillside rather than a wall.
///
/// Without it the lift holds the clifftop at full height as far as the coast
/// reaches, and wherever that terrace meets ground the other regime is holding
/// *down* — a beach across a low neck of land — the difference stands as a
/// scarp running dead straight along the crest of the distance field. Letting
/// the top shelve off returns the ground to itself before it can collide with
/// anything: a face, a shoulder of clifftop, then hillside.
const CLIFF_BACK_PITCH: f32 = 0.25;

/// Distance from the water, in metres, at which a beach's apron has doubled its
/// pitch. Set so that by the edge of [`SHORE_REACH`] the apron is climbing about
/// as steeply as the hillside it has to join.
const APRON_KNEE: f32 = 25.0;

/// How far inland a beach reaches, in metres. Much shorter than
/// [`SHORE_REACH`], which the cliffs use: a cliff builds ground up and can
/// afford to fade out over a long way, whereas a beach cuts ground away and
/// leaves an edge wherever it stops.
const BEACH_REACH: f32 = 60.0;

/// The most a beach may take off the ground beneath it, in metres.
const BEACH_CUT: f32 = 4.0;

/// Exponents the sea bed is bent by, on a beach and at the foot of a cliff.
/// Above one the bed holds its depth and the bright shallows run a long way
/// out; below one it falls away into dark water almost at once.
const BEACH_SHELF: f32 = 1.7;
const CLIFF_PLUNGE: f32 = 0.34;

/// How far inland the coastal reshaping reaches, in metres — measured as
/// distance to the waterline, not as height above it. Roughly a screen and a
/// half at the default zoom, so a cliff and its clifftop are one view.
const SHORE_REACH: f32 = 110.0;

/// The most of a map's beach waterline allowed to stand steeper than
/// [`ROCK_SLOPE`] — the point at which the palette gives up on sand and paints
/// the shore grey. What [`TerrainGenerator::fit_coast_scale`] fits the coastal
/// band against, and set just above what the tuned look measures on its gentle
/// seeds, so those keep the band exactly as tuned.
const BEACH_STEEP_TARGET: f32 = 0.01;

/// The furthest the coastal band may be grown to chase that target. Past this
/// a "beach" is cutting a shelf out of a mountainside, a lesser cliff is over
/// twenty metres of wall, and every seam in the coastal shaping stands tall
/// enough to see; a seed still failing here keeps its few sand ribbons.
const COAST_SCALE_LIMIT: f32 = 2.0;

/// Spacing of the distance-to-water field, in metres. Coarser than the
/// facet grid the coast is drawn on — deliberately, since the field is only
/// ever read to shape and fade over tens of metres, and matching the mesh
/// would cost sixteen times the fitting for smoothness nothing reads. The
/// price is that the field bends linearly between cells where the true
/// distance curves, which is part of the slack the lake test carries.
const COAST_GRID: f32 = 4.0;

/// Wavelength of the skerries — the rock heads left standing offshore of a
/// rocky coast, in metres. Only the peaks of the field clear the water, so each
/// rock is a good deal smaller than this; much below it and they stop being
/// wide enough to make a facet.
const SKERRY_SCALE: f32 = 34.0;
/// How far a skerry stands out of the water, in metres, as the skerry pass
/// itself leaves it. The crag pass runs after and builds on the barest of
/// them — the sea's rocks get the same treatment as everything else the
/// spray strips — so the tallest stacks finish near twice this.
const SKERRY_HEIGHT: f32 = 2.5;

/// Height, in metres, up to which ground is drawn as shore rather than as what
/// grows on it. Kept low deliberately: it is [`shape_coast`] flattening the
/// ground that makes a beach broad, so raising this would only smear the same
/// band of colour up the rocky shores and the cliffs as well.
const SHORE_TOP: f32 = 0.6;

/// How far in from the waterline the sea has any say over what grows, in
/// metres. Height is what the bands above [`SHORE_TOP`] answer to, so
/// distance is what the spray has to be — the rule that holds the mountains
/// off the coast, holding the grass off the spray — and a scrap of land
/// whose every point is within reach is sprayed *everywhere*, which is what
/// keeps a skerry's flat top and a one-chunk islet from coming out as lawns
/// standing in the sea. Read only through [`TerrainGenerator::sprayed`].
const SPRAY_REACH: f32 = 30.0;

/// How far the spray's edge wanders off the pure distance, in metres — the
/// same job [`BAND_WANDER`] does for the treeline, and driven by the same
/// field, because a contour parallel to the coast is as mechanical a line as
/// a level one.
const SPRAY_WANDER: f32 = 10.0;

/// Where the spray weight is cut into painted bare rock — the paint's
/// threshold over [`TerrainGenerator::sprayed`], whose doc says why the two
/// consumers share one number. Falls at about nine metres of collar on an
/// ordinary rocky shore and twenty on a cliff coast, and a beach's zero
/// weight never reaches it.
const SPRAY_BARE: f32 = 0.2;

/// How far a lake's reed margin reaches from its own edge, in metres, on both
/// sides of it — so the fringe is [`LAKE_MARGIN`] of wet ground and the same
/// again of weed standing in the water.
///
/// A sea coast has a shore because the sea *works* one, and it is broad because
/// the surf reaches that far up it. Fresh water does none of that, so grass
/// grows to the edge of a pond. Given the sea's own [`SHORE_TOP`] a lake wore a
/// huge flat apron that read as a drained reservoir.
///
/// A height above the water was the first answer and is the wrong shape of
/// answer: what a bank does with a height is whatever its slope says, so the
/// same cut bought tens of metres of margin on flat ground and under one facet
/// wherever the bank had pitch. A margin thinner than the grid cannot be drawn,
/// and the broken chain of triangles against the smooth waterline under it is
/// the sawtooth a lake used to wear. Written as a reach along the ground it is
/// the same fringe on every bank.
const LAKE_MARGIN: f32 = 5.0;

/// How far past its own edge a lake's surface is still an answer, in metres
/// along the ground.
///
/// The level has to reach past the water — a client draws the waterline itself,
/// by asking where the ground and the surface cross, and cannot find a crossing
/// it was only told one side of. The reach this replaces was a height, which on
/// the flat ground a basin usually ends in is a long way and which ends on the
/// fitting grid's own square cells.
///
/// That mattered because the two waterlines do not quite agree: the flood ran
/// on the landform, and the coastal reshaping afterwards will cut a lake's bank
/// out from under it near a shore. Where that leaves ground below a surface the
/// flood never flooded, the client draws water out to wherever the level
/// stopped being an answer — a rectangle of water lying on the grass. Bounded
/// by a distance instead, a spill is a few metres of apron with the shape of
/// the shore it came off.
const LAKE_APRON: f32 = 8.0;

/// How far the weed reaches out from a lake's margin before the bed is bare
/// silt, in metres along the ground.
///
/// Out from the shore rather than down from the surface for the reason the
/// margin is, and it bites harder here: a bed shelves more gently than a bank
/// climbs, so a depth line falls where the ground has barely any gradient to
/// place it and drew fingers of one tone through the other for tens of metres.
const LAKE_SHALLOWS: f32 = 22.0;

/// Depths, in metres below sea level, at which the sea bed turns from shore
/// colours to the bright shelf, and from the shelf to the deep bed.
const SHALLOW_DEPTH: f32 = 2.0;
const SEABED_DEPTH: f32 = 4.5;

/// Slope, as `1.0 - normal.y`, at which ground shows bare rock however high it
/// is and whatever else is growing on it — roughly 47°, and 63° for the darker
/// face. Without these, cliffs look like grass painted onto a wall.
const ROCK_SLOPE: f32 = 0.34;
const CLIFF_SLOPE: f32 = 0.55;

// --- Where one kind of ground gives way to the next -------------------------
//
// The bands are set by height, and a height threshold read literally draws a
// level contour — which is exactly what a treeline is not. Worse, a contour is
// smooth where everything either side of it is quantised, so the eye reads the
// join as the edge of the artwork rather than as the edge of the woodland.
//
// So the bands are read off a height that has been nudged by [`BAND_SCALE`]
// noise. The edge then wanders by hundreds of metres across gentle ground and
// barely at all on a steep face — which is the right way round, since a level
// line is most obviously a level line where the ground is flat, and a treeline
// against a mountainside really is fairly crisp.

/// Wavelength of the field the band edges wander by, in metres. Sits between
/// [`PATCH_SCALE`] and [`FEATURE_SCALE`]: the coarse octaves have to move a
/// whole hillside's worth of edge at once, or the boundary reads as a fringe
/// applied to a contour rather than as a boundary that was never level.
const BAND_SCALE: f32 = 150.0;

/// How far this moves the treeline and the edge of the bare rock, in metres of
/// height. Both take the same offset, so the moor between them slides up and
/// down the hill as one piece and can never be pinched out — the lowland has no
/// way to reach across it and touch the rock, however far the field swings.
///
/// A little over the depth of the moor itself, which is what makes the edges
/// break up rather than merely bend: where the field swings hardest the moor
/// lands entirely above or below where a level band would have put it, and what
/// is left behind on the other side reads as an outlying island.
const BAND_WANDER: f32 = 9.0;

/// What each parcel of the patchwork is drawn as, at each height it can reach.
///
/// One row per band and one column per parcel, indexed by the *same* bucket of
/// the *same* noise field however high the ground is. That carries the blend: a
/// wood running up a hillside keeps its outline as it crosses the treeline and
/// comes out the other side as heather.
///
/// The rows get flatter towards the top — five greens, three shades of moor,
/// two of rock — so the patchwork thins out with the vegetation without
/// stopping dead. Bare rock has nothing growing on it to make parcels of, and
/// the relief up there is drawn by the slope tests instead;
/// [`Material::RockDark`] is left to them, so a dark facet on a mountain always
/// means a crag.
const LOWLAND_PARCELS: [Material; 5] = [
    Material::Forest,
    Material::GrassDark,
    Material::Grass,
    Material::GrassLight,
    Material::Meadow,
];
const MOOR_PARCELS: [Material; 5] = [
    Material::Heath,
    Material::Heath,
    Material::Upland,
    Material::Fell,
    Material::Fell,
];
const MOUNTAIN_PARCELS: [Material; 5] = [
    Material::Rock,
    Material::Rock,
    Material::Rock,
    Material::Scree,
    Material::Scree,
];

/// Parameters the map is generated from.
#[derive(Clone, Copy, Debug)]
pub struct MapConfig {
    /// Chunks along X and Z. A chunk is [`CHUNK_TILES`] tiles and a tile is a
    /// metre, so each axis is `chunks * CHUNK_TILES` metres — any pair of
    /// counts makes a map, down to a single chunk.
    pub chunks: UVec2,
    pub seed: u32,
}

impl Default for MapConfig {
    fn default() -> Self {
        Self {
            chunks: UVec2::splat(8),
            seed: 20_040_112,
        }
    }
}

impl MapConfig {
    /// A square map of `metres` per side, which must be a whole number of
    /// chunks.
    pub fn square(metres: u32, seed: u32) -> Self {
        debug_assert_eq!(metres % CHUNK_TILES, 0, "maps are whole chunks");
        Self {
            chunks: UVec2::splat(metres / CHUNK_TILES),
            seed,
        }
    }

    /// Tiles along each axis. Tiles are one metre, so this is the map's extent
    /// in metres too.
    pub fn tiles(&self) -> UVec2 {
        self.chunks * CHUNK_TILES
    }

    /// The map's extent in metres, per axis.
    pub fn extent(&self) -> Vec2 {
        self.tiles().as_vec2() * TILE_SIZE
    }

    /// Distance from the map centre to its edge, in metres, per axis.
    pub fn half_extent(&self) -> Vec2 {
        self.extent() * 0.5
    }

    /// Parses a size given in metres — `"1024"` for a square, `"1536x1024"`
    /// for a rectangle — into chunk counts, each axis rounded to the nearest
    /// whole chunk of at least one.
    pub fn parse_size(spec: &str) -> Option<UVec2> {
        let (w, d) = spec.split_once('x').unwrap_or((spec, spec));
        let axis = |s: &str| {
            s.trim()
                .parse::<f32>()
                .ok()
                .map(|m| ((m / CHUNK_TILES as f32).round() as u32).max(1))
        };
        Some(UVec2::new(axis(w)?, axis(d)?))
    }
}

// ---------------------------------------------------------------------------
// Generation
// ---------------------------------------------------------------------------

/// Samples terrain height and surface for a given seed.
///
/// One of these is what an island *is*, behind the layout: everything a server
/// answers about a patch of ground — how high it stands, what grows on it —
/// comes from here.
pub struct TerrainGenerator {
    continent: Noise,
    hills: Noise,
    mountain_mask: Noise,
    ridges: Noise,
    warp: Noise,
    detail: Noise,
    rugged: Noise,
    shore: Noise,
    skerry: Noise,
    /// The three numbers that make a seed's mountains behave like every other
    /// seed's: where the massif field starts counting as a range, what it has
    /// to reach to be the summit, and how far that summit stands above the
    /// rolling country. See [`TerrainGenerator::fit_ranges`].
    range_floor: f32,
    range_span: f32,
    range_gain: f32,
    /// Local ceiling of the massif field — the tallest seat within
    /// [`MASSIF_WINDOW`] of each point — against which each range is partly
    /// measured, so that every massif gets a summit of its own. See
    /// [`MASSIF_EQUALITY`].
    range_ceiling: GridField,
    calibration: Calibration,
    coast: CoastDistance,
    /// The waterline the mountains are held back from. Empty until the fit in
    /// [`TerrainGenerator::new`] has found a coastline to measure, which reads
    /// infinity everywhere — the ceiling declining to bind rather than binding
    /// on a measurement it has not made.
    ///
    /// Deliberately *not* [`TerrainGenerator::coast`]: this one counts only
    /// water broad enough to stand in for the sea, so the ponds and narrow
    /// sounds inland are ground rather than a coast a range has to climb from.
    /// See [`CoastDistance::from_open_water`].
    inland: CoastDistance,
    /// Standing water above sea level — the basins the landform encloses,
    /// flooded once there is a finished landform to flood. Empty until then,
    /// which reads as a world whose only water is the sea's; nothing asks
    /// before the fit is done.
    lakes: Lakes,
    /// How much taller this seed's coastal band is than the one the constants
    /// were tuned on. See [`TerrainGenerator::fit_coast_scale`].
    coast_scale: f32,
    half_extent: Vec2,
    /// The falloff's per-map numbers, worked out once so the per-sample path
    /// doesn't have to: the shared part of the warp's drift in excess of what
    /// the ring can absorb, how much of the bend this map can afford, and how
    /// far outward the reach may push. See [`TerrainGenerator::falloff`].
    drift_excess: Vec2,
    bend_gain: f32,
    reach_max: f32,
    /// How tightly [`TerrainGenerator::continent_at`] samples the landmass
    /// field — see [`feature_zoom`].
    feature_zoom: f32,
}

/// How sharply [`under_ceiling`] turns over as it meets the ceiling. Higher is
/// closer to a plain `min`: later to bend, and flatter once it has.
///
/// Set high enough that ground well under its ceiling is left alone. At 3 the
/// bend started early — a hillside at two thirds of its ceiling lost a tenth
/// of its height — and since most of any map here is low country a short way
/// from water, that came off the middle of the land everywhere and undid
/// [`LOWLAND_FLOOR`]. The ceiling is meant to be a limit, not a tax.
const CEILING_KNEE: f32 = 6.0;

/// `h` brought under `ceiling`, leaving the ground still rising.
///
/// A plain `min` is what this must not be: clipped flat, every headland with a
/// strong massif becomes a mesa, which has no summit for a peak to be made of,
/// and it would print the shape of the distance field on the ground wherever
/// it bound.
///
/// A smooth minimum instead: all but exactly `h` while `h` is well under the
/// ceiling, bending over as it approaches and closing on it from below without
/// ever sitting on it. Ground under a binding ceiling still climbs, just far
/// more slowly than the noise wanted — which is what a worn-down headland
/// looks like.
fn under_ceiling(h: f32, ceiling: f32) -> f32 {
    // Nothing to do below the waterline — the ceiling is about how high land
    // may stand, and depth is the calibration's business. The second half is
    // belt and braces: an infinite ceiling already falls out of the formula as
    // `h` untouched, and what the check is really for is the not-a-number a
    // distance field with no sea in it at all would blur its way to.
    if h <= 0.0 || !ceiling.is_finite() {
        return h;
    }
    // No guard on the divisor: the ceiling is [`CLIFF_HEIGHT`] plus a distance
    // that cannot be negative, so it is never near zero.
    let ratio = h / ceiling;
    h / pow(1.0 + pow(ratio, CEILING_KNEE), 1.0 / CEILING_KNEE)
}

/// One grid point's landform, split at the one term the fit is free to scale.
///
/// Noise is what costs here. Keeping the parts means
/// [`TerrainGenerator::fit_range_height`] can try a scale for the price of a
/// multiply and a sort, instead of rebuilding the whole field from the noise
/// every time it wants to know what a scale would do.
struct Sample {
    /// Continent and hills together, before the falloff.
    base: f32,
    /// The massif, before the falloff and before it is scaled.
    range: f32,
    damp: f32,
    push: f32,
}

impl Sample {
    fn raw(&self, gain: f32) -> f32 {
        (self.base + gain * self.range) * self.damp - self.push
    }
}

/// One point of the fitting grid, as far as the noise can take it before the
/// massif's own floor is known.
///
/// [`TerrainGenerator::fit_ranges`] has to walk the grid twice — once to find
/// where the massif starts counting, and again to build the [`Sample`] that
/// needs it — and noise is what costs here, so the first pass keeps
/// everything the second would otherwise have to sample afresh.
struct GridPoint {
    /// Where on the map, in metres.
    at: Vec2,
    /// The same point once the domain warp has moved it.
    warped: Vec2,
    /// The landmass field there, which the massif is partly seated on.
    continent: f32,
    /// Continent and hills together — the terms the massif does not touch.
    base: f32,
    damp: f32,
    push: f32,
}

impl TerrainGenerator {
    pub fn new(config: &MapConfig) -> Self {
        let seed = config.seed;
        let targets = Targets::for_extent(config.extent(), seed);
        let cycles = config.extent() / CONTINENT_SCALE;
        let room = smoothstep(1.1, 1.55, cycles.x.min(cycles.y));

        let mut generator = Self {
            continent: Noise::new(seed),
            hills: Noise::new(seed.wrapping_add(0x51ED_2701)),
            mountain_mask: Noise::new(seed.wrapping_add(0x9E37_79B9)),
            ridges: Noise::new(seed.wrapping_add(0x1B87_3593)),
            warp: Noise::new(seed.wrapping_add(0x68E3_1DA4)),
            detail: Noise::new(seed.wrapping_add(0xB547_9AA3)),
            rugged: Noise::new(seed.wrapping_add(0x7F4A_7C15)),
            shore: Noise::new(seed.wrapping_add(0x2545_F491)),
            skerry: Noise::new(seed.wrapping_add(0xC2B2_AE35)),
            // All three are fitted below, by looking at the map this seed
            // actually produced. See [`TerrainGenerator::fit`].
            range_floor: 0.0,
            range_span: 1.0,
            range_gain: 1.0,
            range_ceiling: GridField::default(),
            calibration: Calibration::default(),
            coast: CoastDistance::default(),
            inland: CoastDistance::default(),
            lakes: Lakes::default(),
            coast_scale: 1.0,
            half_extent: config.half_extent(),
            drift_excess: Vec2::ZERO,
            bend_gain: 0.25 + 0.75 * room,
            reach_max: 1.0 + 0.15 * room,
            feature_zoom: feature_zoom(config.extent()),
        };
        let centre = generator.warped(0.0, 0.0).1 * FEATURE_SCALE * 0.7;
        let tolerance = generator.half_extent * 0.1;
        generator.drift_excess = centre - centre.clamp(-tolerance, tolerance);

        // One round of fitting, and the ceiling laid over what it produced.
        //
        // Not a loop, though it looks like it ought to be one. Nothing inside
        // [`TerrainGenerator::fit`] reads the ceiling — it is applied in
        // [`TerrainGenerator::landform`], strictly downstream — so fitting
        // again against the coastline this measures returns the same numbers
        // to the bit, which a second round was tried and did. Nor is there a
        // coastline to converge on in principle: the ceiling scales a height
        // rather than subtracting from it, so it moves every contour on the
        // map except the one at zero, which is the only one any of this is
        // measured from.
        //
        // The order below is load-bearing all the same. `inland` is the one
        // thing [`TerrainGenerator::ceiling`] reads, and the coast-band fit
        // asks for a finished height, so it has to run against the ceiling
        // rather than before it exists.
        let raw = generator.fit(config, &targets);
        generator.inland = CoastDistance::from_open_water(&raw, &generator.calibration);
        // The flood reads the ceiling, and the ceiling reads `inland`, so the
        // lakes can only be found once the line above has run: the landform
        // is not finished until the ceiling can bind, and a basin flooded
        // before its rim was held down would carry the wrong level ever
        // after.
        generator.lakes = generator.find_lakes(&raw);
        generator.fit_coast_scale(&raw);
        generator
    }

    /// Floods the finished landform and keeps what stands — see [`Lakes`].
    ///
    /// The metres grid is rebuilt from the raw samples rather than from
    /// [`TerrainGenerator::landform`], which would resample every octave of
    /// noise for values the fitting grid already holds; cell for cell this is
    /// the same arithmetic that function performs, on the same inputs.
    fn find_lakes(&self, raw: &GridField) -> Lakes {
        let (nx, nz) = raw.dims;
        let mut metres = Vec::with_capacity(nx * nz);
        for iz in 0..nz {
            let wz = raw.origin.y + iz as f32 * COAST_GRID;
            for ix in 0..nx {
                let wx = raw.origin.x + ix as f32 * COAST_GRID;
                metres.push(under_ceiling(
                    self.calibration.metres(raw.cells[iz * nx + ix]),
                    self.ceiling(wx, wz),
                ));
            }
        }
        Lakes::from_ground(&GridField::new(metres, raw.dims, raw.origin))
    }

    /// One round of fitting: where the ranges sit, how tall they stand, what
    /// the raw field's spread means in metres, and where that puts the water.
    ///
    /// Each step depends on the one before it, so they cannot be folded
    /// together: the ranges have to be scaled before the height field means
    /// anything, the height field has to exist before its distribution can be
    /// read, and the coast cannot be measured until the calibration has said
    /// where the water is.
    ///
    /// Returns the raw grid, which the coast-band fit still wants afterwards.
    fn fit(&mut self, config: &MapConfig, targets: &Targets) -> GridField {
        let samples = self.fit_ranges(config.tiles(), targets);
        self.fit_range_height(&samples, targets);
        let raw = self.sample_raw(config.tiles());
        self.calibration = Calibration::fit(&mut raw.cells.clone(), targets);
        self.coast = CoastDistance::from_raw(&raw, &self.calibration);
        raw
    }

    /// The most height this ground may carry, in metres: [`PEAK_GRADE`] for
    /// every metre it stands back from the sea.
    ///
    /// A ceiling point by point, and nothing else — the literal form of the
    /// thing being claimed. Ground can only climb so fast on the way inland,
    /// so how far inland it is bounds how high it is. Where there is room this
    /// never binds and the landscape is whatever the noise made it; on a
    /// headland it binds hard, and the ground there is low because there is
    /// nowhere for it to have climbed from.
    ///
    /// Offset by [`CLIFF_HEIGHT`], because a coast may stand a cliff tall
    /// without having climbed from anywhere — which is what the coastal shaping
    /// spends its time building. Without the offset the ceiling bears down on
    /// the ordinary low country too, undoing [`LOWLAND_FLOOR`] and leaving the
    /// drowned sandflat the calibration went to trouble to rule out.
    ///
    /// Biasing the massif *towards* the interior instead was tried and dropped.
    /// A soft preference does not stop anything: a massif the mask made strong
    /// still beats a weaker one with far more room behind it, and the height
    /// fit then puts the summit exactly where the preference was avoiding.
    /// Measured on finished maps that was the common case — summits at one and
    /// a half to two metres of height per metre back from the sea, against the
    /// half-metre the rest of this is written around.
    fn ceiling(&self, wx: f32, wz: f32) -> f32 {
        CLIFF_HEIGHT + PEAK_GRADE * self.inland.metres(wx, wz)
    }

    /// Fits where this seed's mountains sit and how much of the map they cover,
    /// and returns the grid it worked that out on for
    /// [`TerrainGenerator::fit_range_height`] to scale them on.
    ///
    /// Left to the noise, both are luck: the same settings give one map a
    /// five-hundred-metre alp and the next a seventy-metre hill. Two passes
    /// over a coarse grid settle it — the first finds where to start counting
    /// the massif so it covers [`RANGE_FOOTPRINT`] of the map and reaches full
    /// height only at its highest point, the second builds the field either
    /// side of the one term that is still free.
    fn fit_ranges(&mut self, tiles: UVec2, targets: &Targets) -> Vec<Sample> {
        let (nx, nz) = grid_dims(tiles);
        let origin = -tiles.as_vec2() * 0.5;

        // The seat field is sampled on a grid padded by the ceiling's window,
        // so that the window never runs off the edge of what was sampled —
        // otherwise the ceiling near the map edge would depend on the map
        // size, and the same seed would put different mountains at the same
        // world coordinates on different sizes.
        let pad = (MASSIF_WINDOW / COAST_GRID).ceil() as usize;
        let (px, pz) = (nx + 2 * pad, nz + 2 * pad);
        let pad_origin = origin - Vec2::splat(pad as f32 * COAST_GRID);

        // Every in-map grid point's warped position, its share of the terms
        // that do not depend on the massif, and what the falloff does to it —
        // and, over the padded grid, the massif's seat. Worked out once and
        // used by every pass after this one.
        let mut seat = vec![0.0f32; px * pz];
        let mut points = Vec::with_capacity(nx * nz);
        for iz in 0..pz {
            let wz = pad_origin.y + iz as f32 * COAST_GRID;
            for ix in 0..px {
                let wx = pad_origin.x + ix as f32 * COAST_GRID;
                let (n, drift) = self.warped(wx, wz);
                let continent = self.continent_at(n);
                seat[iz * px + ix] = self.range_seat(n, continent);

                if (pad..px - pad).contains(&ix) && (pad..pz - pad).contains(&iz) {
                    let hills = self.hills.fbm(n.x * 0.9, n.y * 0.9, 5);
                    let (damp, push) = self.falloff(wx, wz, n, drift);
                    points.push(GridPoint {
                        at: Vec2::new(wx, wz),
                        warped: n,
                        continent,
                        base: 0.62 * continent + 0.26 * hills,
                        damp,
                        push,
                    });
                }
            }
        }

        // Where the massif starts counting, and the map-wide span — fitted
        // against the map itself, not the padding, since the footprint is a
        // share of *this map*.
        let mut field = Vec::with_capacity(nx * nz);
        for iz in pad..pz - pad {
            for ix in pad..px - pad {
                field.push(seat[iz * px + ix]);
            }
        }
        field.sort_by(|a, b| a.partial_cmp(b).expect("no NaN in a noise field"));

        let footprint = RANGE_FOOTPRINT * targets.land;
        self.range_floor = field[((field.len() - 1) as f32 * (1.0 - footprint)) as usize];
        self.range_span = (field[field.len() - 1] - self.range_floor).max(1e-3);

        // The local ceiling: the tallest seat within the window of each cell.
        // Lightly blurred afterwards — a windowed maximum is continuous but
        // carries gradient creases where the winning peak changes hands, and
        // anything the massif is divided by ends up printed on the mountains.
        let mut ceiling = GridField::new(seat, (px, pz), pad_origin);
        ceiling.window_max(pad);
        ceiling.blur();
        ceiling.blur();
        self.range_ceiling = ceiling;

        points
            .iter()
            .map(|point| Sample {
                base: point.base,
                range: self.ranges(point.at.x, point.at.y, point.warped, point.continent),
                damp: point.damp,
                push: point.push,
            })
            .collect()
    }

    /// Scales the massif until the highest ground on the map stands at
    /// [`peak_height`] — [`HEIGHT_SCALE`], on any map big enough to hold it.
    ///
    /// Fitting the footprint is not enough on its own: how far a summit gets
    /// above the mountain line depends on how sharply this seed's massif comes
    /// to a point, which on the same settings ranges from a hundred metres to
    /// nearly three.
    ///
    /// What is *not* the way to fix it is bending the mapping in
    /// [`Calibration`]; see the note there. This scales the massif term
    /// instead, changing how high a range stands without touching how sharp it
    /// is. The scale and the mapping decide each other, so it is solved for
    /// rather than calculated — cheaply, the noise being sampled already.
    fn fit_range_height(&mut self, samples: &[Sample], targets: &Targets) {
        // What the map's highest ground comes out at, in metres, for a given
        // scale on the massif.
        //
        // Fitted against the field *before* [`TerrainGenerator::ceiling`] gets
        // to it, deliberately. Made to chase the ceiling, the fit pushes the
        // range term as far as it takes to reach a target the ceiling will not
        // allow — the massif ends up many times the share of the field it
        // should be, and since the room a map has depends on the map, the same
        // seed came out a different landscape at two sizes.
        let peak_at = |gain: f32| {
            let mut raw: Vec<f32> = samples.iter().map(|s| s.raw(gain)).collect();
            // Sorts in place, so the last entry is the summit afterwards.
            let calibration = Calibration::fit(&mut raw, targets);
            calibration.metres(raw[raw.len() - 1])
        };

        let target = HEIGHT_SCALE * targets.relief;

        // Two brackets. The lower is well under anything that produces
        // mountains; the upper is a limit as much as a bracket.
        //
        // From a kilometre up this is a genuine solve and the answer is small,
        // under 4 on every seed measured. Below that the curve stops being one
        // a solve can follow: the calibration refits the mapping at every
        // trial, so past a point pushing the massif harder stops raising the
        // summit at all — one 512-metre seed read 54.1 m at every scale from 1
        // to 200 — and far enough past it the massif swamps the quantiles the
        // mapping is anchored on and the whole thing breaks upward. On any of
        // those no scale reaches the target, and the bisection runs to whatever
        // ceiling it was given.
        //
        // Which is the case for keeping it low, and it costs nothing to: the
        // maps either ceiling produces are indistinguishable, so the only
        // difference is the number a degenerate seed comes away with. At 200
        // one seed solved to 1.9 at 640 m, 200 at 768 m and 1.4 at a kilometre,
        // and map size is a dial the player turns.
        let (mut lo, mut hi) = (0.05f32, 12.0f32);
        // Bisection rather than a secant: as above the curve is not reliably
        // monotone, and a run of halvings costs almost nothing here and cannot
        // be thrown off by a flat stretch the way a secant can.
        for _ in 0..18 {
            let mid = 0.5 * (lo + hi);
            if peak_at(mid) < target {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        self.range_gain = 0.5 * (lo + hi);
    }

    /// A grid of raw landform samples covering the whole map, at
    /// [`COAST_GRID`] spacing.
    fn sample_raw(&self, tiles: UVec2) -> GridField {
        let (nx, nz) = grid_dims(tiles);
        let origin = -tiles.as_vec2() * 0.5;

        let mut cells = Vec::with_capacity(nx * nz);
        for iz in 0..nz {
            let wz = origin.y + iz as f32 * COAST_GRID;
            for ix in 0..nx {
                cells.push(self.landform_raw(origin.x + ix as f32 * COAST_GRID, wz));
            }
        }
        GridField {
            cells,
            dims: (nx, nz),
            origin,
        }
    }

    /// Fits how much taller than tuned this seed's coastal band has to be for
    /// its beaches to come out as beaches.
    ///
    /// The band is written in metres, and metres are fitted per seed — so on a
    /// steeper seed the ground climbs past [`CLIFF_HEIGHT`] within a few metres
    /// of the water, the reshaping runs out of room, and the beaches collapse
    /// into sand ribbons with a grey slope-rule line along the waterline.
    ///
    /// No single measurement of the landform predicts that well, so it is
    /// solved for: walk the beach stretches of the waterline, measure what
    /// share the slope rule would paint grey, and take the smallest band that
    /// gets it under [`BEACH_STEEP_TARGET`]. Gentle seeds pass at 1.0.
    fn fit_coast_scale(&mut self, raw: &GridField) {
        let (nx, nz) = raw.dims;
        let cells = &raw.cells;
        // Every waterline crossing on the fitting grid whose stretch of coast
        // the character field calls a beach.
        let sea = self.calibration.sea_level;
        let mut points: Vec<(f32, f32)> = Vec::new();
        for iz in 1..nz - 1 {
            for ix in 1..nx - 1 {
                if cells[iz * nx + ix] <= sea {
                    continue;
                }
                let shoreline = [
                    iz * nx + ix - 1,
                    iz * nx + ix + 1,
                    (iz - 1) * nx + ix,
                    (iz + 1) * nx + ix,
                ]
                .iter()
                .any(|i| cells[*i] <= sea);
                if !shoreline {
                    continue;
                }
                let (wx, wz) = (
                    raw.origin.x + ix as f32 * COAST_GRID,
                    raw.origin.y + iz as f32 * COAST_GRID,
                );
                if self.shore(wx, wz) == Shore::Beach {
                    points.push((wx, wz));
                }
            }
        }

        // Enough of the waterline to make the share meaningful; big maps get
        // thinned rather than walked in full, since the answer is a statistic.
        let stride = (points.len() / 1024).max(1);
        let points: Vec<(f32, f32)> = points.into_iter().step_by(stride).collect();
        if points.len() < 32 {
            return;
        }

        // Walked upwards in steps rather than bisected: the share is a noisy
        // stair of a function, and the first band that satisfies the target is
        // the answer — any larger reshapes more hillside than it has to.
        let mut scale = 1.0;
        loop {
            self.coast_scale = scale;
            let steep = points
                .iter()
                .filter(|(wx, wz)| 1.0 - self.normal(*wx, *wz).y > ROCK_SLOPE)
                .count();
            if steep as f32 / points.len() as f32 <= BEACH_STEEP_TARGET
                || scale >= COAST_SCALE_LIMIT
            {
                return;
            }
            scale = (scale + 0.25).min(COAST_SCALE_LIMIT);
        }
    }

    /// Where a world point lands once the domain warp has moved it, and how far
    /// it moved. Two scales of warp: a long one that bends whole coastlines
    /// into peninsulas and gulfs, and a shorter one for the wandering of the
    /// shore itself. Offsetting the sample point by another noise field is what
    /// turns concentric blobs into meandering, organic shapes.
    ///
    fn warped(&self, wx: f32, wz: f32) -> (Vec2, Vec2) {
        let nx = wx / FEATURE_SCALE;
        let nz = wz / FEATURE_SCALE;

        let sway_x = self.warp.fbm(nx * 0.30 + 3.1, nz * 0.30 - 1.7, 2);
        let sway_z = self.warp.fbm(nx * 0.30 - 5.3, nz * 0.30 + 2.9, 2);
        let warp_x = self.warp.fbm(nx * 0.9 - 8.2, nz * 0.9 + 4.6, 3);
        let warp_z = self.warp.fbm(nx * 0.9 + 6.7, nz * 0.9 - 2.4, 3);

        let drift = Vec2::new(sway_x * 1.35 + warp_x * 0.45, sway_z * 1.35 + warp_z * 0.45);
        (Vec2::new(nx, nz) + drift, drift)
    }

    /// The landmass field at a warped point, sampled [`feature_zoom`] times
    /// ahead of its tuned wavelength — the one field zoomed for small maps,
    /// and why, is explained there.
    fn continent_at(&self, n: Vec2) -> f32 {
        let z = self.feature_zoom;
        self.continent
            .fbm(n.x * z * CONTINENT_FREQ, n.y * z * CONTINENT_FREQ, 4)
    }

    /// Where this map's mountains want to sit: its own mask field, plus a good
    /// share of the landmass field.
    ///
    /// Tying the two together puts ranges on the high ground rather than
    /// wherever the mask happens to fall — which is how real ones sit, and what
    /// stops a seed putting its only massif out at sea. A range running down a
    /// peninsula still gets a range's height, the landmass field being high
    /// there too.
    fn range_seat(&self, n: Vec2, continent: f32) -> f32 {
        self.mountain_mask
            .fbm(n.x * MASSIF_FREQ, n.y * MASSIF_FREQ, 3)
            + 0.55 * continent
    }

    /// How much of a mountain range is here, from nothing to all of one.
    ///
    /// `ridged` gives crest lines rather than blobs, which is what makes a
    /// range a range, and the mask concentrates them into a few parts of the
    /// map instead of corrugating all of it. Only the crests count, and only
    /// upwards: a ridged field taken whole sits either side of its own median,
    /// so it sinks a map as often as it lifts one and adds no high ground at
    /// all. Both cuts are set against the fields' measured spread rather than
    /// their nominal range — a mask cut where the noise never reaches is a mask
    /// that never opens, which is how this map came to have no mountains.
    fn ranges(&self, wx: f32, wz: f32, n: Vec2, continent: f32) -> f32 {
        // The massif: a broad, smooth swell that decides where the mountains
        // are and how far up they go. Its own gradient is what sets the pitch
        // of the flanks, and at this wavelength that is a climb of a couple of
        // hundred metres over several hundred more.
        //
        // Both ends of it are fitted per seed, and both matter.
        //
        // It has to stay a *dome*. Clipped flat — which a smoothstep does the
        // moment the noise passes its upper edge — it becomes a mesa, whose
        // highest ground is barely above its own shoulders.
        //
        // And its footprint has to be about the share of the map meant to end
        // up mountainous. Spread over half of it, the ninetieth percentile of
        // the land sits partway up the dome, and raising the range lifts the
        // mountain line with it so the summits never get any further above
        // their own shoulders. Measured partly against the map's whole span
        // and partly against the local ceiling, so every range has a summit
        // that approaches full height — see [`MASSIF_EQUALITY`].
        let local = (self.range_ceiling.at(wx, wz) - self.range_floor).max(1e-3);
        let span = self.range_span + MASSIF_EQUALITY * (local - self.range_span);
        let massif = ((self.range_seat(n, continent) - self.range_floor) / span).clamp(0.0, 1.0);

        // The ridges on top of it. A ridged field is all cusp, so left to shape
        // the range on its own it gives a row of knife blades standing on end;
        // riding on the massif at less than half strength it puts crests and
        // gullies on a mountain whose shape is already decided. Ramped over
        // most of the field's range rather than its top slice, so a crest is a
        // broad shoulder — taken narrow it is a razor line a couple of facets
        // wide, which at mountain height reads as masonry.
        let crest = smoothstep(0.25, 0.98, self.ridges.ridged(n.x * 0.5, n.y * 0.5, 3));

        // Squared, which is what puts a summit on the dome. Left linear the
        // contour enclosing the top few per cent sits nearly half way up it,
        // so the peak is only twice the mountain line's height however hard
        // the range is pushed. Safe here and nowhere else: the massif is a
        // smooth swell, and the same trick on the ridged field sharpens its
        // creases into blades.
        massif * massif * (0.55 + 0.45 * crest)
    }

    /// The shape of the land, in arbitrary units, before [`Calibration`] decides
    /// what any of it means. Higher is higher; where sea level falls in it is
    /// not settled until the whole field has been looked at.
    fn landform_raw(&self, wx: f32, wz: f32) -> f32 {
        let (n, drift) = self.warped(wx, wz);
        let (nx, nz) = (n.x, n.y);

        // Broad landmass shape, then rolling hills layered on top.
        let continent = self.continent_at(n);
        let hills = self.hills.fbm(nx * 0.9, nz * 0.9, 5);

        let h =
            0.62 * continent + 0.26 * hills + self.range_gain * self.ranges(wx, wz, n, continent);

        let (damp, push) = self.falloff(wx, wz, n, drift);
        h * damp - push
    }

    /// How much the island falloff quietens the height field here, and how far
    /// it pushes what is left downwards.
    ///
    /// Where the land stops is the noise's business, not a radial falloff's.
    /// This only has to see the map *ends* in open water, so it is confined to
    /// the outer third, leaving the coastline to be drawn wherever the field
    /// happens to cross sea level — which is what produces bays and headlands
    /// rather than a rounded square with a fringe. Letting it reach further in
    /// flattens the histogram the calibration reads: it pushes most of the map
    /// far below anything the noise does, sea level lands in the gap between
    /// the two, and every island comes out as a plateau with a cliff round it.
    ///
    /// It damps as well as pushes. Subtracting alone leaves the noise at full
    /// amplitude across the falloff, so the field crosses sea level over and
    /// over on the way out and the coast comes apart into a speckle of islets;
    /// taking the amplitude down first gives one clean shoreline.
    fn falloff(&self, wx: f32, wz: f32, n: Vec2, drift: Vec2) -> (f32, f32) {
        // Measured in the warped frame, against a radius that wanders with its
        // own slow field, so the little of the outline it does decide is not a
        // circle either. The wavelength is short enough to vary *around* the
        // island: a slower field is near enough constant across a kilometre of
        // map, so instead of lobing the outline it scales the whole island.
        //
        // Only how the drift *varies* around the ring draws lobes on it;
        // whatever the whole map's drift has in common displaces the entire
        // ring, which has very little room to be displaced. The warp's dominant
        // component is over a kilometre long, so on maps of that order the
        // shared part is most of the drift: taken raw it slid the ring off the
        // edge of the map, and the coast on that side was drawn by the rim in a
        // dead straight line. Two guards keep the ring on the map, each for the
        // scale the other cannot cover.
        //
        // `drift_excess` is the shared part — read at the map centre — beyond a
        // tolerance of a tenth of each half extent, subtracted everywhere. A
        // constant subtraction changes nothing about how the drift varies, so
        // every lobe survives. Full recentring is deliberately *not* done: past
        // the warp's wavelength the centre stops predicting the drift at the
        // ring, and anchoring to it there pushes maps off their frames.
        //
        // And `bend_gain` damps the whole bend on maps the landmass field
        // cannot break up, where even the drift's local variation outruns the
        // few metres of margin the ring has. On larger maps that variation is a
        // lobe and most of what un-squircles them, so it comes back in full.
        let bend = (drift * FEATURE_SCALE * 0.7 - self.drift_excess) * self.bend_gain;
        let reach = 1.0 + 0.7 * self.continent.fbm(n.x * 0.85 - 12.4, n.y * 0.85 + 9.8, 2);
        // The clamp is asymmetric: the radius may pull well in, carving deep
        // bays out of the ring, but only push a little out — pushed further,
        // the ramp ends up at the rim and the rim ends up drawing the coast.
        // Like the bend, the outward allowance closes almost entirely on the
        // one-blob maps.
        let shaped =
            self.squircle(wx + bend.x, wz + bend.y, 2.2) / reach.clamp(0.72, self.reach_max);

        // Spread over a wide ramp and pushed down gently. A short, hard falloff
        // drops the ground into the sea too fast, and the slope rule then
        // paints a grey cliff right round every island. Starting the ramp much
        // further out has been tried twice and is worse both times: the land
        // reaches the rim and is cut off dead straight, or the sea floods the
        // interior into fragments.
        //
        // A last, unwarped guard on the outcome, the two guards above acting
        // on the bend's *inputs*: a drift that diverges across the map inflates
        // the ring past both frame edges with no net translation to catch and,
        // on a large map, no taper to damp. Where this binds the coast follows
        // an arc, which is the ring showing — far better than the frame
        // showing — and it starts well outside the ring's usual reach.
        //
        // Skipped over the interior, where it is identically zero: a squircle
        // is at most 2^(1/power) times the larger axis fraction, so inside two
        // thirds of either half extent it cannot reach the guard's ramp, and
        // its `pow`s are most of this function's arithmetic.
        let frame = (wx / self.half_extent.x)
            .abs()
            .max((wz / self.half_extent.y).abs());
        let guard = if frame > 0.65 {
            smoothstep(0.86, 0.99, self.squircle(wx, wz, 2.6))
        } else {
            0.0
        };
        let edge = smoothstep(0.72, 1.12, shaped).max(guard);

        // The rim, in unwarped coordinates and narrower still, so that whatever
        // the warp does the very edge of the map is open sea. Not wider: the
        // cells it pushes down land in the quantiles every fit below the
        // waterline reads, and widening it once flattened the whole interior
        // sea into a shelf.
        let rim = smoothstep(0.94, 1.0, self.squircle(wx, wz, 4.0));

        // The damp is squared because it is the only part of the falloff
        // that scales with the field it is fighting: the push is a fixed
        // number of raw units, and a seed whose field runs high towards an
        // edge simply out-shouts it, keeping land deep into the ramp. Since
        // sea level is refitted per seed, land killed in the ramp reappears
        // in the interior rather than vanishing.
        let damp = (1.0 - edge) * (1.0 - rim);
        (damp * damp, edge * 0.7 * (1.0 - rim) + rim * 1.4)
    }

    /// Distance from the middle of the map, as a fraction of each axis's half
    /// extent, measured on a squircle — so on a rectangular map the contours
    /// are rounded rectangles that follow the frame. `power` picks how square:
    /// 2 is a circle, and larger reaches further into the corners.
    fn squircle(&self, wx: f32, wz: f32, power: f32) -> f32 {
        let dx = (wx / self.half_extent.x).abs();
        let dz = (wz / self.half_extent.y).abs();
        pow(pow(dx, power) + pow(dz, power), 1.0 / power)
    }

    /// The landscape before the coast gets to it — continent, hills and ranges
    /// — in metres, with sea level at 0.
    ///
    /// Split out from [`TerrainGenerator::height`] because the coastal
    /// reshaping has to know how far away the water is, and this smooth field
    /// is what that gets measured against. The detail layers are deliberately
    /// not part of it: their gradient is as steep as the landform's own, so
    /// including them would drown out the very thing being measured.
    fn landform(&self, wx: f32, wz: f32) -> f32 {
        under_ceiling(
            self.calibration.metres(self.landform_raw(wx, wz)),
            self.ceiling(wx, wz),
        )
    }

    /// The country-character dial at a point, 0 gentle to 1 rugged — the
    /// field cut at [`RUGGED_GENTLE`]/[`RUGGED_FULL`], nudged by altitude.
    /// Split out of [`height`] so the `island_shape` reporter can measure
    /// the split the cuts actually produce.
    ///
    /// [`height`]: TerrainGenerator::height
    fn rugged_dial(&self, wx: f32, wz: f32, base: f32) -> f32 {
        smoothstep(
            RUGGED_GENTLE,
            RUGGED_FULL,
            self.rugged.fbm(wx / RUGGED_SCALE, wz / RUGGED_SCALE, 2) + base / RUGGED_CLIMB,
        )
    }

    /// Terrain height in metres at a world-space `(x, z)`. Sea level is 0.
    pub fn height(&self, wx: f32, wz: f32) -> f32 {
        let base = self.landform(wx, wz);
        let mut h = base;

        // Surface detail, faded out towards the waterline and absent below
        // it — see [`APRON_DRY`] for why the coast must stay the landform's.
        let apron = smoothstep(APRON_DRY, APRON_FULL, base);
        if apron > 0.0 {
            // Which country this is. The dial suppresses relief rather than
            // adding any, so every bound counted from the reliefs below —
            // [`LAKE_RELIEF`] above all — still holds at its old value.
            let rugged = self.rugged_dial(wx, wz, base);
            let swell = self.detail.fbm(wx / DETAIL_SCALE, wz / DETAIL_SCALE, 3) * DETAIL_RELIEF;
            // The mid band changes shape with the dial, not just size:
            // gentle country rolls, rugged country breaks into the crests
            // and gullies only a ridged field draws. Recentred, since ridged
            // runs 0..1 and the blend has to pivot on the same zero.
            let roll = self.detail.fbm(wx / MICRO_SCALE, wz / MICRO_SCALE, 2);
            let crag = self.ridges.ridged(wx / MICRO_SCALE, wz / MICRO_SCALE, 2) * 2.0 - 1.0;
            let mid = (roll + (crag - roll) * rugged) * MICRO_RELIEF;
            let grain = self.detail.fbm(wx / GRAIN_SCALE, wz / GRAIN_SCALE, 1) * GRAIN_RELIEF;
            h += (swell * (RUGGED_SWELL_FLOOR + (1.0 - RUGGED_SWELL_FLOOR) * rugged)
                + (mid + grain) * (RUGGED_FINE_FLOOR + (1.0 - RUGGED_FINE_FLOOR) * rugged))
                * apron;
        }

        // Keep the detail layer from punching holes below sea level all over the
        // interior — otherwise the map is speckled with puddles.
        if base > 3.0 {
            h = h.max(0.5);
        }

        // The same rule, for the water that does not stand at one height the
        // world over. A lake's surface was found on the landform, before any of
        // the detail above existed, so near one the landform is what the ground
        // has to be: the detail fades out as the bare bowl approaches the water
        // and back in over [`LAKE_RELIEF`] of height either side. The drawn
        // waterline is then the landform's own contour at that level, which is
        // the line the flood found. The detail keeps its freedom to shape the
        // bed and the banks, and loses only its freedom to carry either across
        // the surface.
        let lake_calm = self.lakes.ground_weight(wx, wz);
        h = base + (h - base) * lake_calm;

        // Everything above is the same landscape whatever the coast does with
        // it; the rest of this decides what happens where it meets the sea.
        let distance = self.coast.metres(wx, wz);
        let shore = self.shore_character(wx, wz, distance);

        // Faded out inland rather than cut off at a height. Most of this map's
        // land is under [`CLIFF_HEIGHT`] — it is a gentle island — so a rule
        // written in heights alone would have a cliff coast lifting and
        // terracing plains half a kilometre from the sea.
        h += (shape_coast(h, distance, shore, self.coast_scale) - h) * coastal_weight(distance);
        h = self.skerries(wx, wz, h, shore);

        // The crag pass reworks the *finished* ground — see the constants'
        // header — because the bare coast it exists for is mostly shaped by
        // the coastal pass above. Where it stands is where the palette bares
        // rock: the spray weight is [`sprayed`], the same call the paint
        // thresholds, and the mountain band is read off the same wandered
        // height the parcel rows read — so relief and paint stray on one
        // tide. Faded in above the shore and calmed by lakes like the rest
        // of the detail; the outer test is only a cheap skip for the open
        // sea and the lowland interior, where the weight is identically 0.
        //
        // [`sprayed`]: TerrainGenerator::sprayed
        if distance < SPRAY_REACH + SPRAY_WANDER || h > MOOR_HEIGHT - BAND_WANDER {
            let wander = self
                .detail
                .fbm(wx / BAND_SCALE - 53.0, wz / BAND_SCALE + 29.0, 4);
            let barren = self.sprayed(distance, shore, wander).max(smoothstep(
                MOOR_HEIGHT,
                MOUNTAIN_HEIGHT,
                h + BAND_WANDER * wander,
            ));
            let cragging = barren * smoothstep(SHORE_TOP, CRAG_FULL, h) * lake_calm;
            if cragging > 0.0 {
                let cragged = self.crags(wx, wz, h, cragging);
                // A tarn's surface caps the cut the way the sea's shore band
                // does — [`LAKE_RELIEF`] budgets only the detail stack, and a
                // full cleft out-cuts it — and a drowned bank is not reworked
                // at all: the bed keeps the shape the flood read.
                h = match self.lakes.level(wx, wz) {
                    Some(level) if h >= level => cragged.max(level),
                    Some(_) => h,
                    None => cragged,
                };
            }
        }

        h.max(-MAX_DEPTH)
    }

    /// The surface level of the lake standing at or beside this world point,
    /// in metres above sea level — `None` where the only water is the sea's.
    ///
    /// "Beside" is a real reach: the answer is `Some` for [`LAKE_APRON`] out
    /// from the water, and up the bank within that until the ground is
    /// [`LAKE_RELIEF`] clear of the surface. So a caller with a height in hand
    /// draws the waterline itself — water stands wherever `height < level` —
    /// and finds it well inside the field rather than at its edge.
    pub fn lake_level(&self, wx: f32, wz: f32) -> Option<f32> {
        self.lakes.level(wx, wz)
    }

    /// What kind of coast this stretch is, as a continuous value: `-1.0` for
    /// ground that shelves gently away into sand, `+1.0` for ground that stands
    /// straight up out of the water, and the rocky shores in between.
    ///
    /// Read at the nearest point on the shoreline rather than underfoot, so that
    /// it is a property of a stretch of coast and not of a spot on the map. The
    /// field wanders across a coast as readily as along one, so sampled where
    /// it is used a single stretch can be a beach at the water and a cliff a
    /// hundred metres inland — and it builds both, which is a thing no
    /// coastline does.
    ///
    /// Which way the shore lies comes from the gradient of the distance field,
    /// except along its crest — the line equidistant between two shores — where
    /// the gradient flips a half turn and the nearest waterline point teleports
    /// from one coast to the other. Read naively the character jumps with it,
    /// cracking the height field along a dead-straight hairline scarp down the
    /// middle of every neck and strait. A distance field's slope is 1
    /// everywhere except approaching that crest, where it collapses — so the
    /// collapse *is* the detector, and the displacement is faded out on it.
    /// `distance` is [`CoastDistance::metres`] at the same point, which every
    /// caller already holds.
    fn shore_character(&self, wx: f32, wz: f32, distance: f32) -> f32 {
        let step = COAST_GRID;
        let gradient = Vec2::new(
            self.coast.metres(wx + step, wz) - self.coast.metres(wx - step, wz),
            self.coast.metres(wx, wz + step) - self.coast.metres(wx, wz - step),
        );

        // Already at the water, or on the flat of the field where it has no
        // opinion about which way the sea is.
        let (wx, wz) = if distance > 0.0 && gradient.length() > 1e-3 {
            let slope = gradient.length() / (2.0 * step);
            let shoreward = -gradient.normalize() * distance * smoothstep(0.35, 0.75, slope);
            (wx + shoreward.x, wz + shoreward.y)
        } else {
            (wx, wz)
        };

        let n = self.shore.fbm(wx / SHORE_SCALE, wz / SHORE_SCALE, 3);
        (n * SHORE_GAIN).clamp(-1.0, 1.0)
    }

    /// Which of the three kinds of coast the character field lands on here.
    fn shore(&self, wx: f32, wz: f32) -> Shore {
        let distance = self.coast.metres(wx, wz);
        Shore::of(self.shore_character(wx, wz, distance))
    }

    /// How hard the sea's spray bears on this ground, 0 untouched to 1 fully
    /// bare: the shore character's say, squared, fading out over
    /// [`SPRAY_REACH`] of wandered distance from the waterline.
    ///
    /// The one reading of the spray zone. The palette cuts this at
    /// [`SPRAY_BARE`] to paint the collar and the crag pass reworks ground by
    /// it, so where bare rock is painted and where rock forms stand cannot
    /// drift apart — they are the same number. The square is what keeps a
    /// middling-rocky stretch from wearing a collar half the reach wide; the
    /// `wander` is the band field both callers already hold, so the collar's
    /// edge strays off the coast-parallel the way the treeline strays off the
    /// level.
    fn sprayed(&self, distance: f32, character: f32, wander: f32) -> f32 {
        let bare = smoothstep(ROCKY_SHORE, CLIFF_SHORE, character);
        let seaward = distance + SPRAY_WANDER * wander;
        bare * bare * (1.0 - smoothstep(0.0, SPRAY_REACH, seaward))
    }

    /// Bare ground reworked into rock forms — see the [`CRAG_SCALE`] header
    /// for why and the shape of the parts. `weight` is how bare this point is,
    /// 0 to 1, and everything below scales with it.
    ///
    /// Order matters: spires and clefts first, strata last, so the terraces
    /// cut *across* the forms — ledges running through a stack's flank — as
    /// bedding does, rather than each spire carrying its own private steps.
    ///
    /// The waterline holds by construction and not by the fade alone: a cleft
    /// spends at most [`CRAG_HEADROOM`] of the headroom above the shore band,
    /// and the strata pull toward treads that are never below zero —
    /// `crags_never_reach_the_shore_band` pins the margin that leaves.
    fn crags(&self, wx: f32, wz: f32, h: f32, weight: f32) -> f32 {
        let qx = self
            .detail
            .fbm(wx / CRAG_WARP_SCALE + 71.0, wz / CRAG_WARP_SCALE - 17.0, 2);
        let qz = self
            .detail
            .fbm(wx / CRAG_WARP_SCALE - 43.0, wz / CRAG_WARP_SCALE + 59.0, 2);
        let (cx, cz) = (wx + qx * CRAG_WARP, wz + qz * CRAG_WARP);

        let ridge = self.ridges.ridged(cx / CRAG_SCALE, cz / CRAG_SCALE, 3);
        let spire = pow(ridge, CRAG_SHARP) * CRAG_SPIRE;
        let cleft = (CRAG_CLEFT * (1.0 - ridge)).min((h - SHORE_TOP).max(0.0) * CRAG_HEADROOM);
        let jagged = h + (spire - cleft) * weight;

        let steps = jagged / CRAG_STEP;
        let riser = smoothstep(
            0.5 - CRAG_RISER / 2.0,
            0.5 + CRAG_RISER / 2.0,
            steps.fract(),
        );
        let tread = (steps.floor() + riser) * CRAG_STEP;
        jagged + (tread - jagged) * CRAG_STRATA * weight
    }

    /// Rock heads standing offshore of a rocky coast, and only there.
    ///
    /// Only the top of a noise field is let through, which is what makes these
    /// a scatter of separate rocks rather than a reef: the cut is set off the
    /// field's measured distribution, where 0.20 is about its 83rd percentile
    /// and 0.34 its 95th. They also fade out in deep water, so they stay
    /// inshore of the coast they broke off — which is why a cliff, with its bed
    /// dropping away at once, gets far fewer of them than a rocky shore does.
    fn skerries(&self, wx: f32, wz: f32, height: f32, character: f32) -> f32 {
        if height >= 0.0 || character <= ROCKY_SHORE {
            return height;
        }

        let head = self.skerry.fbm(wx / SKERRY_SCALE, wz / SKERRY_SCALE, 2);
        let emerge = smoothstep(0.20, 0.34, head)
            * smoothstep(ROCKY_SHORE, CLIFF_SHORE, character)
            * (1.0 - smoothstep(3.0, 8.0, -height));

        height + emerge * (SKERRY_HEIGHT - height)
    }

    /// Surface normal, from central differences of the height field one tile
    /// either side. The mesh doesn't use this — flat shading takes its normals
    /// from the triangles themselves — so this is the smooth, tile-resolution
    /// normal for ground queries: which way something standing here would tip,
    /// whether ground is too steep to cross.
    pub fn normal(&self, wx: f32, wz: f32) -> Vec3 {
        normal_at(wx, wz, |x, z| self.height(x, z))
    }

    /// What one cell of ground is made of, picked from a fixed palette.
    ///
    /// Nothing here blends. Every choice is a hard threshold, so a cell gets
    /// exactly one material — that is what makes the ground read as flat
    /// shapes rather than as a wash of gradient, and it is what lets a cell
    /// cross the wire in a byte.
    ///
    /// Which means the work of getting from one band to the next is done by the
    /// *shape* of the boundary rather than by mixing the colours across it. Two
    /// things do it: the edges wander off the level by [`BAND_WANDER`], and the
    /// patchwork either side of them is cut from one field, so the parcels line
    /// up through the join. See [`LOWLAND_PARCELS`].
    pub fn material(&self, wx: f32, wz: f32, height: f32, normal: Vec3) -> Material {
        // 0 on flat ground, approaching 1 on a cliff face.
        let slope = 1.0 - normal.y;

        // A lake first, out of fresh water's own three materials, which is the
        // whole of what tells a lake from an inlet. The sea's bed brightens
        // towards its shore, and a pale shelf under a beach behind it is what
        // draws the turquoise ring every coast wears; give that ring to a lake
        // and it reads as an arm of the sea that happens to be inland. So a
        // lake darkens instead: silt, then weed, then a reed margin where the
        // sea would have sand.
        //
        // The three are measured *out from the lake's own edge* rather than
        // down from its surface — see [`LAKE_MARGIN`] and [`Lakes::shore`].
        // Reeds stand as far out as they can root, not as far up as the water
        // once came.
        //
        // The shore character field takes no part, being a property of
        // stretches of *sea* coast: a lake's margin is marsh where it lies flat
        // and bare rock where it stands steep, which is what tarns and lowland
        // pools do.
        //
        // Ground actually under the water joins them whatever the distance
        // says, which is not belt and braces: the field is measured on the
        // fitting grid and smoothed, so a pool narrower than the smoothing
        // would have its bed fall through to the sea's palette.
        let shore = self.lakes.shore(wx, wz);
        let drowned = self.lakes.level(wx, wz).is_some_and(|level| height < level);
        if shore < LAKE_MARGIN || drowned {
            if shore < -LAKE_SHALLOWS {
                return Material::Silt;
            }
            if shore < -LAKE_MARGIN {
                return Material::Shoal;
            }
            return if slope > ROCK_SLOPE {
                Material::RockDark
            } else {
                Material::Marsh
            };
        }

        // The sea. Two materials of sea bed, both read through translucent
        // water: a dark bottom, then a bright shelf that gives a coast its
        // turquoise ring. How wide that ring is comes from the landform rather
        // than from anything here — [`shape_coast`] gives a beach a long
        // shallow apron and drops a cliff straight past it.
        if height < -SHALLOW_DEPTH {
            // Before either bed colour, a wall is rock: a cell this steep is a
            // cliff's underwater face, and one Shallow cell there is a
            // ten-metre streak of turquoise up the rock, because a cell's
            // colour is stretched over however much face its corners span.
            // Dark at [`ROCK_SLOPE`] where the dry cascade waits for
            // [`CLIFF_SLOPE`], deliberately: below the waterline rock is wet
            // and wet rock is dark, so the flip to the lighter dry grey at
            // the shore band is a tide line, not a seam.
            if slope > ROCK_SLOPE {
                return Material::RockDark;
            }
            if height < -SEABED_DEPTH {
                return Material::Seabed;
            }
            return Material::Shallow;
        }

        // The shore itself, from the low-water mark to the back of the beach.
        // The slope test comes first, so the wave-cut foot of a cliff is rock
        // rather than sand; on a beach there is no slope to speak of, so it
        // never fires and the sand stays clean.
        if height < SHORE_TOP {
            if slope > ROCK_SLOPE {
                return Material::RockDark;
            }
            return match self.shore(wx, wz) {
                Shore::Beach => Material::Sand,
                Shore::Rocky => Material::Shingle,
                Shore::Cliff => Material::RockDark,
            };
        }

        // Steep ground is bare rock whatever height it's at. Above the shore
        // this is what paints the cliff faces, and inland it picks out crags on
        // the hills the same way.
        if slope > CLIFF_SLOPE {
            return Material::RockDark;
        }
        if slope > ROCK_SLOPE {
            return Material::Rock;
        }

        // How far this spot's band edges have strayed from the level.
        let wander = self
            .detail
            .fbm(wx / BAND_SCALE - 53.0, wz / BAND_SCALE + 29.0, 4);

        // The height everything reads its band off.
        let banded = height + BAND_WANDER * wander;

        // The patchwork. Quantising a low-frequency noise field into a few
        // buckets gives irregular parcels with hard edges — woodland against
        // pasture against crop — instead of one smooth green wash.
        //
        // Two fields go into the one bucket, at different scales. The broad
        // one lays out the parcels; the finer [`MOTTLE_SCALE`] one nudges the
        // total, which breaks a big parcel into patches of its neighbours in
        // the palette row rather than leaving it one flat slab. That used to
        // be a brightness step riding on top of the tone, which meant the
        // wire carried a rendering instruction — how much to scale a colour
        // by — next to the material it applied to. Saying *grass, but the
        // lighter kind* with a second material costs nothing extra on the
        // wire and leaves the byte naming a substance and nothing else.
        let patch = self
            .detail
            .fbm(wx / PATCH_SCALE + 11.0, wz / PATCH_SCALE - 7.0, 3);
        let mottle = self.detail.fbm(wx / MOTTLE_SCALE, wz / MOTTLE_SCALE, 2);
        let field = patch + MOTTLE_WEIGHT * mottle;
        // Thresholds are set off the summed field's measured distribution,
        // not off its nominal range, so all five actually get used: it reaches
        // about ±0.7, but four fifths of it is inside ±0.24, which leaves the
        // two outer parcels a quarter of it between them. Measured on the
        // total rather than on [`PATCH_SCALE`]'s field alone — the mottle
        // widens it, a little.
        let bucket = if field < -0.20 {
            0
        } else if field < -0.07 {
            1
        } else if field < 0.08 {
            2
        } else if field < 0.22 {
            3
        } else {
            4
        };

        // Salt spray, before anything is allowed to grow: the one spray
        // weight — [`TerrainGenerator::sprayed`], the same call the crag pass
        // works ground by — cut at [`SPRAY_BARE`]. The character read hides
        // behind the cheap distance test, so the interior never pays for it.
        // Painted from the mountain row rather than as one material: bare is
        // bare, one palette row, and a collar tens of metres wide in a single
        // flat colour was exactly the slab the patchwork exists to prevent.
        let distance = self.coast.metres(wx, wz);
        if distance < SPRAY_REACH + SPRAY_WANDER {
            let character = self.shore_character(wx, wz, distance);
            if self.sprayed(distance, character, wander) > SPRAY_BARE {
                return MOUNTAIN_PARCELS[bucket];
            }
        }

        // Which row of the palette that parcel is drawn from — the only thing
        // height decides up here.
        if banded > MOUNTAIN_HEIGHT {
            MOUNTAIN_PARCELS[bucket]
        } else if banded > MOOR_HEIGHT {
            MOOR_PARCELS[bucket]
        } else {
            LOWLAND_PARCELS[bucket]
        }
    }
}

/// One chunk's [`CORNERS`]-square corner grid of anything: row-major,
/// sampled at [`CELL_METRES`] spacing from `base`. The one place the
/// sampling convention is written, so the payload's grids cannot fall out
/// of register with each other — [`corner_water`] stays its own loop only
/// for the fold it carries.
fn corner_grid<T>(base: Vec2, sample: impl Fn(f32, f32) -> T) -> Vec<T> {
    let mut grid = Vec::with_capacity(CORNERS * CORNERS);
    for iz in 0..CORNERS {
        let wz = base.y + iz as f32 * CELL_METRES;
        for ix in 0..CORNERS {
            grid.push(sample(base.x + ix as f32 * CELL_METRES, wz));
        }
    }
    grid
}

/// The corner heights one chunk of ground is built from — and what the
/// payload carries.
///
/// The loop order is the format, so a caller may also *look* at the grid before
/// deciding whether the chunk is worth sending at all — the open world skips
/// chunks whose every corner sits on the ocean floor.
pub(crate) fn corner_heights(base: Vec2, height: impl Fn(f32, f32) -> f32) -> Vec<f32> {
    corner_grid(base, height)
}

/// The standing water one chunk carries, on the same grid and in the same
/// order as [`corner_heights`] — or `None` where the chunk has no lake water
/// on it, which is most of them.
///
/// Sampled at the corners rather than derived from the heights, because a
/// lake's level is not a property of the ground under it: it comes from a rim
/// saddle that may be nowhere near the chunk. A corner the lakes have no
/// answer for stores [`protocol::ground::NO_WATER`].
///
/// The grid is kept only where some corner is actually *under* its level, which
/// is the difference between a lake's water and a lake's mere presence:
/// [`TerrainGenerator::lake_level`] answers out past a lake's edge, so a chunk
/// catching only bank has no water to draw and would otherwise pay a full grid
/// to say so.
pub(crate) fn corner_water(
    base: Vec2,
    heights: &[f32],
    level: impl Fn(f32, f32) -> Option<f32>,
) -> Option<Vec<u16>> {
    debug_assert_eq!(
        heights.len(),
        CORNERS * CORNERS,
        "not a chunk's corner grid"
    );
    let mut levels = vec![protocol::ground::NO_WATER; CORNERS * CORNERS];
    let mut awash = false;
    for iz in 0..CORNERS {
        let wz = base.y + iz as f32 * CELL_METRES;
        for ix in 0..CORNERS {
            let wx = base.x + ix as f32 * CELL_METRES;
            let Some(level) = level(wx, wz) else { continue };
            levels[iz * CORNERS + ix] = protocol::ground::quantize(level);
            awash |= heights[iz * CORNERS + ix] < level;
        }
    }
    awash.then_some(levels)
}

/// When each of one chunk's corners sees the sun, on the same grid and in
/// the same order as [`corner_heights`] — read off the island's bake, which
/// is where the answer lives; see [`crate::sunlight`].
pub(crate) fn corner_lit(base: Vec2, lit: impl Fn(f32, f32) -> [u8; 2]) -> Vec<[u8; 2]> {
    corner_grid(base, lit)
}

/// The material of every cell of one chunk's grid, row-major — the order a
/// payload carries them in.
///
/// `heights` is what [`corner_heights`] returned for the same `base`. A cell
/// sits between the corners either side of it, so its centre is half a cell
/// in from its lower corner.
///
/// The normal a cell is classified by is the one its four corners describe,
/// not the generator's own gradient and not a triangle's. That is what keeps
/// the answer independent of how anybody draws the cell: a client is free to
/// split the square either way, or to draw it as a textured quad and never
/// triangulate it at all, and the ground it draws will be made of the same
/// stuff either way. It used to be a triangle's own normal, which quietly
/// made the palette depend on a triangulation the wire no longer carries.
pub(crate) fn cell_materials(
    base: Vec2,
    heights: &[f32],
    material: impl Fn(f32, f32, f32, Vec3) -> Material,
) -> Vec<Material> {
    debug_assert_eq!(
        heights.len(),
        CORNERS * CORNERS,
        "not a chunk's corner grid"
    );
    let corner = |ix: usize, iz: usize| heights[iz * CORNERS + ix];

    let mut materials = Vec::with_capacity(CELL_COUNT);
    for iz in 0..CELLS {
        for ix in 0..CELLS {
            let (sw, se) = (corner(ix, iz), corner(ix + 1, iz));
            let (nw, ne) = (corner(ix, iz + 1), corner(ix + 1, iz + 1));

            // The bilinear patch's slope at the cell's centre, which is the
            // mean of the two edges running each way. Written from the four
            // corners around the cell rather than sampled afresh: a client
            // reading the same heights arrives at the same normal, so the
            // ground it lights matches the ground it was sent.
            let along = (se + ne - sw - nw) / (2.0 * CELL_METRES);
            let across = (nw + ne - sw - se) / (2.0 * CELL_METRES);
            let normal = Vec3::new(-along, 1.0, -across).normalize();

            let mid = base + (Vec2::new(ix as f32, iz as f32) + Vec2::splat(0.5)) * CELL_METRES;
            let height = (sw + se + nw + ne) / 4.0;
            materials.push(material(mid.x, mid.y, height, normal));
        }
    }
    materials
}

/// Dimensions of a [`COAST_GRID`]-spaced grid covering a map of `tiles`,
/// inclusive of both edges.
fn grid_dims(tiles: UVec2) -> (usize, usize) {
    (
        (tiles.x as f32 / COAST_GRID).ceil() as usize + 1,
        (tiles.y as f32 / COAST_GRID).ceil() as usize + 1,
    )
}

/// Turns the raw landform field into metres, fitted to the map it was sampled
/// from so that every seed comes out with the same *amount* of landscape.
///
/// Noise gives shape, never proportion. A seed whose field runs low makes a
/// drowned map and one that runs high makes a plateau, and a threshold
/// hard-coded to suit one is wrong for the other — the reason the ranges never
/// appeared once was a mask cut at 0.55 on a field whose 99th percentile is
/// 0.38. Reading the field's own distribution and solving for the numbers that
/// hit [`LAND_FRACTION`] and [`MOUNTAIN_FRACTION`] makes the targets true by
/// construction.
///
/// Above the waterline the mapping is a straight line — bent, on the drowned
/// seeds only, at one place and in one direction. A curve that steepens towards
/// the top is the obvious way to raise peaks out of a gentle field, and it is a
/// trap: what is up there is a ridged field whose maximum is a crease, so the
/// peaks come out as vertical knife blades, and on seeds with no sharp top the
/// fit has nothing to bite on at all. Keeping it linear leaves the mountains
/// the shape the massif gives them, and their height to
/// [`TerrainGenerator::fit_range_height`].
///
/// The one exception is the [`LOWLAND_FLOOR`]: a seed whose land median would
/// come out under it gets the segment *below* the median steepened until the
/// median lands on the floor. That magnifies the bottom of the field rather
/// than the ridged top, and it is anchored on the land median rather than on
/// the mountain quantile the range fit steers by — an earlier attempt hung a
/// fitted exponent on that same quantile and the two fits chased each other.
#[derive(Default)]
struct Calibration {
    /// Raw value that sea level sits at.
    sea_level: f32,
    /// Raw distance below sea level that the sea's median sits at — where the
    /// two depth slopes meet. The mirror of `knee`.
    sea_knee: f32,
    /// Metres of depth the sea knee maps to.
    sea_knee_depth: f32,
    /// Metres per raw unit above the sea knee, and below it. Equal on any
    /// seed the floor leaves alone, like the land slopes.
    depth_shallow: f32,
    depth_deep: f32,
    /// Raw distance above sea level that the land's median sits at — where the
    /// two land slopes meet.
    knee: f32,
    /// Metres the knee maps to.
    knee_height: f32,
    /// Metres per raw unit below the knee, and above it. Equal on any seed the
    /// floor leaves alone, so the line is straight unless it had to bend.
    slope_low: f32,
    slope_high: f32,
}

/// One half of the mapping, bent if it has to be to clear a floor.
///
/// Both halves of [`Calibration`] are fitted this way — the land above the
/// waterline and the sea below it — and they are the same arithmetic with the
/// sign turned round, so it is written once here rather than twice there.
///
/// A healthy seed gets a straight line of `gain` per raw unit, which would put
/// its median at `gain * knee`. Where that falls short of `floor` the line
/// bends at the median instead: the near segment steepens until the median
/// lands exactly on the floor, and the far segment is re-fitted so that `far`
/// still maps to `far_value` and the two meet without a step.
///
/// Returns what the knee maps to and the slopes either side of it. A seed that
/// needed no bend comes back with both slopes equal, which is what lets
/// [`Calibration::metres`] read the bent and the straight case with one
/// expression instead of asking which kind of seed it has.
fn bent_line(knee: f32, gain: f32, floor: f32, far: f32, far_value: f32) -> (f32, f32, f32) {
    let natural = gain * knee;
    // A knee at the waterline, or one that has already reached the far anchor,
    // leaves no segment to bend — take the straight line rather than dividing
    // by the width of a gap that isn't there.
    if natural < floor && knee > 1e-4 && far - knee > 1e-4 {
        (floor, floor / knee, (far_value - floor) / (far - knee))
    } else {
        (natural, gain, gain)
    }
}

impl Calibration {
    /// Fits to a grid of raw samples covering the whole map, steering by the
    /// map's [`Targets`]. `raw` is sorted in place — it is the caller's
    /// scratch, not a field of anything.
    fn fit(raw: &mut [f32], targets: &Targets) -> Self {
        let Targets {
            land,
            relief,
            shoal,
        } = *targets;
        raw.sort_by(|a, b| a.partial_cmp(b).expect("no NaN in a height field"));
        let quantile = |q: f32| raw[((raw.len() - 1) as f32 * q) as usize];

        let sea_level = quantile(1.0 - land);

        // The above-water heights this map is aimed at. Scaling both together
        // keeps the lowland floor below the mountain line whatever the map
        // size, so the bent mapping below can never fold back on itself.
        let mountain_height = MOUNTAIN_HEIGHT * relief;
        let lowland_floor = LOWLAND_FLOOR * relief;

        // One point fixes the line: the height mountains start at, placed so
        // that exactly the intended share of land is above it. Everything else
        // — how low the lowlands are, how high the peaks reach — follows from
        // the field's own shape, which is the point.
        let mountain = quantile(1.0 - land * MOUNTAIN_FRACTION) - sea_level;
        let slope = if mountain > 1e-4 {
            mountain_height / mountain
        } else {
            // A flat field, or one with no land in it at all. Nothing to fit;
            // take something harmless rather than dividing by zero.
            HEIGHT_SCALE
        };

        // The land's median, and where the straight line would put it. Only a
        // median that lands under the floor bends the line; the knee fields are
        // still filled in on the straight seeds, with both slopes equal, so
        // `metres` never has to ask which kind of seed this is.
        let knee = quantile(1.0 - land * 0.5) - sea_level;
        let (knee_height, slope_low, slope_high) =
            bent_line(knee, slope, lowland_floor, mountain, mountain_height);

        // The same again below the waterline: the sea's median — slid to the
        // shallow side on the smallest maps, whose median is in the falloff
        // ring, see [`shoal_shift`] — and where the straight slope through
        // the deep anchor would put it. Only an anchor that comes out
        // shallower than the floor bends the line. The floor eases with the
        // slide: the shallower the anchored cells, the less depth they have
        // to be guaranteed.
        let deep_fraction = DEEP_FRACTION + (DEEP_FRACTION_SMALL - DEEP_FRACTION) * shoal;
        let deep = sea_level - quantile(deep_fraction);
        let depth_gain = MAX_DEPTH / deep.max(1e-4);
        let shallows_floor = SHALLOWS_FLOOR - 1.5 * shoal;
        let sea_knee = sea_level - quantile((1.0 - land) * (0.5 + 0.35 * shoal));
        let (sea_knee_depth, depth_shallow, depth_deep) =
            bent_line(sea_knee, depth_gain, shallows_floor, deep, MAX_DEPTH);

        Self {
            sea_level,
            sea_knee,
            sea_knee_depth,
            depth_shallow,
            depth_deep,
            knee,
            knee_height,
            slope_low,
            slope_high,
        }
    }

    fn metres(&self, raw: f32) -> f32 {
        let t = raw - self.sea_level;
        if t <= 0.0 {
            let depth = if -t <= self.sea_knee {
                -t * self.depth_shallow
            } else {
                self.sea_knee_depth + (-t - self.sea_knee) * self.depth_deep
            };
            (-depth).max(-MAX_DEPTH)
        } else if t <= self.knee {
            t * self.slope_low
        } else {
            self.knee_height + (t - self.knee) * self.slope_high
        }
    }
}

/// How far every point on the map is from the waterline, in metres.
///
/// The coast has to know this, and no local measurement can stand in for it:
/// the slope at a point says how fast the ground is falling, never whether that
/// fall ever reaches the sea. A distance transform over the whole map answers
/// it properly and costs almost nothing — one pass over a [`COAST_GRID`] grid
/// per map, then an O(1) lookup per vertex.
///
/// Where the water is comes from the landform alone, before any of the detail
/// layers, which keeps the field smooth and keeps it from depending on the very
/// coastline it is about to shape.
#[derive(Default)]
struct CoastDistance {
    /// Distance in cells. Metres come from multiplying by [`COAST_GRID`].
    field: GridField,
}

impl CoastDistance {
    /// Measures out from wherever the calibration put the waterline in an
    /// already-sampled grid of raw landform values.
    fn from_raw(raw: &GridField, calibration: &Calibration) -> Self {
        let seeds = raw
            .cells
            .iter()
            .map(|v| {
                if *v <= calibration.sea_level {
                    0.0
                } else {
                    f32::INFINITY
                }
            })
            .collect();
        Self::measure(seeds, raw.dims, raw.origin)
    }

    /// The same, but measured from open water only — from the sea, and from
    /// the inland bodies big enough to stand in for it. A pond counts for
    /// nothing and distance climbs straight across it.
    ///
    /// Which is what the mountains have to be held back from. These maps are
    /// riddled with inland water, and measured from all of it a range is
    /// forbidden its height for having a pond beside it — the pond is a feature
    /// *of* the upland, not a coast it has to climb from. On a two-kilometre
    /// map that was the difference between one range and several.
    ///
    /// What separates the two is **size**, and deliberately not whether the
    /// water joins the sea. Reaching the frame was tried first and is a
    /// property of the map's topology, so it changed the map in steps: a lagoon
    /// joined by a single cell of strait counted wholly as sea, and a map grown
    /// one notch silted the strait up and made it wholly ground. Over
    /// 128-metre steps of size, one seed's largest distance went 101, 131, 288,
    /// 320, 340 m — a 157-metre jump that turned a headland into a range.
    ///
    /// Size has no such cliff: a pool grows and shrinks by a cell at a time, so
    /// the field moves with it. It is also the better rule on its merits —
    /// every drop of water here sits at the one fitted sea level, so a big
    /// enclosed lagoon *is* at base level whether or not a spit closes it off.
    ///
    /// A pool's say is graded rather than granted: full sea seeds the transform
    /// at zero, a pond seeds it [`INLAND_REACH`] out — far enough to bind
    /// nothing — and the sizes between seed proportionally. The chamfer then
    /// relaxes each seed against every better one.
    fn from_open_water(raw: &GridField, calibration: &Calibration) -> Self {
        let wet: Vec<f32> = raw
            .cells
            .iter()
            .map(|v| {
                if *v <= calibration.sea_level {
                    1.0
                } else {
                    0.0
                }
            })
            .collect();
        let openness = water_fraction(&wet, raw.dims, (OPEN_RADIUS / COAST_GRID).round() as usize);

        let inert = INLAND_REACH / COAST_GRID;
        let seeds = wet
            .iter()
            .zip(&openness)
            .map(|(wet, open)| {
                if *wet < 0.5 {
                    return f32::INFINITY;
                }
                inert * (1.0 - smoothstep(OPEN_POND, OPEN_SEA, *open))
            })
            .collect();
        Self::measure(seeds, raw.dims, raw.origin)
    }

    /// Distance out from a set of seeds — zero at a cell that counts wholly as
    /// water, higher at one that counts partly, infinite at ground.
    fn measure(seeds: Vec<f32>, dims: (usize, usize), origin: Vec2) -> Self {
        let mut field = GridField::new(seeds, dims, origin);

        field.chamfer();
        // The transform is exact, and exact is the problem: everywhere two
        // wavefronts meet the field folds in a sharp crease, and everything
        // built from it — cliff faces most of all, being the field times a
        // steep pitch — prints those creases into the ground as dead-straight
        // hairline ridges. Four passes approximate a Gaussian a couple of
        // cells wide, which rounds them off while leaving a field read at
        // coastline scale unchanged.
        for _ in 0..4 {
            field.blur();
        }
        Self { field }
    }

    /// Distance to the waterline at a world-space point, in metres. Before
    /// generation has run there is no field yet, and everywhere counts as far
    /// from water; outside the grid it reads the edge, which is open sea on
    /// every map.
    fn metres(&self, wx: f32, wz: f32) -> f32 {
        if self.field.dims.0 == 0 {
            return f32::INFINITY;
        }
        self.field.at(wx, wz) * COAST_GRID
    }
}

// --- Lakes -------------------------------------------------------------------
//
// Noise digs hollows at every altitude, and one whose floor stands above sea
// level was, until here, a dry green bowl — a thing rain does not permit.
// Anywhere the terrain encloses, water stands.
//
// Where that water's surface sits cannot be read off any point of the ground:
// it is set by the lowest saddle on the whole rim of the basin, which may be
// half a kilometre from the shore it decides. So lakes are found in one pass
// over the fitting grid at construction, by a priority flood — walk out from
// the sea always taking the lowest frontier cell first, and each cell is first
// reached along the route whose highest point is lowest, which is exactly the
// level water must rise to before that cell drains. Terraced and nested basins
// fall out of the same walk without being special cases.
//
// The flood runs on the landform, not the finished height: flooding the drawn
// field would find a thousand puddle-sized dimples rather than basins.
//
// Reconciling the two fields by letting the drawn ground wander across the
// answer, the way it wanders across sea level, does not work — sea level is
// one number over the whole world, so wandering only moves a coast, where a
// lake's level holds over its own basin and ground wandering across it
// *outside* the basin is water the flood never found, ending in a straight
// grid-aligned edge with no shore. Inside, the same wander beached any basin
// shallower than the texture over it: a quarter of the flooded area on the
// seeds surveyed came out drawn as dry ground.
//
// So the ground gives way to the water instead. The detail fades out as the
// landform approaches a lake's surface and back in over [`LAKE_RELIEF`] either
// side, making the drawn waterline the landform's own contour at that level.
// Only the detail — fading the coastal reshaping out as well took the flat out
// of any beach standing near a lake.
//
// A lake's *colours* are a separate question and not answered by a height at
// all. Silt, weed and reed margin were depths and a height above the surface
// once, and a lake is the worst place on the map for a height threshold: the
// fade leaves its banks smooth and its bed shelves gently, so the margin came
// out under a facet wide and the weed's edge came out fractal. They are
// measured out from the water's edge instead — see [`Lakes::shore`].

/// Metres a lake's surface stands below the saddle it would otherwise spill
/// over.
///
/// Real lakes sit below their outlets, but the margin's real job is keeping
/// the feature continuous in the map-size control. A basin's depth moves
/// smoothly as the size sweeps, so a lake whose saddle is silting up drains
/// gradually through the freeboard and slips off the shallow end, where a
/// keep-or-drop test on depth or area would pop whole lakes in and out
/// between neighbouring sizes. It also keeps the surface clear of its own
/// rim, where water at exactly the saddle's height would shave along the
/// ground.
const LAKE_FREEBOARD: f32 = 0.5;

/// How far a lake's surface has any say over the ground, in metres of landform
/// height above or below it: at the surface the ground is the bare landform,
/// and by this much clear of it everything laid over the landform is back at
/// full strength.
///
/// Written as height rather than as a distance along the ground, so that a
/// steep bank gets a narrow margin and a shallow one a wide one — which is
/// what keeps the fade from reading as a ring stamped around the water.
///
/// The size of it is not a taste: it is what holds the ground to the side of
/// the surface the flood put it. The weight is [`smoothstep`], so the ground
/// is displaced by at most `relief * t²(3-2t)` where the surface is
/// `relief * t` away, and `t(3-2t)` peaks at 1.125 — so the displacement
/// itself can never carry ground across the water. What can, by centimetres,
/// is the fade being *read* off the fitting grid: between two of its cells
/// the true weight curves where the bilinear read runs straight, and the
/// finer the detail bands the further the field curves past the line. The
/// lake test measures that leak and carries it as its slack — a few
/// centimetres, against a guarantee that holds in metres.
///
/// Sized against the detail and only the detail. The coastal reshaping happens
/// downstream and answers to the sea, so a lake within [`SHORE_REACH`] of the
/// sea's waterline can still have its bank bent out from under this — which is
/// what [`LAKE_APRON`] is for: where the guarantee does not hold, what leaks is
/// a few metres of apron.
const LAKE_RELIEF: f32 = (DETAIL_RELIEF + MICRO_RELIEF + GRAIN_RELIEF) * 1.125;

/// Standing water above sea level: how high a lake stands over the ground
/// around it, and how far it holds that ground to the shape it was found in.
///
/// Three grids over the fitting grid, because a lake is asked three different
/// questions and they do not have the same answer. *Which lake stands here,
/// and how high* is a step function — a surface is dead level over its own
/// basin and absent a stride outside it — and is what the water and the wire
/// read. *How much of what is laid over the landform survives here* has to be
/// smooth, or the ground itself steps, and a step in the ground along a grid
/// line is the one thing the whole fitting grid is careful never to produce.
/// *How far from the water is this* is what the painting reads, and is neither
/// of the other two: it is a distance, measured out along the ground.
#[derive(Default)]
struct Lakes {
    field: GridField,
    weight: GridField,
    shore: GridField,
}

/// Which way a lake's level is allowed to travel out of its own water, in
/// [`Lakes::spread`].
///
/// The two questions want different answers, and the difference is a rim.
/// `Climbing` never drops back towards the water, so it stops on the crest and
/// the answer is *this lake's own basin* — which is what the water wants, a
/// level handed out past a rim being water offered to a hillside that drains
/// elsewhere.
///
/// The ground has to be held over a wider set, the landform on a four-metre
/// grid not climbing monotonically: a bank that dips a few centimetres on its
/// way up stops a climbing spread dead, and the cells past the stall keep full
/// detail hard against water they stand level with — two metres of ground dug
/// out below a lake's surface a stride from its shore.
#[derive(Clone, Copy)]
enum Bank {
    Climbing,
    Anywhere,
}

impl Lakes {
    /// Finds every lake on a grid of landform metres: floods from the sea,
    /// keeps whatever the flood leaves under water less the freeboard, and
    /// works out from there what each lake's surface means for the ground.
    fn from_ground(ground: &GridField) -> Self {
        let fill = priority_flood(ground);
        let mut levels: Vec<f32> = ground
            .cells
            .iter()
            .zip(&fill)
            .map(|(ground, fill)| {
                let level = fill - LAKE_FREEBOARD;
                if level > *ground {
                    level
                } else {
                    f32::NEG_INFINITY
                }
            })
            .collect();
        // Measured before either spread, off the water itself: the spreads are
        // about how far a lake's *level* carries, and this is about where its
        // edge is.
        let shore = Self::shore_distance(&levels, ground.dims, ground.origin);
        let mut reach = levels.clone();
        Self::spread(ground, &mut levels, Bank::Climbing);
        Self::spread(ground, &mut reach, Bank::Anywhere);
        let weight = Self::weights(ground, &reach);
        Self {
            field: GridField::new(levels, ground.dims, ground.origin),
            weight: GridField::new(weight, ground.dims, ground.origin),
            shore,
        }
    }

    /// How far each cell is from the nearest lake's edge, in metres — negative
    /// under water, positive on the ground around it, and zero on the
    /// waterline the flood found.
    ///
    /// Two distance transforms, one out of the water and one into it, which is
    /// the cheapest way to a signed one and reuses the transform the coast is
    /// already built on. Each measures from the *cell* it seeded rather than
    /// from the line between two cells, so both are half a cell long; taking
    /// that half back is what puts the zero on the waterline instead of half a
    /// stride behind it, on both sides at once.
    ///
    /// Blurred once, for the reason [`CoastDistance::measure`] blurs. Once and
    /// no more, unlike the coast's four: this field is read at the scale of a
    /// few metres, and smoothing that reaches further than the band it places
    /// walks the band's edges off the water — worst on small lakes, where
    /// enough of it drags the zero inside the pool.
    ///
    /// Capped at a reach nothing reads past, so that a map's far corner holds
    /// a number rather than an infinity for the blur to spread.
    fn shore_distance(levels: &[f32], dims: (usize, usize), origin: Vec2) -> GridField {
        let wet: Vec<bool> = levels.iter().map(|l| *l > f32::NEG_INFINITY).collect();
        if !wet.iter().any(|wet| *wet) {
            return GridField::default();
        }
        let seeded = |water: bool| {
            let cells = wet
                .iter()
                .map(|wet| if *wet == water { 0.0 } else { f32::INFINITY })
                .collect();
            let mut field = GridField::new(cells, dims, origin);
            field.chamfer();
            field.cells
        };
        let (out, into) = (seeded(true), seeded(false));

        let cap = 2.0 * LAKE_SHALLOWS;
        let cells = wet
            .iter()
            .zip(out.iter().zip(&into))
            .map(|(wet, (out, into))| {
                let cells = if *wet { 0.5 - into } else { out - 0.5 };
                (cells * COAST_GRID).clamp(-cap, cap)
            })
            .collect();
        let mut field = GridField::new(cells, dims, origin);
        field.blur();
        field
    }

    /// Spreads each lake's level from the cells it covers out over the ground
    /// around them: every cell standing above the surface but within
    /// [`LAKE_RELIEF`] of it, taking the higher where two lakes' banks meet —
    /// which matches the surface a client would draw across the join.
    ///
    /// The reach is what lets a caller draw the waterline itself. Answered for
    /// one cell past the water and no further — true of the landform the flood
    /// ran on, and not of the ground drawn over it — any lake whose banks the
    /// detail had dug into stopped dead on a straight grid-aligned edge.
    fn spread(ground: &GridField, levels: &mut [f32], bank: Bank) {
        let (nx, nz) = ground.dims;
        if nx == 0 || nz == 0 {
            return;
        }
        let mut queue: std::collections::VecDeque<usize> = (0..levels.len())
            .filter(|cell| levels[*cell] > f32::NEG_INFINITY)
            .collect();
        while let Some(cell) = queue.pop_front() {
            let level = levels[cell];
            let here = ground.cells[cell];
            let (x, z) = (cell % nx, cell / nx);
            for (dx, dz) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                let (cx, cz) = (x as i32 + dx, z as i32 + dz);
                if !(0..nx as i32).contains(&cx) || !(0..nz as i32).contains(&cz) {
                    continue;
                }
                let next = cz as usize * nx + cx as usize;
                let step = ground.cells[next];
                let wanted = match bank {
                    Bank::Climbing => step >= here,
                    Bank::Anywhere => true,
                };
                let above = step > level && step < level + LAKE_RELIEF;
                if wanted && above && levels[next] < level {
                    levels[next] = level;
                    queue.push_back(next);
                }
            }
        }
    }

    /// How much of what is laid over the landform survives at each cell: none
    /// at a lake's surface, all of it [`LAKE_RELIEF`] clear of one, and all of
    /// it everywhere no lake has a say.
    ///
    /// Taken down to the lowest of each cell's own neighbourhood before it is
    /// read back blended, which is what makes the smooth field safe as a
    /// guarantee: blending alone would let a neighbour lend a cell weight the
    /// ground there has no room for, worst on a lake pinched into a single
    /// cell. Erring low costs only a slightly wider apron of smoothed ground.
    fn weights(ground: &GridField, levels: &[f32]) -> Vec<f32> {
        let (nx, nz) = ground.dims;
        let raw: Vec<f32> = ground
            .cells
            .iter()
            .zip(levels)
            .map(|(ground, level)| {
                if *level > f32::NEG_INFINITY {
                    smoothstep(0.0, LAKE_RELIEF, (ground - level).abs())
                } else {
                    1.0
                }
            })
            .collect();
        (0..raw.len())
            .map(|cell| {
                let (x, z) = (cell % nx, cell / nx);
                let mut lowest = raw[cell];
                for (dx, dz) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                    let (cx, cz) = (x as i32 + dx, z as i32 + dz);
                    if (0..nx as i32).contains(&cx) && (0..nz as i32).contains(&cz) {
                        lowest = lowest.min(raw[cz as usize * nx + cx as usize]);
                    }
                }
                lowest
            })
            .collect()
    }

    /// The surface level of the lake with a say over a world point, in metres
    /// above sea level, or `None` where the only water is the sea's.
    ///
    /// The cell the point falls in, unblended: a lake's surface is dead level,
    /// so there is nothing to interpolate, and blending across the shore would
    /// tilt the rim of every lake down into its own banks.
    ///
    /// Where the answer *stops* is [`LAKE_APRON`] out from the water rather
    /// than wherever the spread happened to stall — a step costs nothing on
    /// ground clear of the water, and a rectangle of water on the grass where
    /// it is not.
    fn level(&self, wx: f32, wz: f32) -> Option<f32> {
        let (nx, nz) = self.field.dims;
        if nx == 0 {
            return None;
        }
        let fx = ((wx - self.field.origin.x) / COAST_GRID).clamp(0.0, (nx - 1) as f32);
        let fz = ((wz - self.field.origin.y) / COAST_GRID).clamp(0.0, (nz - 1) as f32);
        let level = self.field.cells[fz.round() as usize * nx + fx.round() as usize];
        (level > f32::NEG_INFINITY && self.shore(wx, wz) < LAKE_APRON).then_some(level)
    }

    /// How much of what is laid over the landform survives at a world point —
    /// 1.0 out of every lake's reach, and on a map that has no lakes at all.
    fn ground_weight(&self, wx: f32, wz: f32) -> f32 {
        if self.weight.dims.0 == 0 {
            return 1.0;
        }
        self.weight.at(wx, wz)
    }

    /// Metres from the nearest lake's edge at a world point, negative under
    /// water — and infinite on a map with no lakes, which is most of them.
    ///
    /// Blended, unlike [`Lakes::level`]: this one *is* a smooth field, and it
    /// is read to place a boundary rather than to answer a yes or no, so the
    /// grid it was measured on must not show through.
    fn shore(&self, wx: f32, wz: f32) -> f32 {
        if self.shore.dims.0 == 0 {
            return f32::INFINITY;
        }
        self.shore.at(wx, wz)
    }
}

/// The level water must rise to before each cell of a landform grid drains,
/// in metres: the cell's own ground where it drains freely, and the height of
/// its basin's lowest rim saddle where it does not.
///
/// Every cell at or below sea level seeds the frontier, not just the map's
/// border: a hollow the calibration already flooded is the sea's however
/// landlocked, so it is an outlet here and no lake is raised over it. The
/// border alone would have turned every enclosed lagoon into a lake at its
/// saddle's height, redrawing coasts the whole coastal machinery had been
/// fitted to.
///
/// Neighbours are the four edge-adjacent cells: letting water slip diagonally
/// between two corner-touching cells would drain any lake with a pinch in it.
///
/// The result is exact whatever order ties are popped in — every value is a
/// max of ground heights along a route, never an accumulation — so the
/// tie-break on the cell index is belt and braces rather than something the
/// digests depend on.
fn priority_flood(ground: &GridField) -> Vec<f32> {
    /// A frontier cell, ordered so the heap surfaces the *lowest* level
    /// first, ties broken by cell index.
    struct Frontier {
        level: f32,
        cell: usize,
    }
    impl Ord for Frontier {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            other
                .level
                .total_cmp(&self.level)
                .then(other.cell.cmp(&self.cell))
        }
    }
    impl PartialOrd for Frontier {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }
    impl PartialEq for Frontier {
        fn eq(&self, other: &Self) -> bool {
            self.cmp(other) == std::cmp::Ordering::Equal
        }
    }
    impl Eq for Frontier {}

    let (nx, nz) = ground.dims;
    let mut fill = vec![f32::INFINITY; nx * nz];
    let mut frontier = std::collections::BinaryHeap::new();
    for (cell, g) in ground.cells.iter().enumerate() {
        if *g <= 0.0 {
            fill[cell] = *g;
            frontier.push(Frontier { level: *g, cell });
        }
    }

    while let Some(Frontier { level, cell }) = frontier.pop() {
        // A stale entry: the cell was reached again by a lower route after
        // this one was queued.
        if level > fill[cell] {
            continue;
        }
        let (x, z) = (cell % nx, cell / nx);
        for (dx, dz) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
            let (cx, cz) = (x as i32 + dx, z as i32 + dz);
            if !(0..nx as i32).contains(&cx) || !(0..nz as i32).contains(&cz) {
                continue;
            }
            let next = cz as usize * nx + cx as usize;
            let reach = ground.cells[next].max(level);
            if reach < fill[next] {
                fill[next] = reach;
                frontier.push(Frontier {
                    level: reach,
                    cell: next,
                });
            }
        }
    }
    fill
}

/// A grid of values over the map at [`COAST_GRID`] spacing, read back with
/// bilinear interpolation, clamped to its edges outside it.
///
/// The shaping passes below are methods rather than free functions over a slice
/// and a `(width, height)` pair, threading the two separately only giving them
/// a chance to disagree.
#[derive(Default)]
struct GridField {
    cells: Vec<f32>,
    dims: (usize, usize),
    /// World coordinate of the first cell.
    origin: Vec2,
}

impl GridField {
    fn new(cells: Vec<f32>, dims: (usize, usize), origin: Vec2) -> Self {
        Self {
            cells,
            dims,
            origin,
        }
    }

    fn at(&self, wx: f32, wz: f32) -> f32 {
        let (nx, nz) = self.dims;
        if nx == 0 {
            return 0.0;
        }
        let fx = ((wx - self.origin.x) / COAST_GRID).clamp(0.0, (nx - 1) as f32);
        let fz = ((wz - self.origin.y) / COAST_GRID).clamp(0.0, (nz - 1) as f32);

        let (x0, z0) = (fx.floor() as usize, fz.floor() as usize);
        let (x1, z1) = ((x0 + 1).min(nx - 1), (z0 + 1).min(nz - 1));
        let (tx, tz) = (fx - x0 as f32, fz - z0 as f32);
        let at = |x: usize, z: usize| self.cells[z * nx + x];

        let near = at(x0, z0) + (at(x1, z0) - at(x0, z0)) * tx;
        let far = at(x0, z1) + (at(x1, z1) - at(x0, z1)) * tx;
        near + (far - near) * tz
    }

    /// Two-pass chamfer distance transform over a grid of zeroes (the sea) and
    /// infinities (everything else), leaving each cell holding its distance
    /// from the nearest zero, in cells.
    ///
    /// Weighting a diagonal step by √2 keeps the result close enough to a true
    /// Euclidean distance for something only ever used to shape and fade a
    /// coast, at two linear passes rather than a search.
    fn chamfer(&mut self) {
        const DIAGONAL: f32 = std::f32::consts::SQRT_2;
        let (nx, nz) = self.dims;
        let cells = &mut self.cells;

        let mut pass = |x: usize, z: usize, neighbours: [(isize, isize, f32); 4]| {
            let mut best = cells[z * nx + x];
            for (dx, dz, cost) in neighbours {
                let (cx, cz) = (x as isize + dx, z as isize + dz);
                if (0..nx as isize).contains(&cx) && (0..nz as isize).contains(&cz) {
                    best = best.min(cells[cz as usize * nx + cx as usize] + cost);
                }
            }
            cells[z * nx + x] = best;
        };

        // Forward, reaching back at the cells already settled this pass...
        for z in 0..nz {
            for x in 0..nx {
                pass(
                    x,
                    z,
                    [
                        (-1, -1, DIAGONAL),
                        (0, -1, 1.0),
                        (1, -1, DIAGONAL),
                        (-1, 0, 1.0),
                    ],
                );
            }
        }
        // ...then backward over the mirror image of the same neighbourhood,
        // which is what lets distance travel in every direction.
        for z in (0..nz).rev() {
            for x in (0..nx).rev() {
                pass(
                    x,
                    z,
                    [
                        (1, 1, DIAGONAL),
                        (0, 1, 1.0),
                        (-1, 1, DIAGONAL),
                        (1, 0, 1.0),
                    ],
                );
            }
        }
    }

    /// Replaces every cell with the largest value within `radius` cells of it
    /// — a square sliding-window maximum, done as two 1D passes with a
    /// monotonic deque, so the whole thing is linear in the grid size.
    fn window_max(&mut self, radius: usize) {
        let (nx, nz) = self.dims;
        let cells = &mut self.cells;
        let window = |line: &mut Vec<f32>, out: &mut Vec<f32>| {
            // Deque of indices whose values are decreasing; the front is
            // always the maximum of the window around `i`.
            let mut deque: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
            out.clear();
            for i in 0..line.len() + radius {
                if i < line.len() {
                    while deque.back().is_some_and(|&b| line[b] <= line[i]) {
                        deque.pop_back();
                    }
                    deque.push_back(i);
                }
                if i >= radius {
                    let centre = i - radius;
                    while deque.front().is_some_and(|&f| f + radius < centre) {
                        deque.pop_front();
                    }
                    out.push(line[deque[0]]);
                }
            }
        };

        let mut line = Vec::with_capacity(nx.max(nz));
        let mut out = Vec::with_capacity(nx.max(nz));
        for row in 0..nz {
            line.clear();
            line.extend_from_slice(&cells[row * nx..(row + 1) * nx]);
            window(&mut line, &mut out);
            cells[row * nx..(row + 1) * nx].copy_from_slice(&out);
        }
        for col in 0..nx {
            line.clear();
            line.extend((0..nz).map(|row| cells[row * nx + col]));
            window(&mut line, &mut out);
            for (row, v) in out.iter().enumerate() {
                cells[row * nx + col] = *v;
            }
        }
    }

    /// One pass of 3×3 box blur, in place. Used to take the creases off the
    /// distance field; run twice it approximates a small tent kernel.
    fn blur(&mut self) {
        let (nx, nz) = self.dims;
        let cells = &mut self.cells;
        let mut pass = |stride: usize, len: usize, lanes: usize, lane_stride: usize| {
            for lane in 0..lanes {
                let base = lane * lane_stride;
                let mut previous = cells[base];
                for i in 0..len {
                    let here = cells[base + i * stride];
                    let next = cells[base + (i + 1).min(len - 1) * stride];
                    cells[base + i * stride] = (previous + here + next) / 3.0;
                    previous = here;
                }
            }
        };
        // Rows, then columns — a box blur is separable.
        pass(1, nx, nz, nx);
        pass(nx, nz, nx, 1);
    }
}

/// Share of the ground within `radius` cells of each cell that is water, from a
/// 0-or-1 mask of it — a separable box blur, done with a running sum so the
/// radius costs nothing.
///
/// Everything off the edge of the grid counts as water: the map ends in open
/// sea on every seed, and counting it that way keeps a coast near the frame
/// reading the same whatever size map is drawn around it.
fn water_fraction(wet: &[f32], dims: (usize, usize), radius: usize) -> Vec<f32> {
    let (nx, nz) = dims;
    let window = (2 * radius + 1) as f32;
    let r = radius as isize;
    let mut line = vec![0.0f32; nx.max(nz)];

    let mut rows = vec![0.0f32; wet.len()];
    for iz in 0..nz {
        let row = &wet[iz * nx..iz * nx + nx];
        let at = |i: isize| {
            if (0..nx as isize).contains(&i) {
                row[i as usize]
            } else {
                1.0
            }
        };
        let mut sum: f32 = (-r..=r).map(at).sum();
        line[0] = sum / window;
        for (ix, value) in line[..nx].iter_mut().enumerate().skip(1) {
            sum += at(ix as isize + r) - at(ix as isize - r - 1);
            *value = sum / window;
        }
        rows[iz * nx..iz * nx + nx].copy_from_slice(&line[..nx]);
    }

    let mut out = vec![0.0f32; wet.len()];
    for ix in 0..nx {
        let at = |i: isize| {
            if (0..nz as isize).contains(&i) {
                rows[i as usize * nx + ix]
            } else {
                1.0
            }
        };
        let mut sum: f32 = (-r..=r).map(at).sum();
        line[0] = sum / window;
        for (iz, value) in line[..nz].iter_mut().enumerate().skip(1) {
            sum += at(iz as isize + r) - at(iz as isize - r - 1);
            *value = sum / window;
        }
        for (iz, value) in line[..nz].iter().enumerate() {
            out[iz * nx + ix] = *value;
        }
    }
    out
}

/// The three kinds of coast, in the order the character field runs through
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shore {
    /// Shelves gently away: broad sand, and a wide turquoise shallows offshore.
    Beach,
    /// Neither one thing nor the other — pebble and boulder, roughly at the
    /// height the untouched landscape would have been.
    Rocky,
    /// Stands up out of the sea, with deep water at its foot and grass on top.
    Cliff,
}

impl Shore {
    fn of(character: f32) -> Self {
        if character < ROCKY_SHORE {
            Self::Beach
        } else if character < CLIFF_SHORE {
            Self::Rocky
        } else {
            Self::Cliff
        }
    }
}

/// Bends the ground either side of the waterline into the kind of coast
/// `character` calls for: `-1.0` shelves away into sand, `+1.0` stands straight
/// up out of the water, and at `0.0` the height field comes back untouched.
/// `distance` is how far the point is from the waterline, in metres.
///
/// A beach and a cliff are not made the same way, not being the same kind of
/// claim about the ground.
///
/// A beach is a claim about how *gently* the land shelves, so it is a remap of
/// height onto height: a power curve across the coastal band, which agrees with
/// the untouched field at both ends of it.
///
/// A cliff is a claim about the *angle of a face*, and no remap of height can
/// make one — where the natural ground gains half a metre in ten there is no
/// height to redistribute into a wall, and squeezing it yields a step one facet
/// wide. So a cliff is built outwards from the water: the ground is lifted to
/// meet a fixed rise per metre of distance from it, up to [`CLIFF_HEIGHT`], and
/// left alone wherever the real landscape is already higher.
///
/// Below the waterline both are power curves, the only question down there
/// being how quickly the bed falls away.
///
/// `scale` grows the whole band — how tall a cliff stands, how much a beach
/// may cut — on seeds whose terrain is steeper than the constants were tuned
/// for. It scales heights and never angles: a cliff's pitch and a beach's
/// flatness are what make them what they are, at any size.
fn shape_coast(height: f32, distance: f32, character: f32, scale: f32) -> f32 {
    // The sea bed. Either the bright shelf carried a long way out, or dark
    // water right at the foot of the rock.
    if height < 0.0 {
        let exponent = if character >= 0.0 {
            1.0 + (CLIFF_PLUNGE - 1.0) * cliffiness(character)
        } else {
            1.0 + (BEACH_SHELF - 1.0) * beachiness(character)
        };
        let t = (-height / MAX_DEPTH).min(1.0);
        return -MAX_DEPTH * pow(t, exponent);
    }

    let band = CLIFF_HEIGHT * scale;

    // Above the band the coast has no business touching the landscape — these
    // are hillsides that happen to be near the sea, not shore.
    if height >= band {
        return height;
    }

    // A beach is the same idea as a cliff with the sign turned round: instead
    // of lifting the ground to a steep face, hold it down to a shallow one.
    // Built from distance for the same reason — a remap of height onto height
    // gives a beach whose width depends on how steep the hill behind it is, so
    // the taller the island the thinner its beaches.
    if character < 0.0 {
        // Steepening with distance rather than a straight ramp: a straight one
        // holds the ground flat out to the edge of the coast's reach and lets
        // go all at once, leaving a scarp along the back of every beach.
        //
        // And released well before that full reach. The apron *cuts the ground
        // down*, so wherever it stops cutting it leaves what it did not cut
        // standing — carried too far inland it shaves the land either side of
        // the ridge midway between two coasts and leaves the ridge as a wall.
        let apron = BEACH_RISE * distance * (1.0 + distance / APRON_KNEE);
        let within = 1.0 - smoothstep(BEACH_REACH * 0.45, BEACH_REACH, distance);

        // How much the apron is allowed to take off, tapering to nothing as the
        // ground approaches the height the test above stops touching. Without
        // it the two meet as a step — land a centimetre under the cliff height
        // cut to the apron, land a centimetre over left alone — which lays a
        // six-metre wall along the eleven-metre contour.
        let allowance = BEACH_CUT * scale * (1.0 - smoothstep(band * 0.45, band, height));
        let apron = apron.max(height - allowance);

        return height + (height.min(apron) - height) * beachiness(character) * within;
    }

    // A lesser cliff is a shorter one, never a gentler one — the angle of the
    // face is the whole of what makes it a cliff, and one laid back far enough
    // to come out green is not one at all. So the strength caps the height of
    // the face and leaves its pitch alone.
    //
    // Lifting the ground to meet the face rather than replacing it with the
    // face, so that where a cliff runs into rising land it simply stops being
    // a cliff instead of cutting a notch out of the hillside.
    //
    // Behind the top of the face the profile shelves back down at
    // [`CLIFF_BACK_PITCH`], so the clifftop is a shoulder that returns to the
    // natural ground rather than a terrace held at full height across
    // everything low behind it.
    let top = band * cliffiness(character);
    let face = (distance * CLIFF_RISE).min(top)
        - CLIFF_BACK_PITCH * (distance - top / CLIFF_RISE).max(0.0);
    height.max(face)
}

/// How much of the coastal reshaping applies, given how far the ground is from
/// the water: all of it at the waterline, none of it well inland.
fn coastal_weight(distance: f32) -> f32 {
    1.0 - smoothstep(SHORE_REACH * 0.4, SHORE_REACH, distance)
}

/// How committed a stretch of coast is to being a cliff, and to being a beach:
/// 0 where the character field is undecided, rising to 1 a little past the
/// point where the palette commits to one or the other.
///
/// Ramping them against the same thresholds the colour uses is what keeps the
/// two agreeing. Blending on the raw character instead leaves a stretch painted
/// as cliff with a quarter of a cliff's face under it, which reads as a green
/// slope with a grey line along the top.
fn cliffiness(character: f32) -> f32 {
    smoothstep(0.0, CLIFF_SHORE * 1.8, character)
}

fn beachiness(character: f32) -> f32 {
    smoothstep(0.0, -ROCKY_SHORE * 1.8, -character)
}

/// Heightfield normal at a world point, from central differences one tile out.
///
/// Takes the field rather than four sampled heights because every owner of a
/// height field wants exactly this, and the spacing is part of the answer.
pub(crate) fn normal_at(wx: f32, wz: f32, height: impl Fn(f32, f32) -> f32) -> Vec3 {
    Vec3::new(
        height(wx - TILE_SIZE, wz) - height(wx + TILE_SIZE, wz),
        2.0 * TILE_SIZE,
        height(wx, wz - TILE_SIZE) - height(wx, wz + TILE_SIZE),
    )
    .normalize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{digest, floats, ints};

    fn generator(chunks_x: u32, chunks_z: u32, seed: u32) -> (MapConfig, TerrainGenerator) {
        let config = MapConfig {
            chunks: UVec2::new(chunks_x, chunks_z),
            seed,
        };
        let generator = TerrainGenerator::new(&config);
        (config, generator)
    }

    #[test]
    fn every_seed_pays_for_its_summit_in_ground() {
        // The massif fit puts every seed's raw summit on the same number, but
        // reaching it is not a seed's to decide: the ceiling holds every point
        // under what its distance from the open sea has earned.
        //
        // So there is no constant across seeds — not the summit, and not the
        // grade either. What holds is weaker and worth more: a summit stands
        // within reach of what its own ground has earned, both ways. The old
        // fixed-height test passed happily on a seed with a hundred and twenty
        // metres of rock fifty metres from the sea.
        for seed in [20_040_112u32, 1, 7, 99, 12_345, 808, 2_024, 31_337] {
            let (config, gen) = generator(8, 8, seed);
            let half = config.half_extent();

            // On the same grid the fit itself used, so this is testing the
            // solve rather than how the summit falls between samples.
            let (mut peak, mut room) = (0.0f32, 0.0f32);
            for iz in (0..config.tiles().y).step_by(COAST_GRID as usize) {
                for ix in (0..config.tiles().x).step_by(COAST_GRID as usize) {
                    let (wx, wz) = (ix as f32 - half.x, iz as f32 - half.y);
                    let h = gen.landform(wx, wz);
                    if h > peak {
                        (peak, room) = (h, gen.inland.metres(wx, wz));
                    }
                }
            }

            // Most of what that ground has earned — the ceiling being what
            // decides the summit, rather than a bound so far above the
            // landscape that it never comes into it.
            //
            // Only the lower half of that is worth asserting, and it is worth
            // saying why the other half is missing. `peak <= earned` is not a
            // property of these maps but arithmetic: `earned` here is the very
            // ceiling [`under_ceiling`] applied at this point, and a smooth
            // minimum is strictly under its ceiling for any height above the
            // waterline. An assertion that cannot fail tests nothing.
            //
            // What *can* fail is the summit falling away from its ceiling.
            // That is the coupling worth holding: [`INLAND_REACH`] fixes
            // [`PEAK_GRADE`], and if the reach were lengthened much further
            // than the massif can build against, every map would sit well
            // below a ceiling that had stopped meaning anything. Measured, the
            // eight seeds run from 0.57 to 0.96 of their ceiling.
            let earned = CLIFF_HEIGHT + PEAK_GRADE * room;
            assert!(
                peak > earned * 0.4,
                "seed {seed} peaked at {peak:.0} m with {room:.0} m of ground behind it, \
                 well under the {earned:.0} m that has earned — is the ceiling still \
                 what decides a summit?"
            );
            assert!(
                peak > MOUNTAIN_HEIGHT,
                "seed {seed} peaked at {peak:.0} m, which is not a mountain"
            );
        }
    }

    #[test]
    fn no_seed_is_a_drowned_sandflat() {
        // A minority of seeds put nearly all their land within a metre or two
        // of sea level, and the whole map painted as shore. The calibration now
        // guarantees the middle of the land a minimum height; measured on the
        // landform rather than the finished height, because the coast is
        // entitled to cut the shore itself down and the detail noise averages
        // out to nothing.
        for seed in [20_040_112u32, 1, 7, 99, 12_345, 808, 2_024, 31_337] {
            let (config, gen) = generator(8, 8, seed);
            let half = config.half_extent();
            let tiles = config.tiles();

            let mut land: Vec<f32> = (0..tiles.y)
                .step_by(COAST_GRID as usize)
                .flat_map(|iz| {
                    (0..tiles.x)
                        .step_by(COAST_GRID as usize)
                        .map(move |ix| (ix, iz))
                })
                .map(|(ix, iz)| gen.landform(ix as f32 - half.x, iz as f32 - half.y))
                .filter(|h| *h > 0.0)
                .collect();
            land.sort_by(|a, b| a.partial_cmp(b).expect("no NaN in a height field"));

            let median = land[land.len() / 2];
            assert!(
                median > LOWLAND_FLOOR * 0.9,
                "seed {seed}'s land median is {median:.1} m — a drowned sandflat"
            );
        }
    }

    #[test]
    fn a_big_map_gets_several_real_mountains() {
        // The massif fit used to anchor everything on the map's single highest
        // point, so a large map came out as many grey lumps under one white
        // cap. The local ceiling — [`MASSIF_EQUALITY`] — is what entitles every
        // range to a summit of its own.
        //
        // Read against [`MOUNTAIN_HEIGHT`] rather than a share of
        // [`HEIGHT_SCALE`]: [`TerrainGenerator::inland`] rations height by how
        // much room a range has behind it, so a pecking order is a difference
        // the map is *meant* to show. What is worth guarding is that several
        // ranges are real mountains, not that they are all nearly as tall.
        //
        // Counted over the whole seed list rather than seed by seed, which
        // buys back the strength given up by reading against the lower line:
        // the eight seeds get 3, 3, 4, 4, 3, 7, 3, 5 summits, so a per-seed
        // floor of three would fail on any tuning that cost one seed one
        // summit without the maps having got worse.
        let seeds = [20_040_112u32, 1, 7, 99, 12_345, 808, 2_024, 31_337];
        let mut total = 0;
        for seed in seeds {
            let config = MapConfig::square(2048, seed);
            let gen = TerrainGenerator::new(&config);
            let half = config.half_extent();

            // Distinct summits: high ground on a coarse grid, greedily
            // clustered so that one massif counts once. Measured on the
            // landform, so the detail layers cannot invent one.
            let mut peaks: Vec<(f32, f32, f32)> = Vec::new();
            for iz in (0..config.tiles().y as i32).step_by(8) {
                for ix in (0..config.tiles().x as i32).step_by(8) {
                    let (wx, wz) = (ix as f32 - half.x, iz as f32 - half.y);
                    let h = gen.landform(wx, wz);
                    if h < MOUNTAIN_HEIGHT {
                        continue;
                    }
                    match peaks
                        .iter_mut()
                        .find(|(px, pz, _)| Vec2::new(px - wx, pz - wz).length() < 400.0)
                    {
                        Some(peak) => {
                            if h > peak.2 {
                                *peak = (wx, wz, h);
                            }
                        }
                        None => peaks.push((wx, wz, h)),
                    }
                }
            }

            // Two is the floor for a map this size on its own: one summit is
            // the failure this test was written for.
            assert!(
                peaks.len() >= 2,
                "seed {seed} got only {} summit clear of {MOUNTAIN_HEIGHT} m",
                peaks.len()
            );
            total += peaks.len();
        }

        assert!(
            total >= 24,
            "{} summits clear of {MOUNTAIN_HEIGHT} m over {} seeds — a big map is \
             supposed to get several each",
            total,
            seeds.len()
        );
    }

    #[test]
    fn heights_are_finite_and_bounded() {
        // On a rectangular map, so the whole pipeline is exercised off the
        // square path it grew up on.
        let (config, gen) = generator(2, 1, 7);
        let half = config.half_extent();

        for iz in 0..=config.tiles().y {
            for ix in 0..=config.tiles().x {
                let wx = ix as f32 - half.x;
                let wz = iz as f32 - half.y;
                let h = gen.height(wx, wz);
                assert!(h.is_finite(), "height at ({wx}, {wz}) was {h}");
                assert!(h >= -MAX_DEPTH, "height {h} below the depth floor");
                assert!(h <= HEIGHT_SCALE * 2.0, "height {h} implausibly high");
            }
        }
    }

    #[test]
    fn normals_are_unit_length_and_point_upwards() {
        let (config, gen) = generator(2, 2, 7);
        let half = config.half_extent();

        for iz in 0..=256 {
            for ix in 0..=256 {
                let n = gen.normal(ix as f32 - half.x, iz as f32 - half.y);
                assert!(
                    (n.length() - 1.0).abs() < 1e-3,
                    "normal {n:?} not normalised"
                );
                // A heightfield can never overhang, so every normal has a
                // positive Y component.
                assert!(n.y > 0.0, "normal {n:?} points downwards");
            }
        }
    }

    /// Walks the full perimeter of the map, metre by metre, asserting open
    /// water the whole way round.
    fn assert_edges_are_sea(config: &MapConfig, gen: &TerrainGenerator) {
        let half = config.half_extent();
        for i in 0..=config.tiles().x {
            let t = i as f32 - half.x;
            for wz in [-half.y, half.y] {
                assert!(
                    gen.height(t, wz) < 0.0,
                    "map edge at ({t}, {wz}) is above sea level"
                );
            }
        }
        for i in 0..=config.tiles().y {
            let t = i as f32 - half.y;
            for wx in [-half.x, half.x] {
                assert!(
                    gen.height(wx, t) < 0.0,
                    "map edge at ({wx}, {t}) is above sea level"
                );
            }
        }
    }

    #[test]
    fn map_edges_are_under_water() {
        // On a rectangle, where the falloff has a different half extent on
        // each axis to get wrong.
        let (config, gen) = generator(4, 2, 12345);
        assert_edges_are_sea(&config, &gen);
    }

    #[test]
    fn a_single_chunk_map_is_an_islet_with_a_sea_margin() {
        // The smallest map there is — one chunk, 128 m — is far below the
        // wavelength of every field that shapes an island, and it should still
        // come out as an island in miniature: some land in the middle, open
        // water along every edge.
        for seed in [1u32, 7, 99, 12_345] {
            let (config, gen) = generator(1, 1, seed);
            let half = config.half_extent();

            let mut land = 0;
            for iz in 0..=128 {
                for ix in 0..=128 {
                    if gen.height(ix as f32 - half.x, iz as f32 - half.y) > 0.0 {
                        land += 1;
                    }
                }
            }
            let share = land as f32 / (129.0 * 129.0);
            assert!(
                share > 0.05,
                "seed {seed}'s single-chunk map is {:.0}% land — all sea",
                share * 100.0
            );

            assert_edges_are_sea(&config, &gen);
        }
    }

    #[test]
    fn same_seed_gives_same_map() {
        let (_, a) = generator(2, 2, 99);
        let (_, b) = generator(2, 2, 99);
        let (_, c) = generator(2, 2, 100);

        assert_eq!(a.height(3.0, -7.0), b.height(3.0, -7.0));
        assert_ne!(a.height(3.0, -7.0), c.height(3.0, -7.0));
    }

    #[test]
    fn a_seed_is_the_same_map_down_to_the_bit() {
        // `same_seed_gives_same_map` says two generators in one process agree.
        // This pins the map itself: golden digests of the height field and the
        // palette entry every point is painted, recorded once and held to ever
        // after. It is what makes "a seed is a map" a tested property rather
        // than a habit.
        //
        // What is being bet is that a *world* survives being re-hosted: a seed
        // handed to a machine with a different libm has to raise the same
        // islands, or a saved position and a server moved between hosts
        // quietly mean somewhere else.
        //
        // When this fails because the generator was *meant* to change,
        // re-record the digests — run with `--nocapture` and they are printed.
        // When it fails anywhere else, that is the bet being lost, and the
        // first place to look is the powers, the only maths here the hardware
        // does not pin down. They go through [`pow`] rather than `f32::powf`,
        // which is what lets this pass on more than the machine that recorded
        // it.
        let cases = [
            (20_040_112u32, UVec2::new(4, 4), 0x14DC_93A4_566B_C197u64),
            (99, UVec2::new(3, 2), 0xC171_0209_1C7A_C7FBu64),
        ];

        for (seed, chunks, expected) in cases {
            let config = MapConfig { chunks, seed };
            let gen = TerrainGenerator::new(&config);
            let half = config.half_extent();

            // Every reader of the map, over the whole of it: the height field
            // and its normals on a 4 m grid, the palette entry each of those
            // points is painted — the surface as a byte, since that is now the
            // form it leaves this machine in — and the level of any standing
            // water over it, quantised the way the wire quantises it.
            //
            // The water is pinned on its own rather than left to the colours
            // it produces: a lake moving a few centimetres repaints almost
            // nothing, but it is a different surface to float a boat on.
            let mut values = Vec::new();
            let mut painted = Vec::new();
            for iz in (0..=config.tiles().y).step_by(4) {
                for ix in (0..=config.tiles().x).step_by(4) {
                    let (wx, wz) = (ix as f32 - half.x, iz as f32 - half.y);
                    let height = gen.height(wx, wz);
                    let normal = gen.normal(wx, wz);
                    let material = gen.material(wx, wz, height, normal);
                    values.extend([height, normal.x, normal.y, normal.z]);
                    painted.push(material as i64);
                    painted.push(
                        gen.lake_level(wx, wz)
                            .map_or(protocol::ground::NO_WATER, protocol::ground::quantize)
                            as i64,
                    );
                }
            }

            let got = digest(floats(values).chain(ints(painted)));
            println!("seed {seed} digests to {got:#018X}");
            assert_eq!(
                got, expected,
                "seed {seed} no longer digests to its recorded value — see \
                 this test's comment for what that means"
            );
        }
    }

    #[test]
    fn map_size_changes_extent_not_feature_size() {
        // Every wavelength is fixed in metres, so the same world coordinate
        // sits on the same landscape however big the map around it is. That is
        // a claim about the raw field and not about metres: how many metres a
        // raw unit is worth is fitted to each map's own distribution, and a
        // bigger map is a different distribution, so the two agree on the shape
        // of the land without agreeing on its height.
        // The bigger map is rectangular too, so growing one axis alone also
        // has to leave the landscape where it was.
        let (_, small) = generator(8, 8, 42);
        let (_, big) = generator(16, 8, 42);

        // Along a transect through the middle of both — far enough inside the
        // smaller one to be clear of its falloff — the same hills should turn
        // up in the same places. Compared by correlation rather than by value,
        // because how tall those hills come out is exactly what the per-map
        // fitting is entitled to disagree about.
        let transect = |gen: &TerrainGenerator| {
            (0..120)
                .map(|i| {
                    let t = i as f32 * 3.0 - 180.0;
                    gen.landform_raw(t, t * 0.4)
                })
                .collect::<Vec<f32>>()
        };
        let (a, b) = (transect(&small), transect(&big));

        let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
        let (ma, mb) = (mean(&a), mean(&b));
        let dot: f32 = a.iter().zip(&b).map(|(x, y)| (x - ma) * (y - mb)).sum();
        let norm = |v: &[f32], m: f32| v.iter().map(|x| (x - m) * (x - m)).sum::<f32>().sqrt();
        let correlation = dot / (norm(&a, ma) * norm(&b, mb));

        assert!(
            correlation > 0.97,
            "the two maps only correlate at {correlation}, so the features are \
             not the same size on both"
        );
    }

    /// Walks the waterline of a map and reports, for each metre of it, what
    /// kind of coast it is, how steep the shore itself is, and how steep the
    /// steepest ground within a facet or two inland is.
    ///
    /// The two slopes answer different questions and a single one answers
    /// neither. A cliff is a claim about ground a little inland of the water,
    /// so it needs the second; a beach is a claim about the shore underfoot,
    /// and judging it by the second calls every beach at the foot of a hill a
    /// cliff — which, on an island with mountains on it, is most of them.
    fn waterline(size: u32, seed: u32) -> Vec<(Shore, f32, f32)> {
        let config = MapConfig::square(size, seed);
        let gen = TerrainGenerator::new(&config);
        let half = config.half_extent();
        let at = |ix: i32, iz: i32| gen.height(ix as f32 - half.x, iz as f32 - half.y);

        let mut found = Vec::new();
        for iz in (0..size as i32).step_by(2) {
            for ix in (0..size as i32).step_by(2) {
                if at(ix, iz) <= 0.0 {
                    continue;
                }
                // Land with sea two metres away in one of the four directions.
                let shore = [(-2, 0), (2, 0), (0, -2), (0, 2)]
                    .iter()
                    .any(|(dx, dz)| at(ix + dx, iz + dz) <= 0.0);
                if !shore {
                    continue;
                }

                let (wx, wz) = (ix as f32 - half.x, iz as f32 - half.y);
                let steepest = (0..8)
                    .map(|i| {
                        let a = i as f32 * std::f32::consts::TAU / 8.0;
                        1.0 - gen.normal(wx + a.cos() * 4.0, wz + a.sin() * 4.0).y
                    })
                    .fold(0.0f32, f32::max);
                found.push((gen.shore(wx, wz), 1.0 - gen.normal(wx, wz).y, steepest));
            }
        }
        found
    }

    #[test]
    fn every_map_gets_beaches_and_rocky_shores_and_cliffs() {
        // The whole point of the coast field: no seed should come out all one
        // thing. A sixth of the waterline each is well short of the even split
        // the thresholds aim for, and still far more than a map that had gone
        // one way would leave for the other two.
        for seed in [20_040_112, 1, 7, 99, 12_345, 808, 2_024] {
            let coast = waterline(1024, seed);
            for kind in [Shore::Beach, Shore::Rocky, Shore::Cliff] {
                let share =
                    coast.iter().filter(|(k, ..)| *k == kind).count() as f32 / coast.len() as f32;
                assert!(
                    share > 0.15,
                    "seed {seed} gave only {:.0}% {kind:?} coastline",
                    share * 100.0
                );
            }
        }
    }

    #[test]
    fn the_landform_and_the_palette_agree_about_each_coast() {
        // Colour is chosen by slope and shore kind, and the landform is bent by
        // the same field, so the two have to say the same thing: sand only ever
        // gets painted onto ground flat enough to be a beach, and a stretch
        // painted as cliff has a cliff's face under it.
        let coast = waterline(1024, 20_040_112);

        // `inland` looks a facet or two back from the water, `underfoot` at the
        // shore itself.
        let steep = |kind: Shore, inland: bool| {
            let of_kind: Vec<f32> = coast
                .iter()
                .filter(|(k, ..)| *k == kind)
                .map(|(_, underfoot, nearby)| if inland { *nearby } else { *underfoot })
                .collect();
            of_kind.iter().filter(|s| **s > ROCK_SLOPE).count() as f32 / of_kind.len() as f32
        };

        assert!(
            steep(Shore::Cliff, true) > 0.8,
            "cliffs are not steep enough"
        );
        assert!(
            steep(Shore::Beach, false) < 0.02,
            "beaches are not flat enough"
        );
        // Rocky shores are the middle ground, and have to stay there — all
        // three kinds collapse into two the moment this matches either.
        let rocky = steep(Shore::Rocky, true);
        assert!(
            (0.05..0.7).contains(&rocky),
            "rocky shores came out {:.0}% steep, which is a beach or a cliff",
            rocky * 100.0
        );
    }

    #[test]
    fn crags_never_reach_the_shore_band() {
        // The crag pass promises the drawn shore stays the landform's: however
        // hard it cuts, ground that entered above the shore band leaves above
        // it. That holds by arithmetic — the cleft spends [`CRAG_HEADROOM`] of
        // the headroom, the strata pull [`CRAG_STRATA`] of the way to a tread
        // — but the margin it leaves is centimetres, and nothing else goes red
        // when a retune of any of the four constants eats it. So this sweeps
        // the worst case the fade allows: the weight at each height is the
        // most the caller can ever hand crags(), and the positions sweep the
        // ridge and warp fields through their range.
        let (_, gen) = generator(4, 4, 20_040_112);

        let mut clearance = f32::INFINITY;
        for iz in 0..32 {
            for ix in 0..32 {
                let (wx, wz) = (ix as f32 * 37.3 - 600.0, iz as f32 * 41.7 - 650.0);
                let mut h = SHORE_TOP + 0.01;
                while h < 60.0 {
                    let weight = smoothstep(SHORE_TOP, CRAG_FULL, h);
                    clearance = clearance.min(gen.crags(wx, wz, h, weight) - SHORE_TOP);
                    h += 0.19;
                }
            }
        }

        println!("worst crag clearance over the shore band: {clearance:.3} m");
        assert!(
            clearance > 0.0,
            "a crag cut reached the shore band, by {:.3} m",
            -clearance
        );
    }

    #[test]
    fn the_coast_leaves_the_interior_alone() {
        // The reshaping is gated on distance to water rather than on height,
        // because most of this island is low: without that a cliff coast lifts
        // and terraces plains half a kilometre inland.
        let config = MapConfig::square(1024, 20_040_112);
        let gen = TerrainGenerator::new(&config);
        let half = config.half_extent();

        let (mut land, mut inland) = (0, 0);
        for iz in (0..1024).step_by(4) {
            for ix in (0..1024).step_by(4) {
                let (wx, wz) = (ix as f32 - half.x, iz as f32 - half.y);
                if gen.height(wx, wz) <= 0.0 {
                    continue;
                }
                land += 1;

                let distance = gen.coast.metres(wx, wz);
                if distance <= SHORE_REACH {
                    continue;
                }
                inland += 1;
                assert_eq!(
                    coastal_weight(distance),
                    0.0,
                    "the coast still had a hold {distance} m inland at ({wx}, {wz})"
                );
            }
        }

        // A good share of a 1 km island has to be out of the coast's reach —
        // otherwise the gate is doing nothing and the loop above proves
        // nothing. It is only a modest share because raggedness is the point
        // of this generator: with a third of the map as land, sounds and
        // inland water put most land within a hundred metres of a shore.
        assert!(
            inland * 6 > land,
            "only {inland} of {land} land samples were beyond the coast's reach"
        );
    }

    #[test]
    fn open_water_adds_no_step_of_its_own_as_the_map_grows() {
        // The map size is a slider, so every field behind it has to move
        // smoothly as it sweeps. This one decides the ceiling, so a step in it
        // is a step in how tall a whole region of the map may be.
        //
        // Measured against the all-water field rather than a number of metres,
        // because some movement is honest: sea level is refitted per map, so
        // growing one does redraw its coast a little and both fields inherit
        // that. What must not happen is this field adding a step the other
        // does not have — which telling sea from pond by whether the water
        // reaches the frame used to do, moving one seed's deepest point from
        // 131 m to 288 m across a single 128-metre notch against 126 m to
        // 179 m for the all-water field beside it.
        for seed in [20_040_112u32, 808] {
            let (mut open_step, mut all_step) = (0.0f32, 0.0f32);
            let (mut open_last, mut all_last): (Option<f32>, Option<f32>) = (None, None);

            for chunks in 6..=12u32 {
                let config = MapConfig {
                    chunks: UVec2::splat(chunks),
                    seed,
                };
                let gen = TerrainGenerator::new(&config);
                let deepest = |field: &CoastDistance| {
                    field.field.cells.iter().copied().fold(0.0f32, f32::max) * COAST_GRID
                };
                let (open, all) = (deepest(&gen.inland), deepest(&gen.coast));

                if let (Some(o), Some(a)) = (open_last, all_last) {
                    open_step = open_step.max((open - o).abs());
                    all_step = all_step.max((all - a).abs());
                }
                (open_last, all_last) = (Some(open), Some(all));
            }

            // Half as much again, which is slack for the two fields not
            // tracking each other exactly rather than room for a real step:
            // measured, this one is the smoother of the two about as often as
            // not.
            assert!(
                open_step <= all_step * 1.5,
                "seed {seed}'s open-water distance jumped {open_step:.0} m over one size step, \
                 against {all_step:.0} m for the all-water field it should be as smooth as"
            );
        }
    }

    #[test]
    fn distance_to_water_is_zero_at_sea_and_grows_inland() {
        // At the default size — a small map's land share is deliberately
        // shrunk, and with it how far inland anywhere can be.
        let config = MapConfig::square(1024, 42);
        let gen = TerrainGenerator::new(&config);
        let half = config.half_extent();

        // The map edge is open sea on every map.
        assert_eq!(gen.coast.metres(-half.x, 0.0), 0.0);
        assert_eq!(gen.coast.metres(0.0, half.y), 0.0);

        let mut deepest_inland = 0.0f32;
        for iz in (0..1024).step_by(8) {
            let wz = iz as f32 - half.y;
            for ix in (0..1024).step_by(8) {
                let wx = ix as f32 - half.x;
                deepest_inland = deepest_inland.max(gen.coast.metres(wx, wz));

                // Never climbs faster than a metre per metre — a distance
                // field cannot grow quicker than you can walk.
                let step = (gen.coast.metres(wx + 4.0, wz) - gen.coast.metres(wx, wz)).abs();
                assert!(step <= 4.0 + 1e-3, "distance jumped {step} m over 4 m");
            }
        }

        // And somewhere on the map is properly inland, or the coast would apply
        // everywhere and none of the fading would ever be exercised.
        assert!(
            deepest_inland > SHORE_REACH,
            "nowhere on the map is more than {deepest_inland} m from water"
        );
    }

    #[test]
    fn a_cliff_is_a_face_and_not_a_step() {
        // A cliff has to be several facets of visibly steep ground, not one
        // near-vertical facet: from a camera pitched down at 50° a sheer face
        // is edge-on and reads as a dark line rather than as a cliff.
        // At the top of its face — behind that the clifftop is allowed to
        // shelve back down towards the natural ground.
        let full = shape_coast(0.0, CLIFF_HEIGHT / CLIFF_RISE, 1.0, 1.0);
        assert!(
            (full - CLIFF_HEIGHT).abs() < 1e-4,
            "a full cliff reached {full} m, not its height"
        );

        let mut climbed = 0;
        for step in 0..12 {
            let distance = step as f32 * CELL_METRES;
            let rise = shape_coast(0.0, distance + CELL_METRES, 1.0, 1.0)
                - shape_coast(0.0, distance, 1.0, 1.0);
            if rise > 0.1 {
                climbed += 1;
            }
        }
        assert!(
            climbed >= 3,
            "the face climbs over only {climbed} facets, which is a step"
        );
    }

    #[test]
    fn the_coast_never_moves_hillsides_or_the_open_sea() {
        // Two things the reshaping has no business touching: ground already
        // taller than the coastal band, and water already at its floor.
        for character in [-1.0, -0.4, 0.0, 0.4, 1.0] {
            for height in [CLIFF_HEIGHT, 20.0, 55.0] {
                assert_eq!(shape_coast(height, 0.0, character, 1.0), height);
            }
            assert!((shape_coast(-MAX_DEPTH, 0.0, character, 1.0) + MAX_DEPTH).abs() < 1e-4);
            // And with no character at all it is the identity everywhere.
            if character == 0.0 {
                for height in [-6.0, -1.0, 0.0, 2.0, 9.0] {
                    assert!((shape_coast(height, 30.0, 0.0, 1.0) - height).abs() < 1e-4);
                }
            }
        }
    }

    #[test]
    fn size_spec_parses_to_whole_chunks() {
        // Metres in, chunk counts out — each axis rounded to the nearest whole
        // chunk of at least one, and a lone number meaning a square.
        assert_eq!(MapConfig::parse_size("1024"), Some(UVec2::splat(8)));
        assert_eq!(MapConfig::parse_size("1536x1024"), Some(UVec2::new(12, 8)));
        assert_eq!(MapConfig::parse_size("300"), Some(UVec2::splat(2)));
        assert_eq!(MapConfig::parse_size("10"), Some(UVec2::splat(1)));
        assert_eq!(MapConfig::parse_size("islands"), None);
    }

    #[test]
    fn the_facet_grid_is_the_chunk_at_the_drawing_step() {
        // Sampled from the base outwards, row-major, both edges included —
        // which is the order a payload's heights are in and therefore the
        // order a client rebuilds the ground from.
        let (_, gen) = generator(2, 2, 1);
        let base = Vec2::new(-64.0, 32.0);
        let heights = corner_heights(base, |wx, wz| gen.height(wx, wz));

        assert_eq!(heights.len(), CORNERS * CORNERS);
        assert_eq!(heights[0], gen.height(base.x, base.y), "the near corner");
        assert_eq!(
            heights[1],
            gen.height(base.x + CELL_METRES, base.y),
            "the second sample is one facet along x, not along z"
        );
        assert_eq!(
            heights[CORNERS],
            gen.height(base.x, base.y + CELL_METRES),
            "the second row is one facet along z"
        );
        assert_eq!(
            heights[CORNERS * CORNERS - 1],
            gen.height(base.x + CHUNK_METRES, base.y + CHUNK_METRES),
            "the far corner is the chunk's far corner, not one facet short of it"
        );
    }

    #[test]
    fn neighbouring_chunks_agree_along_their_shared_edge() {
        // Two chunks side by side sample the same world points along the plane
        // between them — the far column of one and the near column of the
        // other — so the ground has no seam to show wherever a client puts the
        // two meshes next to each other.
        let (_, gen) = generator(2, 1, 5);
        let left = corner_heights(Vec2::ZERO, |wx, wz| gen.height(wx, wz));
        let right = corner_heights(Vec2::new(CHUNK_METRES, 0.0), |wx, wz| gen.height(wx, wz));

        let column = |grid: &[f32], ix: usize| -> Vec<f32> {
            (0..CORNERS).map(|iz| grid[iz * CORNERS + ix]).collect()
        };
        assert_eq!(
            column(&left, CORNERS - 1),
            column(&right, 0),
            "chunks disagree along their seam"
        );
    }

    #[test]
    fn a_cell_is_classified_at_its_own_centre() {
        // A classifier that answers with where it was asked, so the grid's
        // orientation — row-major, x along a row, the centre half a cell in
        // from the lower corner — is pinned with no terrain in the way.
        // Asked asymmetrically, because a transposed stride would pass a
        // square ask; and pinned here at all because the only other thing
        // holding it is the sent digest, whose remedy when red is to be
        // re-recorded.
        let base = Vec2::new(256.0, -384.0);
        let flat = vec![0.0f32; CORNERS * CORNERS];
        let materials = cell_materials(base, &flat, |wx, wz, _, _| {
            if wx == base.x + 3.5 && wz == base.y + 0.5 {
                Material::Sand
            } else if wx == base.x + 0.5 && wz == base.y + 5.5 {
                Material::Forest
            } else {
                Material::Grass
            }
        });

        let at = |ix, iz| {
            materials[protocol::ground::material_index(ix, iz).expect("a cell on the grid")]
        };
        assert_eq!(materials.len(), CELL_COUNT);
        assert_eq!(at(3, 0), Material::Sand, "three cells along x");
        assert_eq!(at(0, 5), Material::Forest, "five rows along z");
        assert_eq!(at(3, 5), Material::Grass, "the diagonal is nobody's");
    }

    #[test]
    fn every_parcel_of_every_band_gets_used() {
        // The thresholds are set off the noise field's measured distribution
        // rather than its nominal range, which is the kind of number that
        // rots silently: a field whose spread moves leaves the outer buckets
        // unreachable, and the map simply comes out with fewer colours in it
        // than the palette has. That has happened before — a mask cut at 0.55
        // on a field whose 99th percentile was 0.38 — so the palette rows are
        // held to actually being used.
        //
        // On a map big enough to carry all three bands. A small island is all
        // lowland and honestly has no moor row to use, so asking it for heath
        // would be holding the generator to something untrue.
        let (config, gen) = generator(12, 12, 77);
        let half = config.half_extent();

        let mut seen = std::collections::HashSet::new();
        for iz in (0..config.tiles().y).step_by(2) {
            for ix in (0..config.tiles().x).step_by(2) {
                let (wx, wz) = (ix as f32 - half.x, iz as f32 - half.y);
                let height = gen.height(wx, wz);
                seen.insert(gen.material(wx, wz, height, gen.normal(wx, wz)) as u8);
            }
        }

        for (band, parcels) in [
            ("lowland", LOWLAND_PARCELS),
            ("moor", MOOR_PARCELS),
            ("mountain", MOUNTAIN_PARCELS),
        ] {
            for parcel in parcels {
                assert!(
                    seen.contains(&(parcel as u8)),
                    "no {parcel:?} anywhere on this map, though the {band} row calls for it"
                );
            }
        }
    }

    /// A three-row grid whose rows all read the same, so the flood behaves
    /// one-dimensionally and a test can say what it expects column by column.
    fn strip(columns: &[f32]) -> GridField {
        GridField::new(columns.repeat(3), (columns.len(), 3), Vec2::ZERO)
    }

    #[test]
    fn a_basin_fills_to_its_lowest_saddle_and_open_slopes_shed() {
        // Sea, a 5 m wall, a 1 m floor, a 3 m saddle, sea: the floor's escape
        // is over the saddle, whatever stands on its other side.
        let bowl = strip(&[-8.0, 5.0, 1.0, 3.0, -8.0]);
        let fill = priority_flood(&bowl);
        assert_eq!(fill[2], 3.0, "the floor fills to the saddle, not the wall");
        assert_eq!(fill[1], 5.0, "ground that drains freely fills to itself");
        assert_eq!(fill[3], 3.0);

        // The lake the fill leaves: a freeboard below the saddle, over the
        // floor alone. The wall and the saddle carry the level too — they are
        // its bank, and a bank has to know the water it stands over — but the
        // water is only where the level is above the ground.
        let lakes = Lakes::from_ground(&bowl);
        let under = |cell: usize| lakes.field.cells[cell] > bowl.cells[cell];
        assert_eq!(lakes.field.cells[2], 3.0 - LAKE_FREEBOARD);
        assert!(under(2), "the floor is under water");
        assert_eq!(lakes.field.cells[1], 3.0 - LAKE_FREEBOARD);
        assert_eq!(lakes.field.cells[3], 3.0 - LAKE_FREEBOARD);
        assert!(
            !under(1) && !under(3),
            "the wall and the saddle keep dry feet"
        );

        // And a slope with nothing enclosing it holds nothing anywhere.
        let slope = strip(&[-8.0, -2.0, 1.0, 3.0, 6.0]);
        let lakes = Lakes::from_ground(&slope);
        assert!(lakes.field.cells.iter().all(|c| *c == f32::NEG_INFINITY));
    }

    #[test]
    fn terraced_basins_each_hold_their_own_level() {
        // Two basins in a staircase: the upper spills over an 8 m saddle into
        // the lower, which spills over a 4 m one into the sea. Each takes its
        // own saddle's level — the walk needs no special case for one lake
        // draining through another.
        let steps = strip(&[-8.0, 4.0, 2.0, 8.0, 6.0, 12.0, -8.0]);
        let lakes = Lakes::from_ground(&steps);
        assert_eq!(lakes.field.cells[2], 4.0 - LAKE_FREEBOARD);
        assert_eq!(lakes.field.cells[4], 8.0 - LAKE_FREEBOARD);
        // The saddle between them keeps its feet dry, so they are two lakes
        // and not one. It is bank to both, and takes the higher — the surface
        // a client drawing across the join would draw.
        assert!(
            lakes.field.cells[3] < steps.cells[3],
            "the saddle stands out"
        );
        assert_eq!(lakes.field.cells[3], 8.0 - LAKE_FREEBOARD);
    }

    #[test]
    fn a_landlocked_lagoon_is_the_seas_and_never_a_lake() {
        // A hollow below sea level, entirely ringed by land. The calibration
        // already flooded it — its water stands at zero with the rest of the
        // world's — so the flood seeds there and raises nothing over it: a
        // lake at the rim's height would redraw a coast that the beaches, the
        // distance fields and the painting were all fitted to.
        let lagoon = strip(&[-8.0, 6.0, -2.0, 6.0, -8.0]);
        let lakes = Lakes::from_ground(&lagoon);
        assert!(lakes.field.cells.iter().all(|c| *c == f32::NEG_INFINITY));
        assert_eq!(lakes.level(8.0, 4.0), None);
    }

    #[test]
    fn a_lake_answers_at_its_shore_and_not_across_the_map() {
        let bowl = strip(&[-8.0, 5.0, 1.0, 3.0, -8.0]);
        let lakes = Lakes::from_ground(&bowl);

        // Over the flooded cell itself, and from the dry cell beside it —
        // the reach that lets a drawn waterline find its own crossing.
        assert_eq!(lakes.level(8.0, 4.0), Some(3.0 - LAKE_FREEBOARD));
        assert_eq!(lakes.level(5.0, 4.0), Some(3.0 - LAKE_FREEBOARD));
        // But not from the far side of the map.
        assert_eq!(lakes.level(0.5, 4.0), None);
    }

    #[test]
    fn lakes_stand_on_land_and_hold_water_over_it() {
        // Every cell of every seed a lake answers for, checked against the
        // landform proper: the level is above the sea, the ground under it is
        // land, and the ground is either under the surface — water — or bank
        // within [`LAKE_RELIEF`] of it. The last two matter because
        // [`TerrainGenerator::find_lakes`] rebuilds its metres from the raw
        // grid as a shortcut — these assertions are what hold that shortcut
        // to the same arithmetic as [`TerrainGenerator::landform`].
        let mut wet = 0usize;
        for seed in [20_040_112u32, 1, 7, 99, 12_345, 808, 2_024, 31_337] {
            let (_, gen) = generator(8, 8, seed);
            let (nx, _) = gen.lakes.field.dims;
            let origin = gen.lakes.field.origin;
            for (i, level) in gen.lakes.field.cells.iter().enumerate() {
                if *level == f32::NEG_INFINITY {
                    continue;
                }
                let wx = origin.x + (i % nx) as f32 * COAST_GRID;
                let wz = origin.y + (i / nx) as f32 * COAST_GRID;
                let ground = gen.landform(wx, wz);
                wet += usize::from(ground < *level);
                assert!(
                    *level > 0.0,
                    "seed {seed} holds a lake at {level} m, below the sea"
                );
                assert!(
                    ground > 0.0,
                    "seed {seed} raised a lake over the sea at ({wx}, {wz})"
                );
                assert!(
                    ground < *level + LAKE_RELIEF,
                    "seed {seed} answers for ground the water cannot reach at ({wx}, {wz})"
                );
            }
        }
        assert!(
            wet > 100,
            "only {wet} flooded cells across eight seeds — the maps have lost their lakes"
        );
    }

    #[test]
    fn a_lake_is_water_from_its_middle_to_its_own_shore() {
        // The whole point of the fade, stated on the drawn ground rather than
        // on the landform the lakes were found in. Both halves were broken
        // before it, because the detail lays metres of texture over a basin
        // that may be shallower than the texture is thick: beds were drawn
        // standing out of their own lakes — a quarter of the flooded area on
        // some seeds, a swamp rather than a lake with islands in it — and
        // banks were dug out below a surface the level field had already
        // stopped answering for, so the water ended on a straight grid-aligned
        // edge with no shore on it.
        //
        // Sampled at the facet grid: finer than this is finer than anything
        // gets drawn. The slack is for the fade being sized off the landform
        // read on the fitting grid — between two of its cells the real field
        // is free to curve past the straight line the weights are blended
        // along, and how far follows from the *finest* detail layer: when
        // that was half [`MICRO_SCALE`] at a hand's width of relief the
        // curve-past was a millimetre or two, and one quantisation step
        // covered it. [`GRAIN_SCALE`] carries triple the curvature per metre
        // (half a metre of relief across four), and measured across these
        // seeds the worst excursion is now just over two centimetres. Three
        // steps covers that without hiding a real leak — a broken fade shows
        // up in decimetres, not centimetres.
        let slack = 3.0 * protocol::ground::HEIGHT_STEP;
        let step = CELL_METRES;
        let mut wet = 0usize;
        for seed in [20_040_112u32, 1, 7, 99, 808, 2_024, 31_337] {
            let (config, gen) = generator(8, 8, seed);
            let half = config.chunks.as_vec2() * CHUNK_METRES / 2.0;
            let mut wz = -half.y;
            while wz < half.y {
                let mut wx = -half.x;
                while wx < half.x {
                    let here = wx;
                    wx += step;
                    // The coastal reshaping answers to the sea and runs after
                    // the fade, so it is free to bend a bank out from under a
                    // lake within its reach — see [`LAKE_RELIEF`].
                    if coastal_weight(gen.coast.metres(here, wz)) > 0.0 {
                        continue;
                    }
                    let Some(level) = gen.lake_level(here, wz) else {
                        continue;
                    };
                    let height = gen.height(here, wz);
                    if gen.landform(here, wz) < level {
                        wet += 1;
                        assert!(
                            height < level + slack,
                            "seed {seed} draws the bed of a lake standing out of it \
                             at ({here}, {wz}): ground {height} m, surface {level} m"
                        );
                    } else {
                        assert!(
                            height > level - slack,
                            "seed {seed} digs a lake's bank out below its surface \
                             at ({here}, {wz}): ground {height} m, surface {level} m"
                        );
                    }
                }
                wz += step;
            }
        }
        assert!(
            wet > 1_000,
            "only {wet} samples under water across seven seeds — nothing was tested"
        );
    }

    #[test]
    fn no_lake_is_painted_in_the_seas_colours() {
        // What tells a lake from an inlet, said as a rule rather than as a
        // look: every scrap of ground under a lake's surface is fresh water's
        // own tones, or the bare rock a steep bank is everywhere. Never sand,
        // never the bright shelf, never the sea bed. Those three are what draw
        // a coast, and a lake wearing them is the whole of the thing this is
        // here to stop coming back.
        //
        // Under the surface and no further, because the bank above it belongs
        // to the margin rather than to this rule — and above *that* the lake
        // has no say at all and the hillside is painted as its height asks,
        // which near the sea may quite properly be sand.
        let fresh = [
            Material::Silt,
            Material::Shoal,
            Material::Marsh,
            Material::RockDark,
        ];
        let mut painted = 0usize;
        for seed in [20_040_112u32, 1, 7, 99, 808] {
            let (config, gen) = generator(8, 8, seed);
            let half = config.chunks.as_vec2() * CHUNK_METRES / 2.0;
            let mut wz = -half.y;
            while wz < half.y {
                let mut wx = -half.x;
                while wx < half.x {
                    let here = wx;
                    wx += CELL_METRES;
                    let Some(level) = gen.lake_level(here, wz) else {
                        continue;
                    };
                    let height = gen.height(here, wz);
                    if height >= level {
                        continue;
                    }
                    let tone = gen.material(here, wz, height, gen.normal(here, wz));
                    painted += 1;
                    assert!(
                        fresh.contains(&tone),
                        "seed {seed} paints a lake bed {tone:?} at ({here}, {wz}), \
                         {:.2} m under its surface",
                        level - height
                    );
                }
                wz += CELL_METRES;
            }
        }
        assert!(
            painted > 1_000,
            "only {painted} samples of lake bed across five seeds — nothing was tested"
        );
    }

    #[test]
    fn a_lakes_margin_is_wider_than_the_facets_it_is_drawn_on() {
        // The margin used to be a height above the water, and what a height
        // buys depends entirely on the bank: on anything with a pitch to it,
        // under one facet. A band narrower than the grid it is drawn on is not
        // a band — the facets that catch it are a broken chain of triangles
        // against the smooth waterline under them, which is the sawtooth every
        // lake used to wear. So the margin is a reach along the ground, and
        // this is that said as a number: walk out of the water and the fringe
        // is several facets deep essentially every time.
        //
        // Counted on the dry side only. The wet half is under the water, where
        // how far it runs is the water's business and not the eye's.
        let mut crossings = 0usize;
        let mut deep = 0usize;
        for seed in [20_040_112u32, 1, 7, 99, 808] {
            let (config, gen) = generator(8, 8, seed);
            let half = config.chunks.as_vec2() * CHUNK_METRES / 2.0;
            let fresh = |wx: f32, wz: f32| {
                let height = gen.height(wx, wz);
                let tone = gen.material(wx, wz, height, gen.normal(wx, wz));
                matches!(tone, Material::Marsh | Material::RockDark)
            };
            let drowned = |wx: f32, wz: f32| {
                gen.lake_level(wx, wz)
                    .is_some_and(|level| gen.height(wx, wz) < level)
            };

            let mut wz = -half.y;
            while wz < half.y {
                let mut wx = -half.x + CELL_METRES;
                while wx < half.x - 4.0 * CELL_METRES {
                    let here = wx;
                    wx += CELL_METRES;
                    // The first dry step out of a lake, walking east.
                    if !drowned(here - CELL_METRES, wz) || drowned(here, wz) {
                        continue;
                    }
                    crossings += 1;
                    deep += usize::from(fresh(here, wz) && fresh(here + CELL_METRES, wz));
                }
                wz += CELL_METRES;
            }
        }
        assert!(
            crossings > 300,
            "only {crossings} lake shores across five seeds — nothing was tested"
        );
        let share = deep as f32 / crossings as f32;
        assert!(
            share > 0.9,
            "only {:.0}% of {crossings} lake shores have two facets of margin on them",
            share * 100.0
        );
    }

    #[test]
    fn a_chunk_carries_water_only_where_there_is_water_in_it() {
        let base = Vec2::new(-64.0, 32.0);
        let corner =
            |i: usize| base + Vec2::new((i % CORNERS) as f32, (i / CORNERS) as f32) * CELL_METRES;

        // Ground at 10 m with a lake at 12 m over the near half of it: water
        // to draw, so the grid travels — on the same grid as the heights, and
        // carrying the level at every corner the lakes answered for.
        let heights = vec![10.0f32; CORNERS * CORNERS];
        let water = corner_water(base, &heights, |_, wz| (wz < base.y + 60.0).then_some(12.0))
            .expect("a chunk with a lake on it");
        assert_eq!(water.len(), CORNERS * CORNERS);
        for (i, level) in water.iter().enumerate() {
            let want = if corner(i).y < base.y + 60.0 {
                protocol::ground::quantize(12.0)
            } else {
                protocol::ground::NO_WATER
            };
            assert_eq!(*level, want, "corner {i} at {}", corner(i));
        }

        // The same lake, but the ground stands above it everywhere in this
        // chunk — the overhang past a neighbouring lake's edge. Nothing to
        // draw, so nothing is sent.
        let dry = vec![20.0f32; CORNERS * CORNERS];
        assert_eq!(
            corner_water(base, &dry, |_, wz| (wz < base.y + 60.0).then_some(12.0)),
            None
        );

        // And ground with no lake anywhere near it.
        assert_eq!(corner_water(base, &heights, |_, _| None), None);
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    use std::time::Instant;

    /// What each seed actually produced, against what it was aiming for.
    ///
    /// The last column is the shoreline development index — the coast's length
    /// against that of a circle enclosing the same area. 1 is a perfect disc,
    /// and the more of a nuisance the island's outline is, the higher it goes.
    #[test]
    #[ignore]
    fn island_shape() {
        println!(
            "  targets: land {:.0}%, mountain {:.0}% of land above {MOUNTAIN_HEIGHT} m",
            LAND_FRACTION * 100.0,
            MOUNTAIN_FRACTION * 100.0
        );
        for seed in [20_040_112u32, 1, 7, 99, 12_345, 808, 2_024, 31_337] {
            let size = 1024;
            let config = MapConfig::square(size, seed);
            let gen = TerrainGenerator::new(&config);
            let half = config.half_extent();
            let at = |ix: i32, iz: i32| gen.height(ix as f32 - half.x, iz as f32 - half.y);

            let (mut land, mut mountain, mut edges, mut lake) = (0u32, 0u32, 0u32, 0u32);
            let mut peak = 0.0f32;
            let mut heights = Vec::new();
            for iz in (0..size as i32).step_by(2) {
                for ix in (0..size as i32).step_by(2) {
                    let h = at(ix, iz);
                    peak = peak.max(h);
                    let (wx, wz) = (ix as f32 - half.x, iz as f32 - half.y);
                    if gen.lake_level(wx, wz).is_some_and(|level| h < level) {
                        lake += 1;
                    }
                    if h <= 0.0 {
                        continue;
                    }
                    land += 1;
                    heights.push(h);
                    if h > MOUNTAIN_HEIGHT {
                        mountain += 1;
                    }
                    // Every step from land to sea is one unit of coastline.
                    edges += [(2, 0), (0, 2)]
                        .iter()
                        .filter(|(dx, dz)| at(ix + dx, iz + dz) <= 0.0)
                        .count() as u32;
                }
            }

            // How steep the land actually comes out. A map can hit every height
            // target and still be unusable, because height and width are set
            // independently and it is their ratio that you look at.
            let mut slopes: Vec<f32> = Vec::new();
            for iz in (0..size as i32).step_by(8) {
                for ix in (0..size as i32).step_by(8) {
                    let (wx, wz) = (ix as f32 - half.x, iz as f32 - half.y);
                    if gen.height(wx, wz) > 0.0 {
                        slopes.push(gen.normal(wx, wz).y.acos().to_degrees());
                    }
                }
            }
            slopes.sort_by(|a, b| a.partial_cmp(b).expect("no NaN in a height field"));
            let slope = |p: usize| slopes[slopes.len() * p / 100] as u32;

            // Which country the dial deals out — the number that watches
            // [`RUGGED_GENTLE`]/[`RUGGED_FULL`], the way the shore mix
            // watches the coast cuts. Counted over land, dial past halfway.
            let mut rugged = 0u32;
            let mut dialled = 0u32;
            for iz in (0..size as i32).step_by(8) {
                for ix in (0..size as i32).step_by(8) {
                    let (wx, wz) = (ix as f32 - half.x, iz as f32 - half.y);
                    let base = gen.landform(wx, wz);
                    if base > 0.0 {
                        dialled += 1;
                        if gen.rugged_dial(wx, wz, base) > 0.5 {
                            rugged += 1;
                        }
                    }
                }
            }

            let cells = (size / 2) * (size / 2);
            let area = land as f32 * 4.0;
            let coast = edges as f32 * 2.0;
            heights.sort_by(|a, b| a.partial_cmp(b).expect("no NaN in a height field"));
            let decile = |p: usize| heights[heights.len() * p / 100] as u32;
            println!(
                "seed {seed:>9}  land {:4.1}%  mountain {:4.1}% of land  peak {peak:5.0} m  \
                 lakes {:4.1}%  shoreline index {:.2}  slope p50/p90/p99 {:>2}/{:>2}/{:>2} deg  \
                 land height p50/p90 {:>3}/{:>3} m  rugged {:4.1}% of land",
                land as f32 / cells as f32 * 100.0,
                mountain as f32 / land.max(1) as f32 * 100.0,
                lake as f32 / cells as f32 * 100.0,
                coast / (2.0 * (std::f32::consts::PI * area).sqrt()),
                slope(50),
                slope(90),
                slope(99),
                decile(50),
                decile(90),
                rugged as f32 / dialled.max(1) as f32 * 100.0,
            );
        }
    }

    /// How the coastline of a few maps divides between beach, rocky shore and
    /// cliff, and how much of the interior the shore profile reaches into. The
    /// numbers the coastal constants were tuned against.
    #[test]
    #[ignore]
    fn shore_mix() {
        for seed in [20_040_112u32, 1, 7, 99, 12_345] {
            let config = MapConfig::square(1024, seed);
            let gen = TerrainGenerator::new(&config);
            let half = config.half_extent();

            // Measured along the waterline rather than over the area it
            // covers: a beach is far wider than a cliff by area, so counting
            // ground would say the map is nothing but beaches however the
            // thresholds are set.
            let mut coast = [0u32; 3];
            let mut examples = [(0usize, 0i32, 0i32); 3];
            let mut slopes: [Vec<f32>; 3] = Default::default();
            let (mut land, mut reshaped, mut rocks) = (0u32, 0u32, 0u32);

            let at = |ix: i32, iz: i32| gen.height(ix as f32 - half.x, iz as f32 - half.y);

            for iz in (0..1024).step_by(2) {
                for ix in (0..1024).step_by(2) {
                    let (wx, wz) = (ix as f32 - half.x, iz as f32 - half.y);
                    let h = at(ix, iz);
                    if h <= 0.0 {
                        continue;
                    }

                    // Standing in open water with nothing joined to it — a
                    // skerry, or a rock the detail layer left behind.
                    if [(-6, 0), (6, 0), (0, -6), (0, 6)]
                        .iter()
                        .all(|(dx, dz)| at(ix + dx, iz + dz) <= 0.0)
                    {
                        rocks += 1;
                    }

                    land += 1;
                    if gen.coast.metres(wx, wz) < SHORE_REACH * 0.7 {
                        reshaped += 1;
                    }

                    // A waterline crossing: land with sea in one of the four
                    // directions two metres away.
                    let shoreline = [(-2, 0), (2, 0), (0, -2), (0, 2)]
                        .iter()
                        .any(|(dx, dz)| at(ix + dx, iz + dz) <= 0.0);
                    if shoreline {
                        let kind = gen.shore(wx, wz);
                        coast[kind as usize] += 1;
                        // The steepest ground within a facet or two inland —
                        // the cliff face, where there is one. Rock is painted
                        // by slope, so this is what decides whether a cliff
                        // comes out grey or grassy.
                        let steepest = (0..8)
                            .map(|i| {
                                let a = i as f32 * std::f32::consts::TAU / 8.0;
                                let (sx, sz) = (wx + a.cos() * 4.0, wz + a.sin() * 4.0);
                                1.0 - gen.normal(sx, sz).y
                            })
                            .fold(0.0f32, f32::max);
                        slopes[kind as usize].push(steepest);
                        // Keep one spot per kind to point a camera at. The
                        // deepest into its own stretch wins, measured as how
                        // much of the ground around it agrees with it.
                        let agreement = (-3..=3)
                            .flat_map(|dx| (-3..=3).map(move |dz| (dx * 16, dz * 16)))
                            .filter(|(dx, dz)| gen.shore(wx + *dx as f32, wz + *dz as f32) == kind)
                            .count();
                        let best = &mut examples[kind as usize];
                        if agreement > best.0 {
                            *best = (agreement, wx as i32, wz as i32);
                        }
                    }
                }
            }

            let total = coast.iter().sum::<u32>().max(1) as f32;
            let share = coast.map(|c| (c as f32 / total * 100.0).round() as u32);

            // What share of each kind's shoreline is steep enough to be
            // painted as rock at all, and how steep its median is.
            let rocky = slopes.each_ref().map(|s| {
                let steep = s.iter().filter(|v| **v > ROCK_SLOPE).count();
                let mut s = s.clone();
                s.sort_by(|a, b| a.partial_cmp(b).expect("no NaN in a heightfield"));
                let median = s.get(s.len() / 2).copied().unwrap_or(0.0);
                (
                    (steep as f32 / s.len().max(1) as f32 * 100.0).round() as u32,
                    (median * 100.0).round() as u32,
                )
            });

            println!(
                "seed {seed:>9}  waterline beach/rocky/cliff {share:?}%  \
                 bare-rock share and median slope {rocky:?}  \
                 land the coast reshapes {:.0}%  {rocks} offshore rocks  {examples:?}",
                reshaped as f32 / land.max(1) as f32 * 100.0,
            );
        }
    }

    /// Cost of fitting a generator, which is the whole of a map's loading time
    /// before any mesh is built.
    ///
    /// Worth watching, and worth its own bench because nothing else here would
    /// show it: it is one call, it happens once, and it is quietly quadratic in
    /// map size — it samples the noise over the whole map on a fixed grid. A
    /// second round of fitting was once added by mistake and doubled this, with
    /// nothing in the maps or the tests to say so.
    #[test]
    #[ignore]
    fn generator_cost() {
        for metres in [512u32, 1024, 2048, 4096] {
            let start = Instant::now();
            let generator = TerrainGenerator::new(&MapConfig::square(metres, 1));
            let elapsed = start.elapsed();
            println!(
                "{metres:5} m  {elapsed:>8.0?}  range_gain {:.2}",
                std::hint::black_box(&generator).range_gain
            );
        }
    }
}
