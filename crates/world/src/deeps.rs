//! The floor of the sea between islands.
//!
//! Every island's map fits its own sea bed down to [`OCEAN_DEPTH`] by the rim,
//! and its skirt guarantees exactly that at the edge of what the island
//! answers for. Nothing past that edge is ever drawn: water goes blind a
//! couple of metres above the floor, so the open sea could stay one flat
//! plane for ever and no eye would know. But depth is *consequential* —
//! whether an anchor holds, what a sounding says, where a hull may be left —
//! and an ocean in which every strait and every crossing is ten metres deep
//! has no deep water to decide anything with. So between islands the sea
//! deepens, here, out of the layout alone.
//!
//! The shape is a shelf and a slope. Each island's cover — frame and skirt —
//! is the top of its shelf, and the bed falls away from it along a curve that
//! lingers over the first half of the island's reach and drops through the
//! second, into a basin hundreds of metres down. Where covers stand close the
//! shallowest shelf wins, so a sound between skerries stays tens of metres
//! deep while the open sea a kilometre off the last of them reaches the
//! basin. A big island reaches further than a small one, and every reach and
//! every basin is roughened by kilometre-scale noise, so that neither is the
//! rectangle of the cover it hangs off.
//!
//! Two things are promised, and both are load-bearing. The floor is exactly
//! -[`OCEAN_DEPTH`] at every cover's edge — which is what lets an island's
//! skirt and the sea meet without a step — and it is never *above* that
//! anywhere: nothing between islands is sent to a client, so a bank rising
//! into sight out here would be ground with no chunk to carry it. Anything
//! meant to be seen belongs inside an island's frame.
//!
//! Deterministic like the rest of the crate — add, multiply and the crate's
//! own noise — and pinned by the world digest: this never crosses the wire,
//! but a seed re-hosted elsewhere must still say the same water under the
//! same hull.

use glam::Vec2;

use protocol::ground::OCEAN_DEPTH;

use crate::archipelago::{IslandSpec, LARGEST_ISLAND};
use crate::noise::Noise;

/// How deep the basins between islands run, in metres below [`OCEAN_DEPTH`],
/// before the noise has its say — the middle of the swing below.
pub const ABYSS: f32 = 300.0;

/// How far the noise moves a basin either side of [`ABYSS`], as a fraction of
/// it. Half: the deepest basin is three times the shallowest, and the noise
/// never reaching a full swing is what keeps every basin below the shelf top.
const BASIN_SWING: f32 = 0.5;

/// Metres per cycle of the basin noise — a good part of a coarse parcel, so a
/// basin is a feature of a sea rather than of a bay.
const BASIN_SCALE: f32 = 3000.0;

/// The deepest the sea gets, in metres below the surface: the deepest basin
/// the swing allows, under the shelf top. What a depth ramp is drawn against.
pub const DEEPEST: f32 = OCEAN_DEPTH + ABYSS * (1.0 + BASIN_SWING);

/// Metres of shelf every island reaches past its cover, however small.
const SHELF_BASE: f32 = 400.0;

/// Metres of shelf per metre of an island's longer side, on top of the base:
/// a big island stands on a wider shelf.
const SHELF_PER_METRE: f32 = 0.5;

/// How far the noise stretches or shrinks a shelf, as a fraction of its reach.
const SHELF_WANDER: f32 = 0.35;

/// Metres per cycle of the shelf noise.
const SHELF_SCALE: f32 = 1400.0;

/// The furthest any island's shelf can reach past its cover, in metres — the
/// largest island the layout draws, on the noise's longest stretch. How far
/// around a point the layout has to be asked before the floor there can be
/// answered.
pub const REACH: f32 = (SHELF_BASE + SHELF_PER_METRE * LARGEST_ISLAND) * (1.0 + SHELF_WANDER);

/// Salts the world seed for the two noises, so neither is the field any
/// island happens to be generated from. The words are only for being unlike
/// each other.
const BASIN_SALT: u32 = 0x6465_6570; // "deep"
const SHELF_SALT: u32 = 0x7368_6C66; // "shlf"

/// The noise the floor is roughened with, made once for a world.
pub struct Roughness {
    basins: Noise,
    shelves: Noise,
}

impl Roughness {
    pub fn new(seed: u32) -> Self {
        Self {
            basins: Noise::new(seed ^ BASIN_SALT),
            shelves: Noise::new(seed ^ SHELF_SALT),
        }
    }
}

/// The floor of one region of the sea, prepared to be sounded many times:
/// the islands whose shelves can reach into the region, found once, and the
/// world's roughness. [`Deeps::floor`] answers for any point inside the
/// region it was prepared for; asked about a point outside it, it would miss
/// the islands out there and sound a basin where a shelf stands.
pub struct Deeps<'a> {
    roughness: &'a Roughness,
    /// Every island whose cover lies within [`REACH`] of the region — see
    /// [`crate::archipelago::Archipelago::deeps`], which is what finds them.
    islands: Vec<IslandSpec>,
    min: Vec2,
    max: Vec2,
}

impl<'a> Deeps<'a> {
    pub(crate) fn new(
        roughness: &'a Roughness,
        islands: Vec<IslandSpec>,
        min: Vec2,
        max: Vec2,
    ) -> Self {
        Self {
            roughness,
            islands,
            min,
            max,
        }
    }

    /// Height of the sea floor at a point of the region, in metres — at most
    /// -[`OCEAN_DEPTH`], and exactly that at the edge of any island's cover.
    pub fn floor(&self, at: Vec2) -> f32 {
        debug_assert!(
            at.cmpge(self.min).all() && at.cmple(self.max).all(),
            "{at} is outside the region these deeps were prepared for"
        );
        let stretch = 1.0
            + SHELF_WANDER
                * self
                    .roughness
                    .shelves
                    .fbm(at.x / SHELF_SCALE, at.y / SHELF_SCALE, 2);
        // How far down its slope the nearest shelf has got, in `0.0..=1.0`;
        // one where no shelf reaches. The shallowest wins, so a point off two
        // islands stands on the shelf of whichever holds it up.
        let mut slope: f32 = 1.0;
        for island in &self.islands {
            let reach = (SHELF_BASE + SHELF_PER_METRE * island.extent().max_element()) * stretch;
            let t = (island.beyond_cover(at) / reach).min(1.0);
            // A smoothstep of the *square*: flat at both ends like any
            // smoothstep, but lingering near the shelf top so that the water a
            // few hundred metres off a skerry is still tens of metres, not
            // half the basin.
            let t = t * t;
            slope = slope.min(t * t * (3.0 - 2.0 * t));
        }
        let basin = ABYSS
            * (1.0
                + BASIN_SWING
                    * self
                        .roughness
                        .basins
                        .fbm(at.x / BASIN_SCALE, at.y / BASIN_SCALE, 3));
        -OCEAN_DEPTH - basin * slope
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archipelago::{Archipelago, WorldConfig, CHUNK_METRES};

    /// A window holding a few coarse parcels: enough basins and enough
    /// skerry-belts to say something about both.
    const WINDOW: f32 = 12_000.0;

    fn world(seed: u32) -> Archipelago {
        Archipelago::new(&WorldConfig { seed })
    }

    /// Points of open sea across the window, on a lattice, each with how far
    /// it stands past the nearest island's cover.
    fn open_sea(world: &Archipelago) -> Vec<(Vec2, f32)> {
        let reach = Vec2::splat(WINDOW + REACH);
        let islands = world.islands_within(-reach, reach);
        let mut points = Vec::new();
        for iz in -60..=60 {
            for ix in -60..=60 {
                let at = Vec2::new(ix as f32, iz as f32) * (WINDOW / 60.0);
                if world.island_at(at.x, at.y).is_some() {
                    continue;
                }
                let apart = islands
                    .iter()
                    .map(|s| s.beyond_cover(at))
                    .fold(f32::INFINITY, f32::min);
                points.push((at, apart));
            }
        }
        points
    }

    #[test]
    fn the_sea_meets_every_cover_at_the_floor_and_never_rises_above_it() {
        for seed in [1, 7, 20_040_112] {
            let world = world(seed);
            let reach = Vec2::splat(WINDOW);
            for spec in world.islands_within(-reach, reach) {
                // A metre outside the cover on every side, the floor is the
                // skirt's own depth to the centimetre: the seam an island's
                // meshes and the open sea would otherwise show.
                let (lo, hi) = spec.covered();
                let (lo, hi) = (
                    lo.as_vec2() * CHUNK_METRES - 1.0,
                    hi.as_vec2() * CHUNK_METRES + 1.0,
                );
                for i in 0..=8 {
                    let f = i as f32 / 8.0;
                    for at in [
                        Vec2::new(lo.x + (hi.x - lo.x) * f, lo.y),
                        Vec2::new(lo.x + (hi.x - lo.x) * f, hi.y),
                        Vec2::new(lo.x, lo.y + (hi.y - lo.y) * f),
                        Vec2::new(hi.x, lo.y + (hi.y - lo.y) * f),
                    ] {
                        let floor = world.height(at.x, at.y);
                        assert!(
                            (floor + OCEAN_DEPTH).abs() < 0.01,
                            "seed {seed}: the sea stands {floor} m a metre off the cover of {spec:?} at {at}"
                        );
                    }
                }
            }
            for (at, _) in open_sea(&world) {
                let floor = world.height(at.x, at.y);
                assert!(
                    floor <= -OCEAN_DEPTH,
                    "seed {seed}: the sea floor rises to {floor} m at {at}, into what a client draws"
                );
                assert!(
                    floor >= -DEEPEST,
                    "seed {seed}: {floor} m at {at} is below the deepest basin"
                );
            }
        }
    }

    #[test]
    fn the_sea_deepens_away_from_land_into_basins_hundreds_of_metres_down() {
        for seed in [1, 7, 20_040_112] {
            let world = world(seed);
            let points = open_sea(&world);
            // Binned by distance from the nearest cover: close in, mid-shelf,
            // and out past every shelf. Each band runs deeper than the last
            // on average — the shelf and the slope, read off the layout as a
            // whole rather than off one transect the noise could tilt.
            let mean = |lo: f32, hi: f32| {
                let band: Vec<f32> = points
                    .iter()
                    .filter(|(_, apart)| (lo..hi).contains(apart))
                    .map(|(at, _)| -world.height(at.x, at.y))
                    .collect();
                assert!(
                    band.len() > 20,
                    "seed {seed}: only {} points {lo}-{hi} m off a cover",
                    band.len()
                );
                band.iter().sum::<f32>() / band.len() as f32
            };
            let (near, mid, far) = (
                mean(0.0, 200.0),
                mean(200.0, 500.0),
                mean(600.0, f32::INFINITY),
            );
            assert!(
                near < mid && mid < far,
                "seed {seed}: bands sound {near}, {mid}, {far} m"
            );
            // Hard by a cover the water is still an anchorage for something
            // bigger than a sloop; a few hundred metres further out it is
            // nothing of the kind. The layout's fine parcels put an islet
            // within a kilometre of nearly everywhere, so "far" is the sea a
            // small island's shelf has given up on, not mid-ocean.
            assert!(
                near < 30.0,
                "seed {seed}: {near} m on average within 200 m of a cover"
            );
            assert!(
                far > 100.0,
                "seed {seed}: {far} m on average out past the shelves"
            );
        }
    }

    #[test]
    fn a_region_prepared_once_sounds_as_the_world_does() {
        // The renderer prepares a region and sounds it pixel by pixel; the
        // server asks one point at a time. Same floor, or a plan would show a
        // sea the server does not serve.
        let world = world(7);
        let deeps = world.deeps(Vec2::splat(-WINDOW), Vec2::splat(WINDOW));
        for (at, _) in open_sea(&world) {
            assert_eq!(deeps.floor(at), world.height(at.x, at.y), "at {at}");
        }
    }
}
