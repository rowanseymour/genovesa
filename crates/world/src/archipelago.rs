//! The open world: an endless ocean scattered with islands.
//!
//! There is no world map to generate up front, and nothing here ever looks at
//! the whole world at once. The plane is divided into square **parcels** on
//! two scales, each parcel decides for itself — from the world seed and its
//! own coordinates, nothing else — whether it holds an island and what kind,
//! and each island is then generated exactly as a lone map would be:
//! [`TerrainGenerator`] fitted to its own bounded frame, in its own local
//! coordinates. That reuse is the whole design. Everything the generator
//! guarantees a map — its land share, its fitted sea level, its coasts, the
//! open water at its rim — it now guarantees per island, and the world is
//! just a layout of such maps with ocean floor between them.
//!
//! Generating an island costs real time (tens to hundreds of milliseconds),
//! so [`Archipelago`] carries a cache and fills it lazily: the *layout* — who
//! is where, how big — is a few hash mixes and can be asked about any region
//! for free, while the terrain of an island is only paid for when something
//! wants its ground.
//!
//! # How endless is endless
//!
//! World coordinates are `f32` metres, so the horizon is the arithmetic's
//! rather than the layout's. Out to about **1,280 km** neighbouring
//! coordinates are 0.15 m apart and the ground is exactly the ground; by
//! **12,800 km** they are past the height field's half-metre step and
//! coastlines quantise; by **128,000 km** they are past
//! [`protocol::ground::CELL_METRES`] outright and the ground is flat, there
//! being nowhere between the facets left to sample. A player panning at the
//! camera's own speed reaches the first in something over a year of
//! continuous play, so "infinite-ish" is a measurement rather than a hope.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use glam::{IVec2, UVec2, Vec2, Vec3};
use protocol::ground::{quantize, ChunkPayload, Material, ANCHOR_DEPTH};

use crate::noise::smoothstep;
use crate::sunlight::Sunlight;
use crate::terrain::{
    cell_materials, corner_heights, corner_lit, corner_water, normal_at, MapConfig,
    TerrainGenerator, CHUNK_TILES, MAX_DEPTH, TILE_SIZE,
};

pub use protocol::ground::{chunk_at, CHUNK_METRES};

/// Depth of the open ocean floor between islands, in metres. The same floor
/// every island's own sea bed is clamped to, so an island's rim and the ocean
/// around it meet at one level.
pub const OCEAN_DEPTH: f32 = MAX_DEPTH;

// --- The layout --------------------------------------------------------------
//
// One scale of parcel cannot hold an ocean worth sailing. Islands the size the
// generator likes best run a few hundred metres, and a world wants them a
// comfortable sail apart — but it also wants the occasional landmass big
// enough to feel like arriving somewhere, and a parcel able to hold a
// three-kilometre island puts even the small ones three kilometres apart.
//
// So there are two layers of parcels, laid over the same plane. A coarse
// layer holds the big islands, sparsely; a fine layer holds the small ones,
// densely; and where a small island's ground would crowd a big island's, the
// small one simply is not there — decided by a purely local test, so no
// parcel ever has to know about more than its immediate neighbourhood. What
// survives is the mix the ocean wants: frequent small islands, the odd big
// one, and skerry-belts of islets standing off a big island's coast at a
// respectful distance.

/// One scale of the layout. All lengths are in chunks, because islands are
/// whole chunks: keeping the layout on the chunk grid means a streamed chunk
/// belongs to exactly one island or to the open ocean, never partly to either.
struct Layer {
    /// Chunks along the edge of one parcel.
    parcel: i32,
    /// Smallest and largest island the layer places, in chunks along its
    /// longer axis.
    size: (f32, f32),
    /// Share of this layer's parcels that hold an island at all. The gaps are
    /// what stop the ocean reading as a lattice of land.
    occupancy: f32,
    /// Chunks of guaranteed sea between an island's frame and its parcel's
    /// edge. Islands of one layer can therefore never touch, whatever their
    /// parcels decide — twice this, at least, always separates them.
    ///
    /// It must also be at least [`SKIRT_CHUNKS`], and that is load-bearing
    /// rather than incidental: an island's skirt reaches one chunk past its
    /// frame, and [`Archipelago::island_at_chunk`] finds the island answerable
    /// for a chunk by looking in that chunk's *own* parcel and nowhere else.
    /// A margin narrower than the skirt would let a skirt chunk fall in the
    /// neighbouring parcel, where nothing would ever think to ask for it, and
    /// the island would stream in with a hole along that edge.
    margin: i32,
}

/// The two scales: big islands on five-kilometre parcels, half of them empty,
/// and small ones on 1280-metre parcels, most of them full. The numbers are a
/// balance between two sails — from a small island the next land should be
/// visible ambition rather than a committed voyage, while a big island should
/// stay rare enough that reaching one still counts for something.
const LAYERS: [Layer; 2] = [
    Layer {
        parcel: 40,
        size: (12.0, 28.0),
        occupancy: 0.5,
        margin: 3,
    },
    Layer {
        parcel: 10,
        size: (1.0, 7.0),
        occupancy: 0.8,
        margin: 1,
    },
];

/// Chunks of sea guaranteed between a small island's frame and a big one's,
/// on top of each frame's own sea margin. Close enough that the small fry
/// read as *that island's* skerries, far enough that neither map's fitted
/// coast ever has to know the other exists.
const CLEARANCE: i32 = 3;

/// Chunks of guaranteed open water around the origin. No island's frame ever
/// stands within this many chunks of it, on any seed — measured per axis like
/// every other layout gap, so the clearing is a square.
///
/// The origin is the world's one fixed point — where [`Archipelago::spawn`]
/// starts measuring — and the clearing is what keeps that point honest:
/// should the layout ever fail to offer an island at all, the origin is
/// still open water to enter on, on every seed rather than on essentially
/// every one. Two chunks is a modest clearing — a four-by-four-chunk square
/// of sea, a quarter kilometre to the nearest possible coast.
const SPAWN_CLEARING: i32 = 2;

/// Metres of open water between a world's spawn point and the waterline it
/// faces. A judgement about the *camera*, not the layout: at the default
/// zoom the eye sees a few dozen metres past the boat, so this is what puts
/// the first coast on or just off the opening screen rather than a rumour
/// beyond it — arrival in sight of land, with a boat-length or two of
/// margin over a server's [scatter] and the waterline search's own stride.
///
/// [scatter]: Archipelago::spawn
///
/// An offing, not a promise: [`berth_off`] takes what of it the coast can
/// give.
pub const SPAWN_OFFSHORE: f32 = 48.0;

/// The water a hull is put down in, in metres of depth: enough to float it
/// well clear of its own draft, and comfortably inside [`ANCHOR_DEPTH`] —
/// a hull is put down to be *left*, at entry and at the console's berth
/// alike, and the first thing asked of it is the one grant the server
/// refuses in deep water. The margins on both ends cover the sounding
/// stride and the server's scatter of arrivals on any shelving coast.
const BERTH_DEPTHS: (f32, f32) = (2.0, ANCHOR_DEPTH - 1.5);

/// Whether ground standing `height` metres above sea level — negative under
/// water — is a berth. The one place the sign is turned round, so no caller
/// re-derives it.
pub fn a_berth(height: f32) -> bool {
    (-BERTH_DEPTHS.1..=-BERTH_DEPTHS.0).contains(&height)
}

/// Where a hull is put down off a coast: the furthest sounding within
/// [`SPAWN_OFFSHORE`] of `wet` — itself a sounding at or just off the
/// waterline — along the unit direction `seaward`, that [`a_berth`] accepts.
/// Dry soundings along the way (a spit, an islet beside the line) are
/// stepped past rather than ending the walk. Where no sounding qualifies —
/// a wall of a coast, plunging straight past the band — the deepest wet
/// sounding stands in: wet-but-deep leaves the hull afloat and sailable,
/// where dry or ankle-deep leaves it aground.
pub fn berth_off(wet: Vec2, seaward: Vec2, height: impl Fn(f32, f32) -> f32) -> Vec2 {
    let stride = seaward * SOUNDING;
    let (mut fallback, mut deepest) = (wet, height(wet.x, wet.y));
    let mut berth = None;
    for i in 0..(SPAWN_OFFSHORE / SOUNDING) as i32 {
        let at = wet + stride * i as f32;
        let h = height(at.x, at.y);
        if a_berth(h) {
            berth = Some(at);
        }
        if h < deepest {
            (fallback, deepest) = (at, h);
        }
    }
    berth.unwrap_or(fallback)
}

/// Coarse parcels a widening island search spans, doubling until one of them
/// answers. One coarse parcel is five kilometres and half its parcels hold an
/// island, so the first window nearly always does.
///
/// Widening past the last would be searching an ocean that, by the occupancy
/// the layers are written to, cannot be that empty — so finding nothing by
/// then means the layout is broken, not that the sea is wide.
const SPANS: [f32; 4] = [1.0, 2.0, 4.0, 8.0];

/// Metres between soundings when a walk over the heights goes looking for
/// the waterline — [`Archipelago::spawn`]'s walk in from the sea, and the
/// server console's outward from a point somebody asked to be taken to.
/// Fine enough not to step over a beach (coasts the generator draws are
/// hundreds of metres long), coarse enough that a walk costs a few hundred
/// height samples at worst.
pub const SOUNDING: f32 = 4.0;

/// Skews a uniform draw towards zero, so that island sizes come out mostly
/// small: the median lands in the lower quarter of its layer's range and the
/// top of the range stays a rare event, which is the right way round — an
/// ocean of middling islands has no landmarks in it.
///
/// This is `0.8u² + 0.2u³`, which stands in for an exponent of about 2.2 to
/// within a couple of percent across the unit interval. The exponent is what
/// this was written as first, and `f32::powf` is exactly what it cannot be.
/// The layout is a *format*: a seed has to lay out the same islands on every
/// machine there will ever be, and a size in whole chunks is quantised — so a
/// single-ULP difference in `powf` between two platforms is not a rounding
/// error in a height, it is one island a chunk wider than another machine's,
/// standing at a different origin, with a different map inside its frame. Add
/// and multiply are pinned by IEEE 754; transcendentals are not. See
/// `a_seed_is_the_same_world_down_to_the_bit`, which exists to forbid exactly
/// this drifting.
///
/// The first CI run to put that test on three operating systems proved the
/// point twice over: this polynomial gave the same layout on all three, while
/// the height field — which was still calling `f32::powf` — gave three
/// different maps. Heights now go through [`crate::terrain::pow`], so the rest
/// of the world keeps the promise this function was written to keep. A
/// polynomial is still the better answer where one will do, being both exact
/// and free.
fn skewed_small(u: f32) -> f32 {
    u * u * (0.8 + 0.2 * u)
}

/// How far below 1.0 an island's aspect ratio may fall — the shorter axis is
/// the longer times a draw from `SQUEEZE..1.0`. Enough to make ovals and
/// oblongs of them without producing ribbons the generator was never judged
/// on.
const SQUEEZE: f32 = 0.55;

/// Chunks of ocean around an island's frame that still belong to it — where
/// its sea bed is let down onto the flat ocean floor.
///
/// The handover deliberately happens *outside* the frame, not in a band
/// inside it. Inside the frame the island is its map, untouched to the last
/// sample: a band inside was tried first, and on small islands it guillotined
/// the shallow banks — their fitted seas run bright and shallow right to the
/// frame, and any blend short enough to spare the lagoon was a hard line on
/// the page.
///
/// Measured on generated islands, the generator's field is *already* exactly
/// `-OCEAN_DEPTH` at the frame and past it — the rim is the extreme of the
/// distribution `DEEP_FRACTION` anchors — so the blend in [`Island::height`]
/// blends the floor into the floor and nothing in the picture depends on it.
///
/// It is kept as the *guarantee* rather than as a mechanism: nothing in the
/// generator promises a rim at full depth, and a re-fitted calibration that
/// left it a metre shy would print every island's chunk rectangle onto the
/// open water as a step.
///
/// One chunk, and it cannot be more: layout margins guarantee two chunks of
/// gap between islands of a layer and [`CLEARANCE`] across layers, and skirts
/// must never overlap — a chunk of the world belongs to one island or to
/// nobody. It cannot exceed any [`Layer::margin`] either, for the reason given
/// there.
const SKIRT_CHUNKS: i32 = 1;

/// The skirt's width in metres — how far past the frame an island's bed takes
/// to reach the floor.
const SKIRT_METRES: f32 = SKIRT_CHUNKS as f32 * CHUNK_METRES;

/// Parameters the world is generated from — the whole of them: a seed is a
/// world.
///
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorldConfig {
    pub seed: u32,
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self { seed: 20_040_112 }
    }
}

/// One island's place in the world, before any terrain exists: where it
/// stands, how big it is, and the seed its map is generated from. Cheap to
/// compute and to compare, which is what lets streaming ask "what is out
/// there?" across kilometres without generating anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IslandSpec {
    /// World chunk coordinate of the island's lower corner.
    pub origin: IVec2,
    /// Chunks along X and Z — the island's own [`MapConfig`] shape.
    pub chunks: UVec2,
    /// The island map's seed, drawn from the world seed and the parcel.
    pub seed: u32,
}

impl IslandSpec {
    /// The island's map, exactly as a lone map of that shape and seed.
    pub fn config(&self) -> MapConfig {
        MapConfig {
            chunks: self.chunks,
            seed: self.seed,
        }
    }

    /// The island's extent in metres, per axis.
    pub fn extent(&self) -> Vec2 {
        (self.chunks * CHUNK_TILES).as_vec2() * TILE_SIZE
    }

    /// World coordinate of the island map's centre — the origin of its own
    /// local frame.
    pub fn centre(&self) -> Vec2 {
        self.origin.as_vec2() * CHUNK_METRES + self.extent() * 0.5
    }

    /// One past the island's last chunk, per axis.
    fn end(&self) -> IVec2 {
        self.origin + self.chunks.as_ivec2()
    }

    /// The chunk-grid rectangle this island is answerable for — its map plus
    /// its skirt — as an inclusive lower corner and an exclusive upper one.
    /// Streaming builds exactly these chunks and no others.
    pub fn covered(&self) -> (IVec2, IVec2) {
        (self.origin - SKIRT_CHUNKS, self.end() + SKIRT_CHUNKS)
    }

    /// Whether this world chunk is one of the island's own, or of its skirt —
    /// the chunks whose ground this island is answerable for.
    pub fn covers_chunk(&self, chunk: IVec2) -> bool {
        let (min, max) = self.covered();
        chunk.cmpge(min).all() && chunk.cmplt(max).all()
    }

    /// The point of the island's frame nearest a world point — the point
    /// itself anywhere inside the frame. What [`Archipelago::spawn`] measures
    /// coasts by: the frame is not the waterline, but land never stands
    /// outside it, so distance to the frame is the honest lower bound on the
    /// sail to this island.
    pub fn frame_point(&self, world: Vec2) -> Vec2 {
        let min = self.origin.as_vec2() * CHUNK_METRES;
        world.clamp(min, min + self.extent())
    }

    /// How far past the island's frame a world point stands, in metres —
    /// zero anywhere inside it. Chebyshev, like the skirt of chunks it is
    /// read across.
    fn beyond_frame(&self, world: Vec2) -> f32 {
        let out = (world - self.centre()).abs() - self.extent() * 0.5;
        out.max(Vec2::ZERO).max_element()
    }
}

/// Where a world is entered, as [`Archipelago::spawn`] answers it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spawn {
    /// The spawn point itself: water, [`SPAWN_OFFSHORE`] metres off the
    /// island's shore on the side facing the origin.
    pub point: Vec2,
    /// The island the entry stands off — the first land in sight.
    pub island: IslandSpec,
}

/// A deterministic stream of draws for one parcel — splitmix64, whose whole
/// purpose is turning correlated states (neighbouring parcels differ by one
/// bit or two) into unrelated sequences.
///
/// The order of draws below is part of the world format: reordering them
/// reshuffles every world, exactly as changing the noise would.
struct ParcelRng(u64);

impl ParcelRng {
    fn new(seed: u32, layer: usize, parcel: IVec2) -> Self {
        let mut state = (seed as u64).wrapping_mul(0xA24B_AED4_963E_E407);
        state ^= (parcel.x as u32 as u64) | ((parcel.y as u32 as u64) << 32);
        Self(state.wrapping_add((layer as u64) << 17))
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0.0..1.0`, from the high bits.
    fn uniform(&mut self) -> f32 {
        (self.next() >> 40) as f32 * (1.0 / (1 << 24) as f32)
    }
}

/// One generated island: its place, and the fitted generator that is its
/// terrain. Shared out of the cache behind an [`Arc`], because chunk builds on
/// other threads keep hold of it while they sample.
pub struct Island {
    pub spec: IslandSpec,
    generator: TerrainGenerator,
    /// When every point of the island sees the sun — baked here, with the
    /// lakes already flooded, because only whoever holds the whole island
    /// can say. See [`crate::sunlight`].
    sunlight: Sunlight,
}

impl Island {
    fn generate(spec: IslandSpec) -> Self {
        let generator = TerrainGenerator::new(&spec.config());
        // Over every chunk the island answers for, reading the surface the
        // sun actually strikes: the ground, the sea over the drowned shelf,
        // or the lake standing in a basin.
        let (lo, hi) = spec.covered();
        let sunlight = Sunlight::bake(
            lo.as_vec2() * CHUNK_METRES,
            hi.as_vec2() * CHUNK_METRES,
            |wx, wz| {
                let surface = ground_height(&spec, &generator, wx, wz).max(0.0);
                let local = Vec2::new(wx, wz) - spec.centre();
                match generator.lake_level(local.x, local.y) {
                    Some(level) => surface.max(level),
                    None => surface,
                }
            },
        );
        Self {
            spec,
            generator,
            sunlight,
        }
    }

    /// Height at a world point, in metres. Inside the frame this is the
    /// island's map, untouched; across the skirt it is that same field walked
    /// down to exactly [`-OCEAN_DEPTH`] by the skirt's outer edge, where the
    /// open ocean takes over without a seam — see [`ground_height`].
    pub fn height(&self, wx: f32, wz: f32) -> f32 {
        ground_height(&self.spec, &self.generator, wx, wz)
    }

    /// What the ground is painted at a world point, matching
    /// [`Island::height`].
    pub fn material(&self, wx: f32, wz: f32, height: f32, normal: Vec3) -> Material {
        let local = Vec2::new(wx, wz) - self.spec.centre();
        self.generator.material(local.x, local.y, height, normal)
    }

    /// Surface normal at a world point, from central differences one tile out.
    pub fn normal(&self, wx: f32, wz: f32) -> Vec3 {
        normal_at(wx, wz, |x, z| self.height(x, z))
    }

    /// The surface level of the lake standing at or beside a world point —
    /// [`TerrainGenerator::lake_level`], in world coordinates. Lakes are the
    /// island's own: past the frame there is only the sea, and the answer out
    /// there is `None` without needing a guard, since the lake grid's edge
    /// cells are the map's guaranteed sea margin.
    pub fn lake_level(&self, wx: f32, wz: f32) -> Option<f32> {
        let local = Vec2::new(wx, wz) - self.spec.centre();
        self.generator.lake_level(local.x, local.y)
    }

    /// When the surface at a world point sees the sun — the island's bake,
    /// read back for every corner a payload carries. See
    /// [`protocol::ground::ChunkPayload::lit`] for what the pair means.
    pub fn lit(&self, wx: f32, wz: f32) -> [u8; 2] {
        self.sunlight.at(wx, wz)
    }
}

/// [`Island::height`] before there is an [`Island`] to ask: the generator's
/// field inside the frame, walked down to exactly [`-OCEAN_DEPTH`] across the
/// skirt. A free function because [`Island::generate`] bakes the sunlight
/// against this same surface while the struct is still being put together.
///
/// In practice the walk has nowhere to go. As `SKIRT_CHUNKS` explains, the
/// generator's field is already at the floor by the frame, so the blend below
/// is `-OCEAN_DEPTH` blended into `-OCEAN_DEPTH` on every island measured. It
/// stays because it is what *makes* that true rather than merely observing it
/// — the seam this would show is a step of ocean bed around every island in
/// the world, and the fix belongs where it cannot be forgotten.
fn ground_height(spec: &IslandSpec, generator: &TerrainGenerator, wx: f32, wz: f32) -> f32 {
    let beyond = spec.beyond_frame(Vec2::new(wx, wz));
    if beyond >= SKIRT_METRES {
        return -OCEAN_DEPTH;
    }
    let local = Vec2::new(wx, wz) - spec.centre();
    let h = generator.height(local.x, local.y);
    h + (-OCEAN_DEPTH - h) * smoothstep(0.0, SKIRT_METRES, beyond)
}

/// The world: its layout, asked about for free, and its islands, generated on
/// demand and cached.
///
/// Everything is deterministic in the world seed — a seed is a world, whoever
/// generates it, in whatever order its islands happen to be visited.
pub struct Archipelago {
    seed: u32,
    /// Islands generated so far. The [`OnceLock`] is what makes concurrent
    /// demand cheap: the map is locked only long enough to find or add an
    /// island's slot, generation happens outside the lock, and two threads
    /// wanting the same island block on its slot rather than generating it
    /// twice.
    islands: RwLock<HashMap<IslandSpec, Arc<OnceLock<Arc<Island>>>>>,
}

impl Archipelago {
    pub fn new(config: &WorldConfig) -> Self {
        Self {
            seed: config.seed,
            islands: RwLock::new(HashMap::new()),
        }
    }

    /// The seed this world is, for whoever has to name it: a world that
    /// cannot say which one it is cannot be opened a second time.
    pub fn seed(&self) -> u32 {
        self.seed
    }

    /// What a parcel of one layer holds, before the layers are played off
    /// against each other. Everything about the island — whether it exists,
    /// its shape, where in the parcel it stands, its seed — comes from the
    /// parcel's own dice.
    fn raw_spec(&self, layer: usize, parcel: IVec2) -> Option<IslandSpec> {
        let l = &LAYERS[layer];
        let mut rng = ParcelRng::new(self.seed, layer, parcel);

        if rng.uniform() >= l.occupancy {
            return None;
        }

        // The long axis, skewed small; the short axis, squeezed under it; and
        // a coin for which is which. Rounded to whole chunks at the end, so
        // the distribution is continuous even though the maps are not.
        let long = l.size.0 + (l.size.1 - l.size.0) * skewed_small(rng.uniform());
        let short = long * (SQUEEZE + (1.0 - SQUEEZE) * rng.uniform());
        let (sx, sz) = if rng.next() & 1 == 0 {
            (long, short)
        } else {
            (short, long)
        };
        let clamp = |s: f32| (s.round() as i32).clamp(1, l.size.1 as i32);
        let chunks = IVec2::new(clamp(sx), clamp(sz));

        // Where in the parcel it stands: anywhere that keeps the margin.
        //
        // The span is what the parcel has left over once the island and both
        // margins are taken out, and every [`LAYERS`] entry is written so that
        // it cannot go negative — the largest island a layer draws plus two
        // margins fits inside its parcel with room to spare. Asserted rather
        // than clamped because a negative span means the table is wrong, and
        // the failure is quiet either way: at exactly -1 this divides by zero,
        // and below that the cast wraps to an enormous modulus and scatters
        // islands out of their own parcels.
        let jitter = |rng: &mut ParcelRng, island: i32| {
            let span = l.parcel - island - 2 * l.margin;
            debug_assert!(
                span >= 0,
                "layer parcel {} cannot hold a {island}-chunk island with {}-chunk margins",
                l.parcel,
                l.margin
            );
            l.margin + (rng.next() % (span + 1) as u64) as i32
        };
        let offset = IVec2::new(jitter(&mut rng, chunks.x), jitter(&mut rng, chunks.y));

        let spec = IslandSpec {
            origin: parcel * l.parcel + offset,
            chunks: chunks.as_uvec2(),
            seed: (rng.next() >> 32) as u32,
        };

        // The one veto that is not the parcel's own dice: nothing may stand
        // in the spawn clearing. Suppressed here, as though the island was
        // never drawn, rather than in `spec_at` beside the cross-layer test —
        // an island that is not there suppresses no skerries, so land can
        // ring the clearing as densely as the layers allow.
        let apart = spec.origin.max(-spec.end()).max(IVec2::ZERO);
        if apart.max_element() < SPAWN_CLEARING {
            return None;
        }
        Some(spec)
    }

    /// What a parcel of one layer actually holds: its raw island, unless a
    /// bigger layer's island stands too close, in which case nothing. The
    /// test is local — only the coarse parcels the clearance rectangle
    /// touches are consulted — so any point's layout is decided by a handful
    /// of hash mixes, wherever it is.
    pub fn spec_at(&self, layer: usize, parcel: IVec2) -> Option<IslandSpec> {
        let spec = self.raw_spec(layer, parcel)?;

        for (bigger, l) in LAYERS[..layer].iter().enumerate() {
            let min = (spec.origin - CLEARANCE).div_euclid(IVec2::splat(l.parcel));
            let max = (spec.end() + CLEARANCE - 1).div_euclid(IVec2::splat(l.parcel));
            for pz in min.y..=max.y {
                for px in min.x..=max.x {
                    let Some(big) = self.raw_spec(bigger, IVec2::new(px, pz)) else {
                        continue;
                    };
                    let apart = spec.origin - CLEARANCE - big.end();
                    let apart = apart.max(big.origin - CLEARANCE - spec.end());
                    if apart.max_element() < 0 {
                        return None;
                    }
                }
            }
        }
        Some(spec)
    }

    /// The island answerable for this world chunk — map or skirt — if any.
    /// At most one can be: islands of one layer keep two margins apart inside
    /// their own parcels, islands of different layers keep [`CLEARANCE`]
    /// apart, and both gaps exceed two skirts.
    pub fn island_at_chunk(&self, chunk: IVec2) -> Option<IslandSpec> {
        (0..LAYERS.len()).find_map(|layer| {
            let parcel = chunk.div_euclid(IVec2::splat(LAYERS[layer].parcel));
            self.spec_at(layer, parcel)
                .filter(|s| s.covers_chunk(chunk))
        })
    }

    /// Every island whose map touches the given world-space rectangle —
    /// layout only, generating nothing. What streaming and the plan renderer
    /// ask before deciding which ground to pay for.
    pub fn islands_within(&self, min: Vec2, max: Vec2) -> Vec<IslandSpec> {
        let min_chunk = (min / CHUNK_METRES).floor().as_ivec2();
        let max_chunk = (max / CHUNK_METRES).ceil().as_ivec2();

        let mut found = Vec::new();
        for (layer, l) in LAYERS.iter().enumerate() {
            let lo = min_chunk.div_euclid(IVec2::splat(l.parcel));
            let hi = max_chunk.div_euclid(IVec2::splat(l.parcel));
            for pz in lo.y..=hi.y {
                for px in lo.x..=hi.x {
                    let Some(spec) = self.spec_at(layer, IVec2::new(px, pz)) else {
                        continue;
                    };
                    let (lo, hi) = spec.covered();
                    if lo.cmple(max_chunk).all() && hi.cmpge(min_chunk).all() {
                        found.push(spec);
                    }
                }
            }
        }
        found
    }

    /// The island nearest a world point, ranked by whichever of its points
    /// `mark` names — layout only, generating nothing.
    ///
    /// Searched over windows that double until one answers, rather than over
    /// a single wide one: the layout is cheap but not free, and the first
    /// window nearly always answers, so the common case sweeps a couple of
    /// parcels instead of a few hundred.
    ///
    /// A window's own minimum is not the answer until it stands *within* that
    /// window's reach. The windows are squares and the ranking is a straight
    /// line, so an island just outside a window's edge can be nearer than one
    /// inside its corner — a minimum out past the reach therefore widens the
    /// search instead of ending it. The last window answers regardless, there
    /// being nothing wider to ask.
    ///
    /// `islands_within` sweeps its parcels in a fixed order and `min_by`
    /// keeps the first of any tie, so the answer is the seed's and not the
    /// machine's.
    fn nearest_by(&self, near: Vec2, mark: impl Fn(&IslandSpec) -> Vec2) -> Option<IslandSpec> {
        let parcel = LAYERS[0].parcel as f32 * CHUNK_METRES;
        for (i, span) in SPANS.iter().enumerate() {
            let reach = span * parcel;
            let nearest = self
                .islands_within(near - reach, near + reach)
                .into_iter()
                .min_by(|a, b| {
                    let d = |s: &IslandSpec| mark(s).distance_squared(near);
                    d(a).total_cmp(&d(b))
                });
            match nearest {
                Some(island) if mark(&island).distance(near) <= reach => return Some(island),
                Some(island) if i + 1 == SPANS.len() => return Some(island),
                _ => {}
            }
        }
        None
    }

    /// Where this world is entered: a point of open water [`SPAWN_OFFSHORE`]
    /// metres off the waterline of the island whose *frame* is nearest the
    /// origin, facing it from the origin's side.
    ///
    /// Asked once, when a server binds, and sent in every welcome — a client
    /// has no layout to work it out from.
    ///
    /// Entry used to be the origin itself, which spent in sailing what it
    /// saved in questions: the nearest land averages half a kilometre out,
    /// past the haze on every seed's worse days, so a new arrival saw water in
    /// every direction and steered blind.
    ///
    /// Nearest by frame rather than by centre, and the waterline rather than
    /// the frame — a frame is a rectangle of map and not of land, and a fitted
    /// coast can recede hundreds of metres inside it. So the shore is found on
    /// the terrain: the line from the origin to the island's nearest land is
    /// *sounded* [`SOUNDING`] metres a step, and the first ground at sea level
    /// is the shore the spawn backs [`SPAWN_OFFSHORE`] metres off, stepping
    /// further seaward should its own spot prove dry.
    ///
    /// Unlike the rest of the layout's questions this one generates its
    /// island, which is borrowed rather than added — entry being when that
    /// island is about to be generated anyway. Every *other* island keeps its
    /// distance by construction, layout margins leaving hundreds of metres
    /// between frames.
    ///
    /// [`None`] means the layout offered no island at all out to the widest
    /// window the search reaches, which is a broken layout rather than a wide
    /// sea; callers may fall back to the origin, which [`SPAWN_CLEARING`]
    /// keeps open.
    pub fn spawn(&self) -> Option<Spawn> {
        let island = self.nearest_by(Vec2::ZERO, |spec| spec.frame_point(Vec2::ZERO))?;

        // The island's land nearest the origin, off a half-chunk lattice
        // over the frame — fine enough that even a single-chunk islet puts
        // several samples on its ground. Swept in a fixed order with ties
        // kept first, so the landfall is the seed's and not the machine's.
        let terrain = self.island(island);
        let min = island.origin.as_vec2() * CHUNK_METRES;
        let cells = island.chunks.as_ivec2() * 2;
        let landfall = (0..=cells.y)
            .flat_map(|iz| (0..=cells.x).map(move |ix| IVec2::new(ix, iz)))
            .map(|cell| min + cell.as_vec2() * (CHUNK_METRES * 0.5))
            .filter(|at| terrain.height(at.x, at.y) >= 0.0)
            .min_by(|a, b| a.length_squared().total_cmp(&b.length_squared()));

        let point = match landfall {
            Some(landfall) => {
                // Sound the line from the origin — water on every seed, by
                // the clearing — to the landfall: the first ground reached
                // is the island's origin-facing shore.
                let stride = landfall / landfall.length() * SOUNDING;
                let soundings = (landfall.length() / SOUNDING) as i32;
                let shore = (0..=soundings)
                    .map(|i| stride * i as f32)
                    .find(|at| terrain.height(at.x, at.y) >= 0.0)
                    .unwrap_or(landfall);

                // The berth stands off that shore — [`berth_off`], the same
                // walk the console berths a driven hull with, so entry and
                // `goto` agree about what water a ship is left in.
                berth_off(shore - stride, -stride / SOUNDING, |x, z| {
                    terrain.height(x, z)
                })
            }
            // An island with no land on the lattice — the map is nearly all
            // water. The frame's nearest edge is then the best "shore" there
            // is to stand off; no berth is promised here, there being no
            // waterline to walk one off.
            None => {
                let coast = island.frame_point(Vec2::ZERO);
                coast * (1.0 - SPAWN_OFFSHORE / coast.length())
            }
        };
        Some(Spawn { point, island })
    }

    /// This island's terrain, generated now if it never has been. Costs tens
    /// to hundreds of milliseconds on a miss — callers that cannot wait ask
    /// [`Archipelago::ready_height`] instead.
    pub fn island(&self, spec: IslandSpec) -> Arc<Island> {
        let slot = {
            // A read-only fast path first: once the world is warm, nearly
            // every call finds its island without ever taking the write lock.
            let islands = self.islands.read().expect("no poisoned lock");
            islands.get(&spec).cloned()
        };
        let slot = slot.unwrap_or_else(|| {
            let mut islands = self.islands.write().expect("no poisoned lock");
            islands.entry(spec).or_default().clone()
        });
        slot.get_or_init(|| Arc::new(Island::generate(spec)))
            .clone()
    }

    /// The island's terrain only if it has already been generated.
    fn ready_island(&self, spec: IslandSpec) -> Option<Arc<Island>> {
        let islands = self.islands.read().expect("no poisoned lock");
        islands.get(&spec).and_then(|slot| slot.get().cloned())
    }

    /// Terrain height at a world point, in metres, generating whatever island
    /// owns the point. Sea level is 0 everywhere in the world; open ocean is
    /// flat floor at [`-OCEAN_DEPTH`].
    pub fn height(&self, wx: f32, wz: f32) -> f32 {
        match self.island_at(wx, wz) {
            Some(spec) => self.island(spec).height(wx, wz),
            None => -OCEAN_DEPTH,
        }
    }

    /// Terrain height at a world point if it can be answered without
    /// generating anything: open ocean always can, an island only once its
    /// terrain exists. What the camera asks every frame — a frame is not the
    /// place to pay for an island.
    pub fn ready_height(&self, wx: f32, wz: f32) -> Option<f32> {
        match self.island_at(wx, wz) {
            Some(spec) => Some(self.ready_island(spec)?.height(wx, wz)),
            None => Some(-OCEAN_DEPTH),
        }
    }

    /// Surface normal at a world point, from central differences one tile out
    /// — the smooth ground-query normal, exactly as [`TerrainGenerator::normal`]
    /// is for a lone map.
    pub fn normal(&self, wx: f32, wz: f32) -> Vec3 {
        normal_at(wx, wz, |x, z| self.height(x, z))
    }

    /// One world chunk as a client is sent it, or `None` where there is no
    /// ground worth sending — because no island is answerable for the chunk,
    /// or because the island answerable for it has nothing but ocean floor
    /// there.
    ///
    /// The second case is most of an island's chunks, not a corner case. An
    /// island is laid out as a rectangle with a skirt, its land is a lobed
    /// shape inside a fitted sea, and every chunk of the rectangle that misses
    /// the land entirely — the whole skirt, and the corners of most frames —
    /// comes out as a flat plane at minus [`OCEAN_DEPTH`]. Sending those costs
    /// sixty-six kilobytes apiece to draw exactly what a client's ocean-floor
    /// backdrop is already drawing underneath them. Measured over a streaming
    /// radius on five seeds it ran from a quarter of the chunks to nearly two
    /// thirds — the share goes with the mix of island sizes nearby, since a
    /// skirt is a ring of fixed width and a small island is nearly all ring.
    /// Everything between islands is `None` outright.
    ///
    /// The test is on the sampled corners rather than on the layout, which is
    /// what makes it exact: if every corner of the facet grid is *precisely*
    /// the floor then every facet built from them is a flat quad at the floor,
    /// and the backdrop stands in for it perfectly. Ground a centimetre off the
    /// floor fails the test and gets sent.
    ///
    /// Generates the owning island on a miss, so this is where a session pays
    /// for the ocean; it is meant to be called from a worker, not from
    /// anything holding a lock or a frame.
    pub fn chunk_payload(&self, chunk: IVec2) -> Option<ChunkPayload> {
        let island = self.island(self.island_at_chunk(chunk)?);
        let base = chunk.as_vec2() * CHUNK_METRES;

        let heights = corner_heights(base, |wx, wz| island.height(wx, wz));
        if heights.iter().all(|h| *h == -OCEAN_DEPTH) {
            return None;
        }

        Some(ChunkPayload {
            materials: cell_materials(base, &heights, |wx, wz, height, normal| {
                island.material(wx, wz, height, normal)
            }),
            heights: heights.iter().copied().map(quantize).collect(),
            lit: corner_lit(base, |wx, wz| island.lit(wx, wz)),
            water: corner_water(base, &heights, |wx, wz| island.lake_level(wx, wz)),
            plants: crate::plants::plants(&island, chunk),
        })
    }

    /// One chunk's corner heights alone, quantised exactly as
    /// [`chunk_payload`] would send them, or `None` for open water — the same
    /// answer, minus the surfaces, standing water and plants that cost most
    /// of a payload to make.
    ///
    /// For the survey, which reads nothing but the heights and used to pay
    /// for a whole payload to get them — on the connection's own thread,
    /// where a burst of freshly-seen chunks is time the client's other
    /// messages wait behind. The quantise round trip is load-bearing, not an
    /// economy to skip: a coastline is settled on the heights a client was
    /// *sent*, and a corner a centimetre either side of the sea puts the
    /// waterline somewhere else.
    ///
    /// [`chunk_payload`]: Archipelago::chunk_payload
    pub fn chunk_heights(&self, chunk: IVec2) -> Option<Vec<u16>> {
        let island = self.island(self.island_at_chunk(chunk)?);
        let base = chunk.as_vec2() * CHUNK_METRES;

        let heights = corner_heights(base, |wx, wz| island.height(wx, wz));
        if heights.iter().all(|h| *h == -OCEAN_DEPTH) {
            return None;
        }
        Some(heights.iter().copied().map(quantize).collect())
    }

    /// Drops every cached island whose frame lies entirely beyond `radius` of
    /// every one of `foci`. The cache is only a cache — anything dropped
    /// regenerates, identical to the bit, if it is ever wanted again.
    ///
    /// A list rather than a point because a server holds one world for however
    /// many players are in it, and they are not standing together: an island
    /// is worth keeping if *anybody* is near it. An empty list keeps nothing,
    /// which is the right answer for a world nobody is in.
    pub fn retain_near(&self, foci: &[Vec2], radius: f32) {
        let mut islands = self.islands.write().expect("no poisoned lock");
        islands.retain(|spec, slot| {
            // An empty slot is an island being generated right now, on some
            // other thread, by a caller holding its [`OnceLock`]. Evicting it
            // would not stop that work — it would only hide it, so the next
            // caller starts a *second* generation of the same island, and the
            // hundreds of milliseconds already spent are thrown away. The
            // window is real: streaming asks for an island's chunks and then
            // pans, and eviction runs every frame in between.
            //
            // Keeping it costs at most one island's memory until the next
            // sweep, by which time the slot is filled and answers the distance
            // test like any other.
            if slot.get().is_none() {
                return true;
            }
            let half = spec.extent() * 0.5;
            foci.iter().any(|focus| {
                let apart = (spec.centre() - *focus).abs() - half;
                apart.max(Vec2::ZERO).length() <= radius
            })
        });
    }

    /// The island whose map covers this world point, if any.
    pub fn island_at(&self, wx: f32, wz: f32) -> Option<IslandSpec> {
        self.island_at_chunk(chunk_at(Vec2::new(wx, wz)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{digest, floats, ints};

    /// A window of the world big enough to hold many parcels of both layers:
    /// four coarse parcels and sixty-four fine ones per quadrant corner case.
    const WINDOW: f32 = LAYERS[0].parcel as f32 * CHUNK_METRES * 2.0;

    fn world(seed: u32) -> Archipelago {
        Archipelago::new(&WorldConfig { seed })
    }

    fn specs(world: &Archipelago) -> Vec<IslandSpec> {
        world.islands_within(Vec2::splat(-WINDOW), Vec2::splat(WINDOW))
    }

    #[test]
    fn a_world_is_entered_at_a_berth() {
        // [`berth_off`] only promises [`a_berth`]'s band where the sounding
        // line offers it, so these seeds are pinned as having ordinary
        // shelving entry coasts. A generator change that fails one here has
        // probably not broken the walk: look at the coast first, and re-pick
        // the seed if it has turned into a wall.
        for seed in [1, 7, 99, 20_040_112] {
            let world = world(seed);
            let spawn = world.spawn().expect("a world has an entry");
            let height = world.height(spawn.point.x, spawn.point.y);
            assert!(height < 0.0, "seed {seed} enters on dry ground");
            assert!(
                a_berth(height),
                "seed {seed} enters in {} m, outside the berth band",
                -height
            );
        }
    }

    #[test]
    fn the_layout_is_the_seed_and_nothing_else() {
        let (a, b, c) = (world(99), world(99), world(100));
        assert_eq!(specs(&a), specs(&b));
        assert_ne!(specs(&a), specs(&c), "two seeds drew the same ocean");
    }

    #[test]
    fn islands_never_crowd_each_other() {
        // The property everything downstream leans on: a chunk of the world
        // has at most one island answerable for it, so island gaps must
        // exceed two skirts — within a layer by their margins, across layers
        // by the clearance.
        let found = specs(&world(1));
        assert!(found.len() > 20, "a four-parcel window should hold islands");

        for (i, a) in found.iter().enumerate() {
            for b in &found[i + 1..] {
                let apart = (a.origin - b.end()).max(b.origin - a.end());
                // Two skirts may touch — a chunk each — but never share.
                assert!(
                    apart.max_element() >= 2 * SKIRT_CHUNKS,
                    "{a:?} and {b:?} stand within a skirt of each other"
                );
            }
        }
    }

    #[test]
    fn the_ocean_holds_islands_of_all_sizes() {
        let found = specs(&world(1));
        let longest = |s: &IslandSpec| s.chunks.x.max(s.chunks.y);

        // Both layers present: single-chunk islets and real landmasses.
        assert!(found.iter().any(|s| longest(s) <= 2), "no islets");
        assert!(
            found.iter().any(|s| longest(s) >= LAYERS[0].size.0 as u32),
            "no big islands"
        );
        // Oblongs both ways, so neither axis is favoured.
        assert!(found.iter().any(|s| s.chunks.x > s.chunks.y));
        assert!(found.iter().any(|s| s.chunks.y > s.chunks.x));
        // And open water: plenty of fine parcels hold nothing, or the ocean is
        // a lattice of land.
        //
        // Counted the way `islands_within` actually sweeps rather than from
        // the window's nominal width, which is the difference between a guard
        // and a decoration: the sweep rounds the window out to whole parcels
        // on both sides, so it visits 17 rows of fine parcels where the
        // nominal 20480 m over 1280 m says 16. A bound of 289 could not fail
        // even with occupancy at 1.0 and suppression switched off, since the
        // big islands alone hold a chunk of that grid empty.
        let l = &LAYERS[1];
        let min_chunk = (-WINDOW / CHUNK_METRES).floor() as i32;
        let max_chunk = (WINDOW / CHUNK_METRES).ceil() as i32;
        let rows = max_chunk.div_euclid(l.parcel) - min_chunk.div_euclid(l.parcel) + 1;
        let swept = (rows * rows) as f32;
        assert!(
            (found.len() as f32) < 0.9 * swept,
            "{} islands over {swept} swept fine parcels — the ocean is a lattice",
            found.len()
        );
    }

    #[test]
    fn the_world_is_entered_on_open_water_with_land_in_reach() {
        // The spawn guarantee, all of it. The clearing keeps every island's
        // frame at least [`SPAWN_CLEARING`] chunks from the origin; `spawn`
        // stands entry [`SPAWN_OFFSHORE`] metres off the nearest coast, with
        // that much water to every island's frame — so entry is a boat at
        // sea beside land on every seed, not just on essentially every one.
        for seed in [1, 7, 99, 777, 20_040_112] {
            let ocean = world(seed);
            for spec in specs(&ocean) {
                let apart = spec.origin.max(-spec.end()).max(IVec2::ZERO);
                assert!(
                    apart.max_element() >= SPAWN_CLEARING,
                    "seed {seed}: {spec:?} stands in the spawn clearing"
                );
            }

            let spawn = ocean
                .spawn()
                .unwrap_or_else(|| panic!("seed {seed} offers nowhere to enter"));

            // Afloat: the point is water, measured on the terrain itself.
            let depth = ocean.height(spawn.point.x, spawn.point.y);
            assert!(
                depth < 0.0,
                "seed {seed} enters on ground {depth} m above the sea"
            );

            // And in sight of land: walking on away from the origin — the
            // line the spawn was sounded along — reaches shore in about the
            // offshore distance, with a stride's worth of slack for the
            // sounding walking past the exact waterline.
            let towards = spawn.point.normalize();
            let shore = (0..).map(|i| i as f32 * SOUNDING).find(|walked| {
                let at = spawn.point + towards * *walked;
                ocean.height(at.x, at.y) >= 0.0 || *walked > 4_000.0
            });
            assert!(
                shore.unwrap() <= SPAWN_OFFSHORE + 2.0 * SOUNDING,
                "seed {seed}: the first land is {} m out, not within {SPAWN_OFFSHORE}",
                shore.unwrap()
            );

            // No other island's ground is anywhere near: every frame but the
            // spawn's own keeps hundreds of metres away.
            for spec in specs(&ocean) {
                if spec == spawn.island {
                    continue;
                }
                let apart = spec.frame_point(spawn.point).distance(spawn.point);
                assert!(
                    apart >= 2.0 * SKIRT_METRES,
                    "seed {seed}: entry is {apart} m from the frame of {spec:?}"
                );
            }

            // Nearest means nearest: nothing in a generous window around the
            // origin stands a frame closer to it than the island entry took.
            // The search widens over windows and answers from the first that
            // holds anything, so an island just past an early window's edge
            // is exactly what it could overlook.
            let reach = Vec2::splat(LAYERS[0].parcel as f32 * CHUNK_METRES);
            let closest = ocean
                .islands_within(-reach, reach)
                .into_iter()
                .map(|s| s.frame_point(Vec2::ZERO).length())
                .fold(f32::INFINITY, f32::min);
            let taken = spawn.island.frame_point(Vec2::ZERO).length();
            assert!(
                taken <= closest + 1e-3,
                "seed {seed} passed over an island {closest} m out for one {taken} m out"
            );

            // The same question is the same answer, on this machine and by
            // construction on every other — and a tie broken by iteration
            // order would make it the machine's answer rather than the seed's.
            assert_eq!(ocean.spawn(), Some(spawn));
            assert_eq!(world(seed).spawn(), Some(spawn));
        }
    }

    #[test]
    fn every_chunk_answers_to_one_island_or_to_nobody() {
        // `island_at_chunk` walks the layers and takes the first claim, so it
        // has to agree with the full list of claims — anywhere it could
        // disagree, two islands own one chunk and their meshes would fight.
        let world = world(7);
        let found = specs(&world);

        // Sweep a band of chunks crossing several parcels of both layers.
        for cz in -60..60i32 {
            for cx in -60..60i32 {
                let chunk = IVec2::new(cx, cz);
                let claims = found.iter().filter(|s| s.covers_chunk(chunk)).count();
                assert!(claims <= 1, "chunk {chunk} is claimed {claims} times");
                assert_eq!(
                    world.island_at_chunk(chunk).is_some(),
                    claims == 1,
                    "island_at_chunk disagrees with the layout at {chunk}"
                );
            }
        }
    }

    #[test]
    fn an_island_is_its_map_and_the_skirt_lets_it_down() {
        // Inside its frame an island is the lone map of its spec, bit for bit;
        // and everywhere in the skirt band it is flat ocean floor, all the way
        // round, so the hand-over to the backdrop plane has nothing to show.
        //
        // The band assertion is deliberately the flat one rather than a test
        // of the blend. As [`SKIRT_CHUNKS`] says, the generator already
        // delivers `-OCEAN_DEPTH` at the frame, so the blend never has any
        // distance to travel — a test of the blend's own shape would be a test
        // that a smoothstep interpolates between two equal numbers. What the
        // game actually leans on is the *result*: every sample out there is
        // exactly the floor, so island meshes and the backdrop meet at one
        // level and `chunk_geometry` is entitled to drop the chunk entirely.
        let world = world(1);
        let spec = specs(&world)
            .into_iter()
            .min_by_key(|s| s.chunks.x * s.chunks.y)
            .expect("some island");
        let island = world.island(spec);

        let lone = TerrainGenerator::new(&spec.config());
        let centre = spec.centre();
        let half = spec.extent() * 0.5;

        // A transect from the centre out to the east frame: inside it, the
        // island has to be its own map to the bit.
        for i in 0..=200 {
            let wx = centre.x + half.x * i as f32 / 200.0;
            assert_eq!(
                island.height(wx, centre.y),
                lone.height(wx - centre.x, 0.0),
                "inside its frame the island is not its own map at {wx}"
            );
        }

        // Then the whole skirt band, on a grid a few metres apart — not one
        // transect, since a hand-over that held on the east side and failed on
        // the north is exactly the failure worth catching.
        const SPACING: f32 = 4.0;
        let outer = half + SKIRT_METRES;
        let steps = (outer * 2.0 / SPACING).ceil().as_uvec2();
        let mut samples = 0;
        for iz in 0..=steps.y {
            for ix in 0..=steps.x {
                let p = centre - outer + Vec2::new(ix as f32, iz as f32) * SPACING;
                // Chebyshev distance past the frame, signed — negative inside
                // it, and matching `beyond_frame` once it isn't.
                let beyond = ((p - centre).abs() - half).max_element();
                if !(0.0..SKIRT_METRES).contains(&beyond) {
                    continue;
                }
                samples += 1;
                assert_eq!(
                    island.height(p.x, p.y),
                    -OCEAN_DEPTH,
                    "the skirt stands {beyond} m past the frame at {p} and is not ocean floor"
                );
            }
        }
        assert!(samples > 500, "only {samples} samples fell in the skirt");
    }

    #[test]
    fn open_ocean_is_flat_floor_and_sends_nothing() {
        let world = world(1);
        // Find a chunk of open ocean: walk until one has no island.
        let chunk = (0..)
            .map(|i| IVec2::new(i, i))
            .find(|c| world.island_at_chunk(*c).is_none())
            .expect("some ocean");
        assert!(world.chunk_payload(chunk).is_none());

        let w = chunk.as_vec2() * CHUNK_METRES + CHUNK_METRES * 0.5;
        assert_eq!(world.height(w.x, w.y), -OCEAN_DEPTH);
        assert_eq!(world.ready_height(w.x, w.y), Some(-OCEAN_DEPTH));
    }

    #[test]
    fn an_islands_flat_chunks_send_nothing_either() {
        // An island is answerable for its skirt, but a skirt chunk is flat
        // ocean floor — the backdrop plane's job, not a mesh's. Sent, it would
        // be a mesh of thirty thousand identical triangles laid over a plane
        // already drawing the same surface.
        let world = world(1);
        let spec = specs(&world)
            .into_iter()
            .max_by_key(|s| s.chunks.x * s.chunks.y)
            .expect("some island");
        let (min, max) = spec.covered();

        // Every chunk of the skirt: the ring outside the frame.
        let mut skirt = 0;
        for cz in min.y..max.y {
            for cx in min.x..max.x {
                let chunk = IVec2::new(cx, cz);
                if chunk.cmpge(spec.origin).all() && chunk.cmplt(spec.end()).all() {
                    continue;
                }
                skirt += 1;
                assert!(
                    world.chunk_payload(chunk).is_none(),
                    "skirt chunk {chunk} sends a payload of flat floor"
                );
            }
        }
        assert!(skirt > 0, "the island has no skirt to check");

        // And the chunk under the highest ground on the island still does
        // send, or the test above would pass on a world with no ground at all.
        let centre = spec.centre();
        let half = spec.extent() * 0.5;
        let land = (0..64)
            .flat_map(|iz| (0..64).map(move |ix| (ix, iz)))
            .map(|(ix, iz)| centre - half + Vec2::new(ix as f32, iz as f32) * spec.extent() / 63.0)
            .max_by(|a, b| world.height(a.x, a.y).total_cmp(&world.height(b.x, b.y)))
            .expect("some ground");
        assert!(world.height(land.x, land.y) > 0.0, "the island is all sea");
        assert!(
            world.chunk_payload(chunk_at(land)).is_some(),
            "the chunk holding the island's summit sent nothing"
        );
    }

    #[test]
    fn ready_height_never_generates() {
        let world = world(1);
        let spec = specs(&world)[0];
        let centre = spec.centre();
        assert_eq!(
            world.ready_height(centre.x, centre.y),
            None,
            "an ungenerated island answered a ready query"
        );
        world.island(spec);
        assert!(world.ready_height(centre.x, centre.y).is_some());

        world.retain_near(&[centre + Vec2::splat(1.0e6)], 100.0);
        assert_eq!(
            world.ready_height(centre.x, centre.y),
            None,
            "eviction left the island behind"
        );
    }

    /// What one client's arrival costs a server: every chunk within a
    /// streaming radius of where a world is entered, made and measured.
    ///
    /// The number that matters for the shape of the whole arrangement — a
    /// client asks for all of these at once, and the server has to make them
    /// and put them on a socket. The water is nearly free at both ends; the
    /// ground is sixty-six kilobytes apiece and is where a slow link would be
    /// felt, so the split between them is as much the point as the time is.
    #[test]
    #[ignore]
    fn arrival_cost() {
        use std::time::Instant;

        // A client's streaming radius, in metres — the game's own
        // `STREAM_RADIUS`, restated here because this crate has no business
        // importing a camera's reach.
        const RADIUS: f32 = 1024.0;
        let reach = (RADIUS / CHUNK_METRES).ceil() as i32;

        for seed in [1u32, 7, 20_040_112] {
            let world = world(seed);
            let entry = world.spawn().map_or(Vec2::ZERO, |spawn| spawn.point);
            let middle = chunk_at(entry);

            let start = Instant::now();
            let (mut ground, mut water, mut lakes, mut bytes) = (0, 0, 0, 0usize);
            for dz in -reach..=reach {
                for dx in -reach..=reach {
                    match world.chunk_payload(middle + IVec2::new(dx, dz)) {
                        Some(payload) => {
                            ground += 1;
                            lakes += payload.water.is_some() as u32;
                            bytes += protocol::ground::payload_bytes(
                                payload.water.is_some(),
                                payload.plants.len(),
                            );
                            std::hint::black_box(&payload);
                        }
                        None => water += 1,
                    }
                }
            }
            let elapsed = start.elapsed();

            // The lake count is the interesting one for the wire: a watered
            // chunk is half as much again as a dry one, so what it costs to
            // send lakes at all is that share and not the ground's.
            println!(
                "seed {seed:>9}  {ground:>4} ground ({lakes:>3} with lakes)  {water:>4} water  \
                 {:>6.1} MB  {elapsed:>8.0?}",
                bytes as f32 / (1024.0 * 1024.0)
            );
        }
    }

    #[test]
    fn a_seed_is_the_same_world_down_to_the_bit() {
        // The layout of a whole window, and the ground of one island — frame,
        // skirt and all — pinned to recorded digests. When this fails because
        // the world was *meant* to change, re-record (run with `--nocapture`);
        // when it fails anywhere else, a machine or toolchain has stopped
        // agreeing about what a seed means. See the map digest test for the
        // full story.
        let world = world(20_040_112);
        let found = specs(&world);

        let layout = digest(ints(found.iter().flat_map(|s| {
            [
                s.origin.x as i64,
                s.origin.y as i64,
                s.chunks.x as i64,
                s.chunks.y as i64,
                s.seed as i64,
            ]
        })));

        let spec = found
            .iter()
            .copied()
            .min_by_key(|s| s.chunks.x * s.chunks.y)
            .expect("some island");
        let island = world.island(spec);
        let centre = spec.centre();
        let reach = spec.extent() * 0.5 + SKIRT_METRES;
        let mut heights = Vec::new();
        for iz in -20..=20 {
            for ix in -20..=20 {
                let w = centre + reach * Vec2::new(ix as f32, iz as f32) / 20.0;
                heights.push(island.height(w.x, w.y));
            }
        }
        let ground = digest(floats(heights));

        // And one chunk of that island exactly as it would be sent. The
        // heights above pin the *generator*; this pins the wire — the order
        // the grid is sampled in, the rounding, the facet walk and what each
        // triangle comes out painted. None of that moves a raw height, so a
        // transposed grid or a facet walk that started splitting its quads the
        // other way would pass everything above and leave two builds drawing
        // different ground from the same bytes.
        //
        // The chunk is the island's middle, which is the part most likely to
        // hold land; a payload of pure ocean floor would be a digest of the
        // same number eight thousand times over and would notice nothing.
        //
        // The water grid goes in too, and contributes nothing at all where
        // there is no lake — which is what makes its absence part of what is
        // pinned: a chunk that gained or lost standing water changes this
        // digest by the whole length of a grid. The lit grid goes in
        // unconditionally, which pins the sunlight bake with it.
        let middle = chunk_at(centre);
        let payload = world
            .chunk_payload(middle)
            .expect("an island's middle chunk should be ground");
        let sent = digest(
            payload
                .heights
                .iter()
                .flat_map(|h| h.to_le_bytes())
                .chain(payload.materials.iter().map(|tone| tone.to_byte()))
                .chain(payload.lit.iter().flatten().copied())
                .chain(payload.water.iter().flatten().flat_map(|w| w.to_le_bytes())),
        );

        println!(
            "layout digests to {layout:#018X}, ground to {ground:#018X}, sent to {sent:#018X}"
        );
        assert_eq!(layout, 0xF310_7FA9_D557_237C, "the layout changed");
        assert_eq!(ground, 0x49FB_11E9_A669_3026, "the ground changed");
        assert_eq!(
            sent, 0x61F8_B02B_D132_33B3,
            "what a client would be sent changed"
        );
    }
}
