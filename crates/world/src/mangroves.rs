//! Where the mangroves stand.
//!
//! In the water. That is the whole of what makes this a third habitat rather
//! than a third scatter: a palm wants the last dry half metre of a beach, a
//! banana wants the bank above a waterline, and a mangrove wants its feet
//! under one — so the three are stacked up a lake's margin in that order with
//! nothing to arbitrate between them. A cell is on one side of the water or
//! the other, and the tests each rule is held to say which.
//!
//! It also fills the one stretch of the palette nothing stood on. A lake is
//! painted out of fresh water's own three tones — silt, weed and a reed margin
//! — and every one of them was bare ground, so every lake in the world wore a
//! ring of empty colour that nothing in the picture explained.
//!
//! Nothing here shapes ground or paints it, exactly as in [`crate::palms`] and
//! [`crate::bananas`]: the arithmetic only ever reads the finished island, so
//! adding mangroves to a world cannot move the height field or the colours the
//! digests pin.

use glam::{IVec2, Vec2};
use protocol::ground::{Kind, Plant, CHUNK_METRES};

use crate::archipelago::Island;
use crate::plants::{draw, mix};
use crate::terrain::{Country, LakeZone};

/// Metres between the cells a mangrove may stand in — half the palms' spacing
/// and a quarter of the bananas'.
///
/// The tightest lattice of the three, because a mangrove thicket is the one
/// stand in this world that is *meant* to close up. A canopy is very nearly
/// this wide, so trees in a full patch touch and — with the jitter either way
/// — mostly overlap, and the fringe reads as one mass with arches under it. At
/// the palms' spacing the same rule drew a row of separate trees standing in a
/// lake, which is an orchard rather than a swamp.
///
/// It is affordable here where it would not be elsewhere because the ground
/// that qualifies is a fringe a few metres wide, not a whole chunk — see
/// [`OFFSHORE`], which is what keeps this off the open shallows.
const CELL: f32 = 4.0;

/// How far off its cell's centre a tree may stand — short of half a cell, so it
/// stays inside the cell that produced it and the chunk it belongs to is
/// decidable without asking the neighbours.
const JITTER: f32 = 1.8;

/// The share of qualifying cells that carry a tree where the thicket is at its
/// thickest — which is all of them, so a full patch closes right up. The other
/// two rules thin an even scatter with this; here it is the top of a range that
/// [`PATCH_CELLS`] deals out, and a stand that does not close up is not a stand.
const DENSITY: f32 = 1.0;

/// How many cells across the patches a shoreline's thicket is dealt out in —
/// so six of [`CELL`], twenty-four metres, a handful of trees each way.
///
/// Mangroves do not fringe a lake evenly. They take hold where a seedling could
/// strand and hold, and the stretches between are open water and bare reed —
/// so a shoreline is a run of stands with gaps, not a hedge of even thickness.
/// An even scatter along the margin is what this rule drew first, and it read
/// as a planted border round a pond.
///
/// A patch is a second draw off a second hash, taken before any terrain is
/// touched, so the whole of the clumping costs one multiply on cells that were
/// going to be rejected anyway.
const PATCH_CELLS: i32 = 6;

/// How the thickness of a patch is stretched before it is used, as a scale and
/// an offset on a uniform draw.
///
/// Drawn straight, every patch is *some* thickness and none is empty: the
/// fringe thickens and thins along the shore but never actually breaks, which
/// is the thing worth having. Stretched past both ends and clamped, roughly a
/// fifth of patches come out bare and a fifth solid, with the rest graded
/// between — stands with water between them.
const PATCH_SPREAD: f32 = 2.2;
const PATCH_BARE: f32 = 0.35;

/// How much thinner the thicket is out in the weed than in the reeds at the
/// lake's own edge.
///
/// Mangroves crowd the waterline and straggle out from it, which an even band
/// of them across the whole shallows does not say. Reading the tone is what
/// makes this a distance from the edge rather than a depth — see the note in
/// [`in_cell`], and the reason a lake's own tones are laid out that way.
const OFFSHORE: f32 = 0.18;

/// How deep the water over a mangrove's foot may be, in metres.
///
/// What the prop roots are for, and so the one number the model and the rule
/// have to agree about: the arches hold the canopy clear of the water, and a
/// tree standing in more of it than they are tall is a bush floating on a lake.
/// The size range a tree is drawn from is the margin here — see
/// [`Kind::scale`], whose smallest mangrove still carries its canopy well above
/// this.
const DRAUGHT: f32 = 1.5;

/// How level the bed has to be, as the upward component of its normal — about
/// twenty degrees. The draught already keeps a tree out of deep water, so this
/// is not a second way of saying the same thing: it is what keeps one off a
/// bank that plunges, where the shallow water is a strip narrower than the tree
/// standing in it.
const LEVEL: f32 = 0.88;

/// Every mangrove on one chunk of an island.
///
/// The same walk the other two use — a lattice in world coordinates, so the
/// answer for a shoreline does not depend on where the chunk boundaries fell,
/// and a fixed order so that anything downstream truncating the list truncates
/// it the same way everywhere.
pub fn mangroves(island: &Island, chunk: IVec2) -> Vec<Plant> {
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

/// The mangrove standing in one lattice cell, if one does.
///
/// Ordered by what each question costs, and the order differs from the other
/// two rules in one place worth naming: the lake is asked about *before* the
/// height. A lake's surface and the distance to its edge are both grid
/// lookups, where a height is the generator's whole stack of noise — and almost
/// nowhere in the world is within a lake's reach, so this throws away what is
/// left after the density draw for less than a height costs.
fn in_cell(island: &Island, cell: IVec2, base: Vec2) -> Option<Plant> {
    let seed = mix(cell, island.spec.seed);

    // How thick the thicket is here, drawn off the patch this cell belongs to
    // rather than off the cell — so neighbouring cells agree about it and the
    // trees come in stands. The fifth draw, which nothing off a *cell*'s own
    // hash uses, so a cell that is its own patch origin still gets two
    // unrelated numbers.
    let patch = mix(cell.div_euclid(IVec2::splat(PATCH_CELLS)), island.spec.seed);
    let thickness = (draw(patch, 5) * PATCH_SPREAD - PATCH_BARE).clamp(0.0, 1.0);

    // One die, thrown once and read twice: here against the loosest threshold
    // there is, and again below against a tighter one once the ground has said
    // how far out from the edge this is. Re-reading the same draw rather than
    // taking another keeps the cheap test first — a cell out of the water
    // costs this much and no terrain at all — without the two thresholds being
    // two independent chances to grow.
    let dice = draw(seed, 0);
    if dice > DENSITY * thickness {
        return None;
    }

    let at = (cell.as_vec2() + 0.5) * CELL
        + Vec2::new(draw(seed, 1) - 0.5, draw(seed, 2) - 0.5) * 2.0 * JITTER;

    let level = island.lake_level(at.x, at.y)?;

    // Wet feet, and not too wet. Strictly at or under the water: the bank
    // above it is the bananas', and a rule that reached up onto it would be a
    // second opinion about the same strip of ground.
    let height = island.height(at.x, at.y);
    let depth = level - height;
    if !(0.0..=DRAUGHT).contains(&depth) {
        return None;
    }

    let normal = island.normal(at.x, at.y);
    if normal.y < LEVEL {
        return None;
    }

    // How far out from the lake's own edge this stands, which is what a
    // lake's three places are: reeds at the margin, weed in the shallows
    // behind them, and open water beyond — where a tree would be one that had
    // walked out from the shore. Measured out from the edge rather than down
    // from the surface because a bed shelves too gently for a depth line to
    // fall anywhere sensible on it; see [`LakeZone`].
    //
    // It is also what the thicket thins across, so the stand crowds the
    // waterline and straggles away from it: out in the weed a cell has to
    // beat [`OFFSHORE`] of the threshold it has already beaten.
    //
    // This used to read the material and match on marsh and weed. Those are
    // the colours the zones are painted, so it worked, but it made a rule
    // about where a tree can root depend on how a lake is drawn — and the
    // comment defending it had to argue that the tone *was* the band.
    let thinner = match island.ground(at.x, at.y, height, normal).country {
        Country::Lake(LakeZone::Margin) => 1.0,
        Country::Lake(LakeZone::Shallows) => OFFSHORE,
        _ => return None,
    };
    if dice > DENSITY * thickness * thinner {
        return None;
    }

    let (small, large) = Kind::Mangrove.scale();
    Some(Plant {
        kind: Kind::Mangrove,
        at: at - base,
        yaw: draw(seed, 3) * std::f32::consts::TAU,
        scale: small + draw(seed, 4) * (large - small),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archipelago::{Archipelago, WorldConfig};
    use crate::testing::{digest, floats};
    use protocol::ground::chunk_at;

    /// The same window the other two rules are judged over, for the same
    /// reason: several islands of assorted sizes, and so lakes of assorted
    /// shapes.
    const WINDOW: i32 = 20;

    fn world(seed: u32) -> Archipelago {
        Archipelago::new(&WorldConfig { seed })
    }

    /// Every mangrove in the window, with the chunk that carried it.
    fn all_mangroves(world: &Archipelago) -> Vec<(IVec2, Plant)> {
        let mut found = Vec::new();
        for cz in -WINDOW..WINDOW {
            for cx in -WINDOW..WINDOW {
                let chunk = IVec2::new(cx, cz);
                if let Some(payload) = world.chunk_payload(chunk) {
                    found.extend(
                        payload
                            .plants
                            .into_iter()
                            .filter(|plant| plant.kind == Kind::Mangrove)
                            .map(|tree| (chunk, tree)),
                    );
                }
            }
        }
        found
    }

    #[test]
    fn a_seed_grows_the_same_mangroves_wherever_it_is_hosted() {
        // Pinned exactly as the palms and the bananas are: a seed has to mean
        // the same lake with the same thicket round it on any machine that
        // serves it. If you meant to change the rule, re-record these — run
        // with --nocapture and the new values are printed. If you did not, a
        // platform has stopped agreeing about what a seed means.
        let recorded = [
            (20_040_112u32, 0x5E7B_DBD0_BC5D_DEBAu64),
            (1, 0xDBBD_1689_3CD7_0BA1),
            (7, 0xF8F7_48C3_FA15_1B58),
        ];
        let got: Vec<(u32, u64, usize)> = recorded
            .iter()
            .map(|(seed, _)| {
                let trees = all_mangroves(&world(*seed));
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
            println!("seed {seed} grows {count} mangroves, digesting to {d:#018X}");
        }
        for ((seed, d, _), (_, was)) in got.iter().zip(recorded) {
            assert_eq!(
                *d, was,
                "seed {seed} no longer grows the mangroves it grew — see the note above"
            );
        }
    }

    #[test]
    fn every_mangrove_stands_in_the_water() {
        // The whole of the rule, checked against the finished island rather
        // than against the arithmetic that placed them — so a change to how
        // lakes are shaped or painted shows up here as mangroves standing
        // somewhere silly, which is what it would be.
        let world = world(7);
        let mut counted = 0;
        for (chunk, tree) in all_mangroves(&world) {
            let at = chunk.as_vec2() * CHUNK_METRES + tree.at;
            let island = world.island(
                world
                    .island_at_chunk(chunk)
                    .expect("a chunk with a mangrove on it is an island's"),
            );
            let height = island.height(at.x, at.y);
            let level = island
                .lake_level(at.x, at.y)
                .expect("a mangrove stands in fresh water");
            assert!(
                (0.0..=DRAUGHT).contains(&(level - height)),
                "a mangrove stands in {}m of water",
                level - height
            );
            assert!(
                matches!(
                    island
                        .ground(at.x, at.y, height, island.normal(at.x, at.y))
                        .country,
                    Country::Lake(LakeZone::Margin | LakeZone::Shallows)
                ),
                "a mangrove stands on ground a lake has no say over"
            );
            counted += 1;
        }
        assert!(
            counted > 20,
            "only {counted} mangroves — the rule has stopped finding lakes"
        );
    }

    #[test]
    fn every_mangrove_stands_inside_the_chunk_that_carries_it() {
        // What makes a tree drawn exactly once: one past the boundary would be
        // drawn by a client that never asked for that ground, and again by the
        // chunk it really stands on.
        for seed in [20_040_112, 1, 7] {
            for (chunk, tree) in all_mangroves(&world(seed)) {
                let world_at = chunk.as_vec2() * CHUNK_METRES + tree.at;
                assert_eq!(
                    chunk_at(world_at),
                    chunk,
                    "a mangrove strayed off its chunk"
                );
            }
        }
    }

    #[test]
    fn the_lattice_divides_a_chunk_exactly() {
        // Every chunk holds a whole number of cells, none straddling a
        // boundary — which is what lets a chunk decide its own mangroves
        // without asking its neighbours what they claimed.
        assert_eq!((CHUNK_METRES / CELL).fract(), 0.0);
        const { assert!(JITTER < CELL * 0.5, "a tree could leave its own cell") };
    }
}
