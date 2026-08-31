//! Where the bananas grow.
//!
//! Bananas want what a palm does not: shelter, still air and wet feet. So they
//! are put where water collects and stays — the floor of a valley, and the
//! margin of a lake — rather than anywhere a rule about *height* would put
//! them. Both are things the island answers for, so nothing here invents a
//! second opinion about either: a lake margin is where the lake grid says
//! there is water just below the ground being stood on, and a hollow is
//! [`crate::terrain::Lie`].
//!
//! Which is worth knowing the shape of rather than trusting blindly. `Lie` is
//! measured once per island on the same grid the lakes are flooded from — the
//! landform, before the detail octaves, the crags and the coastal reshaping,
//! and at a spacing that cannot resolve a gully narrower than about 8 m. So it
//! is the island's own answer and the only one, but it is an answer about the
//! shape of the land rather than about the ground underfoot, and near a coast
//! the two can part company.
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

    // And not where the salt has stripped the cover off. This used to come
    // for free from reading the material — salt-bared ground is painted out of
    // the mountain row, which was in no list here — and saying it outright is
    // the point: whether a plant minds salt is the rule's business, not
    // something it should inherit from a colour it happened not to match.
    //
    // It is the right test here only because a clump stands well inland of the
    // shore already: [`LOW`] keeps it above the beach, where the flag would
    // have been false however much spray was landing. See
    // [`Ground::bared_by_salt`].
    if ground.bared_by_salt {
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
    use crate::terrain::{AROUND, HOLLOW_REACH};
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

    /// How far a bearing has to rise, and how many of the eight must, for the
    /// finished ground to be called closed in here.
    ///
    /// Deliberately far weaker than the rule that placed the clump — any rise
    /// at all, on a quarter of the ring. The rule reads [`Lie`], which is
    /// measured on the landform grid: no detail octaves, no crags, no coastal
    /// reshaping, and nothing narrower than about 8 m. The finished ground a
    /// player walks is a different field, and the two legitimately disagree
    /// about the edge of a valley — asserting the placement rule back at
    /// itself here would only be re-reading the array the rule read.
    ///
    /// What this catches is the class of failure that array can suffer:
    /// inverted, misindexed, built from the wrong grid, or left empty — any of
    /// which puts clumps on crests and knolls, where nothing rises at all. On
    /// the seed below every clump not beside water clears five of eight; two
    /// is the worst seen on any seed measured, so this holds with margin
    /// without being fitted to one favourite map.
    const TEST_RISE: f32 = 0.0;
    const TEST_RISING: usize = 2;

    /// The eight bearings, live off the finished height field.
    fn rising_around(island: &Island, at: Vec2, height: f32) -> usize {
        AROUND
            .iter()
            .filter(|step| {
                let probe = at + **step * HOLLOW_REACH;
                island.height(probe.x, probe.y) > height + TEST_RISE
            })
            .count()
    }

    #[test]
    fn every_clump_stands_where_the_rule_says() {
        // Checked against the finished island rather than against the
        // arithmetic that placed them, which is the only way this test can
        // fail for a reason worth knowing about. The country, the band and the
        // slope below are all read live; the shelter is re-derived by
        // `rising_around` rather than read back off [`Lie`], whose whole point
        // is that it is a cached field — see [`TEST_RISING`].
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
            let rising = rising_around(&island, at, height);
            assert!(
                rising >= TEST_RISING || beside_water(&island, at, height),
                "a clump stands with the ground rising on {rising} of eight bearings \
                 and no water beside it — that is a crest, not a hollow"
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
