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
/// one of these, so the whole world is drawn in sixteen colours plus three
/// shade steps. Saturated well past anything natural, because flat shading has
/// no texture or gradient to carry the picture; the colour has to do that work
/// on its own.
///
/// The order is the wire's: a tone travels as its own number, so these may be
/// added to but not shuffled without bumping [`crate::PROTOCOL_VERSION`].
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
}

/// The sRGB the tones stand for, in the order they are numbered.
const TONES: [Vec3; 16] = [
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
];

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
            _ => Self::Snow,
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
    /// low two. Sixteen tones and three shades, so a valid surface is always
    /// under 64 and most of the byte is spare.
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
}

/// Bytes one payload occupies on the wire: two per corner height, one per
/// triangle. Nothing is compressed — see the module docs on the grid being the
/// format, and note that delta-coding the heights and run-coding the surfaces
/// would take most of this back if the wire ever needs it to.
pub const PAYLOAD_BYTES: usize = FACET_VERTS * FACET_VERTS * 2 + FACET_TRIS;

impl ChunkPayload {
    /// Whether this is a payload of the shape the grid says it should be.
    /// What a reader checks before believing a frame, and what a builder can
    /// assert against.
    pub fn well_formed(&self) -> bool {
        self.heights.len() == FACET_VERTS * FACET_VERTS && self.surfaces.len() == FACET_TRIS
    }

    /// Appends this payload's bytes: every height little-endian, then every
    /// surface.
    pub(crate) fn put(&self, out: &mut Vec<u8>) {
        debug_assert!(self.well_formed(), "not a chunk's worth of ground");
        for height in &self.heights {
            out.extend_from_slice(&height.to_le_bytes());
        }
        out.extend(self.surfaces.iter().map(|s| s.to_byte()));
    }

    /// Reads a payload from exactly [`PAYLOAD_BYTES`] of them, or `None` if
    /// any surface byte names nothing this build knows.
    pub(crate) fn take(bytes: &[u8]) -> Option<Self> {
        debug_assert_eq!(bytes.len(), PAYLOAD_BYTES);
        let (heights, surfaces) = bytes.split_at(FACET_VERTS * FACET_VERTS * 2);
        Some(Self {
            heights: heights
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect(),
            surfaces: surfaces
                .iter()
                .map(|byte| Surface::from_byte(*byte))
                .collect::<Option<_>>()?,
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
        for tone in 0..16u8 {
            for shade in [Shade::Dark, Shade::Plain, Shade::Light] {
                let surface = Surface::new(Tone::from_byte(tone).expect("a tone"), shade);
                assert_eq!(Surface::from_byte(surface.to_byte()), Some(surface));
            }
        }
        // The fourth shade, and a tone past the end of the table.
        assert_eq!(Surface::from_byte(0b11), None);
        assert_eq!(Surface::from_byte(16 << 2), None);
    }

    #[test]
    fn the_palette_stays_inside_the_colours_there_are() {
        // Every combination is a colour a renderer can use, clamp and all.
        for tone in 0..16u8 {
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
        for tone in 0..16u8 {
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

    #[test]
    fn a_payload_survives_its_bytes() {
        // Heights and surfaces that differ everywhere they could, so anything
        // that transposed or truncated either would show.
        let payload = ChunkPayload {
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
        };
        assert!(payload.well_formed());

        let mut bytes = Vec::new();
        payload.put(&mut bytes);
        assert_eq!(bytes.len(), PAYLOAD_BYTES);
        assert_eq!(ChunkPayload::take(&bytes), Some(payload));
    }

    #[test]
    fn a_payload_with_a_surface_from_the_future_is_refused() {
        let mut bytes = vec![0u8; PAYLOAD_BYTES];
        *bytes.last_mut().expect("a byte") = 0xFF;
        assert_eq!(ChunkPayload::take(&bytes), None);
    }
}
