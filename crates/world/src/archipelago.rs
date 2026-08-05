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
//! World coordinates are `f32` metres, so "endless" has a horizon after all —
//! not one the layout imposes but one the arithmetic does. Measured against
//! the two scales that matter, the height field's half-metre steps and the
//! [`crate::terrain::MESH_STEP`] the ground is drawn at:
//!
//! - out to about **1,280 km** neighbouring `f32` coordinates are 0.15 m
//!   apart, so a half-metre step in the field still resolves and the ground is
//!   exactly the ground everywhere a player could sail to;
//! - by about **12,800 km** they are 1.5 m apart, past the half metre, and the
//!   field has stopped resolving its own smallest steps — coastlines quantise;
//! - by about **128,000 km** they are 15 m apart, past the mesh step outright,
//!   and the ground is flat because there is nowhere between the facets left
//!   to sample.
//!
//! A player panning at the camera's own speed reaches the first of those in
//! something over a year of continuous play, so the practical answer is that
//! the ocean does not end. The numbers are here so that "infinite-ish" is a
//! measurement rather than a hope, and so that anything that ever wants to
//! *place* a world — a saved position, a server's coordinate space — knows
//! where the arithmetic starts costing it.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use glam::{IVec2, UVec2, Vec2, Vec3};

use crate::noise::smoothstep;
use crate::terrain::{
    facet_geometry_from_heights, facet_heights, ChunkGeometry, MapConfig, TerrainGenerator,
    CHUNK_TILES, MAX_DEPTH, SEABED, TILE_SIZE,
};

/// Metres along the edge of one chunk — the world's unit of streaming, and the
/// grid every island is laid out on.
pub const CHUNK_METRES: f32 = CHUNK_TILES as f32 * TILE_SIZE;

/// Depth of the open ocean floor between islands, in metres. The same floor
/// every island's own sea bed is clamped to, so an island's rim and the ocean
/// around it meet at one level.
pub const OCEAN_DEPTH: f32 = MAX_DEPTH;

/// Colour of the open ocean floor — exactly the palette's deep sea bed.
///
/// Public because the game draws the ocean between islands as one flat
/// backdrop plane rather than as chunk meshes, and the two surfaces meet at
/// every island's skirt: any difference between this and the colour the
/// chunks are painted prints the island's chunk rectangle onto the water as
/// a faint seam.
pub const OCEAN_FLOOR_COLOR: Vec3 = SEABED;

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
/// What the skirt actually finds out there is worth stating plainly, because
/// it is not what the name suggests. Measured on generated islands, the
/// generator's own field is *already* exactly `-OCEAN_DEPTH` at the frame and
/// everywhere past it: [`crate::terrain::MAX_DEPTH`] is the deepest the bed
/// may go, `DEEP_FRACTION` anchors the deepest sixth or so of every map there,
/// and the rim — where the falloff has silenced the noise and is pushing
/// everything down — is the extreme of that distribution. So the blend in
/// [`Island::height`] blends the floor into the floor, and nothing about the
/// picture depends on it.
///
/// It is kept as the *guarantee* rather than as a mechanism. Nothing in the
/// generator promises a rim at full depth — it falls out of a calibration that
/// is free to be re-fitted, and a future one that left the rim a metre shy
/// would print every island's chunk rectangle onto the open water as a step.
/// The skirt makes the hand-over true by construction instead of by luck, for
/// the price of a smoothstep on chunks that are mostly not built at all.
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
/// The `bevy` feature is only the derive, as on [`MapConfig`]: the game holds
/// this as an ECS resource, and engine-free builds have no ECS to hold it in.
#[cfg_attr(feature = "bevy", derive(bevy_ecs::prelude::Resource))]
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

    /// How far past the island's frame a world point stands, in metres —
    /// zero anywhere inside it. Chebyshev, like the skirt of chunks it is
    /// read across.
    fn beyond_frame(&self, world: Vec2) -> f32 {
        let out = (world - self.centre()).abs() - self.extent() * 0.5;
        out.max(Vec2::ZERO).max_element()
    }
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
}

impl Island {
    fn generate(spec: IslandSpec) -> Self {
        Self {
            spec,
            generator: TerrainGenerator::new(&spec.config()),
        }
    }

    /// Height at a world point, in metres. Inside the frame this is the
    /// island's map, untouched; across the skirt it is that same field walked
    /// down to exactly [`-OCEAN_DEPTH`] by the skirt's outer edge, where the
    /// open ocean takes over without a seam.
    ///
    /// In practice the walk has nowhere to go. As `SKIRT_CHUNKS` explains,
    /// the generator's field is already at the floor by the frame, so the
    /// blend below is `-OCEAN_DEPTH` blended into `-OCEAN_DEPTH` on every
    /// island measured. It stays because it is what *makes* that true rather
    /// than merely observing it — the seam this would show is a step of ocean
    /// bed around every island in the world, and the fix belongs where it
    /// cannot be forgotten.
    pub fn height(&self, wx: f32, wz: f32) -> f32 {
        let beyond = self.spec.beyond_frame(Vec2::new(wx, wz));
        if beyond >= SKIRT_METRES {
            return -OCEAN_DEPTH;
        }
        let local = Vec2::new(wx, wz) - self.spec.centre();
        let h = self.generator.height(local.x, local.y);
        h + (-OCEAN_DEPTH - h) * smoothstep(0.0, SKIRT_METRES, beyond)
    }

    /// Surface colour at a world point, matching [`Island::height`].
    pub fn color(&self, wx: f32, wz: f32, height: f32, normal: Vec3) -> Vec3 {
        let local = Vec2::new(wx, wz) - self.spec.centre();
        self.generator.color(local.x, local.y, height, normal)
    }

    /// Surface normal at a world point, from central differences one tile out.
    pub fn normal(&self, wx: f32, wz: f32) -> Vec3 {
        crate::terrain::normal_from_neighbours(
            self.height(wx - TILE_SIZE, wz),
            self.height(wx + TILE_SIZE, wz),
            self.height(wx, wz - TILE_SIZE),
            self.height(wx, wz + TILE_SIZE),
        )
    }
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

        Some(IslandSpec {
            origin: parcel * l.parcel + offset,
            chunks: chunks.as_uvec2(),
            seed: (rng.next() >> 32) as u32,
        })
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
                    let (lo, hi) = (spec.origin - SKIRT_CHUNKS, spec.end() + SKIRT_CHUNKS);
                    if lo.cmple(max_chunk).all() && hi.cmpge(min_chunk).all() {
                        found.push(spec);
                    }
                }
            }
        }
        found
    }

    /// The island nearest a world point, by the distance between the point and
    /// the island's centre — layout only, generating nothing.
    ///
    /// What "start somewhere" means in an endless ocean. Every entry into the
    /// world has to choose a point, and the honest default — the origin — is
    /// open water on essentially every seed, so a game that took it opened on
    /// a flat blue plane with the nearest land over the horizon.
    ///
    /// Searched over windows that double until one holds something, rather
    /// than over a single wide one: the layout is cheap but not free, and the
    /// first window nearly always answers, so the common case sweeps a couple
    /// of parcels instead of a few hundred. Widening past the cap would be
    /// searching an ocean that, by the occupancy the layers are written to,
    /// cannot be that empty — so [`None`] here means the layout is broken, not
    /// that the sea is wide.
    pub fn nearest_island(&self, near: Vec2) -> Option<IslandSpec> {
        /// Coarse parcels the search window spans, doubling until it finds
        /// land. One coarse parcel is five kilometres, and half its parcels
        /// hold an island.
        const SPANS: [f32; 4] = [1.0, 2.0, 4.0, 8.0];

        let parcel = LAYERS[0].parcel as f32 * CHUNK_METRES;
        for span in SPANS {
            let reach = Vec2::splat(span * parcel);
            // `islands_within` sweeps its parcels in a fixed order and
            // `min_by` keeps the first of any tie, so the answer is the seed's
            // and not the machine's.
            let nearest = self
                .islands_within(near - reach, near + reach)
                .into_iter()
                .min_by(|a, b| {
                    let d = |s: &IslandSpec| s.centre().distance_squared(near);
                    d(a).total_cmp(&d(b))
                });
            if nearest.is_some() {
                return nearest;
            }
        }
        None
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
        crate::terrain::normal_from_neighbours(
            self.height(wx - TILE_SIZE, wz),
            self.height(wx + TILE_SIZE, wz),
            self.height(wx, wz - TILE_SIZE),
            self.height(wx, wz + TILE_SIZE),
        )
    }

    /// Surface colour at a world point: the owning island's palette, or the
    /// ocean floor's own colour where there is no island to ask.
    pub fn color(&self, wx: f32, wz: f32, height: f32, normal: Vec3) -> Vec3 {
        match self.island_at(wx, wz) {
            Some(spec) => self.island(spec).color(wx, wz, height, normal),
            None => SEABED,
        }
    }

    /// Geometry for one world chunk, or `None` where there is no geometry
    /// worth building — because no island is answerable for the chunk, or
    /// because the island answerable for it has nothing but ocean floor there.
    ///
    /// The second case is most of an island's chunks, not a corner case. An
    /// island is laid out as a rectangle with a skirt, its land is a lobed
    /// shape inside a fitted sea, and every chunk of the rectangle that misses
    /// the land entirely — the whole skirt, and the corners of most frames —
    /// comes out as a flat plane at minus [`OCEAN_DEPTH`]. Meshing those costs 24
    /// thousand vertices apiece to draw exactly what the game's ocean-floor
    /// backdrop is already drawing underneath them. Measured over a streaming
    /// radius on five seeds it ran from a quarter of the chunks to nearly two
    /// thirds — the share goes with the mix of island sizes nearby, since a
    /// skirt is a ring of fixed width and a small island is nearly all ring.
    ///
    /// The test is on the sampled corners rather than on the layout, which is
    /// what makes it exact: if every corner of the facet grid is *precisely*
    /// the floor then every facet built from them is a flat quad at the floor,
    /// and the backdrop stands in for it perfectly. Ground a centimetre off the
    /// floor fails the test and gets its mesh.
    ///
    /// Generates the owning island on a miss, so this is where streaming
    /// pays; it is meant to be called from a worker, not a frame.
    pub fn chunk_geometry(&self, chunk: IVec2) -> Option<ChunkGeometry> {
        let island = self.island(self.island_at_chunk(chunk)?);
        let base = chunk.as_vec2() * CHUNK_METRES;

        let heights = facet_heights(base, |wx, wz| island.height(wx, wz));
        if heights.iter().all(|h| *h == -OCEAN_DEPTH) {
            return None;
        }

        Some(facet_geometry_from_heights(
            base,
            &heights,
            |wx, wz, height, normal| island.color(wx, wz, height, normal),
            // With no map to span, UVs tile per chunk — still seamless across
            // boundaries, since world coordinates are continuous.
            |wx, wz| [wx / CHUNK_METRES, wz / CHUNK_METRES],
        ))
    }

    /// Drops every cached island whose frame lies entirely beyond `radius` of
    /// `focus`. The cache is only a cache — anything dropped regenerates,
    /// identical to the bit, if it is ever wanted again.
    pub fn retain_near(&self, focus: Vec2, radius: f32) {
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
            let apart = (spec.centre() - focus).abs() - half;
            apart.max(Vec2::ZERO).length() <= radius
        });
    }

    /// The island whose map covers this world point, if any.
    pub fn island_at(&self, wx: f32, wz: f32) -> Option<IslandSpec> {
        self.island_at_chunk(chunk_at(Vec2::new(wx, wz)))
    }
}

/// The world chunk a world point stands in.
pub fn chunk_at(world: Vec2) -> IVec2 {
    (world / CHUNK_METRES).floor().as_ivec2()
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn there_is_always_land_within_reach() {
        // What entering the world leans on: wherever a player is put down, the
        // nearest island can be found without generating anything. The origin
        // is the case that matters — it is where every default view starts,
        // and it is open water on essentially every seed.
        for seed in [1, 7, 99, 777, 20_040_112] {
            let ocean = world(seed);
            let near = ocean
                .nearest_island(Vec2::ZERO)
                .unwrap_or_else(|| panic!("seed {seed} has no island near the origin"));

            // Nearest means nearest: nothing in a generous window around the
            // origin stands closer to it.
            let reach = Vec2::splat(LAYERS[0].parcel as f32 * CHUNK_METRES);
            let closest = ocean
                .islands_within(-reach, reach)
                .into_iter()
                .map(|s| s.centre().length())
                .fold(f32::INFINITY, f32::min);
            assert!(
                near.centre().length() <= closest + 1e-3,
                "seed {seed} passed over an island {closest} m out for one {} m out",
                near.centre().length()
            );

            // And the same question twice is the same answer — the search
            // widens over windows, and a tie broken by iteration order would
            // make it the machine's answer rather than the seed's.
            assert_eq!(ocean.nearest_island(Vec2::ZERO), Some(near));
            assert_eq!(world(seed).nearest_island(Vec2::ZERO), Some(near));
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
    fn open_ocean_is_flat_floor_and_builds_no_chunks() {
        let world = world(1);
        // Find a chunk of open ocean: walk until one has no island.
        let chunk = (0..)
            .map(|i| IVec2::new(i, i))
            .find(|c| world.island_at_chunk(*c).is_none())
            .expect("some ocean");
        assert!(world.chunk_geometry(chunk).is_none());

        let w = chunk.as_vec2() * CHUNK_METRES + CHUNK_METRES * 0.5;
        assert_eq!(world.height(w.x, w.y), -OCEAN_DEPTH);
        assert_eq!(world.ready_height(w.x, w.y), Some(-OCEAN_DEPTH));
    }

    #[test]
    fn an_islands_flat_chunks_build_no_geometry_either() {
        // An island is answerable for its skirt, but a skirt chunk is flat
        // ocean floor — the backdrop plane's job, not a mesh's. Building them
        // was most of what streaming spent its time on.
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
                    world.chunk_geometry(chunk).is_none(),
                    "skirt chunk {chunk} built a mesh of flat floor"
                );
            }
        }
        assert!(skirt > 0, "the island has no skirt to check");

        // And the chunk under the highest ground on the island still does
        // build, or the test above would pass on a world with no meshes at all.
        let centre = spec.centre();
        let half = spec.extent() * 0.5;
        let land = (0..64)
            .flat_map(|iz| (0..64).map(move |ix| (ix, iz)))
            .map(|(ix, iz)| centre - half + Vec2::new(ix as f32, iz as f32) * spec.extent() / 63.0)
            .max_by(|a, b| world.height(a.x, a.y).total_cmp(&world.height(b.x, b.y)))
            .expect("some ground");
        assert!(world.height(land.x, land.y) > 0.0, "the island is all sea");
        assert!(
            world.chunk_geometry(chunk_at(land)).is_some(),
            "the chunk holding the island's summit built nothing"
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

        world.retain_near(centre + Vec2::splat(1.0e6), 100.0);
        assert_eq!(
            world.ready_height(centre.x, centre.y),
            None,
            "eviction left the island behind"
        );
    }

    /// FNV-1a over the bytes of a stream of ints and floats — the same digest
    /// idea as the map's own, for the same reason: a seed has to be the same
    /// *world* in every build there will ever be, or a server and its clients
    /// drift apart island by island.
    fn digest(ints: impl IntoIterator<Item = i64>, floats: impl IntoIterator<Item = f32>) -> u64 {
        let mut hash: u64 = 0xCBF2_9CE4_8422_2325;
        let mut eat = |bytes: &[u8]| {
            for byte in bytes {
                hash ^= *byte as u64;
                hash = hash.wrapping_mul(0x100_0000_01B3);
            }
        };
        for value in ints {
            eat(&value.to_le_bytes());
        }
        for value in floats {
            eat(&value.to_bits().to_le_bytes());
        }
        hash
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

        let layout = digest(
            found.iter().flat_map(|s| {
                [
                    s.origin.x as i64,
                    s.origin.y as i64,
                    s.chunks.x as i64,
                    s.chunks.y as i64,
                    s.seed as i64,
                ]
            }),
            [],
        );

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
        let ground = digest([], heights);

        println!("layout digests to {layout:#018X}, ground to {ground:#018X}");
        assert_eq!(layout, 0xF310_7FA9_D557_237C, "the layout changed");
        assert_eq!(ground, 0xFA89_ABF4_A2FC_48A1, "the ground changed");
    }
}
