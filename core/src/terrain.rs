//! Procedural terrain: the height field, its colours, and the chunk geometry.
//! Turning that geometry into engine meshes — and spawning it, with the sea
//! and the sun — is the game crate's `terrain` module.
//!
//! # Scale
//!
//! One world unit is one metre, matching Bevy's own convention (its lighting is
//! in real lux). One terrain tile is one metre square, and a map is a whole
//! number of [`CHUNK_TILES`]-tile chunks along each axis — any number on each,
//! from a single chunk up, square or not.
//!
//! The tile is the map's unit of ground, not the mesh's: the height field is
//! continuous, and it is drawn every [`MESH_STEP`] metres. See [`MESH_STEP`] and
//! [`TerrainGenerator::color`] for why — the ground is flat shaded in a fixed
//! palette, and both the facets and the colour bands want to be large enough to
//! read as deliberate shapes.
//!
//! The mesh is split into [`CHUNK_TILES`]-metre chunks rather than built as one
//! object. That keeps each chunk's bounding box tight enough for frustum culling
//! to do real work — with the camera pitched down at a fixed angle, only a
//! handful of chunks are ever on screen — and it bounds how much has to be
//! rebuilt if terrain is ever deformed.

use glam::{UVec2, Vec2, Vec3};

use crate::noise::{smoothstep, Noise};

/// Metres per terrain tile — the map's unit of ground, and the spacing the
/// height field is sampled at for ground queries.
pub const TILE_SIZE: f32 = 1.0;

/// Metres between mesh vertices. The height field is continuous, so this is
/// only how finely it gets *drawn*, and it is deliberately much coarser than a
/// tile: the ground is flat-shaded, and a facet has to be big enough to read as
/// a facet. At 2 m one covers roughly 50 px at the default zoom, which is about
/// where facets read as deliberate rather than as a low-resolution mesh.
pub const MESH_STEP: u32 = 2;

/// Tiles (so, metres) along the edge of one terrain chunk — a sensible unit of
/// both culling and rebuilding.
pub const CHUNK_TILES: u32 = 128;

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
/// The smallest maps need it moved, because their sorted field is nearly
/// radial order: over half of any map is falloff ring, and with no noise to
/// interleave the two, every quantile from the median down lands *in* the
/// ring. Anchored there, the whole visible lagoon maps to centimetres of
/// water, and the map paints as an islet in the middle of one huge pale
/// bank — the bullseye that made every small map read as a circle. Anchoring
/// on the shallow side puts the knee among the cells the player actually
/// sees, and the lagoon gets a real gradient down to dark water. On maps
/// with wavelengths to spare the noise interleaves ring and interior and the
/// median anchor is both safe and better — a shallow-side anchor there was
/// tried once, bent healthy seeds and starved their beaches — so it fades
/// out entirely before the preset sizes.
fn shoal_shift(extent: Vec2) -> f32 {
    let cycles = mean_extent(extent) / CONTINENT_SCALE;
    1.0 - smoothstep(0.35, 1.0, cycles)
}

/// The land share for a given map extent, continuous in it so nothing jumps
/// as a size control sweeps through it. Measured in how many times the
/// landmass field repeats across the map, since that is what decides what
/// the land can be: down near a fifth of a repeat the coast has to fit
/// inside the falloff ring and gets [`LAND_FRACTION_TINY`], up to about one
/// repeat the land is a single blob and gets [`LAND_FRACTION_SMALL`], and by
/// one and a half it is lobed enough to carry the full [`LAND_FRACTION`].
///
fn land_fraction(extent: Vec2) -> f32 {
    let cycles = mean_extent(extent) / CONTINENT_SCALE;
    LAND_FRACTION_TINY
        + (LAND_FRACTION_SMALL - LAND_FRACTION_TINY) * smoothstep(0.15, 0.7, cycles)
        + (LAND_FRACTION - LAND_FRACTION_SMALL) * smoothstep(1.2, 1.5, cycles)
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
    fn for_extent(extent: Vec2) -> Self {
        Self {
            land: land_fraction(extent),
            relief: peak_height(extent) / HEIGHT_SCALE,
            shoal: shoal_shift(extent),
        }
    }
}

/// Height above which summits hold snow, in metres. High enough that only the
/// tops of the biggest ranges reach it, so it stays an event rather than a
/// band across the whole upland.
const SNOW_LINE: f32 = 112.0;

/// Share of the map that reaches the deepest the sea bed is allowed to go, so
/// that open water reads as open water rather than as one endless shelf.
const DEEP_FRACTION: f32 = 0.18;

/// Depth the middle of the sea is guaranteed to reach, in metres — the mirror
/// of [`LOWLAND_FLOOR`], on the other side of the waterline. A minority of
/// seeds run flat just *below* where the sea lands in the field, and the deep
/// anchor cannot save them: it pins one point far down while the whole middle
/// of the distribution sits centimetres under the surface, so the map comes
/// out one endless bright shelf speckled with sand, with the falloff's rim
/// embossed round the edge of it. Set well past the last of the shallow-water
/// colours, not merely at their edge — a median pinned exactly on the colour
/// threshold leaves half the sea painted as shallows — so the middle of the
/// sea always reads as open water.
const SHALLOWS_FLOOR: f32 = 6.5;

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
// which would raise the interior rather than lower the coast, and would drain
// every inland lagoon on the map. And it is not applied to the massif before
// the height fit: that fit exists to guarantee a summit, so it simply answers
// any suppression by winding the range term up until the least coastal cell on
// the map spikes — which on a ring-shaped island put the full hundred and
// twenty metres of rock fifty metres from the water, worse than the coastal
// ranges this set out to remove. The ceiling has to be the last word, applied
// to metres, after everything else has had its say.

/// How far inland ground has to be, in metres, before it may stand at the full
/// [`HEIGHT_SCALE`]. Absolute rather than a share of the map, for the same
/// reason [`FEATURE_SCALE`] is: a bigger map should mean more landscape, not a
/// stretched copy of the same one.
///
/// This is the dial worth turning. It fixes [`PEAK_GRADE`], and with it how
/// tall a given map's mountains come out. A kilometre-square map's interior
/// stands a hundred and fifty to two hundred and forty metres from open water,
/// depending on the seed, and its summits come out between about fifty-five
/// and ninety metres rather than the full height — a little over seventy on
/// average. They fall short of the reach because a summit sits where the mask
/// put the massif, which is rarely the one most interior point. Snow is rarer
/// still: the tallest ground on any seed measured was 90 m at a kilometre,
/// 108 m at a kilometre and a half and 116 m at two, so
/// nothing but the largest maps crosses [`SNOW_LINE`] outright and what snow
/// appears below that is [`SNOW_WANDER`] carrying the line down to meet a
/// summit. Shortening this gives every size taller mountains and steeper
/// country.
///
/// It is also a frankly generous number. A summit at [`HEIGHT_SCALE`] this far
/// from the sea is a climb of about thirty degrees held for the whole way,
/// where the steepest real islands manage a seventh of that. That is the price
/// of a map a kilometre across having mountains on it at all, and the useful
/// part is not the absolute figure but that height now has to be paid for in
/// ground.
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

/// How much each massif is measured against its own local peak rather than
/// the map's tallest. At 0 the map is scaled by its single highest point, and
/// on a large map with a dozen massifs only that one reaches full height and
/// holds snow — the rest sit lower by pure luck of the mask field. At 1 every
/// blob that clears the footprint reaches full height, however slight it is,
/// and the map turns into a picket of identical cones. In between, the lesser
/// ranges are lifted most of the way to parity while the luck of the field
/// still shows through. Set high because the massif term is squared: a range
/// measured at nine tenths of its neighbour stands at eight tenths the
/// height, so even near-parity here leaves a visible pecking order — and the
/// snow line needs a summit at nine tenths of [`HEIGHT_SCALE`] before it
/// grants a second white cap at all.
const MASSIF_EQUALITY: f32 = 0.85;

/// Wavelength of the undulations within a field, in metres.
const DETAIL_SCALE: f32 = 50.0;
/// Wavelength of the surface roughness, in metres.
const MICRO_SCALE: f32 = 12.0;
/// Wavelength of the woodland/meadow patchwork, in metres. Field-sized on
/// purpose: at the default zoom the camera sees ~50 m of ground, so parcels much
/// bigger than this mean the whole screen is one colour.
const PATCH_SCALE: f32 = 30.0;
/// Wavelength of the shade variation within a parcel, in metres.
const MOTTLE_SCALE: f32 = 18.0;

/// Deepest the sea bed is allowed to go, in metres below sea level. Public
/// because the game hangs its ocean-floor backdrop just beneath it.
pub const MAX_DEPTH: f32 = 8.0;

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
/// `shore_mix` in the tests, which reports the split these produce.
const ROCKY_SHORE: f32 = -0.16;
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
/// *down* — a beach across a low neck of land, or the far side of a character
/// change — the difference stands as a scarp running dead straight down the
/// middle of the neck, conspicuously along the crest of the distance field.
/// Letting the top shelve off means the lift has already returned the ground
/// to itself before it can collide with anything: a cliff is a face, a solid
/// shoulder of clifftop behind it, and then hillside.
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

/// Spacing of the distance-to-water field, in metres. Finer than the mesh step
/// would buy nothing — the field is only ever read to shape and fade a coast
/// that is drawn every [`MESH_STEP`] metres.
const COAST_GRID: f32 = 4.0;

/// Wavelength of the skerries — the rock heads left standing offshore of a
/// rocky coast, in metres. Only the peaks of the field clear the water, so each
/// rock is a good deal smaller than this; much below it and they stop being
/// wide enough to make a facet.
const SKERRY_SCALE: f32 = 34.0;
/// How far a skerry stands out of the water, in metres.
const SKERRY_HEIGHT: f32 = 2.5;

/// Height, in metres, up to which ground is drawn as shore rather than as what
/// grows on it. Kept low deliberately: it is [`shape_coast`] flattening the
/// ground that makes a beach broad, so raising this would only smear the same
/// band of colour up the rocky shores and the cliffs as well.
const SHORE_TOP: f32 = 0.6;

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

/// The same for the snow line, which is free to move on its own — nothing lives
/// above it to be squeezed. Much the larger of the two because it applies much
/// higher up, where the ground is steep enough that a swing of a few metres
/// would not move the edge by even one facet.
const SNOW_WANDER: f32 = 26.0;

/// The ground palette. Small and flat on purpose — every facet gets exactly one
/// of these, so the whole map is drawn in fifteen colours plus three shade
/// steps. Saturated well past anything natural, because flat shading has no
/// texture or gradient to carry the picture; the colour has to do that work on
/// its own.
const SEABED: Vec3 = Vec3::new(0.16, 0.34, 0.38);
const SHALLOW: Vec3 = Vec3::new(0.46, 0.68, 0.62);
const SAND: Vec3 = Vec3::new(0.90, 0.83, 0.58);
/// Pebble and boulder foreshore. Warmer and lighter than [`ROCK`], so a shingle
/// beach reads as its own thing next to the cliffs rather than as more of them.
const SHINGLE: Vec3 = Vec3::new(0.70, 0.65, 0.55);
const FOREST: Vec3 = Vec3::new(0.21, 0.42, 0.22);
const GRASS_DARK: Vec3 = Vec3::new(0.33, 0.55, 0.23);
const GRASS: Vec3 = Vec3::new(0.44, 0.66, 0.26);
const GRASS_LIGHT: Vec3 = Vec3::new(0.56, 0.75, 0.31);
const MEADOW: Vec3 = Vec3::new(0.66, 0.73, 0.34);
/// Moorland, above the trees and below the bare rock. [`HEATH`] and [`FELL`]
/// are what the darkest and lightest lowland parcels turn into as they climb —
/// the one still half green, the other already most of the way to stone — so
/// that the upland reads as the same country drained of colour rather than as
/// a different map laid over the top.
const HEATH: Vec3 = Vec3::new(0.38, 0.45, 0.27);
const UPLAND: Vec3 = Vec3::new(0.50, 0.50, 0.31);
const FELL: Vec3 = Vec3::new(0.63, 0.60, 0.42);
const ROCK: Vec3 = Vec3::new(0.55, 0.53, 0.50);
const ROCK_DARK: Vec3 = Vec3::new(0.40, 0.38, 0.37);
/// Bare stone bleached by the weather, the last step before the snow.
const SCREE: Vec3 = Vec3::new(0.68, 0.65, 0.60);
/// Snow on the summits. Off-white and slightly blue: a pure white would be the
/// only fully saturated thing on the map and would pull the eye off everything
/// else, and it has to stay clearly apart from [`ROCK`] in shadow.
const SNOW: Vec3 = Vec3::new(0.90, 0.92, 0.95);

/// What each parcel of the patchwork is drawn as, at each height it can reach.
///
/// One row per band and one column per parcel, indexed by the *same* bucket of
/// the *same* noise field however high the ground is. That is what carries the
/// blend: a wood running up a hillside keeps its outline as it crosses the
/// treeline and comes out the other side as heather, so the two bands share
/// their shapes instead of meeting along a seam of their own.
///
/// The rows get flatter towards the top — five distinct greens, three shades of
/// moor, two of rock — so the patchwork thins out with the vegetation without
/// ever stopping dead. By the summits it is nearly gone, which is the point:
/// bare rock has nothing growing on it to make parcels out of, and the relief
/// up there is drawn by the slope tests above instead. [`ROCK_DARK`] is left to
/// them, so that a dark facet on a mountain always means a crag.
const LOWLAND_PARCELS: [Vec3; 5] = [FOREST, GRASS_DARK, GRASS, GRASS_LIGHT, MEADOW];
const MOOR_PARCELS: [Vec3; 5] = [HEATH, HEATH, UPLAND, FELL, FELL];
const MOUNTAIN_PARCELS: [Vec3; 5] = [ROCK, ROCK, ROCK, SCREE, SCREE];

/// Parameters the map is generated from.
///
/// The `bevy` feature is only these derives: the game holds one of these as an
/// ECS resource, and a server or wasm build has no ECS to hold it in — the
/// derive rides behind the feature so those builds stay engine-free.
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Resource))]
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

/// Samples terrain height and surface colour for a given seed.
///
/// The game keeps one around after the mesh is built, because the height field
/// is what anything wanting to sit on the ground has to ask — the camera
/// already does.
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Resource))]
pub struct TerrainGenerator {
    continent: Noise,
    hills: Noise,
    mountain_mask: Noise,
    ridges: Noise,
    warp: Noise,
    detail: Noise,
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
    /// [`TerrainGenerator::new`] has found a coastline to measure; nothing
    /// asks for a height before then, and an empty field reads infinity
    /// everywhere in any case, which is the ceiling declining to bind rather
    /// than binding on a measurement it has not made.
    ///
    /// Deliberately *not* [`TerrainGenerator::coast`], even though the two are
    /// measured from the same grid a moment apart. They differ in what counts
    /// as water: this one counts only water broad enough to stand in for the
    /// sea, so the ponds and narrow sounds that fill the interior of these maps
    /// are ground rather than a coast a range has to climb from. See
    /// [`CoastDistance::from_open_water`].
    inland: CoastDistance,
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
/// A plain `min` is what this must not be. Clipped flat, every headland with a
/// strong massif on it becomes a mesa — and a mesa has no summit, so its
/// highest ground is barely above its own shoulders and there is nothing for a
/// peak to be made of. It would also print the shape of the distance field on
/// the ground wherever it bound, creases and all, which is the one thing every
/// other reader of that field takes trouble to avoid.
///
/// A smooth minimum instead: all but exactly `h` while `h` is well under the
/// ceiling, bending over as it approaches, and closing on the ceiling from
/// below without ever sitting on it. Ground under a binding ceiling still
/// climbs, just far more slowly than the noise wanted it to — which is what a
/// worn-down headland looks like.
fn under_ceiling(h: f32, ceiling: f32) -> f32 {
    // Nothing to do below the waterline — the ceiling is about how high land
    // may stand, and depth is the calibration's business.
    //
    // The second half of that is belt and braces rather than arithmetic. An
    // infinite ceiling already falls out of the formula as `h` untouched, and
    // it should not arise anyway: [`TerrainGenerator::inland`] only reads
    // infinite before it has a field to measure, and by the time anything asks
    // for a height it has one. What the check is really for is the not-a-
    // number a distance field with no sea in it at all would blur its way to.
    if h <= 0.0 || !ceiling.is_finite() {
        return h;
    }
    // No guard on the divisor: the ceiling is [`CLIFF_HEIGHT`] plus a distance
    // that cannot be negative, so it is never near zero.
    let ratio = h / ceiling;
    h / (1.0 + ratio.powf(CEILING_KNEE)).powf(1.0 / CEILING_KNEE)
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

impl TerrainGenerator {
    pub fn new(config: &MapConfig) -> Self {
        let seed = config.seed;
        let targets = Targets::for_extent(config.extent());
        let cycles = config.extent() / CONTINENT_SCALE;
        let room = smoothstep(1.1, 1.55, cycles.x.min(cycles.y));

        let mut generator = Self {
            continent: Noise::new(seed),
            hills: Noise::new(seed.wrapping_add(0x51ED_2701)),
            mountain_mask: Noise::new(seed.wrapping_add(0x9E37_79B9)),
            ridges: Noise::new(seed.wrapping_add(0x1B87_3593)),
            warp: Noise::new(seed.wrapping_add(0x68E3_1DA4)),
            detail: Noise::new(seed.wrapping_add(0xB547_9AA3)),
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
            coast_scale: 1.0,
            half_extent: config.half_extent(),
            drift_excess: Vec2::ZERO,
            bend_gain: 0.25 + 0.75 * room,
            reach_max: 1.0 + 0.15 * room,
        };
        let centre = generator.warped(0.0, 0.0).1 * FEATURE_SCALE * 0.7;
        let tolerance = generator.half_extent * 0.1;
        generator.drift_excess = centre - centre.clamp(-tolerance, tolerance);

        // One round of fitting, and the ceiling laid over what it produced.
        //
        // It is worth saying why this is not a loop, because it looks like it
        // ought to be one: the ceiling changes the landform, the landform
        // decides where the water is, and the water is what the ceiling is
        // measured from. But nothing inside [`TerrainGenerator::fit`] reads
        // the ceiling — it is applied in [`TerrainGenerator::landform`],
        // strictly downstream of everything fitted here, and deliberately so;
        // see the note in [`TerrainGenerator::fit_range_height`]. So the
        // coastline this measures is the one the fit produced, and fitting
        // again against it would return the same numbers to the bit. A second
        // round was tried and did exactly that, for twice the noise sampling.
        //
        // Nor is there a coastline to converge on even in principle. The
        // ceiling scales a height rather than subtracting from it, and a
        // positive height stays positive however hard it binds — so it moves
        // every contour on the map except the one at zero, which is the only
        // one any of this is measured from. Whatever it does to the mountains,
        // the waterline it leaves is the waterline it was handed.
        //
        // The order below is load-bearing all the same. `inland` is the one
        // thing [`TerrainGenerator::ceiling`] reads, and the coast-band fit is
        // the one step here that asks for a finished height — it walks the
        // waterline reading [`TerrainGenerator::normal`] — so it has to run
        // against the ceiling rather than before it exists.
        let raw = generator.fit(config, &targets);
        generator.inland = CoastDistance::from_open_water(&raw, &generator.calibration);
        generator.fit_coast_scale(&raw);
        generator
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
    /// Offset by [`CLIFF_HEIGHT`], because ground at the waterline is not
    /// obliged to be at the waterline: a coast may stand a cliff tall without
    /// having climbed from anywhere, which is exactly what the coastal shaping
    /// spends its time building. Without the offset the ceiling bears down on
    /// the ordinary low country too — most of any map here is within a few
    /// tens of metres of water — and it takes enough off the middle of the
    /// land to undo [`LOWLAND_FLOOR`] and leave the map the drowned sandflat
    /// the calibration went to trouble to rule out.
    ///
    /// Biasing the massif *towards* the interior instead was tried first, and
    /// dropped. A soft preference does not stop anything: a massif the mask
    /// made strong still beats a weaker one with far more room behind it —
    /// being held back near the water costs it less than being feeble costs
    /// the other — and the height fit, which has to put a summit somewhere,
    /// then puts it exactly where the preference was trying to avoid. Measured
    /// on the finished maps that was the common case rather than the rare one:
    /// summits standing at one and a half to two metres of height per metre of
    /// ground back from the sea, against the half-metre the rest of this is
    /// written around. A limit does what a preference could not.
    fn ceiling(&self, wx: f32, wz: f32) -> f32 {
        CLIFF_HEIGHT + PEAK_GRADE * self.inland.metres(wx, wz)
    }

    /// Fits where this seed's mountains sit and how much of the map they cover,
    /// and returns the grid it worked that out on for
    /// [`TerrainGenerator::fit_range_height`] to scale them on.
    ///
    /// Left to the noise, both are luck. Whether a seed is mountainous comes
    /// down to how much of its mask field happens to clear a fixed threshold
    /// and whether its ridge lines happen to fall under it — and on the same
    /// settings that gives one map a five-hundred-metre alp and the next a
    /// seventy-metre hill. Two passes over a coarse grid settle it: the first
    /// finds where to start counting the massif so it covers
    /// [`RANGE_FOOTPRINT`] of the map and reaches its full height only at its
    /// highest point, the second builds the field either side of the one term
    /// that is still free.
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
                let continent = self
                    .continent
                    .fbm(n.x * CONTINENT_FREQ, n.y * CONTINENT_FREQ, 4);
                seat[iz * px + ix] = self.range_seat(n, continent);

                if (pad..px - pad).contains(&ix) && (pad..pz - pad).contains(&iz) {
                    let hills = self.hills.fbm(n.x * 0.9, n.y * 0.9, 5);
                    let (damp, push) = self.falloff(wx, wz, n, drift);
                    (points).push((
                        wx,
                        wz,
                        n,
                        continent,
                        0.62 * continent + 0.26 * hills,
                        damp,
                        push,
                    ));
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
        let mut ceiling = seat;
        window_max(&mut ceiling, (px, pz), pad);
        blur(&mut ceiling, (px, pz));
        blur(&mut ceiling, (px, pz));
        self.range_ceiling = GridField {
            cells: ceiling,
            dims: (px, pz),
            origin: pad_origin,
        };

        points
            .iter()
            .map(|(wx, wz, n, continent, base, damp, push)| Sample {
                base: *base,
                range: self.ranges(*wx, *wz, *n, *continent),
                damp: *damp,
                push: *push,
            })
            .collect()
    }

    /// Scales the massif until the highest ground on the map stands at
    /// [`peak_height`] — [`HEIGHT_SCALE`], on any map big enough to hold it.
    ///
    /// Fitting the footprint is not enough on its own. How far a summit gets
    /// above the line where the mountains start is set by how sharply this
    /// seed's massif happens to come to a point, and on the same settings that
    /// ranges from a hundred metres to nearly three — so half the seeds have
    /// nothing that reads as a mountain and the other half have a spike.
    ///
    /// What is *not* the way to fix it is bending the mapping in
    /// [`Calibration`]; see the note there. This scales the massif term instead,
    /// which changes how high a range stands without touching how sharp it is,
    /// and leaves the mapping the straight line it needs to be.
    ///
    /// The catch is that the scale and the mapping decide each other — pushing
    /// the range up moves the mountain line the peak is measured against — so
    /// it is solved for rather than calculated. Cheaply, though: the noise is
    /// already sampled, and every step from here is a multiply and a sort.
    fn fit_range_height(&mut self, samples: &[Sample], targets: &Targets) {
        // What the map's highest ground comes out at, in metres, for a given
        // scale on the massif.
        //
        // Fitted against the field *before* [`TerrainGenerator::ceiling`] gets
        // to it, and deliberately so. The ceiling is what decides how tall a
        // map's mountains actually come out, and it does that from geometry —
        // so the fit has no business chasing it. Made to chase it, the fit
        // pushes the range term as far as it takes to get a summit up to a
        // target the ceiling will not allow, which is a long way: the massif
        // ends up many times the share of the field it should be, and since
        // the room a map has depends on the map, the same seed came out a
        // different landscape at two sizes. Left aiming at a fixed height, the
        // range term keeps the size-independent value it always had, and the
        // ceiling clips whatever stands taller than its ground has earned.
        let peak_at = |gain: f32| {
            let mut raw: Vec<f32> = samples.iter().map(|s| s.raw(gain)).collect();
            // Sorts in place, so the last entry is the summit afterwards.
            let calibration = Calibration::fit(&mut raw, targets);
            calibration.metres(raw[raw.len() - 1])
        };

        let target = HEIGHT_SCALE * targets.relief;

        // Two brackets to start from. The lower is well under anything that
        // produces mountains; the upper is a limit as much as a bracket, and
        // that wants explaining.
        //
        // From a kilometre up this is a genuine solve and the answer is small:
        // under 4 on every seed measured, and under 1 by two kilometres. Below
        // that the curve stops being one a solve can follow. The calibration
        // refits the mapping at every trial, so past a point pushing the massif
        // harder stops raising the summit in metres at all — on one 512-metre
        // seed the summit read 54.1 m at every scale from 1 to 200 — and far
        // enough past it the massif swamps the quantiles the mapping is
        // anchored on and the whole thing breaks upward, the same seed reading
        // 1285 m at a scale of ten thousand. Others turn over instead, rising
        // to a maximum part way and falling back. On any of them no scale
        // reaches the target and the bisection runs to whatever ceiling it was
        // given, so what that ceiling is chooses the answer outright.
        //
        // Which is the case for keeping it low, and it costs nothing to. The
        // maps either ceiling produces are indistinguishable — the calibration
        // absorbing the scale is the same thing that made the curve go flat —
        // so the only difference is the number a degenerate seed comes away
        // with. At 200 that number lands two orders of magnitude off its
        // neighbours': one seed solved to 1.9 at 640 m, 200 at 768 m and 1.4 at
        // a kilometre. Since map size is a dial the player turns, a fitted
        // number that jumps like that between neighbouring sizes is worth not
        // having, even where the picture survives it.
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
    /// seed whose terrain comes out steeper, the ground climbs past
    /// [`CLIFF_HEIGHT`] within a few metres of the water, the reshaping runs
    /// out of room, and the beaches collapse into sand ribbons with a grey
    /// slope-rule line along the waterline.
    ///
    /// No single measurement of the landform predicts that well — seeds with
    /// the same near-shore heights come out with very different beaches — so
    /// like everything else here it is solved for instead: walk the beach
    /// stretches of the waterline, measure what share of them the slope rule
    /// would paint grey, and take the smallest band that gets that share under
    /// [`BEACH_STEEP_TARGET`]. Gentle seeds pass at 1.0 and keep the tuned
    /// look untouched.
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
                let shoreline = [iz * nx + ix - 1, iz * nx + ix + 1]
                    .iter()
                    .chain(&[(iz - 1) * nx + ix, (iz + 1) * nx + ix])
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

    /// The landscape before the coast gets to it — continent, hills and ranges
    /// — in metres, with sea level at 0.
    ///
    /// Split out from [`TerrainGenerator::height`] because the coastal
    /// reshaping has to know how far away the water is, and this smooth field
    /// is what that gets measured against. The detail layers are deliberately
    /// not part of it: their gradient is as steep as the landform's own, so
    /// including them would drown out the very thing being measured.
    /// The shape of the land, in arbitrary units, before [`Calibration`] decides
    /// what any of it means. Higher is higher; where sea level falls in it is
    /// not settled until the whole field has been looked at.
    /// Where a world point lands once the domain warp has moved it, and how far
    /// it moved. Two scales of warp: a long one that bends whole coastlines
    /// into peninsulas and gulfs, and a shorter one for the wandering of the
    /// shore itself. Offsetting the sample point by another noise field is what
    /// turns concentric blobs into meandering, organic shapes.
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

    /// Where this map's mountains want to sit: its own mask field, plus a good
    /// share of the landmass field.
    ///
    /// Tying the two together does two jobs. Ranges end up on the high ground
    /// rather than wherever the mask happens to fall, which is both how real
    /// ones sit and what stops a seed putting its only massif out at sea and
    /// coming back with a map of hills. And a range that runs down a peninsula
    /// still gets a range's height, because the landmass field is high there
    /// too.
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
        // moment the noise passes its upper edge — it becomes a mesa, and a
        // mesa has no summit: its highest ground is barely above its own
        // shoulders, so there is nothing for peaks to be made of.
        //
        // And its footprint has to be about the share of the map that is meant
        // to end up mountainous. Spread over half of it, the ninetieth
        // percentile of the land sits partway up the dome rather than at its
        // foot, and then raising the range lifts the mountain line along with
        // it and the summits never get any further above their own shoulders
        // however hard they are pushed. Left to the noise, that footprint is
        // pure luck of the seed, and the same settings give one map a
        // five-hundred-metre alp and the next a seventy-metre hill.
        // Measured partly against the map's whole span and partly against the
        // local ceiling — the tallest seat within [`MASSIF_WINDOW`] — so that
        // every range has a summit that approaches full height, not just the
        // one that happens to hold the map's highest seat. See
        // [`MASSIF_EQUALITY`].
        let local = (self.range_ceiling.at(wx, wz) - self.range_floor).max(1e-3);
        let span = self.range_span + MASSIF_EQUALITY * (local - self.range_span);
        let massif = ((self.range_seat(n, continent) - self.range_floor) / span).clamp(0.0, 1.0);

        // The ridges on top of it. A ridged field is all cusp — its maximum is
        // a crease, not a summit — so left to shape the range on its own, with
        // the height curve cubing its top end, it gives a row of knife blades
        // standing on end. Riding on the massif at less than half strength it
        // does what it is good at, which is putting crests and gullies on a
        // mountain whose shape has already been decided.
        // Ramped over most of the ridged field's range rather than its top
        // slice, so a crest is a broad shoulder rather than a wall: taken
        // narrow, the term is a razor line a couple of facets wide, and scaled
        // up to mountain height it reads as masonry running across the country.
        let crest = smoothstep(0.25, 0.98, self.ridges.ridged(n.x * 0.5, n.y * 0.5, 3));

        // Squared, which is what puts a summit on the dome. Left linear it is
        // too even-sided: the contour enclosing the top few per cent of the map
        // sits nearly half way up it, so the peak is only twice the height of
        // the mountain line no matter how hard the range is pushed. Squaring
        // drops that contour down the flank and leaves room above it. It is
        // safe to do here and nowhere else, because the massif is a smooth
        // swell — the same trick on the ridged field sharpens its creases into
        // blades.
        massif * massif * (0.55 + 0.45 * crest)
    }

    fn landform_raw(&self, wx: f32, wz: f32) -> f32 {
        let (n, drift) = self.warped(wx, wz);
        let (nx, nz) = (n.x, n.y);

        // Broad landmass shape, then rolling hills layered on top.
        let continent = self
            .continent
            .fbm(nx * CONTINENT_FREQ, nz * CONTINENT_FREQ, 4);
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
        // Measured in the warped frame, and against a radius that wanders with
        // its own slow field, so the little of the outline it does decide is
        // not a circle either.
        // The radius wanders at a wavelength short enough to vary *around* the
        // island — a slower field than this is near enough constant across a
        // kilometre of map, so instead of lobing the outline it just scales the
        // whole island, and seeds come out either filling their map or lost in
        // the middle of it. It matters most on the smallest maps, where the
        // land usually does reach the falloff: at 0.55 the field put barely a
        // lobe on a 768 m island's outline, and every small map read as the
        // same rounded square.
        // Only how the drift *varies* around the ring draws lobes on it;
        // whatever the whole map's drift has in common is a displacement of
        // the entire ring — and the ring has very little room to be
        // displaced, since land survives to nearly the top of the ramp and
        // the ramp ends barely past the frame. The warp's dominant component
        // is over a kilometre long, so on maps up to that order the shared
        // part is most of the drift: taken raw, it slid the ring off the edge
        // of the map, and the coast on that side was drawn by the rim in a
        // dead straight line along the frame. Two guards keep the ring on the
        // map, each for the scale the other cannot cover.
        //
        // `drift_excess` is the shared part — read at the map centre — beyond
        // a tolerance of a tenth of each half extent, subtracted everywhere.
        // A seed whose drift is centred keeps its shape untouched; a seed
        // blown off the map is slid back onto it, with every lobe intact,
        // because a constant subtraction changes nothing about how the drift
        // varies. Full recentring is deliberately *not* done: past the warp's
        // wavelength the centre stops predicting the drift at the ring, and
        // anchoring the ring to it there pushes maps off their frames instead
        // of back onto them.
        //
        // And `bend_gain` damps the whole bend on maps the landmass field
        // cannot break up — the same regime the land share tapers in, judged
        // on the tighter axis — because down there even the drift's local
        // variation outruns the few metres of margin the ring has, and what
        // the subtraction leaves still cuts the coast off at the frame. On
        // larger maps the same variation is a lobe, and is most of what
        // un-squircles them, so it comes back in full as soon as the map can
        // afford it. Both numbers are per-map, worked out once at
        // construction.
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
        // drops the ground into the sea too fast, and the slope rule then paints
        // a grey cliff right the way round every island — which is no more the
        // point than a beach right the way round was. Starting the ramp much
        // further out has been tried twice and is worse both times: the land
        // reaches the rim and is cut off dead straight, or the sea floods the
        // interior into fragments.
        // A last, unwarped guard on the outcome, because the two guards above
        // both act on the bend's *inputs* and the warp has one more trick: a
        // drift that diverges across the map — pulling outward on both ends
        // of an axis at once — inflates the ring past both frame edges
        // without any net translation for the subtraction to catch or, on a
        // large map, any taper to damp. Whatever the warp does, the edge is
        // forced closed by the last few per cent of the frame, so every map
        // keeps a sea margin; where it binds the coast follows the unwarped
        // contour for a stretch, which is an arc, and an arc is the ring
        // showing — far better than the frame showing. It starts well outside
        // the ring's usual reach, so the healthy majority of coasts never
        // touch it.
        // Skipped over the interior, where it is identically zero: a squircle
        // is at most 2^(1/power) times the larger axis fraction, so inside
        // two thirds of either half extent it cannot reach the guard's ramp —
        // and its `powf`s are most of this function's arithmetic.
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
        (dx.powf(power) + dz.powf(power)).powf(1.0 / power)
    }

    /// Terrain height in metres before the coast reshapes it. Sea level is 0.
    fn landform(&self, wx: f32, wz: f32) -> f32 {
        under_ceiling(
            self.calibration.metres(self.landform_raw(wx, wz)),
            self.ceiling(wx, wz),
        )
    }

    /// Terrain height in metres at a world-space `(x, z)`. Sea level is 0.
    pub fn height(&self, wx: f32, wz: f32) -> f32 {
        let base = self.landform(wx, wz);
        let mut h = base;

        // Surface detail, faded out underwater where it just adds noise.
        if base > -1.0 {
            h += self.detail.fbm(wx / DETAIL_SCALE, wz / DETAIL_SCALE, 3) * 5.0;
            // Kept small: at 12 m it is only three mesh vertices across, so any
            // more amplitude turns into facet-to-facet jitter rather than
            // readable surface roughness.
            h += self.detail.fbm(wx / MICRO_SCALE, wz / MICRO_SCALE, 2) * 0.5;
        }

        // Keep the detail layer from punching holes below sea level all over the
        // interior — otherwise the map is speckled with puddles.
        if base > 3.0 {
            h = h.max(0.5);
        }

        // Everything above is the same landscape whatever the coast does with
        // it; the rest of this decides what happens where it meets the water.
        let distance = self.coast.metres(wx, wz);
        let shore = self.shore_character(wx, wz);

        // Faded out inland rather than cut off at a height. Most of this map's
        // land is under [`CLIFF_HEIGHT`] — it is a gentle island — so a rule
        // written in heights alone would have a cliff coast lifting and
        // terracing plains half a kilometre from the sea.
        h += (shape_coast(h, distance, shore, self.coast_scale) - h) * coastal_weight(distance);
        h = self.skerries(wx, wz, h, shore);

        h.max(-MAX_DEPTH)
    }

    /// What kind of coast this stretch is, as a continuous value: `-1.0` for
    /// ground that shelves gently away into sand, `+1.0` for ground that stands
    /// straight up out of the water, and the rocky shores in between.
    ///
    /// Read at the nearest point on the shoreline rather than underfoot, so
    /// that it is a property of a stretch of coast and not of a spot on the
    /// map. It is a two-dimensional field and it wanders across a coast as
    /// readily as along one: sampled where it is used, a single stretch can be
    /// a beach at the water and a cliff a hundred metres inland, and it builds
    /// both — a wall of rock standing behind a sand beach, which is a thing no
    /// coastline does.
    ///
    /// Which way the shore lies comes from the gradient of the distance field,
    /// pointing down the slope of it towards the water.
    ///
    /// Except along the crest of that field — the line equidistant between two
    /// shores — where the gradient flips a whole half turn and the nearest
    /// waterline point teleports from one coast to the other. Read naively,
    /// the character jumps with it, and the height field cracks along the
    /// crest: a dead-straight hairline scarp running down the middle of every
    /// neck and every strait, plainly visible in plan. A distance field's
    /// slope is 1 everywhere except approaching that crest, where opposing
    /// wavefronts meet and it collapses — so the collapse *is* the detector,
    /// and the displacement is faded out on it. The two sides then agree on
    /// the character underfoot by the time they meet.
    fn shore_character(&self, wx: f32, wz: f32) -> f32 {
        let distance = self.coast.metres(wx, wz);
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
        Shore::of(self.shore_character(wx, wz))
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
        let hl = self.height(wx - TILE_SIZE, wz);
        let hr = self.height(wx + TILE_SIZE, wz);
        let hd = self.height(wx, wz - TILE_SIZE);
        let hu = self.height(wx, wz + TILE_SIZE);
        normal_from_neighbours(hl, hr, hd, hu)
    }

    /// Surface colour for one facet, picked from a fixed palette.
    ///
    /// Nothing here blends. Every choice is a hard threshold, so a facet gets
    /// exactly one palette entry — that is what makes the ground read as flat
    /// coloured shapes rather than as a wash of gradient.
    ///
    /// Which means the work of getting from one band to the next is done by the
    /// *shape* of the boundary rather than by mixing the colours across it. Two
    /// things do it: the edges wander off the level by [`BAND_WANDER`], and the
    /// patchwork either side of them is cut from one field, so the parcels line
    /// up through the join. See [`LOWLAND_PARCELS`].
    pub fn color(&self, wx: f32, wz: f32, height: f32, normal: Vec3) -> Vec3 {
        // 0 on flat ground, approaching 1 on a cliff face.
        let slope = 1.0 - normal.y;

        // Water first. Two tones of sea bed, both read through translucent
        // water: a dark bottom, then a bright shelf that gives a coast its
        // turquoise ring. How wide that ring is comes from the landform rather
        // than from anything here — [`shape_coast`] gives a beach a long
        // shallow apron and drops a cliff straight past it.
        if height < -4.5 {
            return SEABED;
        }
        if height < -1.8 {
            return SHALLOW;
        }

        // The shore itself, from the low-water mark to the back of the beach.
        // The slope test comes first, so the wave-cut foot of a cliff is rock
        // rather than sand; on a beach there is no slope to speak of, so it
        // never fires and the sand stays clean.
        if height < SHORE_TOP {
            if slope > ROCK_SLOPE {
                return ROCK_DARK;
            }
            return match self.shore(wx, wz) {
                Shore::Beach => SAND,
                Shore::Rocky => SHINGLE,
                Shore::Cliff => ROCK_DARK,
            };
        }

        // Steep ground is bare rock whatever height it's at. Above the shore
        // this is what paints the cliff faces, and inland it picks out crags on
        // the hills the same way.
        if slope > CLIFF_SLOPE {
            return ROCK_DARK;
        }
        if slope > ROCK_SLOPE {
            return ROCK;
        }

        // How far this spot's band edges have strayed from the level.
        let wander = self
            .detail
            .fbm(wx / BAND_SCALE - 53.0, wz / BAND_SCALE + 29.0, 4);

        // Snow first, and on its own — it is the one band with nothing above it
        // to be squeezed, so it gets its own swing and takes no part in the
        // patchwork below.
        if height + SNOW_WANDER * wander > SNOW_LINE {
            return SNOW;
        }

        // The height everything below reads its band off.
        let banded = height + BAND_WANDER * wander;

        // The patchwork. Quantising a low-frequency noise field into a few
        // buckets gives irregular parcels with hard edges — woodland against
        // pasture against crop — instead of one smooth green wash.
        let patch = self
            .detail
            .fbm(wx / PATCH_SCALE + 11.0, wz / PATCH_SCALE - 7.0, 3);
        // Thresholds are set off the noise's measured distribution, not off its
        // nominal range, so all five actually get used — this field sits inside
        // roughly ±0.65 but four fifths of it is inside ±0.17.
        let bucket = if patch < -0.20 {
            0
        } else if patch < -0.07 {
            1
        } else if patch < 0.08 {
            2
        } else if patch < 0.22 {
            3
        } else {
            4
        };

        // Which row of the palette that parcel is drawn from — the only thing
        // height decides up here.
        let parcel = if banded > MOUNTAIN_HEIGHT {
            MOUNTAIN_PARCELS[bucket]
        } else if banded > MOOR_HEIGHT {
            MOOR_PARCELS[bucket]
        } else {
            LOWLAND_PARCELS[bucket]
        };

        // A finer band takes a lighter or darker cut of the same colour, so a
        // big parcel still breaks into facets rather than reading as one slab.
        // Three steps, not a multiplier curve — a gradient here would undo the
        // whole point of the quantising above.
        // Most facets take the parcel colour untouched; only the tails of the
        // field get shifted, so this reads as occasional patches rather than as
        // constant speckle.
        let shade = self.detail.fbm(wx / MOTTLE_SCALE, wz / MOTTLE_SCALE, 2);
        let tint = if shade > 0.26 {
            1.09
        } else if shade < -0.26 {
            0.92
        } else {
            1.0
        };

        (parcel * tint).clamp(Vec3::ZERO, Vec3::ONE)
    }

    /// Builds the geometry for one chunk. `origin` is the chunk's lower-corner
    /// tile index into the map, whose full extent in tiles is `map_tiles` —
    /// every chunk is full-sized, since a map is a whole number of chunks.
    ///
    /// The geometry is flat shaded: every triangle carries its own normal and
    /// its own single colour, so no vertex is shared between two triangles.
    /// That costs three vertices per triangle instead of roughly one, and buys
    /// it back many times over from drawing at [`MESH_STEP`] rather than per
    /// tile. It also means chunks need no border samples to meet cleanly —
    /// there are no shared normals to disagree about.
    ///
    /// Vertex positions are relative to the chunk's own origin, so whatever
    /// places the chunk carries the world offset and the bounding box stays
    /// tight.
    pub fn build_chunk(&self, origin: UVec2, map_tiles: UVec2) -> ChunkGeometry {
        let half = map_tiles.as_vec2() * TILE_SIZE * 0.5;
        let step = MESH_STEP as f32;

        let quads = (CHUNK_TILES / MESH_STEP) as usize;
        let verts = quads + 1;

        // Corner heights, shared between the quads that meet there even though
        // the vertices themselves won't be.
        let mut heights = vec![0.0f32; verts * verts];
        for iz in 0..verts {
            let wz = (origin.y + iz as u32 * MESH_STEP) as f32 - half.y;
            for ix in 0..verts {
                let wx = (origin.x + ix as u32 * MESH_STEP) as f32 - half.x;
                heights[iz * verts + ix] = self.height(wx, wz);
            }
        }

        let count = quads * quads * 6;
        let mut positions = Vec::with_capacity(count);
        let mut normals = Vec::with_capacity(count);
        let mut uvs = Vec::with_capacity(count);
        let mut colors = Vec::with_capacity(count);

        // Where this chunk's local origin sits in the world.
        let base = Vec2::new(origin.x as f32 - half.x, origin.y as f32 - half.y);

        for iz in 0..quads {
            for ix in 0..quads {
                let (x0, z0) = (ix as f32 * step, iz as f32 * step);
                let (x1, z1) = (x0 + step, z0 + step);
                let h = |cx: usize, cz: usize| heights[cz * verts + cx];

                let tl = Vec3::new(x0, h(ix, iz), z0);
                let tr = Vec3::new(x1, h(ix + 1, iz), z0);
                let bl = Vec3::new(x0, h(ix, iz + 1), z1);
                let br = Vec3::new(x1, h(ix + 1, iz + 1), z1);

                // Which way the quad is split alternates like a checkerboard.
                // Splitting every quad the same way lines the facets up into an
                // obvious herringbone across open ground; alternating breaks
                // that up without costing anything.
                //
                // Both windings are counter-clockwise seen from above (+Y),
                // which is what puts the face normals upwards.
                let split = if (ix + iz) % 2 == 0 {
                    [[tl, bl, tr], [tr, bl, br]]
                } else {
                    [[tl, bl, br], [tl, br, tr]]
                };

                for tri in split {
                    let normal = (tri[1] - tri[0]).cross(tri[2] - tri[0]).normalize();

                    // One sample at the centre decides the whole facet — the
                    // point of flat shading is that there is nothing to
                    // interpolate between its corners.
                    let mid = (tri[0] + tri[1] + tri[2]) / 3.0;
                    let (wx, wz) = (base.x + mid.x, base.y + mid.z);
                    let c = self.color(wx, wz, mid.y, normal);
                    // Vertex colours are consumed in linear space by the PBR
                    // shader.
                    let c = [
                        srgb_to_linear(c.x),
                        srgb_to_linear(c.y),
                        srgb_to_linear(c.z),
                        1.0,
                    ];
                    // UVs span the whole map, so a future overlay lines up
                    // across chunk boundaries.
                    let uv = [
                        (wx + half.x) / map_tiles.x as f32,
                        (wz + half.y) / map_tiles.y as f32,
                    ];

                    for corner in tri {
                        positions.push([corner.x, corner.y, corner.z]);
                        normals.push([normal.x, normal.y, normal.z]);
                        uvs.push(uv);
                        colors.push(c);
                    }
                }
            }
        }

        ChunkGeometry {
            positions,
            normals,
            uvs,
            colors,
        }
    }
}

/// One chunk's vertex buffers, as any renderer wants them: a triangle list,
/// three vertices per triangle in order.
///
/// Deliberately un-indexed: with no vertex shared between triangles an index
/// buffer would be 0, 1, 2, 3, … and save nothing.
pub struct ChunkGeometry {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Spanning the whole map rather than the chunk, so a future overlay lines
    /// up across chunk boundaries.
    pub uvs: Vec<[f32; 2]>,
    /// RGBA, already in linear space — see [`srgb_to_linear`].
    pub colors: Vec<[f32; 4]>,
}

/// One sRGB channel decoded to linear, the standard piecewise transfer
/// function. The palette is authored in sRGB and shaders blend in linear, so
/// the conversion happens here, once, as the geometry is built — every
/// renderer this feeds has to agree on it, and the game's tests hold it equal
/// to what Bevy's own colour types compute.
///
/// Takes a channel in [0, 1], which is all the palette ever holds. Below zero
/// this would carry the linear leg on down where a renderer hands the value
/// back untouched, so the equality the tests check is over that range and no
/// wider — nothing here has any business asking for more.
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
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
/// Noise gives shape, never proportion. A seed whose field happens to run low
/// makes a drowned map and one that runs high makes a plateau, and a sea level
/// or a mountain threshold hard-coded to suit one is wrong for the other — the
/// reason the ranges never appeared before was a mask cut at 0.55 on a field
/// whose 99th percentile is 0.38. Reading the field's own distribution and
/// solving for the numbers that hit [`LAND_FRACTION`] and [`MOUNTAIN_FRACTION`]
/// makes the targets true by construction, on every seed.
///
/// Above the waterline the mapping is a straight line — bent, on the drowned
/// seeds only, at one place and in one direction. The mountains are built into
/// the field rather than curved into it here.
///
/// A curve that steepens towards the top is the obvious way to raise peaks out
/// of a gentle field, and it is a trap. To lift the top of the field it has to
/// magnify whatever is up there, and what is up there is a ridged noise field
/// whose maximum is a crease — so the peaks come out as vertical knife blades.
/// Worse, it only works at all on seeds whose field happens to have a sharp
/// top: on the rest the fit has nothing to bite on and gives up, and the same
/// settings produce a two-hundred-metre range on one map and an eighty-metre
/// hummock on the next.
///
/// Keeping it linear means the mountains have exactly the shape the massif
/// gives them, and getting them to a consistent height is
/// [`TerrainGenerator::fit_range_height`]'s job instead — which is a scale on
/// one term, so it changes how high a range stands without touching how sharp
/// it is.
///
/// The one exception is the [`LOWLAND_FLOOR`]: a seed whose land median would
/// come out under it gets the segment *below* the median steepened until the
/// median lands on the floor. That is the opposite move from the trap above —
/// it magnifies the bottom of the field, never the ridged top — and it is
/// anchored on the land median, not on the mountain quantile that
/// [`TerrainGenerator::fit_range_height`] steers by. An earlier attempt hung a
/// fitted exponent on that same quantile and the two fits chased each other;
/// with separate anchors, raising the massif barely moves the median and the
/// two settle independently.
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
        let natural = slope * knee;
        let (knee_height, slope_low, slope_high) =
            if natural < lowland_floor && knee > 1e-4 && mountain - knee > 1e-4 {
                (
                    lowland_floor,
                    lowland_floor / knee,
                    (mountain_height - lowland_floor) / (mountain - knee),
                )
            } else {
                (natural, slope, slope)
            };

        // The same again below the waterline: the sea's median — slid to the
        // shallow side on the smallest maps, whose median is in the falloff
        // ring, see [`shoal_shift`] — and where the straight slope through
        // the deep anchor would put it. Only an anchor that comes out
        // shallower than the floor bends the line. The floor eases with the
        // slide: the shallower the anchored cells, the less depth they have
        // to be guaranteed.
        let deep = sea_level - quantile(DEEP_FRACTION);
        let depth_gain = MAX_DEPTH / deep.max(1e-4);
        let shallows_floor = SHALLOWS_FLOOR - 1.5 * shoal;
        let sea_knee = sea_level - quantile((1.0 - land) * (0.5 + 0.35 * shoal));
        let natural_depth = depth_gain * sea_knee;
        let (sea_knee_depth, depth_shallow, depth_deep) =
            if natural_depth < shallows_floor && sea_knee > 1e-4 && deep - sea_knee > 1e-4 {
                (
                    shallows_floor,
                    shallows_floor / sea_knee,
                    (MAX_DEPTH - shallows_floor) / (deep - sea_knee),
                )
            } else {
                (natural_depth, depth_gain, depth_gain)
            };

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
/// fall ever reaches the sea, so every gentle rise inland reads as a shore and
/// gets built into one. A distance transform over the whole map answers the
/// question properly, and costs almost nothing — one pass over a grid of
/// [`COAST_GRID`]-metre samples per map, then an O(1) lookup per vertex.
///
/// Where the water is comes from the landform alone, before any of the detail
/// layers. That keeps the field smooth, and keeps it from depending on the very
/// coastline it is about to shape. The visible waterline then wanders either
/// side of the one measured here, which is what leaves the odd cliff standing
/// back off a low rock platform — no bad thing.
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
    /// riddled with inland water: a third of the map is land, sea level is
    /// fitted high enough that hollows flood, and the result is sounds and
    /// lagoons all over the interior. Measured from all of it, a range is
    /// forbidden its height for having a pond beside it, which is neither what
    /// erosion says nor anything a landscape does — the pond is a feature *of*
    /// the upland, not a coast it has to climb from. On a two-kilometre map
    /// that was the difference between one range and several: the interior
    /// massifs each had inland water within a hundred metres and were held
    /// down as if they stood on a beach.
    ///
    /// What separates the two is **size**, and deliberately not whether the
    /// water joins the sea. Reaching the frame was the first thing tried and
    /// it is a property of the map's topology, which is a yes or no — so it
    /// changed the map in steps. A lagoon joined to the sea by a single cell
    /// of strait counted wholly as sea; a map grown by one notch silted the
    /// strait up and the same lagoon counted wholly as ground, handing every
    /// point behind it the whole width of the lagoon in distance at once.
    /// Measured over 128-metre steps of size, the largest distance on one seed
    /// went 101, 131, 288, 320, 340 m — a 157-metre jump for a 128-metre step,
    /// which took the ceiling over that region from about 85 m to about 175 m
    /// and turned a headland into a range. Nothing about a strait one cell
    /// wide should decide how tall a mountain half a kilometre away may be.
    ///
    /// Size has no such cliff: a pool grows and shrinks by a cell at a time as
    /// the map moves under it, so the field moves with it. It is also the
    /// better rule on its own merits. Every drop of water on these maps sits
    /// at the one fitted sea level, so a big enclosed lagoon *is* at base
    /// level whether or not a spit of land closes it off, and ground behind it
    /// really has only climbed from its shore.
    ///
    /// A pool's say is graded rather than granted: full sea seeds the
    /// transform at zero, a pond seeds it [`INLAND_REACH`] out — far enough
    /// that the ceiling it implies is above anything the landform reaches, so
    /// it binds nothing — and the sizes between seed proportionally. The
    /// chamfer then relaxes each seed against every better one, so a pool a
    /// short way off a real coast is measured from that coast rather than from
    /// its own reading.
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
        let mut cells = seeds;

        chamfer(&mut cells, dims);
        // The transform is exact, and exact is the problem: everywhere two
        // wavefronts meet — down the middle of every neck and strait, and in
        // a fan of branches behind every scalloped stretch of coast — the
        // field folds in a sharp crease. Everything built *from* the field
        // (cliff faces most of all, being the field times a steep pitch)
        // prints those creases into the ground as dead-straight hairline
        // ridges. A little smoothing rounds the creases off while leaving the
        // field, which is read at coastline scale, effectively unchanged.
        // Four passes approximate a Gaussian a couple of cells wide — enough
        // that the shore-character displacement sweeps across a crease instead
        // of leaping it.
        for _ in 0..4 {
            blur(&mut cells, dims);
        }
        Self {
            field: GridField {
                cells,
                dims,
                origin,
            },
        }
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

/// Two-pass chamfer distance transform over a grid of zeroes (the sea) and
/// infinities (everything else), leaving each cell holding its distance from
/// the nearest zero, in cells.
///
/// Weighting a diagonal step by √2 keeps the result close enough to a true
/// Euclidean distance for something only ever used to shape and fade a coast,
/// at two linear passes rather than a search.
fn chamfer(cells: &mut [f32], dims: (usize, usize)) {
    const DIAGONAL: f32 = std::f32::consts::SQRT_2;
    let (nx, nz) = dims;

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
    // ...then backward over the mirror image of the same neighbourhood, which
    // is what lets distance travel in every direction.
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

/// A grid of values over the map at [`COAST_GRID`] spacing, read back with
/// bilinear interpolation, clamped to its edges outside it.
#[derive(Default)]
struct GridField {
    cells: Vec<f32>,
    dims: (usize, usize),
    /// World coordinate of the first cell.
    origin: Vec2,
}

impl GridField {
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
}

/// Replaces every cell with the largest value within `radius` cells of it —
/// a square sliding-window maximum, done as two 1D passes with a monotonic
/// deque, so the whole thing is linear in the grid size.
fn window_max(cells: &mut [f32], dims: (usize, usize), radius: usize) {
    let (nx, nz) = dims;
    let window = |line: &mut Vec<f32>, out: &mut Vec<f32>| {
        // Deque of indices whose values are decreasing; the front is always
        // the maximum of the window around `i`.
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

/// One pass of 3×3 box blur over a grid, in place. Used to take the creases
/// off the distance field; run twice it approximates a small tent kernel.
/// Share of the ground within `radius` cells of each cell that is water, from a
/// 0-or-1 mask of it — a separable box blur, done with a running sum so the
/// radius costs nothing.
///
/// Everything off the edge of the grid counts as water. The map ends in open
/// sea on every seed, so a window hanging over the frame really is looking at
/// sea; and counting it that way is what keeps a coast near the frame reading
/// the same whatever size map is drawn around it.
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

fn blur(cells: &mut [f32], dims: (usize, usize)) {
    let (nx, nz) = dims;
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
/// A beach and a cliff are not made the same way, because they are not the same
/// kind of claim about the ground.
///
/// A beach is a claim about how *gently* the land shelves, so it is a remap of
/// height onto height: a power curve across the coastal band, deferring the
/// climb and spreading it out. Being a power curve on the band, it agrees with
/// the untouched field at both ends of it, so nothing discontinuous happens
/// where the reshaping stops.
///
/// A cliff is a claim about the *angle of a face*, and no remap of height can
/// make one. Where the natural ground gains half a metre in ten there is simply
/// no height to redistribute into a wall — squeeze it and all that comes out is
/// a step one facet wide, which from a camera pitched well down is a dark line. So a
/// cliff is built outwards from the water instead: the ground is lifted to meet
/// a fixed rise per metre of distance from it, up to [`CLIFF_HEIGHT`], and left
/// alone again wherever the real landscape is already higher than that.
///
/// Below the waterline both are power curves, because down there the only
/// question is how quickly the bed falls away, and height alone answers it.
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
        return -MAX_DEPTH * t.powf(exponent);
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
    // gives a beach whose width depends on how steep the hill behind it
    // happens to be, so the taller the island the thinner its beaches, until on
    // a mountainous map there are none left and the slope rule paints grey
    // rock along every shore.
    if character < 0.0 {
        // Steepening with distance rather than a straight ramp. A straight one
        // holds the ground flat right out to the edge of the coast's reach and
        // then lets go of it all at once, which leaves a scarp running along
        // the back of every beach — a flat green shelf, then a wall. Curving it
        // up means the apron has already climbed to meet the hillside by the
        // time it stops applying, and the two join without a seam.
        // And released well before the coast's full reach. The apron *cuts the
        // ground down*, so wherever it stops cutting it leaves what it did not
        // cut standing: carried too far inland it shaves the land either side
        // of the ridge line midway between two coasts and leaves that ridge
        // behind as a wall running inland, which is the one landform no beach
        // has ever produced.
        let apron = BEACH_RISE * distance * (1.0 + distance / APRON_KNEE);
        let within = 1.0 - smoothstep(BEACH_REACH * 0.45, BEACH_REACH, distance);

        // How much the apron is allowed to take off, tapering to nothing as the
        // ground approaches the height at which the test above stops touching
        // it at all. Without the taper the two meet as a step: land a
        // centimetre under the cliff height is cut down to the apron and land a
        // centimetre over is left alone, which lays a six-metre wall along the
        // eleven-metre contour and runs it inland from every beach. Since the
        // shore itself is only a metre or two up, capping the cut costs the
        // beach nothing where a beach actually is.
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

/// Heightfield normal from the four neighbouring samples, one tile out.
fn normal_from_neighbours(left: f32, right: f32, down: f32, up: f32) -> Vec3 {
    Vec3::new(left - right, 2.0 * TILE_SIZE, down - up).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // The massif fit still puts every seed's raw summit on the same
        // number — that is what stopped one map feeling tame and the next
        // absurd, when the height a seed reached was the widest-spread number
        // on the map. What has changed is that reaching it is no longer a
        // seed's to decide: [`TerrainGenerator::ceiling`] holds every point
        // under what its distance from the open sea has earned, so a seed
        // whose land is broad keeps most of the fitted height and one whose
        // land is all coast keeps less.
        //
        // So there is no constant across seeds any more — not the summit, and
        // not the grade either, which runs over a fair spread depending on how
        // close a seed's massif happens to fall to its best ground. What holds
        // is weaker and worth more: a summit stands within reach of what its
        // own ground has earned, both ways. That is the property the old
        // fixed-height test could not express — it passed happily on a seed
        // with a hundred and twenty metres of rock fifty metres from the sea.
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
        // point: one summit reached [`HEIGHT_SCALE`] and the rest sat lower by
        // pure luck of the mask field, so a large map came out as many grey
        // lumps under one white cap. The local ceiling — [`MASSIF_EQUALITY`] —
        // is what entitles every range to a summit of its own.
        //
        // Read against [`MOUNTAIN_HEIGHT`], the height the palette starts
        // drawing ground as mountain, and not against a share of
        // [`HEIGHT_SCALE`]. Parity was the whole story when a massif could
        // reach full height wherever the mask happened to put it. It is not
        // now: [`TerrainGenerator::inland`] rations height by how much room a
        // range has behind it, so the pecking order runs on how far each
        // massif sits from the sea. That is a difference the map is *meant*
        // to show — this seed's third range was 101 m of rock standing 81 m
        // from the water, which is a fifty-degree climb from sea to summit and
        // nothing any coast does — so what is worth guarding is that several
        // ranges are real mountains, not that they are all nearly as tall as
        // each other.
        // Counted over the whole seed list and not seed by seed, which is what
        // buys back the strength given up by reading against the lower line.
        // A per-seed floor of three has no margin left in it: measured, the
        // eight seeds get 3, 3, 4, 4, 3, 7, 3, 5 summits, so three of them sit
        // exactly on such a bar and any tuning that costs one seed one summit
        // fails the test without the maps having got worse. The total has room
        // to move — thirty-two against a bar of twenty-four — while still
        // catching the thing this is really about, which is a big map coming
        // out as one mountain and a lot of lumps.
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
        // Measured against the all-water field rather than against a number of
        // metres, because some movement is honest: sea level is refitted per
        // map and the falloff is relative to the frame, so growing a map does
        // genuinely redraw its coast a little, and both fields inherit that.
        // What must not happen is this field adding a step the other does not
        // have — which is what telling sea from pond by whether the water
        // reaches the frame used to do, that being a fact about the map's
        // topology and so a yes or no. One cell of strait silting up moved the
        // deepest point on one seed from 131 m to 288 m across a single
        // 128-metre notch, against 126 m to 179 m for the all-water field
        // beside it.
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
            let distance = step as f32 * MESH_STEP as f32;
            let rise = shape_coast(0.0, distance + MESH_STEP as f32, 1.0, 1.0)
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
    fn a_chunk_is_two_flat_triangles_per_quad() {
        let (config, gen) = generator(2, 2, 1);
        let geometry = gen.build_chunk(UVec2::ZERO, config.tiles());

        // Six vertices per quad: two triangles, sharing nothing.
        let quads = (CHUNK_TILES / MESH_STEP) as usize;
        assert_eq!(geometry.positions.len(), quads * quads * 6);
        for length in [
            geometry.normals.len(),
            geometry.uvs.len(),
            geometry.colors.len(),
        ] {
            assert_eq!(length, geometry.positions.len());
        }
    }

    #[test]
    fn every_triangle_is_one_flat_facet() {
        let (config, gen) = generator(2, 2, 1);
        let geometry = gen.build_chunk(UVec2::ZERO, config.tiles());

        let (normals, colors) = (geometry.normals, geometry.colors);

        for tri in 0..normals.len() / 3 {
            let i = tri * 3;
            for corner in 1..3 {
                assert_eq!(
                    normals[i],
                    normals[i + corner],
                    "triangle {tri} has a varying normal"
                );
                assert_eq!(
                    colors[i],
                    colors[i + corner],
                    "triangle {tri} has a varying colour"
                );
            }
            // A heightfield can never overhang, so every facet faces upwards.
            assert!(normals[i][1] > 0.0, "triangle {tri} faces downwards");
        }
    }

    #[test]
    fn the_palette_is_small_and_the_colours_are_never_blended() {
        // The whole point of the styling: no gradients anywhere on the ground.
        // Every facet on a whole map has to land on one of the palette entries,
        // times one of three shade steps.
        let (config, gen) = generator(4, 4, 77);
        let mut seen = std::collections::HashSet::new();

        for cz in 0..config.chunks.y {
            for cx in 0..config.chunks.x {
                let geometry = gen.build_chunk(UVec2::new(cx, cz) * CHUNK_TILES, config.tiles());
                for c in geometry.colors {
                    seen.insert(c.map(|v| v.to_bits()));
                }
            }
        }

        let palette = [
            SEABED,
            SHALLOW,
            SAND,
            SHINGLE,
            FOREST,
            GRASS_DARK,
            GRASS,
            GRASS_LIGHT,
            MEADOW,
            HEATH,
            UPLAND,
            FELL,
            ROCK,
            ROCK_DARK,
            SCREE,
        ];
        assert!(
            seen.len() <= palette.len() * 3,
            "{} distinct colours is more than the palette allows",
            seen.len()
        );
    }

    #[test]
    fn chunk_positions_are_local_and_offset_by_the_placement() {
        let (config, gen) = generator(2, 2, 1);
        let origin = UVec2::splat(CHUNK_TILES);
        let positions = gen.build_chunk(origin, config.tiles()).positions;

        // First vertex sits at the chunk's own origin, not the map's.
        assert_eq!(positions[0][0], 0.0);
        assert_eq!(positions[0][2], 0.0);

        // And its height matches the world position placing the chunk puts it at.
        let half = config.half_extent();
        let expected = gen.height(
            origin.x as f32 * TILE_SIZE - half.x,
            origin.y as f32 * TILE_SIZE - half.y,
        );
        assert_eq!(positions[0][1], expected);
    }

    #[test]
    fn neighbouring_chunks_agree_along_their_shared_edge() {
        // On a rectangular map, whose two chunks sit side by side.
        let (config, gen) = generator(2, 1, 5);

        let left = gen.build_chunk(UVec2::ZERO, config.tiles());
        let right = gen.build_chunk(UVec2::new(CHUNK_TILES, 0), config.tiles());

        // Every vertex sitting on the shared plane, as (z, height) pairs. Flat
        // shading repeats each corner across the triangles that touch it, so
        // these need deduplicating before the two sides can be compared.
        let edge = |geometry: &ChunkGeometry, local_x: f32| {
            let mut points: Vec<(f32, f32)> = geometry
                .positions
                .iter()
                .filter(|p| p[0] == local_x)
                .map(|p| (p[2], p[1]))
                .collect();
            points.sort_by(|a, b| a.partial_cmp(b).expect("no NaN in a heightfield"));
            points.dedup();
            points
        };

        let seam = edge(&left, CHUNK_TILES as f32);
        assert_eq!(seam.len(), (CHUNK_TILES / MESH_STEP + 1) as usize);
        assert_eq!(seam, edge(&right, 0.0), "chunks disagree along their seam");
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

            let (mut land, mut mountain, mut edges) = (0u32, 0u32, 0u32);
            let mut peak = 0.0f32;
            let mut heights = Vec::new();
            for iz in (0..size as i32).step_by(2) {
                for ix in (0..size as i32).step_by(2) {
                    let h = at(ix, iz);
                    peak = peak.max(h);
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

            let cells = (size / 2) * (size / 2);
            let area = land as f32 * 4.0;
            let coast = edges as f32 * 2.0;
            heights.sort_by(|a, b| a.partial_cmp(b).expect("no NaN in a height field"));
            let decile = |p: usize| heights[heights.len() * p / 100] as u32;
            println!(
                "seed {seed:>9}  land {:4.1}%  mountain {:4.1}% of land  peak {peak:5.0} m  \
                 shoreline index {:.2}  slope p50/p90/p99 {:>2}/{:>2}/{:>2} deg  \
                 land height p50/p90 {:>3}/{:>3} m",
                land as f32 / cells as f32 * 100.0,
                mountain as f32 / land.max(1) as f32 * 100.0,
                coast / (2.0 * (std::f32::consts::PI * area).sqrt()),
                slope(50),
                slope(90),
                slope(99),
                decile(50),
                decile(90),
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

    /// Single-threaded cost of generating a whole map, chunk by chunk. The app
    /// itself spreads these across the task pool.
    #[test]
    #[ignore]
    fn mesh_build_cost() {
        for chunks in [
            UVec2::new(4, 4),
            UVec2::new(8, 8),
            UVec2::new(12, 8),
            UVec2::new(16, 16),
            UVec2::new(32, 32),
        ] {
            let config = MapConfig { chunks, seed: 1 };
            let generator = TerrainGenerator::new(&config);

            let start = Instant::now();
            let mut tris = 0;
            for cz in 0..config.chunks.y {
                for cx in 0..config.chunks.x {
                    let geometry =
                        generator.build_chunk(UVec2::new(cx, cz) * CHUNK_TILES, config.tiles());
                    tris += geometry.positions.len() / 3;
                }
            }
            let elapsed = start.elapsed();

            println!(
                "{:5}x{} m  {:>4} chunks  {tris:>11} tris  {elapsed:>8.0?}",
                config.tiles().x,
                config.tiles().y,
                config.chunks.x * config.chunks.y
            );
        }
    }
}
