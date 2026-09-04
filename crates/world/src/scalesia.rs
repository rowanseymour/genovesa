//! Where the scalesia stand.
//!
//! On the wet shoulders above the grassland — [`Country::Humid`], which is
//! painted as closed forest and until now had nothing standing in it. It is
//! about a sixth of every island, nearly all of it open, level ground with no
//! plant of any kind on it: the bananas reach it, but only where a hollow or
//! a lake bank lets them, which is a sliver of the country. So this is the
//! last country meant to have a tree that had none, and the tree is the
//! one the real islands grow there: *Scalesia pedunculata*, the giant daisy,
//! whose stands are what the humid zone of the Galápagos is named for.
//!
//! The rule is the cacti's with the country swapped, and it asks for the
//! country outright for the reason [`crate::cacti`] gives: the humid band's
//! two edges wander with the ground and swing with an island's climate, and a
//! height written here would be a worse copy of them. What it does *not* ask
//! is [`crate::terrain::Lie`]. A cactus wants open ground and a banana wants
//! a hollow, but a forest tree is at home in either, and keeping it out of
//! the hollows would buy a valley floor with a banana clump on it and no
//! trees over the clump — which is the one thing a wet valley in this country
//! should not look like.
//!
//! Nothing here shapes ground or paints it, exactly as in the other four
//! rules: the arithmetic only ever reads the finished island, so adding
//! scalesia to a world cannot move the height field or the colours the
//! digests pin.

use glam::{IVec2, Vec2};
use protocol::ground::{Kind, Plant, CHUNK_METRES};

use crate::archipelago::Island;
use crate::plants::{draw, mix};
use crate::terrain::Country;

/// Metres between the cells a tree may stand in. The palms' and the cacti's
/// spacing, and it has to divide [`CHUNK_METRES`] exactly so that a chunk
/// holds a whole number of cells and none straddles a boundary.
///
/// Finer than the crown is wide, which is deliberate: the cell is what sets
/// the *texture* of the scatter, and with [`DENSITY`] it comes to a tree
/// every twelve metres or so on average — pairs touching, gaps you could walk
/// a boat through. At the bananas' spacing the same count comes out as a
/// plantation.
const CELL: f32 = 8.0;

/// How far off its cell's centre a tree may stand. Short of half a cell, so
/// it stays inside the cell that produced it — which is what makes the chunk
/// it belongs to decidable without asking the neighbours.
const JITTER: f32 = 3.0;

/// The share of qualifying cells that carry a tree.
///
/// Below the palms' and the mangroves' shares and above the bananas', and
/// set against the ground rather than against them: this is the one country
/// whose whole character is that it is *wooded*, so the trees stand a crown
/// apart and the paint under them reads as a forest rather than a dark green
/// field. It stops short of a closed canopy because a lid over ground drawn
/// in facets a couple of metres across hides the slope, and the shoulders are
/// where a player looks down from. It is also the largest share of a chunk's
/// plant budget any kind takes — see [`crate::plants`].
const DENSITY: f32 = 0.45;

/// How level the ground has to be — the cacti's number, and for their reason:
/// a tree roots on a hillside a banana clump would not, so the test is only
/// that there is ground under it rather than a face. See [`crate::cacti`].
const LEVEL: f32 = 0.82;

/// Every scalesia on one chunk of an island.
///
/// The same walk the other four use — a lattice in world coordinates, so the
/// answer for a shoulder does not depend on where the chunk boundaries fell,
/// and a fixed order so that anything downstream truncating the list
/// truncates it the same way everywhere.
pub fn scalesia(island: &Island, chunk: IVec2) -> Vec<Plant> {
    let base = chunk.as_vec2() * CHUNK_METRES;
    let cells = (CHUNK_METRES / CELL) as i32;
    let origin = (base / CELL).round().as_ivec2();

    let mut found = Vec::new();
    for cz in 0..cells {
        for cx in 0..cells {
            let cell = origin + IVec2::new(cx, cz);
            if let Some(tree) = in_cell(island, cell, base) {
                found.push(tree);
            }
        }
    }
    found
}

/// The tree standing in one lattice cell, if one does.
///
/// Ordered by what each question costs, as the cacti are: the density draw
/// throws away half the cells before any terrain is sampled, and the
/// classification at the end is asked only of ground already known to be
/// standing level.
fn in_cell(island: &Island, cell: IVec2, base: Vec2) -> Option<Plant> {
    let seed = mix(cell, island.spec.seed);
    if draw(seed, 0) > DENSITY {
        return None;
    }

    let at = (cell.as_vec2() + 0.5) * CELL
        + Vec2::new(draw(seed, 1) - 0.5, draw(seed, 2) - 0.5) * 2.0 * JITTER;

    let height = island.height(at.x, at.y);
    let normal = island.normal(at.x, at.y);
    if normal.y < LEVEL {
        return None;
    }

    // The wet shoulder, and nothing else about the lie of it — see the module
    // header for why the hollows are not excluded.
    let ground = island.ground(at.x, at.y, height, normal);
    if ground.country != Country::Humid {
        return None;
    }

    // Salt-bared ground stays bare. The humid country stands well inland, so
    // this is nearly always false and asked anyway, for the bananas' reason:
    // whether a tree minds salt is the rule's business. See
    // [`crate::terrain::Ground::bared_by_salt`].
    if ground.bared_by_salt {
        return None;
    }

    let (small, large) = Kind::Scalesia.scale();
    Some(Plant {
        kind: Kind::Scalesia,
        at: at - base,
        yaw: draw(seed, 3) * std::f32::consts::TAU,
        scale: small + draw(seed, 4) * (large - small),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{digest, floats, sweep, SEEDS};
    use protocol::ground::chunk_at;

    #[test]
    fn a_seed_grows_the_same_scalesia_wherever_it_is_hosted() {
        // Pinned exactly as the other four are: a seed has to mean the same
        // shoulders with the same trees standing on them on any machine that
        // serves it. If you meant to change the rule, re-record these — run
        // with --nocapture and the new values are printed. If you did not, a
        // platform has stopped agreeing about what a seed means.
        let recorded = [
            (20_040_112u32, 0x2588_1A62_A0C0_C448u64),
            (1, 0x4F82_2AA8_B3A8_9556),
            (7, 0x7EA1_4C0B_A1AA_2F73),
        ];
        let got: Vec<(u32, u64, usize)> = recorded
            .iter()
            .map(|(seed, _)| {
                let trees = sweep(*seed).plants(Kind::Scalesia);
                let d = digest(trees.iter().flat_map(|(chunk, tree)| {
                    floats([
                        chunk.x as f32,
                        chunk.y as f32,
                        tree.at.x,
                        tree.at.y,
                        tree.yaw,
                        tree.scale,
                    ])
                }));
                (*seed, d, trees.len())
            })
            .collect();
        for (seed, d, count) in &got {
            println!("seed {seed} grows {count} scalesia, digesting to {d:#018X}");
        }
        for ((seed, d, _), (_, was)) in got.iter().zip(recorded) {
            assert_eq!(
                *d, was,
                "seed {seed} no longer grows the scalesia it grew — see the note above"
            );
        }
    }

    #[test]
    fn every_scalesia_stands_where_the_rule_says() {
        // Checked against the finished island rather than against the
        // arithmetic that placed them, which is the only way this test can
        // fail for a reason worth knowing about. Everything the rule reads is
        // read live and holds tree by tree — there is no [`crate::terrain::Lie`]
        // test here to need the cacti's population-wide bound.
        let world = &sweep(7).world;
        let mut counted = 0;
        for (chunk, tree) in sweep(7).plants(Kind::Scalesia) {
            let at = chunk.as_vec2() * CHUNK_METRES + tree.at;
            let island = world.island(
                world
                    .island_at_chunk(chunk)
                    .expect("a chunk with a tree on it is an island's"),
            );
            let height = island.height(at.x, at.y);
            let normal = island.normal(at.x, at.y);
            assert!(
                normal.y >= LEVEL,
                "a tree stands on a face rather than on ground"
            );
            let ground = island.ground(at.x, at.y, height, normal);
            assert_eq!(
                ground.country,
                Country::Humid,
                "a tree stands outside the wet shoulders"
            );
            assert!(
                !ground.bared_by_salt,
                "a tree stands on ground the salt has scoured"
            );
            counted += 1;
        }
        assert!(
            counted > 200,
            "only {counted} scalesia — the rule has stopped finding humid country"
        );
    }

    #[test]
    fn every_scalesia_stands_inside_the_chunk_that_carries_it() {
        // What makes a tree drawn exactly once: one past the boundary would
        // be drawn by a client that never asked for that ground, and again by
        // the chunk it really stands on.
        for seed in SEEDS {
            for (chunk, tree) in sweep(seed).plants(Kind::Scalesia) {
                let world_at = chunk.as_vec2() * CHUNK_METRES + tree.at;
                assert_eq!(chunk_at(world_at), chunk, "a tree strayed off its chunk");
            }
        }
    }

    #[test]
    fn the_lattice_divides_a_chunk_exactly() {
        // Every chunk holds a whole number of cells, none straddling a
        // boundary — which is what lets a chunk decide its own trees without
        // asking its neighbours what they claimed.
        assert_eq!((CHUNK_METRES / CELL).fract(), 0.0);
        const { assert!(JITTER < CELL * 0.5, "a tree could leave its own cell") };
    }
}
