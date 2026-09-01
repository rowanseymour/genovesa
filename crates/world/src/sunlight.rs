//! When the ground sees the sun: the island-wide bake behind
//! [`protocol::ground::ChunkPayload::lit`].
//!
//! The terrain is fixed and the sun rides one arc, so whether a point stands
//! in the sun is a function of the hour alone — the whole day compresses to
//! the phase a point first sees the sun and the phase it loses it. The wire's
//! doc says what a client does with the pair; this module is how a generator
//! arrives at it.
//!
//! It is an island-wide question. What shadows a point at dawn may be a ridge
//! kilometres away, so the bake runs once per island over one raster of the
//! island's whole visible surface — ground, or the water standing over it,
//! whichever the sun actually strikes. Other islands are ignored: the layout
//! keeps open sea between islands, so one could only shadow another under a
//! sun grazing the horizon, when the light is at its dimmest and reddest and
//! an error errs toward light.
//!
//! The arc is walked at every phase step [`protocol::quantize_phase`] can
//! name between sunrise and sunset, and for each the raster is swept once in
//! the sun's own direction, carrying a running shadow height that each point
//! either ducks under or clears — a pass over the raster per sampled hour,
//! not a ray march per point. The raster shares the wire's own grid
//! ([`protocol::ground::CELL_METRES`]), so a chunk corner lands on a raster
//! point exactly and reads its pair off without interpolating.
//!
//! It shares it because a coarser raster clips the *casting* side. At 4 m —
//! what this was for a long time — a crag a couple of metres wide falls
//! between samples, and with its crest gone the shadow it throws comes out
//! hours short: measured against a ray march of the same surface, a knoll's
//! lee read as lit until 16:00 where the truth was shade from 14:24, a
//! terminator standing tens of metres from the relief the cells it crosses
//! are painted by. At the cells' own spacing the bake agrees with the march
//! to a phase step almost everywhere. The theory the coarse raster rested on
//! — that a lit threshold varies over tens of metres — holds for the ground
//! *receiving* the shade and not for the ridge casting it.
//!
//! That is sixteen times the raster points, which is why the arc is split
//! into bands of phases across threads. Each band sweeps the immutable
//! raster for its own hours and the answers merge by `min` and `max` —
//! associative, commutative, and exact on `u8`, so the bake is the same bit
//! for bit however many threads it runs on. That matters here more than it
//! saves time: the digest tests hold a seed to one answer. The raster itself,
//! and the sweep, are [`crate::raster`]'s — the wind's bake walks the same
//! grid the same way, with a different loss per metre.

use glam::Vec2;

use protocol::ground::{LIT_ALL_DAY, NEVER_LIT};

use crate::raster::{hands, sweep, Grid, Raster};

/// How far above a point's own surface the swept shadow height must stand
/// before the point counts as shadowed. Without it, the interpolation the
/// sweep leans on reads a hair of false occlusion off every slope's own
/// shoulder — the raster equivalent of shadow acne.
const BIAS: f32 = 0.05;

/// One sampled hour of the sun's arc, flattened to what the sweep works in:
/// the unit ground direction toward the sun — east and south components,
/// [`protocol::ground::NORTH`] being north — and the tangent of its altitude,
/// which is how much height an occluder's shadow loses per metre walked
/// toward it.
struct Step {
    east: f32,
    south: f32,
    tan: f32,
}

impl Step {
    /// The arc at one phase of the day — [`protocol::towards_the_sun`], from
    /// the ground's point of view.
    fn at(phase: f32) -> Self {
        let sun = protocol::towards_the_sun(phase);
        let ground = (sun.x * sun.x + sun.z * sun.z).sqrt();
        Self {
            east: sun.x / ground,
            south: sun.z / ground,
            tan: sun.y / ground,
        }
    }
}

/// One island's baked daylight: for every raster point, the first and last
/// phase step at which the sun stands clear of the terrain around it.
pub struct Sunlight {
    grid: Grid,
    /// The pairs, row-major — `[from, until]`, [`NEVER_LIT`] where no sample
    /// of the arc ever reached the point.
    lit: Vec<[u8; 2]>,
}

impl Sunlight {
    /// Bakes the daylight over a raster of the visible surface — ground, or
    /// the standing water over it — on the wire's own grid: see the module
    /// doc for why no coarser one will do.
    pub fn bake(ground: &Raster) -> Self {
        Self::baked_across(ground, hands())
    }

    /// [`Sunlight::bake`] over a named number of threads, which only a test
    /// has any business choosing — that the choice cannot be read back out of
    /// the answer is the whole of what it checks.
    fn baked_across(ground: &Raster, hands: usize) -> Self {
        let grid = ground.grid;
        let points = grid.points();

        // The arc, by bands of phases. Each band sweeps the whole raster for
        // its own hours into its own pairs; the merge below is what makes the
        // split invisible — see the module header.
        let (first, last) = (LIT_ALL_DAY[0], LIT_ALL_DAY[1]);
        let phases = (last - first) as usize + 1;
        let band = phases.div_ceil(hands);
        let mut lit = vec![NEVER_LIT; points];
        std::thread::scope(|scope| {
            let mut bands = Vec::new();
            for b in 0..phases.div_ceil(band) {
                bands.push(scope.spawn(move || {
                    let mut theirs = vec![NEVER_LIT; points];
                    let mut shade = vec![0.0f32; points];
                    let from = first + (b * band) as u8;
                    for phase in from..=(from + (band - 1) as u8).min(last) {
                        let step = Step::at(protocol::dequantize_phase(phase));
                        sweep(
                            &mut shade,
                            ground,
                            Vec2::new(step.east, step.south),
                            step.tan,
                        );
                        for (at, pair) in theirs.iter_mut().enumerate() {
                            if shade[at] - ground.values[at] <= BIAS {
                                pair[0] = pair[0].min(phase);
                                pair[1] = pair[1].max(phase);
                            }
                        }
                    }
                    theirs
                }));
            }
            for done in bands {
                for (pair, theirs) in lit.iter_mut().zip(done.join().expect("a sweep band")) {
                    pair[0] = pair[0].min(theirs[0]);
                    pair[1] = pair[1].max(theirs[1]);
                }
            }
        });

        Self { grid, lit }
    }

    /// The lit pair at a world point — or [`LIT_ALL_DAY`] beyond the raster,
    /// where there is only open sea for the sun to fall on. The nearest
    /// point's own pair, for the reason [`Grid::nearest`] gives: whoever
    /// needs an answer *between* corners weighs them knowing what a
    /// [`NEVER_LIT`] pair means — [`protocol::ground::lit_across`].
    pub fn at(&self, wx: f32, wz: f32) -> [u8; 2] {
        self.grid
            .nearest(wx, wz)
            .map_or(LIT_ALL_DAY, |at| self.lit[at])
    }
}

#[cfg(test)]
mod tests {
    use protocol::ground::NEVER_LIT;

    use super::*;

    #[test]
    fn the_bake_is_the_same_however_many_threads_run_it() {
        // The seed digests hold one answer per seed, so a bake that drifted
        // with the host's core count would make a seed mean somewhere else on
        // a different machine — the failure the whole crate is arranged
        // against. A ridge across a basin, so the raster carries real
        // thresholds and never-lit ground rather than one flat number.
        let relief = |wx: f32, wz: f32| {
            if (wx - 96.0).abs() < 6.0 {
                40.0
            } else {
                (wz / 40.0).sin() * 3.0
            }
        };
        let ground = Raster::sample(Vec2::ZERO, Vec2::splat(256.0), 1.0, relief, 1);
        let one = Sunlight::baked_across(&ground, 1);
        for hands in [2, 3, 5, 16] {
            let many = Sunlight::baked_across(&ground, hands);
            assert_eq!(
                one.lit, many.lit,
                "{hands} threads baked a different day than one did"
            );
        }
        assert!(
            one.lit.iter().any(|pair| *pair != LIT_ALL_DAY),
            "a wall across the raster and nothing fell in its shadow"
        );
    }

    #[test]
    fn open_ground_is_lit_from_sunrise_to_sunset() {
        let flat = Sunlight::bake(&Raster::sample(
            Vec2::ZERO,
            Vec2::splat(256.0),
            1.0,
            |_, _| 0.0,
            2,
        ));
        assert_eq!(flat.at(128.0, 128.0), LIT_ALL_DAY);
        // And past the raster there is only sea, which nothing shadows.
        assert_eq!(flat.at(-1000.0, 40.0), LIT_ALL_DAY);
    }

    #[test]
    fn a_wall_to_the_east_costs_the_morning_and_nothing_else() {
        // A hundred-metre wall along x = 256. West of it the sun has to
        // climb over the wall before the ground sees it, so the morning
        // starts late — later the nearer the wall — while the evening is
        // untouched, the sunset being on the ground's own side.
        let wall = |wx: f32, _wz: f32| if wx >= 256.0 { 100.0 } else { 0.0 };
        let baked = Sunlight::bake(&Raster::sample(
            Vec2::ZERO,
            Vec2::splat(512.0),
            1.0,
            wall,
            2,
        ));

        let near = baked.at(224.0, 256.0);
        let far = baked.at(32.0, 256.0);
        assert!(
            near[0] > far[0] && far[0] > LIT_ALL_DAY[0],
            "the wall's shadow does not deepen toward it: {near:?} against {far:?}"
        );
        assert_eq!(near[1], LIT_ALL_DAY[1], "the wall stole the evening");
        // On top of the wall there is nothing in the way at all.
        assert_eq!(baked.at(400.0, 256.0), LIT_ALL_DAY);
    }

    #[test]
    fn a_pit_no_sun_can_reach_never_lights() {
        // A point a hundred metres down a four-metre hole: the sun would
        // need to stand twenty-five heights above the rim, and the arc never
        // climbs past two and a half.
        let pit = |wx: f32, wz: f32| {
            if (wx - 128.0).abs() < 2.0 && (wz - 128.0).abs() < 2.0 {
                -100.0
            } else {
                0.0
            }
        };
        let baked = Sunlight::bake(&Raster::sample(Vec2::ZERO, Vec2::splat(256.0), 1.0, pit, 2));
        assert_eq!(baked.at(128.0, 128.0), NEVER_LIT);
    }
}
