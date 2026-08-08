//! The ground, as it travels: the grid a chunk is drawn on, the small palette
//! it is painted from, and the payload one chunk of it encodes to.
//!
//! This is the half of the wire that used to be nobody's business but the
//! generator's. A client no longer generates anything, so everything it needs
//! in order to *draw* a chunk has to be spelled out here, in a form that says
//! nothing about how the ground was arrived at: corner heights on a fixed
//! grid, and one palette entry per triangle. Whoever is holding a payload can
//! build the mesh from it and has no way to ask what noise made it.
//!
//! Two things follow from that, and both are deliberate.
//!
//! The **grid is the format**. [`FACET_METRES`] is how finely the ground is
//! drawn, and moving it would move the payload, so it lives here rather than
//! in the generator that samples it. Drawing at some other density — level of
//! detail, say — is a change to the wire and not an implementation detail of
//! either end.
//!
//! And the **palette is the format**. Colours are named rather than sent: a
//! [`Surface`] is a byte, and [`Surface::color`] is what it means. That keeps
//! a chunk under twenty kilobytes instead of carrying three floats per
//! triangle, and it keeps the two ends unable to disagree about what sand
//! looks like — there is one table, and this is it.
//!
//! The same goes for **standing water**. The sea is a plane at zero that a
//! client draws whether or not it has been told anything, but a lake is not:
//! it stands at a height decided by a rim saddle that may be half a
//! kilometre away, on ground that says nothing about it. So where a chunk
//! carries water, its surface crosses the wire as a second grid — see
//! [`ChunkPayload::water`] — because there is no arithmetic a client could do
//! on the heights it already has that would find the level.

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
/// which a client draws as a plane rather than as a mesh of eight thousand
/// identical triangles. Every island's own sea bed is clamped to the same
/// level, so the plane and the meshes meet along every coast with nothing to
/// show for it; a client drawing its backdrop at some other depth would print
/// a step around every island in the world.
pub const OCEAN_DEPTH: f32 = 8.0;

/// Metres between the corners the ground is drawn from.
///
/// The height field behind it is continuous, so this is only how finely it
/// gets *drawn*, and it is deliberately coarse: the ground is flat-shaded, and
/// a facet has to be big enough to read as a facet. At 2 m one covers roughly
/// 50 px at the default zoom, which is about where facets read as deliberate
/// rather than as a low-resolution mesh.
pub const FACET_METRES: f32 = 2.0;

/// Quads along one edge of a chunk's facet grid.
pub const FACET_QUADS: usize = (CHUNK_METRES / FACET_METRES) as usize;

/// Corners along one edge of that grid — one more than the quads, since the
/// corners at both ends are shared.
pub const FACET_VERTS: usize = FACET_QUADS + 1;

/// Triangles in one chunk. Two per quad, and each gets its own [`Surface`]:
/// flat shading means there is nothing to interpolate between a triangle's
/// corners, so the colour is a property of the triangle rather than of the
/// grid.
pub const FACET_TRIS: usize = FACET_QUADS * FACET_QUADS * 2;

// --- Heights ----------------------------------------------------------------

/// The height a stored zero means, in metres. Well below the deepest sea bed
/// the world builds, so no honest ground ever clamps against it.
pub const HEIGHT_FLOOR: f32 = -16.0;

/// Metres per step of a stored height — two centimetres, which is a fiftieth
/// of the smallest thing anyone can see at the closest zoom and a two
/// thousandth of a facet's own width.
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

/// The ground palette. Small and flat on purpose — every triangle gets exactly
/// one of these, so the whole world is drawn in nineteen colours plus three
/// shade steps. Saturated well past anything natural, because flat shading has
/// no texture or gradient to carry the picture; the colour has to do that work
/// on its own.
///
/// The order is the wire's: a tone travels as its own number, so the only
/// shape a change to this may take is another entry on the end — anything
/// shuffled repaints the world of every build that disagrees. Either way it is
/// a change to the format and bumps [`crate::PROTOCOL_VERSION`], because a
/// client that has never heard of a tone cannot draw the triangle it names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Tone {
    /// The deep bed, seen through the water.
    Seabed = 0,
    /// The bright shelf that gives a coast its turquoise ring.
    Shallow = 1,
    Sand = 2,
    /// Pebble and boulder foreshore. Warmer and lighter than [`Tone::Rock`],
    /// so a shingle beach reads as its own thing next to the cliffs rather
    /// than as more of them.
    Shingle = 3,
    Forest = 4,
    GrassDark = 5,
    Grass = 6,
    GrassLight = 7,
    Meadow = 8,
    /// Moorland, above the trees and below the bare rock. [`Tone::Heath`] and
    /// [`Tone::Fell`] are what the darkest and lightest lowland parcels turn
    /// into as they climb — the one still half green, the other already most
    /// of the way to stone — so that the upland reads as the same country
    /// drained of colour rather than as a different map laid over the top.
    Heath = 9,
    Upland = 10,
    Fell = 11,
    Rock = 12,
    RockDark = 13,
    /// Bare stone bleached by the weather, the last step before the snow.
    Scree = 14,
    /// Snow on the summits. Off-white and slightly blue: a pure white would be
    /// the only fully saturated thing in the world and would pull the eye off
    /// everything else, and it has to stay clearly apart from [`Tone::Rock`]
    /// in shadow.
    Snow = 15,
    /// The bed of standing fresh water, deep enough to be dark. Green where
    /// [`Tone::Seabed`] is blue, and darker than it: a lake bottoms out in
    /// silt and drowned vegetation rather than in sand, and it is what a lake
    /// is *seen through* that has to say fresh water rather than sea.
    Silt = 16,
    /// The weedy shallows of a lake — what [`Tone::Shallow`] is to the sea,
    /// except that it deliberately refuses the turquoise. A ring of bright
    /// water is the strongest thing that says *coast* in this palette, so a
    /// lake wearing one reads as an arm of the sea that happens to be inland.
    Shoal = 17,
    /// The margin a lake leaves around itself: reed, mud and wet ground, from
    /// just under the waterline to just above it. Takes the place a beach
    /// holds on the sea coast, and is dull and dark where sand is bright —
    /// fresh water has no surf to wash a shore clean.
    Marsh = 18,
}

/// The sRGB the tones stand for, in the order they are numbered.
const TONES: [Vec3; 19] = [
    Vec3::new(0.16, 0.34, 0.38), // Seabed
    Vec3::new(0.46, 0.68, 0.62), // Shallow
    Vec3::new(0.90, 0.83, 0.58), // Sand
    Vec3::new(0.70, 0.65, 0.55), // Shingle
    Vec3::new(0.21, 0.42, 0.22), // Forest
    Vec3::new(0.33, 0.55, 0.23), // GrassDark
    Vec3::new(0.44, 0.66, 0.26), // Grass
    Vec3::new(0.56, 0.75, 0.31), // GrassLight
    Vec3::new(0.66, 0.73, 0.34), // Meadow
    Vec3::new(0.38, 0.45, 0.27), // Heath
    Vec3::new(0.50, 0.50, 0.31), // Upland
    Vec3::new(0.63, 0.60, 0.42), // Fell
    Vec3::new(0.55, 0.53, 0.50), // Rock
    Vec3::new(0.40, 0.38, 0.37), // RockDark
    Vec3::new(0.68, 0.65, 0.60), // Scree
    Vec3::new(0.90, 0.92, 0.95), // Snow
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

impl Tone {
    /// The tone a stored number names, or `None` for one this build has never
    /// heard of.
    fn from_byte(byte: u8) -> Option<Self> {
        // The table and the enum are numbered together, so anything inside the
        // table is a tone and the transmute-free way to say so is a match on
        // the count.
        if byte as usize >= TONES.len() {
            return None;
        }
        // SAFETY-free equivalent of a cast: an explicit table, so adding a
        // tone without adding it here fails to compile.
        Some(match byte {
            0 => Self::Seabed,
            1 => Self::Shallow,
            2 => Self::Sand,
            3 => Self::Shingle,
            4 => Self::Forest,
            5 => Self::GrassDark,
            6 => Self::Grass,
            7 => Self::GrassLight,
            8 => Self::Meadow,
            9 => Self::Heath,
            10 => Self::Upland,
            11 => Self::Fell,
            12 => Self::Rock,
            13 => Self::RockDark,
            14 => Self::Scree,
            15 => Self::Snow,
            16 => Self::Silt,
            17 => Self::Shoal,
            _ => Self::Marsh,
        })
    }

    /// The sRGB this tone is drawn in, before any shade step.
    pub fn color(self) -> Vec3 {
        TONES[self as usize]
    }
}

/// A lighter or darker cut of the same tone, so a big parcel of one colour
/// still breaks into facets rather than reading as one slab.
///
/// Three steps and not a multiplier curve: a gradient here would undo the
/// point of a quantised palette. Most triangles are [`Shade::Plain`] — only
/// the tails of the field behind it get shifted, so this reads as occasional
/// patches rather than as constant speckle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Shade {
    Dark = 0,
    Plain = 1,
    Light = 2,
}

impl Shade {
    /// What this step does to a tone.
    fn factor(self) -> f32 {
        match self {
            Self::Dark => 0.92,
            Self::Plain => 1.0,
            Self::Light => 1.09,
        }
    }
}

/// What one triangle of ground is painted: a tone, and how bright a cut of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Surface {
    pub tone: Tone,
    pub shade: Shade,
}

impl Surface {
    /// A tone at its plain shade, which is what everything but the vegetated
    /// bands ever wants.
    pub const fn plain(tone: Tone) -> Self {
        Self {
            tone,
            shade: Shade::Plain,
        }
    }

    pub const fn new(tone: Tone, shade: Shade) -> Self {
        Self { tone, shade }
    }

    /// The sRGB this surface is drawn in.
    ///
    /// Clamped because a light cut of an already bright tone leaves the range
    /// — [`Tone::Snow`] does, and would come out of a renderer as whatever
    /// that renderer does with a colour past white. Nothing that paints the
    /// world ever shades the snow, so the clamp is a guard on the arithmetic
    /// rather than a thing anyone sees; the tones that *are* shaded all have
    /// room for it, which the tests hold them to.
    pub fn color(self) -> Vec3 {
        (self.tone.color() * self.shade.factor()).clamp(Vec3::ZERO, Vec3::ONE)
    }

    /// The byte this travels as: the tone in the high bits, the shade in the
    /// low two. Nineteen tones and three shades, so a valid surface is always
    /// under 80 and a good part of the byte is spare.
    fn to_byte(self) -> u8 {
        ((self.tone as u8) << 2) | self.shade as u8
    }

    /// The surface a byte names, or `None` if it names none — an unknown tone,
    /// or the fourth shade that does not exist.
    fn from_byte(byte: u8) -> Option<Self> {
        let shade = match byte & 0b11 {
            0 => Shade::Dark,
            1 => Shade::Plain,
            2 => Shade::Light,
            _ => return None,
        };
        Some(Self {
            tone: Tone::from_byte(byte >> 2)?,
            shade,
        })
    }
}

// --- The payload ------------------------------------------------------------

/// One chunk of ground, as it crosses the wire.
///
/// Everything a renderer needs and nothing else. The corners are a
/// [`FACET_VERTS`]-square grid sampled from the chunk's lower corner outwards
/// at [`FACET_METRES`] spacing, row-major; the surfaces are one per triangle,
/// in the order the quads are walked and each quad's two triangles are split.
/// [`facets`] is the walk both ends build from, so neither has to restate the
/// order in prose.
///
/// A chunk of open ocean has no payload at all — see
/// [`crate::ToClient::Chunk`]. What arrives here is ground worth drawing.
#[derive(Clone, Debug, PartialEq)]
pub struct ChunkPayload {
    /// `FACET_VERTS * FACET_VERTS` corner heights, quantised — see
    /// [`quantize`].
    pub heights: Vec<u16>,
    /// [`FACET_TRIS`] surfaces, one per triangle.
    pub surfaces: Vec<Surface>,
    /// Where standing water above sea level covers this chunk, and how high
    /// it stands: one quantised level per corner on the same grid as
    /// [`ChunkPayload::heights`], or `None` for a chunk with no lake on or
    /// beside it — which is most of them.
    ///
    /// A stored [`NO_WATER`] means dry. Everywhere else the corner has a lake
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
    /// flood the other. Sent only where there is water because lakes are
    /// occasional — most ground carries none, and a grid of "dry" on every
    /// chunk in the world would be half as much again on the wire for
    /// nothing.
    pub water: Option<Vec<u16>>,
    /// The palms standing on this chunk, in no order anything may rely on
    /// beyond its being the same order every time.
    ///
    /// Here for the same reason a lake's level is: there is no arithmetic a
    /// client could do on the heights and surfaces it already has that would
    /// find them. Where a palm stands is a decision made against the seed —
    /// which the client has never seen and has no use for — so it travels, or
    /// two players anchored off the same beach would see different trees on
    /// it.
    ///
    /// A palm belongs to the chunk its foot stands in and to no other, so a
    /// client draws each exactly once and drops it with the ground it came
    /// on. What is *not* here is how high it stands: a palm's foot sits on
    /// the height field at its own position, which the client already holds
    /// and already interpolates for everything else that rides the world. A
    /// height sent alongside would be a second opinion about the same ground,
    /// and the one the eye would catch out — a palm hovering a hand's breadth
    /// over its own shadow.
    pub palms: Vec<Palm>,
}

/// One palm, standing on the ground of the chunk that carries it.
///
/// The tree itself is the drawing end's business — this says where one is and
/// how it is turned, not what a palm looks like, in the same way the wire
/// names a [`Tone`] rather than sending a colour per triangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palm {
    /// Where it stands, in metres from the chunk's own lower corner. Always
    /// inside the chunk — a palm on the far side of a boundary belongs to the
    /// chunk over there.
    pub at: Vec2,
    /// Which way it is turned about the vertical, in radians.
    ///
    /// A modelled palm leans, so its bearing is most of what stops a stand of
    /// them reading as one tree stamped repeatedly along a beach. Sent rather
    /// than derived from the position, so that a client picking its own would
    /// not be a client seeing a different beach.
    pub yaw: f32,
    /// How big, as a multiple of the model's own size — see [`PALM_SCALE_MIN`]
    /// and [`PALM_SCALE_MAX`].
    pub scale: f32,
}

/// The range a palm's size is drawn from. Narrow on purpose: a palm is a palm,
/// and enough variation to break up a row is far less than enough to read as
/// two different species.
pub const PALM_SCALE_MIN: f32 = 0.78;
pub const PALM_SCALE_MAX: f32 = 1.24;

/// The most palms one chunk may carry.
///
/// A ceiling rather than a target — palms stand along the back of a beach,
/// which is a thin band, and a chunk that is all beach still holds only a
/// fraction of this. It exists so that "how much can one answer cost" keeps
/// having an answer: it is what [`crate::ToClient`]'s frame ceiling is
/// derived against, and a reader refuses a chunk claiming more.
pub const MAX_PALMS: usize = 64;

/// Bytes one palm occupies: two per axis of its position, one for its bearing
/// and one for its size.
///
/// The position gets sixteen bits an axis because it is the one number here
/// the eye can check — a palm is drawn against a shadow it casts on ground the
/// client interpolates continuously, so a position on a coarse lattice would
/// put the tree beside its own foot. The bearing and the size get eight: a
/// palm turned to within a degree and a half, and sized to within half a
/// percent, is a palm nobody can tell from an exact one.
pub const PALM_BYTES: usize = 6;

impl Palm {
    fn put(&self, out: &mut Vec<u8>) {
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

        let span = PALM_SCALE_MAX - PALM_SCALE_MIN;
        let step = ((self.scale - PALM_SCALE_MIN) / span).clamp(0.0, 1.0) * u8::MAX as f32;
        out.push(step.round() as u8);
    }

    fn take(bytes: &[u8]) -> Self {
        let axis = |pair: &[u8]| {
            u16::from_le_bytes([pair[0], pair[1]]) as f32 / u16::MAX as f32 * CHUNK_METRES
        };
        Self {
            at: Vec2::new(axis(&bytes[0..2]), axis(&bytes[2..4])),
            yaw: bytes[4] as f32 / 256.0 * std::f32::consts::TAU,
            scale: PALM_SCALE_MIN
                + bytes[5] as f32 / u8::MAX as f32 * (PALM_SCALE_MAX - PALM_SCALE_MIN),
        }
    }
}

/// The stored water level that means no water — the bottom of the quantised
/// range, [`HEIGHT_FLOOR`], which is metres below the sea a lake cannot be
/// under. Standing water above sea level is the only kind that travels, so
/// every real level is far above this and there is no honest value to
/// collide with.
pub const NO_WATER: u16 = 0;

/// Bytes a chunk's heights and surfaces occupy on the wire: two per corner
/// height, one per triangle. Nothing is compressed — see the module docs on
/// the grid being the format, and note that delta-coding the heights and
/// run-coding the surfaces would take most of this back if the wire ever
/// needs it to.
pub const PAYLOAD_BYTES: usize = FACET_VERTS * FACET_VERTS * 2 + FACET_TRIS;

/// Bytes a chunk's water grid adds when it carries one — two per corner,
/// like the heights it is compared against.
pub const WATER_BYTES: usize = FACET_VERTS * FACET_VERTS * 2;

/// What one payload occupies on the wire, which depends on the two things
/// about a chunk that are not fixed: whether it carries standing water, and
/// how many palms stand on it. The flag and the count in
/// [`crate::ToClient::Chunk`] are what say which, and so how many bytes a
/// reader is about to be handed.
pub const fn payload_bytes(water: bool, palms: usize) -> usize {
    PAYLOAD_BYTES + if water { WATER_BYTES } else { 0 } + palms * PALM_BYTES
}

impl ChunkPayload {
    /// Whether this is a payload of the shape the grid says it should be.
    /// What a reader checks before believing a frame, and what a builder can
    /// assert against.
    pub fn well_formed(&self) -> bool {
        let corners = FACET_VERTS * FACET_VERTS;
        self.heights.len() == corners
            && self.surfaces.len() == FACET_TRIS
            && self
                .water
                .as_ref()
                .is_none_or(|water| water.len() == corners)
            && self.palms.len() <= MAX_PALMS
            && self.palms.iter().all(|palm| {
                (0.0..CHUNK_METRES).contains(&palm.at.x) && (0.0..CHUNK_METRES).contains(&palm.at.y)
            })
    }

    /// Appends this payload's bytes: every height little-endian, then every
    /// surface, then the water grid where there is one.
    ///
    /// The water goes after them, and the palms after that, so that a reader
    /// of any kind of chunk finds the heights and the surfaces at the same
    /// offsets — a lake and a stand of palms are things a chunk carries in
    /// addition, never a rearrangement of what it already carried.
    pub(crate) fn put(&self, out: &mut Vec<u8>) {
        debug_assert!(self.well_formed(), "not a chunk's worth of ground");
        for height in &self.heights {
            out.extend_from_slice(&height.to_le_bytes());
        }
        out.extend(self.surfaces.iter().map(|s| s.to_byte()));
        for level in self.water.iter().flatten() {
            out.extend_from_slice(&level.to_le_bytes());
        }
        for palm in &self.palms {
            palm.put(out);
        }
    }

    /// Reads a payload from exactly [`payload_bytes`] of them — `water` and
    /// `palms` say which length, and come from the flag and the count the
    /// caller has already read. `None` if the bytes are not that many, or if
    /// any surface byte names nothing this build knows.
    ///
    /// The length is checked rather than asserted because it is the one thing
    /// here a *frame* can be wrong about: the flag, the count and the length
    /// are written separately, so a build that disagreed with this one about
    /// how long a watered chunk is would otherwise be read as a chunk whose
    /// lake silently vanished.
    pub(crate) fn take(bytes: &[u8], water: bool, palms: usize) -> Option<Self> {
        if bytes.len() != payload_bytes(water, palms) || palms > MAX_PALMS {
            return None;
        }
        let levels = |bytes: &[u8]| {
            bytes
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<u16>>()
        };
        let (heights, rest) = bytes.split_at(FACET_VERTS * FACET_VERTS * 2);
        let (surfaces, rest) = rest.split_at(FACET_TRIS);
        let (water, palms) = rest.split_at(rest.len() - palms * PALM_BYTES);
        Some(Self {
            heights: levels(heights),
            surfaces: surfaces
                .iter()
                .map(|byte| Surface::from_byte(*byte))
                .collect::<Option<_>>()?,
            water: (!water.is_empty()).then(|| levels(water)),
            palms: palms.chunks_exact(PALM_BYTES).map(Palm::take).collect(),
        })
    }
}

/// One triangle of a chunk's facet grid: the three corners it is built from,
/// as indices into a payload's height grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Facet {
    /// The quad this belongs to, in grid coordinates from the chunk's lower
    /// corner.
    pub quad: (usize, usize),
    /// The three corners, each `(ix, iz)` into the height grid, wound
    /// counter-clockwise seen from above so the face normal points up.
    pub corners: [(usize, usize); 3],
}

/// Every triangle of a chunk, in the order a payload's surfaces are in.
///
/// The generator paints through this walk and a renderer builds through it, so
/// the two cannot fall out of step over which triangle a surface belongs to —
/// the ordering is written once, here, rather than twice in prose.
///
/// Which way a quad is split alternates like a checkerboard. Splitting every
/// quad the same way lines the facets up into an obvious herringbone across
/// open ground; alternating breaks that up without costing anything.
pub fn facets() -> impl Iterator<Item = Facet> {
    (0..FACET_QUADS).flat_map(|iz| {
        (0..FACET_QUADS).flat_map(move |ix| {
            let (tl, tr) = ((ix, iz), (ix + 1, iz));
            let (bl, br) = ((ix, iz + 1), (ix + 1, iz + 1));
            let split = if (ix + iz).is_multiple_of(2) {
                [[tl, bl, tr], [tr, bl, br]]
            } else {
                [[tl, bl, br], [tl, br, tr]]
            };
            split.into_iter().map(move |corners| Facet {
                quad: (ix, iz),
                corners,
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grid_divides_a_chunk_exactly() {
        assert_eq!(FACET_QUADS as f32 * FACET_METRES, CHUNK_METRES);
        assert_eq!(FACET_VERTS, 65);
        assert_eq!(FACET_TRIS, 8192);
        assert_eq!(PAYLOAD_BYTES, 65 * 65 * 2 + 8192);
        assert_eq!(WATER_BYTES, 65 * 65 * 2);
        assert_eq!(payload_bytes(false, 0), PAYLOAD_BYTES);
        assert_eq!(payload_bytes(true, 0), PAYLOAD_BYTES + WATER_BYTES);
        assert_eq!(
            payload_bytes(true, 3),
            PAYLOAD_BYTES + WATER_BYTES + 3 * PALM_BYTES
        );
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
    fn every_surface_survives_its_byte() {
        for tone in 0..TONES.len() as u8 {
            for shade in [Shade::Dark, Shade::Plain, Shade::Light] {
                let surface = Surface::new(Tone::from_byte(tone).expect("a tone"), shade);
                assert_eq!(Surface::from_byte(surface.to_byte()), Some(surface));
            }
        }
        // The fourth shade, and a tone past the end of the table.
        assert_eq!(Surface::from_byte(0b11), None);
        assert_eq!(Surface::from_byte((TONES.len() as u8) << 2), None);
    }

    #[test]
    fn the_palette_stays_inside_the_colours_there_are() {
        // Every combination is a colour a renderer can use, clamp and all.
        for tone in 0..TONES.len() as u8 {
            for shade in [Shade::Dark, Shade::Plain, Shade::Light] {
                let c = Surface::new(Tone::from_byte(tone).expect("a tone"), shade).color();
                assert!(
                    c.cmpge(Vec3::ZERO).all() && c.cmple(Vec3::ONE).all(),
                    "tone {tone} at {shade:?} is {c}"
                );
            }
        }

        // And only the snow reaches the clamp, which is why nothing shades
        // it: a lighter cut of any other tone still moves the colour by the
        // full step, so a shaded parcel really does break into three.
        for tone in 0..TONES.len() as u8 {
            let tone = Tone::from_byte(tone).expect("a tone");
            let lit = tone.color() * Shade::Light.factor();
            assert_eq!(
                lit.cmple(Vec3::ONE).all(),
                tone != Tone::Snow,
                "{tone:?} is the wrong side of white when lightened"
            );
        }
    }

    #[test]
    fn the_walk_covers_every_triangle_of_the_grid() {
        let all: Vec<Facet> = facets().collect();
        assert_eq!(all.len(), FACET_TRIS);

        // Every corner index is on the grid, and each quad contributes two
        // triangles that between them use all four of its corners.
        for pair in all.chunks_exact(2) {
            assert_eq!(pair[0].quad, pair[1].quad);
            let (ix, iz) = pair[0].quad;
            let mut used: Vec<(usize, usize)> = pair
                .iter()
                .flat_map(|facet| facet.corners)
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            used.sort();
            assert_eq!(
                used,
                [(ix, iz), (ix, iz + 1), (ix + 1, iz), (ix + 1, iz + 1)]
                    .into_iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
            );
        }
    }

    /// Palms enough to tell one from another, spread across the chunk so that
    /// a position written to the wrong axis would land outside it.
    fn some_palms(count: usize) -> Vec<Palm> {
        (0..count)
            .map(|i| Palm {
                at: Vec2::new(
                    i as f32 / MAX_PALMS as f32 * CHUNK_METRES,
                    CHUNK_METRES - 1.0 - i as f32,
                ),
                yaw: i as f32 / MAX_PALMS as f32 * std::f32::consts::TAU,
                scale: PALM_SCALE_MIN
                    + (i as f32 / MAX_PALMS as f32) * (PALM_SCALE_MAX - PALM_SCALE_MIN),
            })
            .collect()
    }

    /// A payload whose every value differs from every other, so anything that
    /// transposed or truncated one of its grids would show.
    fn a_payload(water: bool, palms: usize) -> ChunkPayload {
        ChunkPayload {
            heights: (0..FACET_VERTS * FACET_VERTS)
                .map(|i| (i * 7 % 65_535) as u16)
                .collect(),
            surfaces: (0..FACET_TRIS)
                .map(|i| {
                    Surface::new(
                        Tone::from_byte((i % 16) as u8).expect("a tone"),
                        [Shade::Dark, Shade::Plain, Shade::Light][i % 3],
                    )
                })
                .collect(),
            water: water.then(|| {
                (0..FACET_VERTS * FACET_VERTS)
                    .map(|i| (i * 11 % 65_533) as u16)
                    .collect()
            }),
            palms: some_palms(palms),
        }
    }

    #[test]
    fn a_payload_survives_its_bytes() {
        for water in [false, true] {
            for palms in [0, 1, MAX_PALMS] {
                let payload = a_payload(water, palms);
                assert!(payload.well_formed());

                let mut bytes = Vec::new();
                payload.put(&mut bytes);
                assert_eq!(bytes.len(), payload_bytes(water, palms));

                // Palms are the one part of a payload that does not survive
                // exactly — a position is sixteen bits an axis and a bearing
                // is eight — so they are compared to the tolerance the wire
                // promises rather than for equality.
                let back = ChunkPayload::take(&bytes, water, palms).expect("a payload");
                assert_eq!(back.heights, payload.heights);
                assert_eq!(back.surfaces, payload.surfaces);
                assert_eq!(back.water, payload.water);
                assert_eq!(back.palms.len(), payload.palms.len());
                for (got, sent) in back.palms.iter().zip(&payload.palms) {
                    assert!(
                        (got.at - sent.at).length() < 0.01,
                        "a palm at {:?} came back at {:?}",
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
    fn a_chunk_claiming_more_palms_than_it_may_is_refused() {
        // The ceiling is what the frame size is derived against, so a count
        // past it is a frame that could not have been written by a build that
        // agrees with this one about how much an answer costs.
        let payload = a_payload(false, MAX_PALMS);
        let mut bytes = Vec::new();
        payload.put(&mut bytes);
        bytes.extend_from_slice(&[0; PALM_BYTES]);
        assert_eq!(ChunkPayload::take(&bytes, false, MAX_PALMS + 1), None);
    }

    #[test]
    fn a_palm_outside_its_own_chunk_is_malformed() {
        // What a generator is held to. A palm belongs to the chunk its foot
        // stands in, so one placed past the boundary would be drawn by a
        // client that never asked for it — and drawn again by the chunk it
        // really stands on.
        let mut strayed = a_payload(false, 1);
        strayed.palms[0].at.x = CHUNK_METRES;
        assert!(!strayed.well_formed());
        strayed.palms[0].at = Vec2::new(1.0, -0.5);
        assert!(!strayed.well_formed());
    }

    #[test]
    fn the_water_grid_is_the_only_thing_a_lake_adds() {
        // A watered payload is a dry one with a grid on the end: the heights
        // and the surfaces encode to exactly the same bytes in the same
        // places, so a reader of either finds them without knowing which it
        // has until it reaches the tail.
        let (mut dry, mut wet) = (Vec::new(), Vec::new());
        a_payload(false, 0).put(&mut dry);
        a_payload(true, 0).put(&mut wet);
        assert_eq!(wet[..PAYLOAD_BYTES], dry[..], "the water moved the ground");
        assert_eq!(wet.len() - dry.len(), WATER_BYTES);
    }

    #[test]
    fn a_payload_with_a_surface_from_the_future_is_refused() {
        // The last surface byte, which is the last byte of a dry payload and
        // sits in the middle of a watered one — so this also catches a reader
        // that stopped checking surfaces once it knew there was water to come.
        for water in [false, true] {
            let mut bytes = Vec::new();
            a_payload(water, 2).put(&mut bytes);
            bytes[PAYLOAD_BYTES - 1] = 0xFF;
            assert_eq!(ChunkPayload::take(&bytes, water, 2), None);
        }
    }

    #[test]
    fn a_payload_is_malformed_if_its_grids_are_the_wrong_size() {
        // What a builder is held to. A short water grid is the one worth
        // naming: heights and surfaces have been fixed-size since there was a
        // wire, but the water is built per chunk from whatever ground has a
        // lake on it, and a payload carrying half a grid would encode to a
        // frame no reader could believe.
        let mut short = a_payload(true, 0);
        short.water.as_mut().expect("a lake").truncate(4);
        assert!(!short.well_formed());

        let mut empty = a_payload(true, 0);
        empty.water = Some(Vec::new());
        assert!(!empty.well_formed());

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
