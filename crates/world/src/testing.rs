//! What the determinism tests are built out of.
//!
//! Both of this crate's "a seed is the same thing everywhere" tests — the
//! map's, over a height field and its colours, and the world's, over a layout
//! and an island's ground — fold their subject down to one number and pin it.
//! They had a digest apiece, spelled the same way twice; this is that function,
//! written once, so the two tests cannot drift into hashing differently and
//! quietly stop being comparable.
//!
//! The other half is the ground the tests of what grows are run over: one
//! window of world per seed, served once and shared — see [`sweep`].

use std::sync::LazyLock;

use glam::IVec2;
use protocol::ground::{ChunkPayload, Kind, Plant};

use crate::archipelago::{Archipelago, WorldConfig};

/// FNV-1a over a stream of bytes.
///
/// Dependency-free like the noise, and for the same reason: the digest has to
/// mean the same thing in every build there will ever be, so it cannot be
/// whatever hasher a library version happens to ship.
///
/// Taken as bytes rather than as values because what each test feeds it
/// differs — one a run of floats, the other a layout's integers — while the
/// folding is the same. The *order* the bytes arrive in is part of what the
/// recorded digests pin, so a caller that reorders its stream has changed the
/// answer, exactly as if it had changed the data.
pub fn digest(bytes: impl IntoIterator<Item = u8>) -> u64 {
    let mut hash: u64 = 0xCBF2_9CE4_8422_2325;
    for byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01B3);
    }
    hash
}

/// The bytes a float digests as: its bit pattern, little-endian. Never its
/// decimal form, which would round away exactly the last-bit differences
/// between two machines that these digests exist to catch.
pub fn floats(values: impl IntoIterator<Item = f32>) -> impl Iterator<Item = u8> {
    values
        .into_iter()
        .flat_map(|value| value.to_bits().to_le_bytes())
}

/// The same for whole numbers — island origins, sizes and seeds.
pub fn ints(values: impl IntoIterator<Item = i64>) -> impl Iterator<Item = u8> {
    values.into_iter().flat_map(i64::to_le_bytes)
}

/// The seeds every rule for what grows is judged over and pinned on: the
/// world's own, a wet one and a dry one.
pub const SEEDS: [u32; 3] = [20_040_112, 1, 7];

/// A window of world wide enough to hold several islands of assorted sizes
/// — and so several coasts, valleys, lakes and dry collars of assorted
/// characters — measured in chunks either side of the origin.
pub const WINDOW: i32 = 20;

/// One seed's world, with every chunk of the window served from it.
pub struct Sweep {
    pub world: Archipelago,
    /// Every chunk in the window that is an island's, with what the wire
    /// would carry for it, in the order the window was walked.
    pub chunks: Vec<(IVec2, ChunkPayload)>,
}

impl Sweep {
    fn of(seed: u32) -> Self {
        let world = Archipelago::new(&WorldConfig { seed });
        let mut chunks = Vec::new();
        for cz in -WINDOW..WINDOW {
            for cx in -WINDOW..WINDOW {
                let chunk = IVec2::new(cx, cz);
                if let Some(payload) = world.chunk_payload(chunk) {
                    chunks.push((chunk, payload));
                }
            }
        }
        Self { world, chunks }
    }

    /// Every plant of one kind in the window, with the chunk that carried it.
    /// One kind only: a digest of all of them would move whenever any other
    /// rule did. The order is the walk's and then the chunk's own, which is
    /// the order the digests were recorded in.
    pub fn plants(&self, kind: Kind) -> Vec<(IVec2, Plant)> {
        self.chunks
            .iter()
            .flat_map(|(chunk, payload)| {
                payload
                    .plants
                    .iter()
                    .filter(move |plant| plant.kind == kind)
                    .map(move |plant| (*chunk, *plant))
            })
            .collect()
    }
}

/// The window served once for one of the [`SEEDS`], and kept for the rest of
/// the process.
///
/// Serving the window is nearly all of what a plant test costs — every island
/// in it is raised and painted, sunlight and all, before a single lattice
/// cell is asked — and every test of every kind of plant reads the same
/// three windows. Each test served them itself once, which made the crate's
/// tests a thirteen-fold repeat of one sweep and the slowest thing in CI by
/// a distance. The three seeds are served on three threads because nothing
/// the sweep produces depends on the order it was produced in.
pub fn sweep(seed: u32) -> &'static Sweep {
    static SWEEPS: LazyLock<Vec<Sweep>> = LazyLock::new(|| {
        std::thread::scope(|scope| {
            let served: Vec<_> = SEEDS
                .iter()
                .map(|&seed| scope.spawn(move || Sweep::of(seed)))
                .collect();
            served
                .into_iter()
                .map(|thread| thread.join().expect("a sweep panicked"))
                .collect()
        })
    });
    SWEEPS
        .iter()
        .find(|sweep| sweep.world.seed() == seed)
        .expect("only the seeds in SEEDS are swept")
}
