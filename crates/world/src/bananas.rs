//! Where the bananas grow.
//!
//! Bananas want what a palm does not: shelter, still air and wet feet. So they
//! are put where water collects and stays — the floor of a valley, and the
//! margin of a lake — rather than anywhere a rule about *height* would put
//! them. Both of those are things the finished island can be asked about
//! without inventing a second opinion: a hollow is ground with higher ground
//! most of the way round it, and a lake margin is where the lake grid says
//! there is water just below the ground being stood on.
//!
//! Nothing here shapes ground or paints it, exactly as in [`crate::palms`] —
//! the arithmetic only ever reads, so adding bananas to a world cannot move
//! the height field or the colours the digests pin.
//!
//! The two rules deliberately do not overlap. A palm wants the last half metre
//! of dry sand; a banana starts above where sand stops and will not stand on a
//! slope. A cell either is one place or the other, and the pair of them read as
//! two habitats rather than as one scatter in two shapes.

use glam::{IVec2, Vec2};
use protocol::ground::{Kind, Material, Plant, CHUNK_METRES};

use crate::archipelago::Island;
use crate::plants::{draw, mix, AROUND};

/// Metres between the cells a clump may stand in.
///
/// Twice the palms' spacing, because a banana is drawn as a *clump* — three
/// stems and a dozen leaves, four metres across — and clumps at eight metres
/// would close into a hedge with no floor showing between them. It has to
/// divide [`CHUNK_METRES`] exactly, so a chunk holds a whole number of cells
/// and none straddles a boundary.
const CELL: f32 = 16.0;

/// How far off its cell's centre a clump may stand — comfortably short of half
/// a cell, so a clump stays inside the cell that produced it and the chunk it
/// belongs to is decidable without asking the neighbours.
const JITTER: f32 = 5.5;

/// The share of qualifying cells that carry a clump. Lower than the palms'
/// share on purpose: a valley floor is an *area* where the back of a beach is
/// a line, so the same fraction would carpet it.
const DENSITY: f32 = 0.42;

/// How far above the sea the ground must stand, in metres. Above the beach,
/// which is where the palms are, so that neither rule is ever the second
/// opinion about a strip of sand.
const LOW: f32 = 2.0;

/// And how far above it at most. Bananas are a lowland crop and the hillside
/// above them is forest; this is roughly where an island stops being a valley
/// and starts being a mountain.
const HIGH: f32 = 70.0;

/// How level the ground has to be, as the upward component of its normal.
/// About twenty-five degrees: a clump wants a floor, and the whole point of
/// the rule is to find the flat bottom of somewhere rather than its sides.
const LEVEL: f32 = 0.90;

/// How high above a lake's surface a clump may stand, in metres. A bank, not a
/// hillside — and above the water rather than in it, since the lake grid
/// answers on both sides of its own shoreline.
const BANK: f32 = 2.5;

/// How far out a cell looks to see whether the ground closes in around it.
///
/// Wide enough to reach the sides of a valley rather than the roughness of its
/// floor: at a few metres every hollow between two facets would qualify, and
/// the world would grow bananas in its own noise.
const REACH: f32 = 22.0;

/// How much higher a bearing has to be to count as ground rising away, in
/// metres, and how many of the eight must be for a cell to be a valley floor.
///
/// Five of eight is what tells a valley from a hillside. On any slope, four of
/// the eight are uphill; requiring a clear majority means the ground has to
/// close in from more directions than a single gradient can account for.
const RISE: f32 = 2.5;
const CLOSED_IN: usize = 5;

/// Every banana clump on one chunk of an island.
///
/// The same walk the palms use — a lattice in world coordinates, so the answer
/// for a valley does not depend on where the chunk boundaries fell, and a fixed
/// order so that anything downstream truncating the list truncates it the same
/// way everywhere.
pub fn bananas(island: &Island, chunk: IVec2) -> Vec<Plant> {
    let base = chunk.as_vec2() * CHUNK_METRES;
    let cells = (CHUNK_METRES / CELL) as i32;
    let origin = (base / CELL).round().as_ivec2();

    let mut found = Vec::new();
    for cz in 0..cells {
        for cx in 0..cells {
            let cell = origin + IVec2::new(cx, cz);
            if let Some(clump) = in_cell(island, cell, base) {
                found.push(clump);
            }
        }
    }
    found
}

/// The clump standing in one lattice cell, if one does.
///
/// Ordered by what each question costs. The density draw is one multiply and
/// throws away most of the world before any terrain is sampled at all; the
/// ring of eight probes at the end is eight height lookups and is asked only
/// of ground that has already turned out to be low, level and green.
fn in_cell(island: &Island, cell: IVec2, base: Vec2) -> Option<Plant> {
    let seed = mix(cell, island.spec.seed);
    if draw(seed, 0) > DENSITY {
        return None;
    }

    let at = (cell.as_vec2() + 0.5) * CELL
        + Vec2::new(draw(seed, 1) - 0.5, draw(seed, 2) - 0.5) * 2.0 * JITTER;

    let height = island.height(at.x, at.y);
    if !(LOW..=HIGH).contains(&height) {
        return None;
    }

    let normal = island.normal(at.x, at.y);
    if normal.y < LEVEL {
        return None;
    }

    // Asked of the painter rather than worked out again from the height, for
    // the reason the palms' sand test gives: what a plant stands on has to be
    // the ground a player can *see*, and there is one thing that decides that.
    //
    // Every green the lowland has, and the wet forest above it, but nothing
    // the arid coast is painted in: a clump wants a floor that holds water,
    // and the dry country is where the ground stops doing that.
    if !matches!(
        island.material(at.x, at.y, height, normal),
        Material::Forest
            | Material::GrassDark
            | Material::Grass
            | Material::GrassLight
            | Material::Meadow
            | Material::Jungle
            | Material::Canopy
            | Material::Marsh
    ) {
        return None;
    }

    if !(beside_water(island, at, height) || closed_in(island, at, height)) {
        return None;
    }

    let (small, large) = Kind::Banana.scale();
    Some(Plant {
        kind: Kind::Banana,
        at: at - base,
        yaw: draw(seed, 3) * std::f32::consts::TAU,
        scale: small + draw(seed, 4) * (large - small),
    })
}

/// Whether this point stands on a lake's bank: fresh water just below it, and
/// no more than [`BANK`] of dry ground between the two.
///
/// The lake grid answers on both sides of its own shoreline, which is what
/// makes this a bank test rather than a depth one — a point *under* the level
/// is in the water, and nothing grows there.
fn beside_water(island: &Island, at: Vec2, height: f32) -> bool {
    island
        .lake_level(at.x, at.y)
        .is_some_and(|level| height > level && height <= level + BANK)
}

/// Whether the ground rises away from this point on most sides — a valley
/// floor, a hollow, the head of a basin.
///
/// A count of bearings rather than an average height around the ring: a valley
/// runs *somewhere*, so two of the eight are always level with the floor and
/// would drag a mean back down, while the sides that make it a valley are
/// unmistakable one bearing at a time.
fn closed_in(island: &Island, at: Vec2, height: f32) -> bool {
    AROUND
        .iter()
        .filter(|step| {
            let probe = at + **step * REACH;
            island.height(probe.x, probe.y) > height + RISE
        })
        .count()
        >= CLOSED_IN
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archipelago::{Archipelago, WorldConfig};
    use crate::testing::{digest, floats};
    use protocol::ground::chunk_at;

    /// The same window the palms are judged over, for the same reason: several
    /// islands of assorted sizes, and so valleys of assorted shapes.
    const WINDOW: i32 = 20;

    fn world(seed: u32) -> Archipelago {
        Archipelago::new(&WorldConfig { seed })
    }

    /// Every banana clump in the window, with the chunk that carried it.
    fn all_bananas(world: &Archipelago) -> Vec<(IVec2, Plant)> {
        let mut found = Vec::new();
        for cz in -WINDOW..WINDOW {
            for cx in -WINDOW..WINDOW {
                let chunk = IVec2::new(cx, cz);
                if let Some(payload) = world.chunk_payload(chunk) {
                    found.extend(
                        payload
                            .plants
                            .into_iter()
                            .filter(|plant| plant.kind == Kind::Banana)
                            .map(|clump| (chunk, clump)),
                    );
                }
            }
        }
        found
    }

    #[test]
    fn a_seed_grows_the_same_bananas_wherever_it_is_hosted() {
        // Pinned exactly as the palms are: a seed has to mean the same valley
        // full of bananas on any machine that serves it. If you meant to change
        // the rule, re-record these — run with --nocapture and the new values
        // are printed. If you did not, a platform has stopped agreeing about
        // what a seed means.
        let recorded = [
            (20_040_112u32, 0x9DA8_FE85_9915_7746u64),
            (1, 0xECB0_A44F_EEF6_3FCB),
            (7, 0x8429_3492_A24B_7018),
        ];
        let got: Vec<(u32, u64, usize)> = recorded
            .iter()
            .map(|(seed, _)| {
                let bananas = all_bananas(&world(*seed));
                let d = digest(bananas.iter().flat_map(|(chunk, clump)| {
                    floats([
                        chunk.x as f32,
                        chunk.y as f32,
                        clump.at.x,
                        clump.at.y,
                        clump.yaw,
                        clump.scale,
                    ])
                }));
                (*seed, d, bananas.len())
            })
            .collect();
        for (seed, d, count) in &got {
            println!("seed {seed} grows {count} banana clumps, digesting to {d:#018X}");
        }
        for ((seed, d, _), (_, was)) in got.iter().zip(recorded) {
            assert_eq!(
                *d, was,
                "seed {seed} no longer grows the bananas it grew — see the note above"
            );
        }
    }

    #[test]
    fn every_clump_stands_where_the_rule_says() {
        // The whole of the rule, checked against the finished island rather
        // than against the arithmetic that placed them — so a change to how
        // valleys are shaped or painted shows up here as bananas standing
        // somewhere silly, which is what it would be.
        let world = world(7);
        let mut counted = 0;
        for (chunk, clump) in all_bananas(&world) {
            let at = chunk.as_vec2() * CHUNK_METRES + clump.at;
            let island = world.island(
                world
                    .island_at_chunk(chunk)
                    .expect("a chunk with a clump on it is an island's"),
            );
            let height = island.height(at.x, at.y);
            assert!(
                (LOW..=HIGH).contains(&height),
                "a clump stands {height}m up, outside the lowland band"
            );
            assert!(
                island.normal(at.x, at.y).y >= LEVEL,
                "a clump stands on a slope it could not hold onto"
            );
            assert!(
                beside_water(&island, at, height) || closed_in(&island, at, height),
                "a clump stands on open ground, neither in a hollow nor beside water"
            );
            counted += 1;
        }
        assert!(
            counted > 20,
            "only {counted} clumps — the rule has stopped finding valleys"
        );
    }

    #[test]
    fn every_clump_stands_inside_the_chunk_that_carries_it() {
        // What makes a clump drawn exactly once: one past the boundary would be
        // drawn by a client that never asked for that ground, and again by the
        // chunk it really stands on.
        for seed in [20_040_112, 1, 7] {
            for (chunk, clump) in all_bananas(&world(seed)) {
                let world_at = chunk.as_vec2() * CHUNK_METRES + clump.at;
                assert_eq!(chunk_at(world_at), chunk, "a clump strayed off its chunk");
            }
        }
    }

    #[test]
    fn the_lattice_divides_a_chunk_exactly() {
        // Every chunk holds a whole number of cells, none straddling a
        // boundary — which is what lets a chunk decide its own bananas without
        // asking its neighbours what they claimed.
        assert_eq!((CHUNK_METRES / CELL).fract(), 0.0);
        const { assert!(JITTER < CELL * 0.5, "a clump could leave its own cell") };
    }
}
