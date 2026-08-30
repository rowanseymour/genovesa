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
use protocol::ground::{Kind, Plant, CHUNK_METRES};

use crate::archipelago::Island;
use crate::plants::{draw, mix};
use crate::terrain::{Country, LakeZone, Lie};

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

    // The grassland, the wet forest above it, and the reed margin of a lake —
    // but nothing the arid coast holds: a clump wants a floor that keeps
    // water, and the dry country is where the ground stops doing that.
    //
    // Asked as a country rather than as a list of materials. The list this
    // replaces was exactly the lowland and humid rows written out, which made
    // it a copy of two tables with nothing holding it to them — moving a
    // material between rows moved the bananas, silently. It also could not
    // say what it meant: `Forest` and `GrassDark` stand in both rows, so no
    // reading of a material can tell lowland woodland from the wet forest
    // above it.
    let ground = island.ground(at.x, at.y, height, normal);
    if !matches!(
        ground.country,
        Country::Lowland | Country::Humid | Country::Lake(LakeZone::Margin)
    ) {
        return None;
    }

    // And not where the salt has got at it. Spray bares ground the country
    // would otherwise have covered, and a clump wanting shelter is the last
    // thing that belongs in a collar of it. This used to come for free from
    // reading the material — sprayed ground is painted out of the mountain
    // row, which was in no list here — and saying it outright is the point:
    // whether a plant minds salt is the rule's business, not something it
    // should inherit from a colour it happened not to match.
    if ground.sprayed {
        return None;
    }

    // Shelter or wet feet, and the island answers for both. A hollow used to
    // be worked out here, eight height samples at a time, which a rule can
    // afford for the few points it is seriously considering and no cell of
    // ground could — so it was knowledge this module kept to itself. It is
    // [`Lie`] now, and the ferns and the land animals that will want the same
    // shelter will not each be inventing their own valley.
    if ground.lie != Lie::Hollow && !beside_water(island, at, height) {
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
            (20_040_112u32, 0xB009_6AA0_16F4_D121u64),
            (1, 0x1C8A_CF26_9D11_DFFF),
            (7, 0xE16F_5E2F_FF77_1C81),
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
            let normal = island.normal(at.x, at.y);
            assert!(
                island.ground(at.x, at.y, height, normal).lie == Lie::Hollow
                    || beside_water(&island, at, height),
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
