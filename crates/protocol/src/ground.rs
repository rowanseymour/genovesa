//! The ground, as it travels: the grid a chunk is drawn on, the small palette
//! it is painted from, and the payload one chunk of it encodes to.
//!
//! A client generates nothing, so everything it needs in order to *draw* a
//! chunk is spelled out here in a form that says nothing about how the ground
//! was arrived at: corner heights on a fixed grid, and one material per cell.
//!
//! The **grid is the format**. [`CELL_METRES`] is how finely the ground is
//! *sampled*, and moving it moves every payload, so it lives here rather than
//! in the generator that samples it. How finely the ground is *drawn* is not
//! the wire's business at all — see [`CELL_COUNT`]. A client that lays one
//! lozenge over sixteen cells out at half a kilometre is throwing away
//! samples it was sent, which is the whole of what level of detail is, and
//! the format neither knows nor minds.
//!
//! What travels for the ground's *appearance* is a [`Material`] per cell, one
//! byte, naming a substance rather than a colour. That much is the format: two
//! builds numbering the materials differently would read each other's beaches
//! as moorland.
//!
//! [`Material::color`] is **not** the format. It is a reference rendering — the
//! flat palette this world is authored in, which mapgen draws maps out of and
//! a client may draw ground out of if it wants to. A client that paints its
//! ground some other way, from textures say, is not disagreeing with anything;
//! it is answering a question the wire never asked.
//!
//! The same goes for **standing water**. The sea is a plane at zero any client
//! can draw, but a lake stands at a height decided by a rim saddle that may be
//! half a kilometre away, so where a chunk carries water its surface crosses
//! the wire as a second grid — see [`ChunkPayload::water`].
//!
//! And for **sunlight**. The terrain is fixed and the sun rides one arc — see
//! [`crate::towards_the_sun`] — so whether a corner stands in the sun is a
//! function of the hour alone, and the whole day's answer is two phases: when
//! it first sees the sun and when it loses it. What shadows a corner at dawn
//! may be a ridge many chunks away that a client has never been sent, so only
//! whoever holds the whole island can say — the generator bakes the pair per
//! corner and it crosses the wire as [`ChunkPayload::lit`]. It is what lets a
//! client draw the terrain's own shadows without a shadow map. The pairs read
//! back anywhere on the grid — [`lit_across`], which owns what a never-lit
//! corner does to its neighbours — and how finely a drawing end cuts its
//! shadow out of them is its own business, like the grid itself. The same
//! pair asked with `phase + 0.5` answers for the moon, which rides the same
//! arc half a day out of phase.
//!
//! And for **shelter**. Land takes the wind out of the water behind it, and
//! the headland doing it may be chunks upwind of the bay that is calmed — so
//! this is the sunlight problem again, and it crosses the wire for the same
//! reason: only whoever holds the whole island can say. What travels is a
//! coarse lattice of exposures, one per [`BEARINGS`] compass points, read
//! back anywhere by [`shelter_across`].
//!
//! It rides beside a chunk's ground rather than inside it — see
//! [`crate::ToClient::Chunk`] — because the two are not the same question.
//! Most of an island's own chunks are bare ocean floor that no client needs
//! a mesh for, and they are exactly the water its headlands shelter: a lee
//! carried inside the payload would mean sending eighty kilobytes of flat
//! bed to say something a few hundred bytes says, for every skirt chunk of
//! every island in the world.
//!
//! Coarse on purpose, and coarse on one axis only. Shelter is a wake: it
//! varies smoothly across the water, so [`SHELTER_METRES`] samples it far
//! more sparsely than the ground is drawn and loses nothing. It does *not*
//! vary smoothly with the wind's bearing — swing the wind thirty degrees and
//! a headland stops covering you outright — so the bearings are where the
//! resolution goes. Four of them would be worse than useless: two islands
//! with a strait between them read as sheltered from north and from east,
//! and a plain blend would call the north-east channel sheltered too, when
//! it is the one direction the wind comes howling down.
//!
//! **No angle is taken anywhere** in reading it back. `atan2` is a libm
//! function like the sine the weather goes out of its way to avoid: not
//! correctly rounded, and adrift between platforms — and the lattice is read
//! on both sides of the wire, so a server and a client disagreeing in the
//! last bit would be a boat drawing a wind it is not sailing. The blend
//! between two bearings runs off the ratio of the wind's smaller component
//! to its larger, which is exact, monotone across the sector, and lands on
//! `0` and `1` at the sector's own ends. It is `tan` rather than the angle,
//! so the sweep across a sector is very slightly uneven; against a field
//! this coarse that is nothing, and against a wind whose bearing has to mean
//! the same thing on two machines it is the point.

use glam::{IVec2, Vec2, Vec3};

/// Metres along the edge of one world chunk — the unit a client asks for
/// ground in, and the grid every island is laid out on.
pub const CHUNK_METRES: f32 = 128.0;

/// The chunk a world point stands in. Sea level is zero and the ground plane
/// runs in metres, so this is the whole of the coordinate system a client
/// needs in order to know what to ask for.
pub fn chunk_at(point: Vec2) -> IVec2 {
    (point / CHUNK_METRES).floor().as_ivec2()
}

/// Which way north lies on the ground plane: the direction a map drawn in
/// plan puts at the top of the page.
///
/// Nothing in the world's arithmetic cares — the direction is a convention,
/// not a computation — but it is a convention two clients have to share, or
/// their compasses would disagree about the same sea. So it is written down
/// here with the rest of the coordinate system rather than left for each
/// client to pick.
pub const NORTH: Vec2 = Vec2::NEG_Y;

/// How deep the open ocean's floor lies, in metres below sea level.
///
/// This is what an answer of *no ground* means. A chunk with no payload is
/// not "unknown" and not "nothing" — it is flat floor at exactly this depth,
/// which a client draws as a plane rather than as a mesh of thirty-odd
/// thousand identical triangles. Every island's own sea bed is clamped to the same
/// level, so the plane and the meshes meet along every coast with nothing to
/// show for it; a client drawing its backdrop at some other depth would print
/// a step around every island in the world.
pub const OCEAN_DEPTH: f32 = 10.0;

/// The deepest water a boat's anchor holds in, in metres.
///
/// Set short of [`OCEAN_DEPTH`] on purpose: the open ocean's floor is out of
/// the anchor's reach everywhere, so a ship can only be left riding at anchor
/// over an island's own shelf — never abandoned in the middle of the sea.
/// The server holds the line — it is what grants leaving a helm — and it is
/// written here rather than there because a client wants the same number, to
/// let a key that cannot be granted do nothing instead of asking.
pub const ANCHOR_DEPTH: f32 = 8.0;

const _: () = assert!(ANCHOR_DEPTH < OCEAN_DEPTH);

/// Metres between the corners the ground is drawn from.
///
/// The height field behind it is continuous, so this is only how finely it
/// gets *drawn*. It was 2 m for a long time — big flat-shaded facets read as
/// deliberate shapes — and moving to 1 m traded that cut-gem quality for
/// ground the field can actually articulate: the generator now carries
/// detail down to a few metres' wavelength, which a 2 m mesh could only
/// alias. The move is wire-wide: it quadrupled the payload, which is what
/// pushed the frame prefix to a u32.
pub const CELL_METRES: f32 = 1.0;

/// Cells along one edge of a chunk's ground grid.
pub const CELLS: usize = (CHUNK_METRES / CELL_METRES) as usize;

/// Corners along one edge of that grid — one more than the cells, since the
/// corners at both ends are shared.
pub const CORNERS: usize = CELLS + 1;

/// Cells in one chunk, each of which carries a [`Material`].
///
/// The wire says what a square metre of ground *is* and stops there. How that
/// square is drawn — two triangles split one way or the other, a textured
/// quad, nothing at all at distance — is the drawing end's own business, and
/// the format deliberately holds no opinion about it. That is what lets one
/// client flat-shade the ground and another paint it while both draw the same
/// world.
pub const CELL_COUNT: usize = CELLS * CELLS;

// --- Shelter ----------------------------------------------------------------

/// Compass points the shelter lattice answers for, evenly spaced and
/// starting at [`NORTH`], running clockwise the way a card does: N, NE, E,
/// SE, S, SW, W, NW.
///
/// Eight rather than four because a blend across 90° smears a headland's
/// cover over a quadrant and inverts outright at a strait — the module doc
/// has the case. Eight puts the samples 45° apart, which a plain blend
/// between neighbours reconstructs honestly enough that nothing cleverer
/// than [`shelter_across`] is called for.
pub const BEARINGS: usize = 8;

/// The eight compass points as unit vectors, in the order the lattice
/// stores them — the one definition of that order, which the generator
/// writes slots by and [`shelter_across`] reads them by.
///
/// Built from a square root rather than a sine: this feeds a bake that is
/// under the seed digests' promise of the same island on every machine, and
/// `sqrt` is correctly rounded where `sin` is not.
pub fn bearings() -> [Vec2; BEARINGS] {
    let d = 0.5f32.sqrt();
    [
        Vec2::new(0.0, -1.0),
        Vec2::new(d, -d),
        Vec2::new(1.0, 0.0),
        Vec2::new(d, d),
        Vec2::new(0.0, 1.0),
        Vec2::new(-d, d),
        Vec2::new(-1.0, 0.0),
        Vec2::new(-d, -d),
    ]
}

/// Metres between points of the shelter lattice.
///
/// Sixteen times [`CELL_METRES`], and deliberately nothing like as fine. A
/// wake is a smooth field — walk twenty metres further into a bay and the
/// cover changes by a little, never by a lot — so sampling it at the
/// ground's own density would be storing the same number over and over.
/// What that coarseness costs is a headland's *edge*, which arrives as a
/// gradient a chunk-sixteenth wide rather than as a line; on water, where
/// there is no relief for the eye to hold it against, that is what it should
/// look like anyway.
pub const SHELTER_METRES: f32 = 16.0;

/// Cells along one edge of the shelter lattice.
pub const SHELTER_CELLS: usize = (CHUNK_METRES / SHELTER_METRES) as usize;

/// Points along one edge of it — one more than the cells, exactly as
/// [`CORNERS`] is, and for a reason worth more than the symmetry: the extra
/// point is the chunk's far edge, which is its neighbour's near edge, so the
/// two chunks either side of a boundary carry the *same* sample there and a
/// blend across the seam is continuous. Without it a boat crossing a chunk
/// line would find the wind step.
pub const SHELTER_CORNERS: usize = SHELTER_CELLS + 1;

/// Points on one chunk's shelter lattice.
pub const SHELTER_COUNT: usize = SHELTER_CORNERS * SHELTER_CORNERS;

/// What a point wide open to the wind stores — the top of the byte, so that
/// the sea a client has not been told about, which reads as untouched
/// ground, is also the sea nothing is sheltering.
pub const EXPOSED: u8 = u8::MAX;

/// The least of the wind any lee leaves standing, as a fraction — the floor
/// a generator never stores a byte below, so the deepest lee on the wire is
/// `LEAST_EXPOSURE * 255` and not zero.
///
/// The floor that keeps a hull sailing is [`crate::LIGHT_AIR`], in metres
/// per second. This one is smaller-scale and about the *drawing*: it stops
/// the deepest lee reading as a hole in the weather, so a gale behind a
/// mountain is a quiet corner of a gale rather than a different day. Here
/// rather than in the generator because the client's curves start from it —
/// the sea's own floor sits on top of this one, and a reader tuning either
/// needs to know both are there.
pub const LEAST_EXPOSURE: f32 = 0.18;

/// One lattice point's exposure to a wind blowing toward each of
/// [`BEARINGS`], as a fraction of the open sea's wind over `255` —
/// [`EXPOSED`] for water nothing stands upwind of, and smaller the deeper
/// into some island's lee the point lies.
///
/// A *fraction of the wind* rather than a shadow depth, because that is the
/// quantity both ends want and neither should be deriving: how a wake's
/// height above the water turns into air a sail can hold is the generator's
/// question, settled once where the terrain is, not twice where it is not.
pub type Exposure = [u8; BEARINGS];

/// The exposure somewhere inside a lattice cell, blended across the four
/// points around it and across the two bearings the wind falls between —
/// `points` in the south-west, south-east, north-west, north-east order the
/// material grid uses, `at` in lattice widths from the south-west one, and
/// `toward` the way the wind is blowing, as [`crate::ToClient::Weather`]
/// gives it. Comes back in `0.0..=1.0`. Bilinear first and bearings second;
/// both are weighted sums with weights the other does not touch, so the
/// order is arithmetic and not a shortcut. Why no angle is taken is the
/// module header's.
pub fn shelter_across(points: [Exposure; 4], at: Vec2, toward: Vec2) -> f32 {
    let weights = [
        (1.0 - at.x) * (1.0 - at.y),
        at.x * (1.0 - at.y),
        (1.0 - at.x) * at.y,
        at.x * at.y,
    ];
    let mut slots = [0.0f32; BEARINGS];
    for (point, weight) in points.into_iter().zip(weights) {
        for (slot, stored) in slots.iter_mut().zip(point) {
            *slot += stored as f32 * weight;
        }
    }
    exposure_toward(slots, toward) / EXPOSED as f32
}

/// One lattice point's exposure to a wind blowing `toward`, blended between
/// the two bearings either side of it — still on the stored `0..=255` scale.
/// See [`shelter_across`], which is the whole of why this takes no angle.
fn exposure_toward(slots: [f32; BEARINGS], toward: Vec2) -> f32 {
    // North is up the card, so the quadrant is read off the wind's east and
    // *north* components — the second being the negative of its y, since
    // NORTH is NEG_Y. Each quadrant runs from one cardinal to the next, and
    // both components are non-negative once measured from its own corner.
    let north = -toward.y;
    let (base, from, to) = match (toward.x >= 0.0, north >= 0.0) {
        (true, true) => (0, north, toward.x),
        (true, false) => (2, toward.x, -north),
        (false, false) => (4, -north, -toward.x),
        (false, true) => (6, -toward.x, north),
    };

    // Which half of the quadrant, and how far across it. The half is decided
    // by which component is larger, so the ratio below is always the smaller
    // over the larger and never leaves `0.0..=1.0`; and the second half is
    // walked backwards, since there the ratio shrinks as the wind swings on.
    let (a, b, t) = if from >= to {
        (base, base + 1, if from > 0.0 { to / from } else { 0.0 })
    } else {
        (base + 1, (base + 2) % BEARINGS, 1.0 - from / to)
    };
    slots[a] * (1.0 - t) + slots[b] * t
}

// --- Heights ----------------------------------------------------------------

/// The height a stored zero means, in metres. Well below the deepest sea bed
/// the world builds, so no honest ground ever clamps against it.
pub const HEIGHT_FLOOR: f32 = -16.0;

/// Metres per step of a stored height — two centimetres, a fiftieth of the
/// smallest thing anyone can see at the closest zoom.
///
/// Sixteen bits at this step reach from [`HEIGHT_FLOOR`] to something over a
/// kilometre, and the tallest ground the generator builds is a few hundred
/// metres, so the ceiling is margin rather than a limit anything approaches.
pub const HEIGHT_STEP: f32 = 0.02;

/// A height as it travels. Rounded to the nearest step and held inside the
/// range sixteen bits can say — ground outside that range would be a broken
/// generator, and clamping is the one answer that cannot make a frame
/// unreadable.
pub fn quantize(height: f32) -> u16 {
    let steps = (height - HEIGHT_FLOOR) / HEIGHT_STEP;
    // Written as a match on the ordering rather than as comparisons, because
    // a NaN height has to land somewhere and `None` is the only branch that
    // says so out loud. It goes to the floor, which is where clamping would
    // have put it and the only answer that is not a panic.
    match steps.partial_cmp(&0.0) {
        None | Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal) => 0,
        Some(std::cmp::Ordering::Greater) if steps >= u16::MAX as f32 => u16::MAX,
        Some(std::cmp::Ordering::Greater) => steps.round() as u16,
    }
}

/// What a stored height means, in metres.
///
/// One multiply and one add, both pinned by IEEE 754, so every machine reading
/// a payload puts the ground in exactly the same place — which is the whole
/// reason heights travel as integers rather than as floats that a sender might
/// have rounded differently on the way in.
pub fn dequantize(stored: u16) -> f32 {
    HEIGHT_FLOOR + stored as f32 * HEIGHT_STEP
}

// --- The palette ------------------------------------------------------------

/// The ground materials. Small and flat on purpose — every cell of ground is
/// exactly one of these, so the whole world is made of twenty-three
/// substances. In the reference palette they are saturated well past anything
/// natural, because flat shading has no texture or gradient to carry the
/// picture and the colour has to do that work on its own.
///
/// They are numbered in the order the ground climbs, sea bed to summit, and
/// within each zone from its shadiest cover to its barest — so the palette
/// below reads as a section through an island, and a material's number says
/// roughly where it is found.
///
/// The order is also the wire's: a material travels as its own number, so
/// anything inserted, shuffled or removed repaints the world of every build
/// that disagrees. That is a change to the format — re-record
/// `the_wire_is_a_format` and the map digests, and rebuild both ends together,
/// because a client that has never heard of a material cannot draw the cell it
/// names. Appending would dodge the repaint, and is still the wrong answer: a
/// number that no longer says where the material is found costs more than a
/// re-record nobody is left running to be broken by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Material {
    /// The deep bed, seen through the water.
    Seabed = 0,
    /// The bright shelf that gives a coast its turquoise ring.
    Shallow = 1,
    Sand = 2,
    /// Pebble and boulder foreshore. Warmer and lighter than [`Material::Rock`],
    /// so a shingle beach reads as its own thing next to the cliffs rather
    /// than as more of them.
    Shingle = 3,
    /// Dry brush: the darkest cover the arid coastal country carries, and the
    /// only green in it. Warm where [`Material::Heath`] is cool, the two being
    /// the shadiest cover of their own zone and never seen at one height.
    Scrub = 4,
    /// Sun-bleached grass, standing between the brush and the bare ground.
    Parched = 5,
    /// Bare dry earth, which is what the arid zone comes to where nothing
    /// holds. Browner and darker than [`Material::Sand`]: a beach is washed,
    /// and this is only unwatered.
    Dust = 6,
    Forest = 7,
    GrassDark = 8,
    Grass = 9,
    GrassLight = 10,
    Meadow = 11,
    /// The closed canopy of the humid country above the grassland, and
    /// [`Material::Canopy`] its lighter crowns. Deeper than
    /// [`Material::Forest`], which is the same woodland thinning out as it
    /// runs down into the grass — so the three read as one forest getting
    /// wetter uphill rather than as two.
    Jungle = 12,
    Canopy = 13,
    /// Moorland, above the trees and below the bare rock. [`Material::Heath`]
    /// and [`Material::Fell`] are what the darkest and lightest cover of the
    /// wet forest below turns into as it climbs — the one still half green,
    /// the other already most of the way to stone — so that the upland reads
    /// as the same country drained of colour rather than as a different map
    /// laid over the top.
    Heath = 14,
    Upland = 15,
    Fell = 16,
    Rock = 17,
    RockDark = 18,
    /// Bare stone bleached by the weather — the palest the summits get.
    Scree = 19,
    /// The bed of standing fresh water, deep enough to be dark. Green where
    /// [`Material::Seabed`] is blue, and darker than it: a lake bottoms out in
    /// silt and drowned vegetation rather than in sand, and it is what a lake
    /// is *seen through* that has to say fresh water rather than sea.
    Silt = 20,
    /// The weedy shallows of a lake — what [`Material::Shallow`] is to the sea,
    /// except that it deliberately refuses the turquoise. A ring of bright
    /// water is the strongest thing that says *coast* in this palette, so a
    /// lake wearing one reads as an arm of the sea that happens to be inland.
    Shoal = 21,
    /// The margin a lake leaves around itself: reed, mud and wet ground, from
    /// just under the waterline to just above it. Takes the place a beach
    /// holds on the sea coast, and is dull and dark where sand is bright —
    /// fresh water has no surf to wash a shore clean.
    Marsh = 22,
}

/// The sRGB the materials stand for, in the order they are numbered.
const PALETTE: [Vec3; 23] = [
    Vec3::new(0.16, 0.34, 0.38), // Seabed
    Vec3::new(0.46, 0.68, 0.62), // Shallow
    // A step darker than it once was (0.90, 0.83, 0.58): the surf paints
    // near-white foam along the waterline now, and sand pale enough to
    // shoulder it read as more foam rather than as the beach under it.
    Vec3::new(0.86, 0.78, 0.52), // Sand
    Vec3::new(0.70, 0.65, 0.55), // Shingle
    Vec3::new(0.44, 0.42, 0.27), // Scrub
    Vec3::new(0.66, 0.56, 0.30), // Parched
    Vec3::new(0.76, 0.66, 0.48), // Dust
    Vec3::new(0.21, 0.42, 0.22), // Forest
    Vec3::new(0.33, 0.55, 0.23), // GrassDark
    Vec3::new(0.44, 0.66, 0.26), // Grass
    Vec3::new(0.56, 0.75, 0.31), // GrassLight
    Vec3::new(0.66, 0.73, 0.34), // Meadow
    Vec3::new(0.13, 0.29, 0.17), // Jungle
    Vec3::new(0.18, 0.37, 0.20), // Canopy
    Vec3::new(0.38, 0.45, 0.27), // Heath
    Vec3::new(0.50, 0.50, 0.31), // Upland
    Vec3::new(0.63, 0.60, 0.42), // Fell
    Vec3::new(0.55, 0.53, 0.50), // Rock
    Vec3::new(0.40, 0.38, 0.37), // RockDark
    Vec3::new(0.68, 0.65, 0.60), // Scree
    Vec3::new(0.13, 0.24, 0.20), // Silt
    Vec3::new(0.33, 0.48, 0.32), // Shoal
    Vec3::new(0.42, 0.42, 0.25), // Marsh
];

/// What the sea is drawn in, and what standing fresh water is drawn in.
///
/// Not tones — no triangle of ground is ever painted these, and they travel
/// nowhere. They are here because they are the other half of what a client
/// needs in order to draw the world, and because the two ends have to agree:
/// the sea a client draws for itself as a plane at zero has to be the same
/// substance as the sea in a map rendered by whatever generated the ground.
///
/// A lake is deliberately *not* the sea. The sea's blue is a bright open one
/// with the sky in it; fresh water is darker, greener and stiller, which is
/// the difference the eye actually uses at a distance — before it can see
/// whether there is a beach. How far either is seen through is the drawing
/// end's own business.
pub const SEA_WATER: Vec3 = Vec3::new(0.10, 0.42, 0.62);
pub const LAKE_WATER: Vec3 = Vec3::new(0.12, 0.34, 0.38);

impl Material {
    /// How many materials there are. The palette and the enum are numbered
    /// together, so this is also one past the largest number
    /// [`Material::from_byte`] answers for.
    pub const KINDS: usize = PALETTE.len();

    /// The material a stored number names, or `None` for one this build has
    /// never
    /// heard of.
    pub fn from_byte(byte: u8) -> Option<Self> {
        // The table and the enum are numbered together, so anything inside the
        // table is a material and the transmute-free way to say so is a match on
        // the count.
        if byte as usize >= PALETTE.len() {
            return None;
        }
        // SAFETY-free equivalent of a cast: an explicit table, so adding a
        // a material without adding it here fails to compile.
        Some(match byte {
            0 => Self::Seabed,
            1 => Self::Shallow,
            2 => Self::Sand,
            3 => Self::Shingle,
            4 => Self::Scrub,
            5 => Self::Parched,
            6 => Self::Dust,
            7 => Self::Forest,
            8 => Self::GrassDark,
            9 => Self::Grass,
            10 => Self::GrassLight,
            11 => Self::Meadow,
            12 => Self::Jungle,
            13 => Self::Canopy,
            14 => Self::Heath,
            15 => Self::Upland,
            16 => Self::Fell,
            17 => Self::Rock,
            18 => Self::RockDark,
            19 => Self::Scree,
            20 => Self::Silt,
            21 => Self::Shoal,
            _ => Self::Marsh,
        })
    }

    /// The byte this material travels as.
    pub fn to_byte(self) -> u8 {
        self as u8
    }

    /// The sRGB this material is drawn in, in the reference palette.
    ///
    /// See the module docs: this is a rendering the wire ships alongside the
    /// materials, not a thing the two ends have to agree about.
    pub fn color(self) -> Vec3 {
        PALETTE[self as usize]
    }
}

// --- The payload ------------------------------------------------------------

/// One chunk of ground, as it crosses the wire.
///
/// Everything a renderer needs and nothing else. The corners are a
/// [`CORNERS`]-square grid sampled from the chunk's lower corner outwards
/// at [`CELL_METRES`] spacing, row-major; the materials are one per cell on
/// the grid those corners bound, also row-major. The two grids are offset
/// half a cell from each other, which is simply what it means for corners
/// to bound cells.
///
/// A chunk of open ocean has no payload at all — see
/// [`crate::ToClient::Chunk`]. What arrives here is ground worth drawing.
#[derive(Clone, Debug, PartialEq)]
pub struct ChunkPayload {
    /// `CORNERS * CORNERS` corner heights, quantised — see
    /// [`quantize`].
    pub heights: Vec<u16>,
    /// [`CELL_COUNT`] materials, one per cell of the chunk's own grid,
    /// row-major — [`ChunkPayload::material`] does the indexing.
    ///
    /// The chunk's own cells and no ring of its neighbours': a renderer that
    /// paints a cell by looking at what is next to it reads the neighbouring
    /// *chunks*, which is what the one client here does — it keeps every
    /// delivered grid and blends by world point, so a seam is between two
    /// answers it already holds. (An overhang was carried for a while,
    /// against a renderer that wanted each chunk drawable alone the moment
    /// it arrived; nothing ever read it.)
    pub materials: Vec<Material>,
    /// When each corner sees the sun: the first and last phase of the day —
    /// [`crate::quantize_phase`] steps — at which the sun stands clear of the
    /// terrain around it, one pair per corner on the grid of
    /// [`ChunkPayload::heights`]. Between the two the corner is in direct
    /// sun; outside them the terrain itself is in the way and the ground
    /// stands in its own shadow. `from > until` is a corner that never sees
    /// the sun at all. Why this crosses the wire, and what a client does with
    /// it, is the module doc's sunlight note.
    ///
    /// A corner under standing water answers for the *surface* over it — the
    /// lake's sheet or the sea's — since that is the ground a shadow falls
    /// on; the bed beneath is seen through it and wears the same light.
    ///
    /// One interval, by construction: a corner lit, re-shadowed by a second
    /// ridge and lit again is flattened to its outer ends, which errs toward
    /// light and only where the terrain is convoluted enough to earn it.
    pub lit: Vec<[u8; 2]>,
    /// Where standing water above sea level covers this chunk, and how high
    /// it stands: one quantised level per corner on the same grid as
    /// [`ChunkPayload::heights`], or `None` for a chunk with no lake on or
    /// beside it — which is most of them.
    ///
    /// A stored [`NO_WATER`] means dry — though a reader after one corner
    /// asks [`ChunkPayload::water_level`], which folds the sentinel and the
    /// `None` below into one answer. Everywhere else the corner has a lake
    /// level over it, and water stands wherever that level is above the
    /// height at the same corner. The grid deliberately reaches well past the
    /// water's edge and up the bank behind it, so a client has a level on
    /// both sides of every shoreline and can put the waterline where the two
    /// fields cross rather than on the last wet corner. A generator owes it
    /// that reach: where the levels stop, the ground has to have climbed out
    /// of the water already, or the client draws a straight edge on the grid
    /// in place of a shore.
    ///
    /// Sent per corner rather than as one level and a mask because a chunk
    /// may hold more than one lake, and two basins a hillside apart stand at
    /// different heights; a single level per chunk would drain one of them or
    /// flood the other. `None` rather than a grid of "dry" because lakes are
    /// occasional and the absence is worth saying outright: a client can hang
    /// the whole question of standing water on whether this is `Some`, rather
    /// than scanning sixteen thousand corners of every chunk in the world to
    /// find out that none of them is wet.
    pub water: Option<Vec<u16>>,
    /// Everything growing on this chunk, of every kind, in no order anything
    /// may rely on beyond its being the same order every time.
    ///
    /// One list rather than one per kind, and each plant says what it is —
    /// see [`Plant::kind`]. A chunk is a beach or a swamp or a hillside, very
    /// rarely all three, so a budget per kind would hand every chunk an
    /// allowance for the kinds that do not grow on it while the kind that
    /// does ran out. Shared, the same ceiling lets a coast spend itself on
    /// palms and a lake margin spend itself on mangroves.
    ///
    /// Here for the same reason a lake's level is: there is no arithmetic a
    /// client could do on the heights and materials it already has that would
    /// find them. Where a plant stands is a decision made against the seed —
    /// which the client has never seen and has no use for — so it travels, or
    /// two players anchored off the same beach would see different trees on
    /// it.
    ///
    /// A plant belongs to the chunk its foot stands in and to no other, so a
    /// client draws each exactly once and drops it with the ground it came
    /// on. What is *not* here is how high it stands: a plant's foot sits on
    /// the height field at its own position, which the client already holds
    /// and already interpolates for everything else that rides the world. A
    /// height sent alongside would be a second opinion about the same ground,
    /// and the one the eye would catch out — a palm hovering a hand's breadth
    /// over its own shadow.
    pub plants: Vec<Plant>,
}

/// One palm, standing on the ground of the chunk that carries it.
///
/// The tree itself is the drawing end's business — this says where one is and
/// how it is turned, not what a palm looks like, in the same way the wire
/// names a [`Material`] rather than sending a colour per cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plant {
    /// Which kind is standing here, and so which model a client puts on the
    /// spot. First in the bytes because the rest cannot be read without it —
    /// a size is a step through [`Kind::scale`]'s range, and the ranges
    /// differ.
    pub kind: Kind,
    /// Where it stands, in metres from the chunk's own lower corner. Always
    /// inside the chunk — a plant on the far side of a boundary belongs to
    /// the chunk over there.
    pub at: Vec2,
    /// Which way it is turned about the vertical, in radians.
    ///
    /// A modelled palm leans, so its bearing is most of what stops a stand of
    /// them reading as one tree stamped repeatedly along a beach. Sent rather
    /// than derived from the position, so that a client picking its own would
    /// not be a client seeing a different beach.
    pub yaw: f32,
    /// How big, as a multiple of the model's own size — see [`Kind::scale`].
    pub scale: f32,
}

/// What kind of plant one is, which is all a client needs to know which model
/// to stand on the spot.
///
/// A kind is a byte on the wire and the numbers are the format: a build that
/// renumbered them would read every other build's swamps as beaches. New
/// kinds go on the end, and a byte naming one this build has never heard of
/// is refused rather than guessed at — see [`Kind::from_byte`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Kind {
    Palm = 0,
    Banana = 1,
    Mangrove = 2,
    Cactus = 3,
}

impl Kind {
    /// The kind a wire byte names, or `None` for a byte naming nothing this
    /// build knows.
    pub const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Palm),
            1 => Some(Self::Banana),
            2 => Some(Self::Mangrove),
            3 => Some(Self::Cactus),
            _ => None,
        }
    }

    pub const fn to_byte(self) -> u8 {
        self as u8
    }

    /// The range this kind's size is drawn from, smallest first.
    ///
    /// Per kind rather than one range for everything, because what counts as
    /// variety differs: a palm is a palm, and enough to break up a row is far
    /// less than enough to read as two species. A kind whose oldest and
    /// youngest look genuinely unalike can say so here without widening
    /// anything else.
    pub const fn scale(self) -> (f32, f32) {
        match self {
            Self::Palm => (0.78, 1.24),
            // Wider than the palm's, and that is the point of the table
            // rather than a constant: a banana clump is a few stems of
            // whatever age happened to sucker there, and the young ones are
            // half the old ones. A row of identically sized clumps reads as
            // planted, which is the one thing a jungle must not.
            Self::Banana => (0.68, 1.34),
            // Narrower than either, and for the opposite reason to the
            // banana's. Mangroves grow in a thicket, close enough to touch, so
            // two neighbours a couple of metres apart are seen against each
            // other rather than each against open ground — and at the banana's
            // spread that reads as one of them being wrong rather than as
            // variety. They also all raced the same water in, so a stand of
            // them really is much of an age.
            Self::Mangrove => (0.82, 1.14),
            // The widest of the four, and the arid ground is what earns it.
            // A mangrove is judged against its neighbours and a palm against a
            // beach full of palms; a cactus stands alone on open dust with
            // nothing beside it to be wrong against, so a spread that would
            // read as inconsistency in a thicket reads here as age. Which it
            // is: nothing in this world grows slower or lives longer, so the
            // young and the old genuinely are two sizes of the same plant.
            Self::Cactus => (0.62, 1.42),
        }
    }
}

/// The most plants one chunk may carry, of all kinds together.
///
/// Derived rather than picked: a chunk's plant count is one byte in
/// [`crate::ToClient::Chunk`], so this is simply what that byte can say, and
/// raising it further is a change to the frame rather than to a number here.
/// It is a ceiling and nowhere near a target — plants of a kind stand where
/// that kind grows, which is a band or a margin rather than a whole chunk.
/// The fullest chunk yet measured is a lake with a mangrove thicket standing
/// in it, at a third of this; a beach of palms carries single figures.
///
/// It exists so that "how much can one answer cost" keeps having an answer:
/// it is what [`crate::ToClient`]'s frame ceiling is derived against, and a
/// reader refuses a chunk claiming more.
pub const MAX_PLANTS: usize = u8::MAX as usize;

/// Bytes one plant occupies: one for its kind, two per axis of its position,
/// one for its bearing and one for its size.
///
/// The position gets sixteen bits an axis because it is the one number here
/// the eye can check — a plant is drawn against a shadow it casts on ground
/// the client interpolates continuously, so a position on a coarse lattice
/// would put the tree beside its own foot. The bearing and the size get
/// eight: a palm turned to within a degree and a half, and sized to within
/// half a percent, is a palm nobody can tell from an exact one.
pub const PLANT_BYTES: usize = 7;

impl Plant {
    fn put(&self, out: &mut Vec<u8>) {
        out.push(self.kind.to_byte());

        let axis = |v: f32| {
            let steps = (v / CHUNK_METRES).clamp(0.0, 1.0) * u16::MAX as f32;
            (steps.round() as u16).to_le_bytes()
        };
        out.extend_from_slice(&axis(self.at.x));
        out.extend_from_slice(&axis(self.at.y));

        let turns = self.yaw.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
        // `min` rather than a wrap: a yaw a hair under a full turn rounds to
        // 256, which is not a byte. It comes back as the same direction.
        out.push(((turns * 256.0).round() as u32).min(255) as u8);

        let (small, large) = self.kind.scale();
        let step = ((self.scale - small) / (large - small)).clamp(0.0, 1.0) * u8::MAX as f32;
        out.push(step.round() as u8);
    }

    /// `None` for a plant whose first byte names a kind this build has never
    /// heard of. Refused rather than skipped or defaulted: the size that
    /// follows is a step through *that kind's* range, so a build guessing at
    /// the kind would be guessing at the size as well, and would stand
    /// something of the wrong sort at the wrong size on somebody's beach.
    fn take(bytes: &[u8]) -> Option<Self> {
        let kind = Kind::from_byte(bytes[0])?;
        let axis = |pair: &[u8]| {
            u16::from_le_bytes([pair[0], pair[1]]) as f32 / u16::MAX as f32 * CHUNK_METRES
        };
        let (small, large) = kind.scale();
        Some(Self {
            kind,
            at: Vec2::new(axis(&bytes[1..3]), axis(&bytes[3..5])),
            yaw: bytes[5] as f32 / 256.0 * std::f32::consts::TAU,
            scale: small + bytes[6] as f32 / u8::MAX as f32 * (large - small),
        })
    }
}

/// The stored water level that means no water — the bottom of the quantised
/// range, [`HEIGHT_FLOOR`], which is metres below the sea a lake cannot be
/// under. Standing water above sea level is the only kind that travels, so
/// every real level is far above this and there is no honest value to
/// collide with.
pub const NO_WATER: u16 = 0;

/// The lit pair of a corner nothing ever shadows: in the sun from the moment
/// it clears the horizon to the moment it sets — [`crate::SUNRISE`] and
/// [`crate::SUNSET`] as [`crate::quantize_phase`] spells them. What open
/// water far from any island would carry if it carried anything, and the
/// honest filler for a payload built where no generator has said otherwise.
pub const LIT_ALL_DAY: [u8; 2] = [64, 192];

/// The lit pair of a corner the sun never reaches — `from` past the end of
/// the day and `until` before its start, so no phase at all lies between
/// them. Any pair with `from > until` reads the same way; this is merely the
/// one a writer with nothing subtler to say writes.
pub const NEVER_LIT: [u8; 2] = [u8::MAX, 0];

/// Whether a lit pair names any daylight at all — false for [`NEVER_LIT`] and
/// for every other pair that runs backwards, which say the same thing.
pub fn ever_lit(pair: [u8; 2]) -> bool {
    pair[0] <= pair[1]
}

/// The lit interval somewhere inside a cell, read across the four corners
/// around it — `corners` in [`ChunkPayload::materials`]' own south-west,
/// south-east, north-west, north-east order, `at` in cell widths from the
/// south-west one. The middle of a cell is `Vec2::splat(0.5)`.
///
/// A corner the sun never reaches is left out of the weighing rather than
/// averaged in. Its `from` stands past the end of the day, so a plain
/// bilinear drags a neighbour's honest interval toward noon and prints a
/// cliff foot's shade over the lit ground beside it — and two such corners
/// give an interval running backwards, which reads as ground that never sees
/// the sun at all. What is left is what the corners that *do* see it agree
/// on, which errs toward light exactly as the bake does under a grazing sun.
/// With no lit corner there is nothing to err with, and the answer is
/// [`NEVER_LIT`].
pub fn lit_across(corners: [[u8; 2]; 4], at: Vec2) -> [u8; 2] {
    let weights = [
        (1.0 - at.x) * (1.0 - at.y),
        at.x * (1.0 - at.y),
        (1.0 - at.x) * at.y,
        at.x * at.y,
    ];

    let (mut sum, mut total) = ([0.0f32; 2], 0.0);
    for (corner, weight) in corners.into_iter().zip(weights) {
        if !ever_lit(corner) {
            continue;
        }
        sum[0] += corner[0] as f32 * weight;
        sum[1] += corner[1] as f32 * weight;
        total += weight;
    }

    if total <= 0.0 {
        return NEVER_LIT;
    }
    [
        (sum[0] / total).round() as u8,
        (sum[1] / total).round() as u8,
    ]
}

/// Bytes a chunk's heights, materials and lit grid occupy on the wire: two
/// per corner height, one per cell of the material grid, two per corner lit
/// pair. Nothing is compressed — see the module docs on the grid being the
/// format, and note that delta-coding the heights and run-coding the
/// materials would take most of this back if the wire ever needs it to.
pub const PAYLOAD_BYTES: usize = CORNERS * CORNERS * 2 + CELL_COUNT + LIT_BYTES;

/// Bytes a chunk's lit grid occupies — a pair per corner, unconditionally:
/// unlike a lake, there is no chunk of ground whose light is not worth
/// saying.
pub const LIT_BYTES: usize = CORNERS * CORNERS * 2;

/// Bytes a chunk's shelter lattice occupies when it carries one — one per
/// bearing per lattice point. See [`crate::ToClient::Chunk`], which is what
/// carries it.
pub const SHELTER_BYTES: usize = SHELTER_COUNT * BEARINGS;

/// Bytes a chunk's water grid adds when it carries one — two per corner,
/// like the heights it is compared against.
pub const WATER_BYTES: usize = CORNERS * CORNERS * 2;

/// What one payload occupies on the wire, which depends on the two things
/// about a chunk that are not fixed: whether it carries standing water, and
/// how many plants grow on it. The flag and the count in
/// [`crate::ToClient::Chunk`] are what say which, and so how many bytes a
/// reader is about to be handed.
pub const fn payload_bytes(water: bool, plants: usize) -> usize {
    PAYLOAD_BYTES + if water { WATER_BYTES } else { 0 } + plants * PLANT_BYTES
}

/// Where cell `(ix, iz)` sits in a payload's material grid, or `None` for a
/// cell the grid does not reach — anything outside `0 .. CELLS` on either
/// axis.
///
/// Coordinates are the chunk's own, so `(0, 0)` is its lower corner cell.
/// Written once here because both ends index the same grid and neither
/// should be restating the arithmetic.
pub const fn material_index(ix: i32, iz: i32) -> Option<usize> {
    let last = CELLS as i32 - 1;
    if ix < 0 || ix > last || iz < 0 || iz > last {
        return None;
    }
    Some(iz as usize * CELLS + ix as usize)
}

impl ChunkPayload {
    /// What cell `(ix, iz)` of this chunk is made of, in the chunk's own cell
    /// coordinates — `None` off the grid. See [`material_index`].
    pub fn material(&self, ix: i32, iz: i32) -> Option<Material> {
        material_index(ix, iz).map(|at| self.materials[at])
    }

    /// The standing water over corner `(ix, iz)` of the heights grid, still
    /// quantised — see [`quantize`] — or `None` where the corner is dry or
    /// off the grid.
    ///
    /// Dry is spelt two ways in the bytes — a lakeless chunk carries no grid
    /// at all, and a dry corner of a wet chunk stores [`NO_WATER`] — and this
    /// is where the two become one answer, so no reader ever meets the
    /// encoding. Still quantised because the question a level exists to
    /// settle — does the water stand above the height at this same corner? —
    /// is exact between two values on one lattice, and rounding luck between
    /// the two floats made from them.
    pub fn water_level(&self, ix: i32, iz: i32) -> Option<u16> {
        if !(0..CORNERS as i32).contains(&ix) || !(0..CORNERS as i32).contains(&iz) {
            return None;
        }
        let level = self.water.as_ref()?[iz as usize * CORNERS + ix as usize];
        (level != NO_WATER).then_some(level)
    }

    /// When corner `(ix, iz)` sees the sun — see [`ChunkPayload::lit`] — or
    /// `None` off the grid.
    pub fn lit(&self, ix: i32, iz: i32) -> Option<[u8; 2]> {
        if !(0..CORNERS as i32).contains(&ix) || !(0..CORNERS as i32).contains(&iz) {
            return None;
        }
        Some(self.lit[iz as usize * CORNERS + ix as usize])
    }

    /// Whether this is a payload of the shape the grid says it should be.
    /// What a reader checks before believing a frame, and what a builder can
    /// assert against.
    pub fn well_formed(&self) -> bool {
        let corners = CORNERS * CORNERS;
        self.heights.len() == corners
            && self.materials.len() == CELL_COUNT
            && self.lit.len() == corners
            && self
                .water
                .as_ref()
                .is_none_or(|water| water.len() == corners)
            && self.plants.len() <= MAX_PLANTS
            && self.plants.iter().all(|plant| {
                (0.0..CHUNK_METRES).contains(&plant.at.x)
                    && (0.0..CHUNK_METRES).contains(&plant.at.y)
            })
    }

    /// Appends this payload's bytes: every height little-endian, then every
    /// surface, then every lit pair, then the water grid where there is one.
    ///
    /// The water goes after them, and the plants after that, so that a reader
    /// of any kind of chunk finds the heights, the materials and the light at
    /// the same offsets — a lake and a stand of palms are things a chunk
    /// carries in addition, never a rearrangement of what it already carried.
    pub(crate) fn put(&self, out: &mut Vec<u8>) {
        debug_assert!(self.well_formed(), "not a chunk's worth of ground");
        for height in &self.heights {
            out.extend_from_slice(&height.to_le_bytes());
        }
        out.extend(self.materials.iter().map(|material| material.to_byte()));
        for pair in &self.lit {
            out.extend_from_slice(pair);
        }
        for level in self.water.iter().flatten() {
            out.extend_from_slice(&level.to_le_bytes());
        }
        for plant in &self.plants {
            plant.put(out);
        }
    }

    /// Reads a payload from exactly [`payload_bytes`] of them — `water` and
    /// `plants` say which length, and come from the flag and the count the
    /// caller has already read. `None` if the bytes are not that many, or if
    /// any material or plant byte names nothing this build knows.
    ///
    /// The length is checked rather than asserted because it is the one thing
    /// here a *frame* can be wrong about: the flag, the count and the length
    /// are written separately, so a build that disagreed with this one about
    /// how long a watered chunk is would otherwise be read as a chunk whose
    /// lake silently vanished.
    pub(crate) fn take(bytes: &[u8], water: bool, plants: usize) -> Option<Self> {
        if bytes.len() != payload_bytes(water, plants) || plants > MAX_PLANTS {
            return None;
        }
        let levels = |bytes: &[u8]| {
            bytes
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<u16>>()
        };
        let (heights, rest) = bytes.split_at(CORNERS * CORNERS * 2);
        let (materials, rest) = rest.split_at(CELL_COUNT);
        let (lit, rest) = rest.split_at(LIT_BYTES);
        let (water, plants) = rest.split_at(rest.len() - plants * PLANT_BYTES);
        Some(Self {
            heights: levels(heights),
            materials: materials
                .iter()
                .map(|byte| Material::from_byte(*byte))
                .collect::<Option<_>>()?,
            lit: lit.chunks_exact(2).map(|pair| [pair[0], pair[1]]).collect(),
            water: (!water.is_empty()).then(|| levels(water)),
            plants: plants
                .chunks_exact(PLANT_BYTES)
                .map(Plant::take)
                .collect::<Option<_>>()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grid_divides_a_chunk_exactly() {
        assert_eq!(CELLS as f32 * CELL_METRES, CHUNK_METRES);
        assert_eq!(CORNERS, 129);
        assert_eq!(CELL_COUNT, 16_384);
        assert_eq!(SHELTER_CELLS, 8);
        assert_eq!(SHELTER_CORNERS, 9);
        assert_eq!(SHELTER_COUNT, 81);
        assert_eq!(SHELTER_BYTES, 81 * 8);
        assert_eq!(SHELTER_CELLS as f32 * SHELTER_METRES, CHUNK_METRES);
        assert_eq!(PAYLOAD_BYTES, 129 * 129 * 2 + 16_384 + 129 * 129 * 2);
        assert_eq!(WATER_BYTES, 129 * 129 * 2);
        assert_eq!(LIT_BYTES, 129 * 129 * 2);
        assert_eq!(payload_bytes(false, 0), PAYLOAD_BYTES);
        assert_eq!(payload_bytes(true, 0), PAYLOAD_BYTES + WATER_BYTES);
        assert_eq!(
            payload_bytes(true, 3),
            PAYLOAD_BYTES + WATER_BYTES + 3 * PLANT_BYTES
        );
    }

    #[test]
    fn a_wind_on_a_compass_point_reads_that_points_own_slot() {
        // The blend has to be exact at the eight, or every stored value is
        // read somewhere other than where it was measured — and since the
        // sectors are walked forwards in one half and backwards in the other,
        // an off-by-one in that split shows here and nowhere else.
        for (slot, toward) in bearings().into_iter().enumerate() {
            let mut slots = [0.0f32; BEARINGS];
            slots[slot] = 200.0;
            let read = exposure_toward(slots, toward);
            assert!(
                (read - 200.0).abs() < 1e-3,
                "the wind toward slot {slot} read {read}, not the 200 stored there"
            );
        }
    }

    #[test]
    fn a_wind_between_two_points_reads_between_their_slots() {
        // Halfway between north and north-east, by bearing. The blend runs
        // off a tangent rather than an angle — see `shelter_across` — so this
        // does not land on the arithmetic mean, and pinning it to one would
        // be pinning the warp rather than the property that matters: the
        // answer is strictly between the two neighbours and touches neither.
        let mut slots = [0.0f32; BEARINGS];
        slots[0] = 0.0;
        slots[1] = 240.0;
        let toward = Vec2::new(22.5f32.to_radians().sin(), -22.5f32.to_radians().cos());
        let read = exposure_toward(slots, toward);
        assert!(
            read > 1.0 && read < 239.0,
            "a wind between north and north-east read {read}, which is one of them"
        );
        // And nothing outside the pair leaks in: the other six slots are full
        // and the answer still sits under the only one of the two that is.
        let mut only = [255.0f32; BEARINGS];
        only[0] = 0.0;
        only[1] = 0.0;
        assert_eq!(
            exposure_toward(only, toward),
            0.0,
            "a bearing between north and north-east read a slot that is neither"
        );
    }

    #[test]
    fn a_strait_between_two_islands_is_not_read_as_sheltered() {
        // The case the module doc says four bearings would get backwards, run
        // against the eight: land due north and land due east, open channel
        // between them. A blend that only had the cardinals to work with
        // would average two sheltered readings into a sheltered channel; with
        // north-east stored in its own slot the wind down it comes through.
        let mut point = [EXPOSED; BEARINGS];
        point[0] = 20;
        point[2] = 20;
        let d = 0.5f32.sqrt();
        let down_the_strait = shelter_across([point; 4], Vec2::splat(0.5), Vec2::new(d, -d));
        assert!(
            down_the_strait > 0.9,
            "the channel between two islands read {down_the_strait} exposed"
        );
        let onto_the_land = shelter_across([point; 4], Vec2::splat(0.5), Vec2::new(0.0, -1.0));
        assert!(
            onto_the_land < 0.1,
            "the lee of the northern island read {onto_the_land} exposed"
        );
    }

    #[test]
    fn shelter_reads_across_the_four_points_around_it() {
        // A lattice cell sheltered along its west edge and open along its
        // east: the reading has to walk between them, and land on each
        // point's own value at that point's own corner.
        let (lee, open) = ([0u8; BEARINGS], [EXPOSED; BEARINGS]);
        let north = Vec2::new(0.0, -1.0);
        let cell = [lee, open, lee, open];
        assert_eq!(shelter_across(cell, Vec2::ZERO, north), 0.0);
        assert_eq!(shelter_across(cell, Vec2::new(1.0, 0.0), north), 1.0);
        let middle = shelter_across(cell, Vec2::new(0.5, 0.5), north);
        assert!(
            (middle - 0.5).abs() < 1e-3,
            "halfway across the cell read {middle}"
        );
    }

    #[test]
    fn every_wind_reads_somewhere_between_nothing_and_everything() {
        // Swept right round the card, including the exact axes where the
        // quadrant test flips and the diagonals where the sector does. A
        // blend that ran off the end of the ratio would overshoot here rather
        // than panicking, and an overshoot is wind out of nowhere.
        let slots: [u8; BEARINGS] = std::array::from_fn(|b| (b * 31) as u8);
        for step in 0..720 {
            let angle = step as f32 * 0.5f32.to_radians() * 2.0;
            let toward = Vec2::new(angle.sin(), -angle.cos());
            let read = shelter_across([slots; 4], Vec2::splat(0.5), toward);
            assert!(
                (0.0..=1.0).contains(&read),
                "a wind {step} half-degrees round the card read {read}"
            );
        }
        // And the zero vector, which names no bearing at all and must still
        // come back with a number rather than a NaN.
        assert!(shelter_across([slots; 4], Vec2::splat(0.5), Vec2::ZERO).is_finite());
    }

    #[test]
    fn a_cell_indexes_to_its_own_place_on_the_material_grid() {
        // Corners first: the lowest cell the grid reaches and the highest.
        assert_eq!(material_index(0, 0), Some(0));
        assert_eq!(
            material_index(CELLS as i32 - 1, CELLS as i32 - 1),
            Some(CELL_COUNT - 1)
        );

        // Row-major, so a step along x moves one and a step along z moves a
        // whole row. Written as two separate checks because the failure that
        // matters here is a transpose, which a symmetric one would pass.
        assert_eq!(material_index(1, 0), material_index(0, 0).map(|at| at + 1));
        assert_eq!(
            material_index(0, 1),
            material_index(0, 0).map(|at| at + CELLS)
        );

        // And nothing off the grid, on either axis or either side.
        assert_eq!(material_index(-1, 0), None);
        assert_eq!(material_index(0, -1), None);
        assert_eq!(material_index(CELLS as i32, 0), None);
        assert_eq!(material_index(0, CELLS as i32), None);

        // Every place on the grid is some cell's, and no two share one.
        let mut seen = std::collections::BTreeSet::new();
        for iz in 0..CELLS as i32 {
            for ix in 0..CELLS as i32 {
                assert!(
                    seen.insert(material_index(ix, iz).expect("a cell on the grid")),
                    "({ix}, {iz}) landed on a place already taken"
                );
            }
        }
        assert_eq!(seen.len(), CELL_COUNT);
    }

    #[test]
    fn no_water_is_a_depth_no_lake_could_stand_at() {
        // The sentinel has to be a value no honest level can take, and what
        // makes it one is that lakes stand *above* the sea while this is the
        // bottom of the quantised range, far below it.
        assert_eq!(dequantize(NO_WATER), HEIGHT_FLOOR);
        assert!(dequantize(NO_WATER) < 0.0);
        assert_ne!(quantize(0.0), NO_WATER, "sea level would read as dry");
    }

    #[test]
    fn a_height_survives_the_rounding_it_is_worth() {
        // Two centimetres of step means a centimetre of error at worst, which
        // is what the boat and the mesh are allowed to disagree by.
        for tenth in -800..=30_000i32 {
            let height = tenth as f32 / 100.0;
            let back = dequantize(quantize(height));
            assert!(
                (back - height).abs() <= HEIGHT_STEP,
                "{height} came back as {back}"
            );
        }
    }

    #[test]
    fn heights_outside_the_range_clamp_rather_than_wrap() {
        assert_eq!(quantize(HEIGHT_FLOOR), 0);
        assert_eq!(quantize(-1.0e9), 0);
        assert_eq!(quantize(f32::NEG_INFINITY), 0);
        assert_eq!(quantize(f32::NAN), 0);
        assert_eq!(quantize(1.0e9), u16::MAX);
        assert_eq!(quantize(f32::INFINITY), u16::MAX);
    }

    #[test]
    fn every_material_survives_its_byte() {
        for byte in 0..PALETTE.len() as u8 {
            let material = Material::from_byte(byte).expect("a material");
            assert_eq!(material.to_byte(), byte);
            assert_eq!(Material::from_byte(material.to_byte()), Some(material));
        }
        // One past the end of the table names nothing.
        assert_eq!(Material::from_byte(PALETTE.len() as u8), None);
        assert_eq!(Material::from_byte(u8::MAX), None);
    }

    #[test]
    fn the_reference_palette_stays_inside_the_colours_there_are() {
        // Not a claim about the format — a client may paint the ground any
        // way it likes. It is a claim about the palette this world is
        // authored in, which mapgen renders straight out of: a colour outside
        // the unit cube is one somebody has typed wrong.
        for byte in 0..PALETTE.len() as u8 {
            let material = Material::from_byte(byte).expect("a material");
            let c = material.color();
            assert!(
                c.cmpge(Vec3::ZERO).all() && c.cmple(Vec3::ONE).all(),
                "{material:?} is {c}"
            );
        }
    }

    #[test]
    fn no_two_materials_are_the_same_colour() {
        // Eighteen materials that a reader cannot tell apart would be
        // eighteen materials for nothing. Sharpest pair in the palette is
        // some way above this, so it is a guard against a typo rather than a
        // threshold anything is tuned against.
        for (a, first) in PALETTE.iter().enumerate() {
            for (b, second) in PALETTE.iter().enumerate().skip(a + 1) {
                let apart = (*first - *second).length();
                assert!(apart > 0.02, "materials {a} and {b} are {apart} apart");
            }
        }
    }

    /// Plants enough to tell one from another, spread across the chunk so that
    /// a position written to the wrong axis would land outside it.
    fn some_plants(count: usize) -> Vec<Plant> {
        (0..count)
            .map(|i| {
                let kind = Kind::Palm;
                let (small, large) = kind.scale();
                Plant {
                    kind,
                    at: Vec2::new(
                        i as f32 / MAX_PLANTS as f32 * CHUNK_METRES,
                        (CHUNK_METRES - 1.0 - i as f32).max(0.0),
                    ),
                    yaw: i as f32 / MAX_PLANTS as f32 * std::f32::consts::TAU,
                    scale: small + (i as f32 / MAX_PLANTS as f32) * (large - small),
                }
            })
            .collect()
    }

    /// A payload whose every value differs from every other, so anything that
    /// transposed or truncated one of its grids would show.
    fn a_payload(water: bool, plants: usize) -> ChunkPayload {
        ChunkPayload {
            heights: (0..CORNERS * CORNERS)
                .map(|i| (i * 7 % 65_535) as u16)
                .collect(),
            materials: (0..CELL_COUNT)
                .map(|i| Material::from_byte((i % PALETTE.len()) as u8).expect("a material"))
                .collect(),
            lit: (0..CORNERS * CORNERS)
                .map(|i| [(i * 3 % 251) as u8, (i * 5 % 253) as u8])
                .collect(),
            water: water.then(|| {
                (0..CORNERS * CORNERS)
                    .map(|i| (i * 11 % 65_533) as u16)
                    .collect()
            }),
            plants: some_plants(plants),
        }
    }

    #[test]
    fn a_payload_survives_its_bytes() {
        for water in [false, true] {
            for plants in [0, 1, MAX_PLANTS] {
                let payload = a_payload(water, plants);
                assert!(payload.well_formed());

                let mut bytes = Vec::new();
                payload.put(&mut bytes);
                assert_eq!(bytes.len(), payload_bytes(water, plants));

                // Plants are the one part of a payload that does not survive
                // exactly — a position is sixteen bits an axis and a bearing
                // is eight — so they are compared to the tolerance the wire
                // promises rather than for equality. The kind is not one of
                // those: a byte for a byte, and a plant that came back as
                // another sort would be a different tree entirely.
                let back = ChunkPayload::take(&bytes, water, plants).expect("a payload");
                assert_eq!(back.heights, payload.heights);
                assert_eq!(back.materials, payload.materials);
                assert_eq!(back.lit, payload.lit);
                assert_eq!(back.water, payload.water);
                assert_eq!(back.plants.len(), payload.plants.len());
                for (got, sent) in back.plants.iter().zip(&payload.plants) {
                    assert_eq!(got.kind, sent.kind);
                    assert!(
                        (got.at - sent.at).length() < 0.01,
                        "a plant at {:?} came back at {:?}",
                        sent.at,
                        got.at
                    );
                    assert!((got.yaw - sent.yaw).abs() < 0.03);
                    assert!((got.scale - sent.scale).abs() < 0.01);
                }
            }
        }
    }

    #[test]
    fn a_corner_answers_its_water_in_one_spelling() {
        // Dry is spelt two ways in the bytes — no grid at all, and the
        // sentinel within one — and the accessor owes every reader the same
        // `None` for both.
        let dry = a_payload(false, 0);
        assert_eq!(dry.water_level(0, 0), None);

        let mut wet = a_payload(true, 0);
        let grid = wet.water.as_mut().expect("a lake");
        grid[0] = NO_WATER;
        grid[1] = quantize(12.0);
        grid[2 * CORNERS + 3] = quantize(31.0);
        assert_eq!(wet.water_level(0, 0), None, "the sentinel read as a level");
        assert_eq!(wet.water_level(1, 0), Some(quantize(12.0)));
        // Row-major on the corner grid, exactly as the heights are — and
        // asked asymmetrically, because a transpose would pass a square ask.
        assert_eq!(wet.water_level(3, 2), Some(quantize(31.0)));

        // Nothing past the grid.
        assert_eq!(wet.water_level(-1, 0), None);
        assert_eq!(wet.water_level(0, CORNERS as i32), None);
    }

    #[test]
    fn a_chunk_claiming_more_plants_than_it_may_is_refused() {
        // The ceiling is what the frame size is derived against, so a count
        // past it is a frame that could not have been written by a build that
        // agrees with this one about how much an answer costs. The count is a
        // byte and the ceiling is what a byte can say, so no frame can carry
        // this claim — but the length it implies can still be handed here.
        let payload = a_payload(false, MAX_PLANTS);
        let mut bytes = Vec::new();
        payload.put(&mut bytes);
        bytes.extend_from_slice(&[0; PLANT_BYTES]);
        assert_eq!(ChunkPayload::take(&bytes, false, MAX_PLANTS + 1), None);
    }

    #[test]
    fn a_plant_of_an_unknown_kind_is_refused() {
        // A build reading a kind it has never heard of cannot draw it, and
        // cannot skip it either: the size byte behind it is a step through
        // that kind's own range. So the chunk is refused whole rather than
        // arriving with something of the wrong sort standing on it.
        let payload = a_payload(false, 2);
        let mut bytes = Vec::new();
        payload.put(&mut bytes);
        let first = payload_bytes(false, 0);
        bytes[first] = 200;
        assert_eq!(ChunkPayload::take(&bytes, false, 2), None);
    }

    #[test]
    fn a_plant_outside_its_own_chunk_is_malformed() {
        // What a generator is held to. A plant belongs to the chunk its foot
        // stands in, so one placed past the boundary would be drawn by a
        // client that never asked for it — and drawn again by the chunk it
        // really stands on.
        let mut strayed = a_payload(false, 1);
        strayed.plants[0].at.x = CHUNK_METRES;
        assert!(!strayed.well_formed());
        strayed.plants[0].at = Vec2::new(1.0, -0.5);
        assert!(!strayed.well_formed());
    }

    #[test]
    fn a_cell_is_lit_by_the_corners_that_see_the_sun() {
        // The whole point of [`lit_across`] over a plain bilinear: the
        // never-lit sentinel is not a phase, and weighing it as one is what
        // printed a cliff foot's shade over the ground beside it.
        let day = LIT_ALL_DAY;
        let middle = Vec2::splat(0.5);

        assert_eq!(lit_across([day; 4], middle), day, "nothing in the way");
        assert_eq!(
            lit_across([NEVER_LIT; 4], middle),
            NEVER_LIT,
            "no corner to err toward the light with"
        );
        assert_eq!(
            lit_across([day, day, day, NEVER_LIT], middle),
            day,
            "three corners see the whole day; the fourth cannot shorten it"
        );
        assert_eq!(
            lit_across([day, day, NEVER_LIT, NEVER_LIT], middle),
            day,
            "and two cannot turn the day around into ground that is never lit"
        );

        // A pair that runs backwards without being the sentinel says the same
        // thing, and is left out on the same terms.
        assert!(!ever_lit([200, 100]));
        assert_eq!(lit_across([day, day, day, [200, 100]], middle), day);

        // Corners that disagree honestly are still met in the middle, and the
        // weights still favour the corner asked nearest.
        let late = [128u8, 192];
        assert_eq!(lit_across([day, late, day, late], middle), [96, 192]);
        assert_eq!(
            lit_across([day, late, day, late], Vec2::new(1.0, 0.5)),
            late,
            "asked at the eastern edge, the eastern corners answer alone"
        );
    }

    #[test]
    fn the_lit_sentinels_say_what_the_arc_says() {
        // All day is sunrise to sunset as the wire spells them, not two
        // numbers that happen to look right.
        assert_eq!(
            LIT_ALL_DAY,
            [
                crate::quantize_phase(crate::SUNRISE),
                crate::quantize_phase(crate::SUNSET)
            ]
        );
        // And never is an interval no phase can fall inside.
        assert!(NEVER_LIT[0] > NEVER_LIT[1]);
    }

    #[test]
    fn a_corner_answers_when_it_is_lit() {
        let payload = a_payload(false, 0);
        // Row-major on the corner grid, asked asymmetrically so a transpose
        // cannot pass.
        assert_eq!(payload.lit(3, 2), Some(payload.lit[2 * CORNERS + 3]));
        assert_eq!(payload.lit(-1, 0), None);
        assert_eq!(payload.lit(0, CORNERS as i32), None);
    }

    #[test]
    fn the_water_grid_is_the_only_thing_a_lake_adds() {
        // A watered payload is a dry one with a grid on the end: the heights
        // and the materials encode to exactly the same bytes in the same
        // places, so a reader of either finds them without knowing which it
        // has until it reaches the tail.
        let (mut dry, mut wet) = (Vec::new(), Vec::new());
        a_payload(false, 0).put(&mut dry);
        a_payload(true, 0).put(&mut wet);
        assert_eq!(wet[..PAYLOAD_BYTES], dry[..], "the water moved the ground");
        assert_eq!(wet.len() - dry.len(), WATER_BYTES);
    }

    #[test]
    fn a_payload_with_a_material_from_the_future_is_refused() {
        // The last material byte, which sits in the middle of the payload —
        // so this also catches a reader that stopped checking materials once
        // it knew there were grids still to come.
        for water in [false, true] {
            let mut bytes = Vec::new();
            a_payload(water, 2).put(&mut bytes);
            bytes[CORNERS * CORNERS * 2 + CELL_COUNT - 1] = 0xFF;
            assert_eq!(ChunkPayload::take(&bytes, water, 2), None);
        }
    }

    #[test]
    fn a_payload_is_malformed_if_its_grids_are_the_wrong_size() {
        // What a builder is held to. A short water grid is the one worth
        // naming: heights and materials have been fixed-size since there was a
        // wire, but the water is built per chunk from whatever ground has a
        // lake on it, and a payload carrying half a grid would encode to a
        // frame no reader could believe.
        let mut short = a_payload(true, 0);
        short.water.as_mut().expect("a lake").truncate(4);
        assert!(!short.well_formed());

        let mut empty = a_payload(true, 0);
        empty.water = Some(Vec::new());
        assert!(!empty.well_formed());

        // The lit grid has no "none" to hide behind: every chunk of ground
        // has a day over it, so a short grid is the only way to be wrong.
        let mut dim = a_payload(false, 0);
        dim.lit.truncate(4);
        assert!(!dim.well_formed());

        // And no water at all is well formed — most chunks have none.
        assert!(a_payload(false, 0).well_formed());
    }

    #[test]
    fn a_payload_whose_length_belies_its_flag_is_refused() {
        // Dry bytes read as watered, and watered bytes read as dry. Either
        // way the answer is `None` rather than a payload that quietly gained
        // or lost a lake.
        let mut dry = Vec::new();
        a_payload(false, 0).put(&mut dry);
        assert_eq!(ChunkPayload::take(&dry, true, 0), None);

        let mut wet = Vec::new();
        a_payload(true, 0).put(&mut wet);
        assert_eq!(ChunkPayload::take(&wet, false, 0), None);
    }
}
