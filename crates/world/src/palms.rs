//! Where the palms stand.
//!
//! Palms grow at the back of a beach — on the sand, above the reach of the
//! water, where it gives out to whatever the island is wearing behind it. That
//! is a real place on a real coast and it is also the one this generator can
//! name without inventing a second opinion about anything: the sand is already
//! painted, the height is already decided, and the back of the beach is where
//! the two stop agreeing.
//!
//! So nothing here shapes ground or paints it. A palm is *found* rather than
//! placed, by walking a lattice over a chunk and asking the finished island
//! about each cell. That is what keeps this out of the digests the height
//! field and the colours are pinned by: adding palms to a world cannot move
//! it, because the arithmetic only ever reads.
//!
//! The lattice is in world coordinates rather than chunk-local, so the answer
//! for a stretch of coast does not depend on where the chunk boundaries fell.
//! A palm belongs to the chunk its foot stands in and is jittered no further
//! than its own cell, so no two chunks can claim the same tree and none can
//! lose one between them.

use glam::{IVec2, Vec2};
use protocol::ground::{Kind, Plant, Tone, CHUNK_METRES};

use crate::archipelago::Island;
use crate::plants::{draw, mix};

/// Metres between the cells a palm may stand in — and so, less the jitter
/// below, the closest two of them ever come.
///
/// A coconut palm wants some yards to itself, and the number is doing a second
/// job at this scale: a lattice much finer than this would stand trees a
/// stride apart down a beach, and they would read as a hedge rather than as
/// trees.
const CELL: f32 = 8.0;

/// How far off its cell's centre a palm may stand. Short of half a cell, so a
/// tree stays inside the cell that produced it — which is what makes the chunk
/// it belongs to decidable without asking the neighbours.
const JITTER: f32 = 3.0;

/// The share of cells that qualify which actually carry a tree.
///
/// Higher than it looks, because the qualifying cells are already a thin
/// selection — the band of dry sand with the beach ending behind it is a line
/// rather than an area, so most of a coast offers nothing to thin out. What
/// this is really setting is the length of the gaps *along* that line, and
/// somewhat under two thirds is where a stand of palms still has holes in it.
const DENSITY: f32 = 0.62;

/// How far above the sea a palm's foot must stand, in metres.
///
/// Small, and it has to be: most of the sand this generator paints is *under*
/// the water — the pale shelf that gives a coast its turquoise ring — and the
/// dry part of a beach is only the last half metre of it. So this is a hand's
/// breadth above the waterline rather than the metre or two that sounds right,
/// and setting it by what sounds right is how this shipped its first version
/// with no palms anywhere in the world.
const LOW: f32 = 0.15;

/// And how far above it at most. Above the sand there is on any beach, so it
/// is a guard rather than a threshold: sand painted high up is a shelf inland
/// somewhere, and whatever it is it is not a beach with palms on it.
const HIGH: f32 = 2.0;

/// How far to look for the back of the beach, in metres. A palm is wanted
/// where the sand gives out, so a cell qualifies only if there is something
/// other than sand within this — otherwise the middle of a wide beach would
/// carry trees, which is a car park rather than a coast.
const EDGE_REACH: f32 = 5.0;

/// How much sand a palm wants around its foot, in metres.
///
/// The ground is *painted* per triangle, from the tone at each triangle's
/// middle, while this asks about points — so a palm standing on the last sandy
/// point of a facet whose middle is rock gets drawn on rock. Rather than
/// working out which facet a palm lands in and reproducing the painter's
/// arithmetic, which would be a second copy of it to keep in step, a palm is
/// simply required to have sand a facet's width all round. That also reads
/// better as a rule: a tree wants a footing, not a foothold.
const FOOTING: f32 = 1.4;

/// Every palm standing on one chunk of an island.
///
/// Walks the lattice cells whose centres fall inside the chunk, in a fixed
/// order, and asks each in turn. The order is what keeps a truncated chunk the
/// same chunk everywhere — see [`crate::plants`], which is where the budget a
/// chunk's plants share is spent, and where any truncating is done.
pub fn palms(island: &Island, chunk: IVec2) -> Vec<Plant> {
    let base = chunk.as_vec2() * CHUNK_METRES;
    let cells = (CHUNK_METRES / CELL) as i32;
    // The lattice cell the chunk's own corner falls in. CELL divides
    // CHUNK_METRES exactly, so this is a whole number of cells and every chunk
    // holds the same count with none straddling a boundary.
    let origin = (base / CELL).round().as_ivec2();

    let mut found = Vec::new();
    for cz in 0..cells {
        for cx in 0..cells {
            let cell = origin + IVec2::new(cx, cz);
            if let Some(palm) = in_cell(island, cell, base) {
                found.push(palm);
            }
        }
    }
    found
}

/// The palm standing in one lattice cell, if one does.
///
/// Every decision comes out of the same hash of the cell, so a cell is a tree
/// or is not, at a fixed spot, turned a fixed way, for as long as the seed is
/// the seed. Four draws off one hash rather than four hashes, because the
/// draws are independent enough for a scatter and one multiply is cheaper than
/// four.
fn in_cell(island: &Island, cell: IVec2, base: Vec2) -> Option<Plant> {
    let seed = mix(cell, island.spec.seed);

    // The density draw first, so that most cells cost one multiply and no
    // terrain lookups at all. The overwhelming majority of the world is not
    // beach, but the overwhelming majority of *cells* are rejected here.
    if draw(seed, 0) > DENSITY {
        return None;
    }

    let at = (cell.as_vec2() + 0.5) * CELL
        + Vec2::new(draw(seed, 1) - 0.5, draw(seed, 2) - 0.5) * 2.0 * JITTER;

    let height = island.height(at.x, at.y);
    if !(LOW..=HIGH).contains(&height) || !is_sand(island, at, height) {
        return None;
    }

    // Sand all round the foot before anything else about the surroundings:
    // this is the cheap half of the question and it rejects the boundary
    // cases the edge test below would otherwise accept.
    let footed = [Vec2::X, Vec2::NEG_X, Vec2::Y, Vec2::NEG_Y]
        .into_iter()
        .all(|step| {
            let probe = at + step * FOOTING;
            is_sand(island, probe, island.height(probe.x, probe.y))
        });
    if !footed {
        return None;
    }

    // The back of the beach: somewhere within reach the sand has to give out,
    // and give out *upwards*. Checking the height as well as the tone is what
    // tells the top of the beach from the bottom of it — the sand ends at both
    // ends, and at the seaward one it ends in water.
    let inland = [Vec2::X, Vec2::NEG_X, Vec2::Y, Vec2::NEG_Y]
        .into_iter()
        .any(|step| {
            let probe = at + step * EDGE_REACH;
            let there = island.height(probe.x, probe.y);
            there > height && !is_sand(island, probe, there)
        });
    if !inland {
        return None;
    }

    let (small, large) = Kind::Palm.scale();
    Some(Plant {
        kind: Kind::Palm,
        at: at - base,
        yaw: draw(seed, 3) * std::f32::consts::TAU,
        scale: small + draw(seed, 4) * (large - small),
    })
}

/// Whether the island paints this point as beach sand.
///
/// Asked of the painter rather than worked out again from the shore character
/// behind it: what a palm has to stand on is the sand a player can *see*, and
/// there is exactly one thing that decides what a player sees. A rule of its
/// own here would be a second answer to the same question, and the two would
/// drift — palms in a line along a shore that had since been repainted shingle.
///
/// Point-wise, where the painter works per triangle. [`FOOTING`] is what
/// closes the gap between the two.
fn is_sand(island: &Island, at: Vec2, height: f32) -> bool {
    let normal = island.normal(at.x, at.y);
    island.surface(at.x, at.y, height, normal) == Tone::Sand
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archipelago::{Archipelago, WorldConfig};
    use crate::plants::{draw, mix};
    use crate::testing::{digest, floats};
    use protocol::ground::chunk_at;

    /// A window of world wide enough to hold several islands of assorted
    /// sizes, and so several coasts of assorted characters.
    const WINDOW: i32 = 20;

    fn world(seed: u32) -> Archipelago {
        Archipelago::new(&WorldConfig { seed })
    }

    /// Every palm in the window, with the chunk that carried it. Palms only:
    /// a chunk's list holds every kind now, and a digest of all of them would
    /// move whenever any other rule did.
    fn all_palms(world: &Archipelago) -> Vec<(IVec2, Plant)> {
        let mut found = Vec::new();
        for cz in -WINDOW..WINDOW {
            for cx in -WINDOW..WINDOW {
                let chunk = IVec2::new(cx, cz);
                if let Some(payload) = world.chunk_payload(chunk) {
                    found.extend(
                        payload
                            .plants
                            .into_iter()
                            .filter(|plant| plant.kind == Kind::Palm)
                            .map(|palm| (chunk, palm)),
                    );
                }
            }
        }
        found
    }

    #[test]
    fn a_seed_grows_the_same_palms_wherever_it_is_hosted() {
        // The palms are part of what a seed means now, so they are pinned the
        // way the ground is: two players anchored off the same beach have to
        // see the same trees on it, and a server moved between machines has to
        // keep growing them. If you meant to change the rule, re-record these
        // — run with --nocapture and the new values are printed. If you did
        // not, a platform has stopped agreeing about what a seed means.
        let recorded = [
            (20_040_112u32, 0x7919_6D68_4EB4_038Fu64),
            (1, 0xC7CC_05C9_81FF_F461),
            (7, 0x1AC0_0B7B_05CF_A21D),
        ];
        // Every seed digested before any is judged, so a re-recording run
        // prints all three rather than stopping at the first that moved.
        let got: Vec<(u32, u64, usize)> = recorded
            .iter()
            .map(|(seed, _)| {
                let palms = all_palms(&world(*seed));
                let d = digest(palms.iter().flat_map(|(chunk, palm)| {
                    floats([
                        chunk.x as f32,
                        chunk.y as f32,
                        palm.at.x,
                        palm.at.y,
                        palm.yaw,
                        palm.scale,
                    ])
                }));
                (*seed, d, palms.len())
            })
            .collect();
        for (seed, d, count) in &got {
            println!("seed {seed} grows {count} palms, digesting to {d:#018X}");
        }
        for ((seed, d, _), (_, was)) in got.iter().zip(recorded) {
            assert_eq!(
                *d, was,
                "seed {seed} no longer grows the palms it grew — see the note above"
            );
        }
    }

    #[test]
    fn every_palm_stands_inside_the_chunk_that_carries_it() {
        // What the wire is held to at the other end, and what makes a palm
        // drawn exactly once: a tree past the boundary would be drawn by a
        // client that never asked for that ground, and again by the chunk it
        // really stands on.
        for seed in [20_040_112, 1, 7] {
            for (chunk, palm) in all_palms(&world(seed)) {
                assert!(
                    (0.0..CHUNK_METRES).contains(&palm.at.x)
                        && (0.0..CHUNK_METRES).contains(&palm.at.y),
                    "seed {seed} put a palm at {:?} in chunk {chunk}",
                    palm.at
                );
                let world_at = chunk.as_vec2() * CHUNK_METRES + palm.at;
                assert_eq!(chunk_at(world_at), chunk, "a palm strayed off its chunk");
            }
        }
    }

    #[test]
    fn no_chunk_carries_more_plants_than_the_wire_will_take() {
        for seed in [20_040_112, 1, 7] {
            // The world once, outside the walk. Built inside it, this test
            // regenerated every island for every chunk and took seven minutes.
            let world = world(seed);
            for cz in -WINDOW..WINDOW {
                for cx in -WINDOW..WINDOW {
                    let chunk = IVec2::new(cx, cz);
                    if let Some(payload) = world.chunk_payload(chunk) {
                        assert!(payload.plants.len() <= protocol::ground::MAX_PLANTS);
                        assert!(payload.well_formed(), "seed {seed} chunk {chunk}");
                    }
                }
            }
        }
    }

    #[test]
    fn every_palm_stands_on_dry_beach_sand() {
        // The whole of the rule, checked against the finished island rather
        // than against the arithmetic that placed them — so a change to how
        // coasts are painted shows up here as palms standing in the wrong
        // place, which is what it would be.
        let world = world(7);
        let mut counted = 0;
        for (chunk, palm) in all_palms(&world) {
            let at = chunk.as_vec2() * CHUNK_METRES + palm.at;
            let island = world.island(
                world
                    .island_at_chunk(chunk)
                    .expect("a chunk with a palm on it is an island's"),
            );
            let height = island.height(at.x, at.y);
            assert!(
                (LOW..=HIGH).contains(&height),
                "a palm stands {height}m up, outside the beach band"
            );
            assert!(is_sand(&island, at, height), "a palm stands off the sand");
            counted += 1;
        }
        assert!(
            counted > 20,
            "only {counted} palms — the rule has stopped finding beaches"
        );
    }

    #[test]
    fn the_draws_off_one_cell_are_not_the_same_number() {
        // The scatter leans on these being independent: if the jitter tracked
        // the density draw, every palm in the world would sit at the same
        // corner of its own cell and the lattice would be plain to see.
        let seed = mix(IVec2::new(3, -9), 42);
        let drawn: Vec<f32> = (0..5).map(|nth| draw(seed, nth)).collect();
        for (i, a) in drawn.iter().enumerate() {
            assert!((0.0..1.0).contains(a), "a draw left the unit interval");
            for b in &drawn[i + 1..] {
                assert!((a - b).abs() > 1e-6, "two draws off one cell agreed");
            }
        }
    }

    #[test]
    fn the_lattice_divides_a_chunk_exactly() {
        // Every chunk holds a whole number of cells, none straddling a
        // boundary — which is what lets a chunk decide its own palms without
        // asking its neighbours what they claimed.
        assert_eq!((CHUNK_METRES / CELL).fract(), 0.0);
        const { assert!(JITTER < CELL * 0.5, "a palm could leave its own cell") };
    }
}
