//! Where the cacti stand.
//!
//! In the dry collar every island wears between its beach and its grassland —
//! [`Country::Arid`], which until now grew nothing at all. That collar is the
//! one stretch of the world a player crosses on foot every time they land, and
//! it was the only country with no plant of its own: palms stop at the back of
//! the beach, bananas start above the arid line, and mangroves never leave a
//! lake. So this is the gap in the picture rather than a fourth scatter for
//! its own sake.
//!
//! And it is the country, not a height, that decides. The other three rules
//! each carry a band in metres because each is placed relative to a *water*
//! line — the sand, the bank, the surface — which is not a country. This one
//! is a country outright, so it asks for one and nothing else: the arid collar
//! already begins where the shore gives out and ends where the grass starts,
//! and both of those lines wander with the ground and swing with an island's
//! climate. A height band written here would be a worse copy of them that
//! nothing held in step.
//!
//! The rule also reads [`Lie::Open`], which is the bananas' test the other
//! way about — a banana wants the hollow the water and the still air collect
//! in, a cactus wants what is left over — and it is worth knowing how little
//! that buys here before reading it as a second habitat. Hollows are 3% to 5%
//! of the arid collar on the seeds measured, because the collar is a coastal
//! band on ground that shelves to the sea and the basins are all further in.
//! So it keeps cacti out of the few damp dips there are, which is right, and
//! it is a trim rather than a shaping test. The symmetry with the bananas is
//! real; the weight either side of it is not equal.
//!
//! Nothing here shapes ground or paints it, exactly as in [`crate::palms`],
//! [`crate::bananas`] and [`crate::mangroves`]: the arithmetic only ever reads
//! the finished island, so adding cacti to a world cannot move the height
//! field or the colours the digests pin.

use glam::{IVec2, Vec2};
use protocol::ground::{Kind, Plant, CHUNK_METRES};

use crate::archipelago::Island;
use crate::plants::{draw, mix};
use crate::terrain::{Country, Lie};

/// Metres between the cells a cactus may stand in — the palms' spacing, and it
/// has to divide [`CHUNK_METRES`] exactly so that a chunk holds a whole number
/// of cells and none straddles a boundary.
///
/// A cactus is a narrow column rather than a canopy, so this is not keeping
/// crowns from closing the way the bananas' and the mangroves' spacings are.
/// It is the finer half of what sets the scatter, [`DENSITY`] being the other:
/// the two together come to one plant per four hundred square metres, and the
/// cell is what decides whether that is delivered as an even grid or as a
/// scatter with clusters and gaps in it. Coarser cells and the same plant
/// count, a collar only a few cells deep starts reading as rows.
const CELL: f32 = 8.0;

/// How far off its cell's centre a cactus may stand. Short of half a cell, so
/// it stays inside the cell that produced it — which is what makes the chunk
/// it belongs to decidable without asking the neighbours.
const JITTER: f32 = 3.0;

/// The share of qualifying cells that carry one.
///
/// The lowest of the four rules by some way, and the qualifying ground is why:
/// the arid collar is an *area* tens of metres deep round a whole coastline,
/// where the back of a beach is a line and a lake's fringe is a few metres of
/// it. At the palms' share, that area carries a plant every ten metres in
/// both directions, which is a plantation; at this one it is a plant every
/// twenty, which is a desert with things growing in it. Bare ground between
/// them is most of what makes it read as one, so this is set by how much of it
/// shows rather than by how many plants it grows.
const DENSITY: f32 = 0.16;

/// How level the ground has to be, as the upward component of its normal —
/// about thirty-five degrees.
///
/// Looser than the bananas' and the mangroves', which want a floor and a bed.
/// A cactus is the thing that will stand where nothing else does, so the test
/// is only that it has ground under it rather than a face: what it excludes is
/// a column growing out of the side of a bluff.
const LEVEL: f32 = 0.82;

/// Every cactus on one chunk of an island.
///
/// The same walk the other three use — a lattice in world coordinates, so the
/// answer for a stretch of dry country does not depend on where the chunk
/// boundaries fell, and a fixed order so that anything downstream truncating
/// the list truncates it the same way everywhere.
pub fn cacti(island: &Island, chunk: IVec2) -> Vec<Plant> {
    let base = chunk.as_vec2() * CHUNK_METRES;
    let cells = (CHUNK_METRES / CELL) as i32;
    let origin = (base / CELL).round().as_ivec2();

    let mut found = Vec::new();
    for cz in 0..cells {
        for cx in 0..cells {
            let cell = origin + IVec2::new(cx, cz);
            if let Some(cactus) = in_cell(island, cell, base) {
                found.push(cactus);
            }
        }
    }
    found
}

/// The cactus standing in one lattice cell, if one does.
///
/// Ordered by what each question costs: the density draw is one multiply and
/// throws away five cells in six before any terrain is sampled, and the
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

    // The dry collar, and open ground within it. Both off the one read, and
    // neither of them a number this module chose: where the arid country
    // begins and ends is [`Country`]'s business, and whether the ground closes
    // in around a point is [`Lie`]'s.
    let ground = island.ground(at.x, at.y, height, normal);
    if ground.country != Country::Arid || ground.lie != Lie::Open {
        return None;
    }

    // Salt-bared ground stays bare, and this rule is the one with real cause
    // to ask: the arid collar and the spray collar are the same ground seen
    // two ways, so between an eighth and a quarter of the cells that get this
    // far have had their cover stripped by salt — against the bananas, which
    // stand well inland and for which the flag is nearly always false.
    //
    // It is the right question rather than an inherited one. See
    // [`crate::terrain::Ground::bared_by_salt`]: it says the salt is what
    // bared this cell, which is the claim wanted here, and not that the cell
    // is exposed to salt, which a cactus would not mind. What it excludes is
    // ground scoured to rock, and nothing grows on that.
    if ground.bared_by_salt {
        return None;
    }

    let (small, large) = Kind::Cactus.scale();
    Some(Plant {
        kind: Kind::Cactus,
        at: at - base,
        yaw: draw(seed, 3) * std::f32::consts::TAU,
        scale: small + draw(seed, 4) * (large - small),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archipelago::{Archipelago, WorldConfig};
    use crate::terrain::{AROUND, HOLLOW_REACH};
    use crate::testing::{digest, floats};
    use protocol::ground::chunk_at;

    /// The same window the other three rules are judged over, for the same
    /// reason: several islands of assorted sizes, and so arid collars of
    /// assorted depths — the climate line swings per island, so a dry seed
    /// wears a far deeper one than a wet seed does.
    const WINDOW: i32 = 20;

    fn world(seed: u32) -> Archipelago {
        Archipelago::new(&WorldConfig { seed })
    }

    /// Every cactus in the window, with the chunk that carried it.
    fn all_cacti(world: &Archipelago) -> Vec<(IVec2, Plant)> {
        let mut found = Vec::new();
        for cz in -WINDOW..WINDOW {
            for cx in -WINDOW..WINDOW {
                let chunk = IVec2::new(cx, cz);
                if let Some(payload) = world.chunk_payload(chunk) {
                    found.extend(
                        payload
                            .plants
                            .into_iter()
                            .filter(|plant| plant.kind == Kind::Cactus)
                            .map(|cactus| (chunk, cactus)),
                    );
                }
            }
        }
        found
    }

    #[test]
    fn a_seed_grows_the_same_cacti_wherever_it_is_hosted() {
        // Pinned exactly as the other three are: a seed has to mean the same
        // dry collar with the same columns standing in it on any machine that
        // serves it. If you meant to change the rule, re-record these — run
        // with --nocapture and the new values are printed. If you did not, a
        // platform has stopped agreeing about what a seed means.
        let recorded = [
            (20_040_112u32, 0xFB0E_41B3_1D69_742Eu64),
            (1, 0x505E_3728_FAB8_61F7),
            (7, 0xF2B4_00FA_12A4_89E9),
        ];
        let got: Vec<(u32, u64, usize)> = recorded
            .iter()
            .map(|(seed, _)| {
                let cacti = all_cacti(&world(*seed));
                let d = digest(cacti.iter().flat_map(|(chunk, cactus)| {
                    floats([
                        chunk.x as f32,
                        chunk.y as f32,
                        cactus.at.x,
                        cactus.at.y,
                        cactus.yaw,
                        cactus.scale,
                    ])
                }));
                (*seed, d, cacti.len())
            })
            .collect();
        for (seed, d, count) in &got {
            println!("seed {seed} grows {count} cacti, digesting to {d:#018X}");
        }
        for ((seed, d, _), (_, was)) in got.iter().zip(recorded) {
            assert_eq!(
                *d, was,
                "seed {seed} no longer grows the cacti it grew — see the note above"
            );
        }
    }

    /// The share of cacti the *finished* ground may still call closed in.
    ///
    /// A share rather than a per-plant bound, and that is the honest shape of
    /// this question rather than a softening of it. The rule reads [`Lie`],
    /// measured on the landform grid — no detail octaves, no crags, and no
    /// coastal reshaping. The arid collar is the one country where that last
    /// omission bites hardest, because the collar *is* the coast: the finished
    /// height field there has been bent into beaches and cliffs since the
    /// landform was measured, so the two fields genuinely disagree about
    /// individual points. Re-measuring the rule's own test on the finished
    /// ground puts a handful of legitimately-placed cacti in what that ground
    /// calls a hollow, and a per-plant assertion could only be made to pass by
    /// weakening it until it asserted nothing.
    ///
    /// So the claim is about the population, where the two fields do agree.
    /// Measured with the rule's own reach and rise, off the finished height
    /// field: the placed cacti come out 5% closed in on seed 7, 9% on seed 1
    /// and 12% on 20040112. The same rule with its [`Lie`] test inverted comes
    /// out between 67% and 82% on those three, so a quarter separates the two
    /// with a wide margin either side and is not fitted to one map.
    ///
    /// What it cannot catch is the field going *missing* — an empty enclosure
    /// grid calls everything open, and hollows are only about 4% of the arid
    /// collar to begin with, so losing the test entirely would move this by
    /// less than the seeds move it. That is a fact about the arid country
    /// rather than a hole in the test: see the module header on how little the
    /// [`Lie`] test does here compared with what it does for the bananas.
    const TEST_CLOSED: f32 = 0.25;

    /// Whether the finished ground closes in around a point, by the same
    /// measure [`crate::terrain::Lie`] is cut from — [`AROUND`] sampled
    /// [`HOLLOW_REACH`] out — but taken live off the height field a player
    /// walks rather than read back off the grid the rule read.
    fn closed_in(island: &Island, at: Vec2, height: f32) -> bool {
        let rising = AROUND
            .iter()
            .filter(|step| {
                let probe = at + **step * HOLLOW_REACH;
                island.height(probe.x, probe.y) > height + TEST_RISE
            })
            .count();
        rising >= TEST_ENCLOSED
    }

    /// The two numbers `closed_in` cuts at, which are [`crate::terrain`]'s own
    /// `HOLLOW_RISE` and `HOLLOW_CLOSED`. Copied rather than borrowed on
    /// purpose: they are private to the module that shapes the field, and a
    /// test that moved with them would agree with the rule by construction
    /// instead of checking it.
    const TEST_RISE: f32 = 1.75;
    const TEST_ENCLOSED: usize = 5;

    #[test]
    fn every_cactus_stands_where_the_rule_says() {
        // Checked against the finished island rather than against the
        // arithmetic that placed them, which is the only way this test can
        // fail for a reason worth knowing about. The country, the slope and
        // the salt are read live and hold plant by plant; the openness is
        // re-measured by `closed_in` rather than read back off [`Lie`], whose
        // whole point is that it is a cached field, and holds over the
        // population — see [`TEST_CLOSED`].
        let world = world(7);
        let mut counted = 0;
        let mut closed = 0;
        for (chunk, cactus) in all_cacti(&world) {
            let at = chunk.as_vec2() * CHUNK_METRES + cactus.at;
            let island = world.island(
                world
                    .island_at_chunk(chunk)
                    .expect("a chunk with a cactus on it is an island's"),
            );
            let height = island.height(at.x, at.y);
            let normal = island.normal(at.x, at.y);
            assert!(
                normal.y >= LEVEL,
                "a cactus stands on a face rather than on ground"
            );
            let ground = island.ground(at.x, at.y, height, normal);
            assert_eq!(
                ground.country,
                Country::Arid,
                "a cactus stands outside the dry collar"
            );
            assert!(
                !ground.bared_by_salt,
                "a cactus stands on ground the salt has scoured"
            );
            counted += 1;
            closed += usize::from(closed_in(&island, at, height));
        }
        assert!(
            counted > 20,
            "only {counted} cacti — the rule has stopped finding dry country"
        );
        let share = closed as f32 / counted as f32;
        assert!(
            share < TEST_CLOSED,
            "{closed} of {counted} cacti stand where the finished ground closes \
             in around them — that is a basin full of them, not open country"
        );
    }

    #[test]
    fn every_cactus_stands_inside_the_chunk_that_carries_it() {
        // What makes a cactus drawn exactly once: one past the boundary would
        // be drawn by a client that never asked for that ground, and again by
        // the chunk it really stands on.
        for seed in [20_040_112, 1, 7] {
            for (chunk, cactus) in all_cacti(&world(seed)) {
                let world_at = chunk.as_vec2() * CHUNK_METRES + cactus.at;
                assert_eq!(chunk_at(world_at), chunk, "a cactus strayed off its chunk");
            }
        }
    }

    #[test]
    fn the_lattice_divides_a_chunk_exactly() {
        // Every chunk holds a whole number of cells, none straddling a
        // boundary — which is what lets a chunk decide its own cacti without
        // asking its neighbours what they claimed.
        assert_eq!((CHUNK_METRES / CELL).fract(), 0.0);
        const { assert!(JITTER < CELL * 0.5, "a cactus could leave its own cell") };
    }
}
