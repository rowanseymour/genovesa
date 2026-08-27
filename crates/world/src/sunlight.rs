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
//! ([`STEP_METRES`]), so a chunk corner lands on a raster point exactly and
//! reads its pair off without interpolating.
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
//! That is sixteen times the raster points, which is why both passes here
//! are threaded. Neither has anything to synchronise: the surface fill
//! writes disjoint rows, and a phase's sweep reads only the immutable ground
//! raster, so the arc splits into bands whose answers merge by `min` and
//! `max` — associative, commutative, and exact on `u8`, so the bake is the
//! same bit for bit however many threads it runs on. That matters here more
//! than it saves time: the digest tests hold a seed to one answer.

use glam::Vec2;

use protocol::ground::{CELL_METRES, LIT_ALL_DAY, NEVER_LIT};

/// How many threads the two passes below split across — the machine's, less
/// one for whoever asked, and never none. Only the *speed* rides on this: the
/// answer is the same at any count, which is what lets it be the host's.
fn hands() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(1).max(1))
        .unwrap_or(1)
}

/// Metres between raster points — the wire's own grid, [`CELL_METRES`], for
/// the reasons the module header gives.
const STEP_METRES: f32 = CELL_METRES;

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
    /// World position of raster point `(0, 0)`.
    min: Vec2,
    /// Raster points along x and z.
    columns: usize,
    rows: usize,
    /// The pairs, row-major — `[from, until]`, [`NEVER_LIT`] where no sample
    /// of the arc ever reached the point.
    lit: Vec<[u8; 2]>,
}

impl Sunlight {
    /// Bakes the daylight over `min..=max`, reading the visible surface —
    /// ground or the standing water over it — from `surface`.
    pub fn bake(min: Vec2, max: Vec2, surface: impl Fn(f32, f32) -> f32 + Sync) -> Self {
        Self::baked_across(min, max, surface, hands())
    }

    /// [`Sunlight::bake`] over a named number of threads, which only a test
    /// has any business choosing — that the choice cannot be read back out of
    /// the answer is the whole of what it checks.
    fn baked_across(
        min: Vec2,
        max: Vec2,
        surface: impl Fn(f32, f32) -> f32 + Sync,
        hands: usize,
    ) -> Self {
        let columns = ((max.x - min.x) / STEP_METRES).ceil() as usize + 1;
        let rows = ((max.y - min.y) / STEP_METRES).ceil() as usize + 1;
        let points = columns * rows;

        // The surface, by bands of whole rows — disjoint slices, so the
        // threads never look at each other's.
        let mut ground = vec![0.0f32; points];
        let band = rows.div_ceil(hands);
        std::thread::scope(|scope| {
            for (b, rows_of) in ground.chunks_mut(band * columns).enumerate() {
                let surface = &surface;
                scope.spawn(move || {
                    for (r, row) in rows_of.chunks_mut(columns).enumerate() {
                        let wz = min.y + (b * band + r) as f32 * STEP_METRES;
                        for (ix, point) in row.iter_mut().enumerate() {
                            *point = surface(min.x + ix as f32 * STEP_METRES, wz);
                        }
                    }
                });
            }
        });

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
                let ground = &ground;
                bands.push(scope.spawn(move || {
                    let mut theirs = vec![NEVER_LIT; points];
                    let mut shade = vec![0.0f32; points];
                    let from = first + (b * band) as u8;
                    for phase in from..=(from + (band - 1) as u8).min(last) {
                        let step = Step::at(protocol::dequantize_phase(phase));
                        sweep(&mut shade, ground, columns, rows, &step);
                        for (at, pair) in theirs.iter_mut().enumerate() {
                            if shade[at] - ground[at] <= BIAS {
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

        Self {
            min,
            columns,
            rows,
            lit,
        }
    }

    /// The lit pair at a world point — or [`LIT_ALL_DAY`] beyond the raster,
    /// where there is only open sea for the sun to fall on.
    ///
    /// The nearest raster point's own pair, not a blend of the ones around
    /// it. The raster is on [`STEP_METRES`] and the payload asks at its
    /// corners, so every real query lands on a point exactly and there is
    /// nothing to blend; and a pair is not a quantity to average anyway —
    /// [`NEVER_LIT`] is `from` past the end of the day, so weighing it
    /// against a lit neighbour invents an hour neither corner ever saw.
    /// Whoever needs an answer *between* corners weighs them knowing that:
    /// [`protocol::ground::lit_across`].
    pub fn at(&self, wx: f32, wz: f32) -> [u8; 2] {
        let gx = (wx - self.min.x) / STEP_METRES;
        let gz = (wz - self.min.y) / STEP_METRES;
        if gx < 0.0 || gz < 0.0 || gx > (self.columns - 1) as f32 || gz > (self.rows - 1) as f32 {
            return LIT_ALL_DAY;
        }
        let x = (gx.round() as usize).min(self.columns - 1);
        let z = (gz.round() as usize).min(self.rows - 1);
        self.lit[z * self.columns + x]
    }
}

/// One pass over the raster for one hour of the arc: afterwards `shade[p]`
/// is the height the terrain's shadow stands at over point `p` — its own
/// surface where nothing upwind of it reaches higher.
///
/// The pass walks the raster away from the sun, so each point needs only the
/// line before it: the shadow height there, read between the two nearest
/// points, dropped by what the sun's altitude costs over one step, against
/// the point's own surface. Beyond the raster lies open sea, whose shadow
/// height is its own surface at zero — which is why the first line seeds
/// from nothing.
fn sweep(shade: &mut [f32], ground: &[f32], columns: usize, rows: usize, step: &Step) {
    let x_major = step.east.abs() >= step.south.abs();
    let (majors, minors, toward, cross) = if x_major {
        (columns, rows, step.east, step.south)
    } else {
        (rows, columns, step.south, step.east)
    };
    let at = |major: usize, minor: usize| {
        if x_major {
            minor * columns + major
        } else {
            major * columns + minor
        }
    };
    // The line toward the sun is walked one major step at a time, sliding
    // this much along the minor axis — a fraction, since the major axis is
    // by construction the direction's larger component.
    let slide = cross / toward.abs();
    let drop = step.tan * (1.0 + slide * slide).sqrt() * STEP_METRES;

    for k in 0..majors {
        // From the raster's sun side inward, so the line upwind of this one
        // is already swept.
        let (major, upwind) = if toward > 0.0 {
            (majors - 1 - k, majors - k)
        } else {
            (k, k.wrapping_sub(1))
        };
        for minor in 0..minors {
            let carried = if upwind >= majors {
                0.0
            } else {
                let m = (minor as f32 + slide).clamp(0.0, (minors - 1) as f32);
                let m0 = m.floor() as usize;
                let m1 = (m0 + 1).min(minors - 1);
                let t = m - m0 as f32;
                shade[at(upwind, m0)] * (1.0 - t) + shade[at(upwind, m1)] * t
            };
            let here = at(major, minor);
            shade[here] = ground[here].max(carried - drop);
        }
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
        let one = Sunlight::baked_across(Vec2::ZERO, Vec2::splat(256.0), relief, 1);
        for hands in [2, 3, 5, 16] {
            let many = Sunlight::baked_across(Vec2::ZERO, Vec2::splat(256.0), relief, hands);
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
        let flat = Sunlight::bake(Vec2::ZERO, Vec2::splat(256.0), |_, _| 0.0);
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
        let baked = Sunlight::bake(Vec2::ZERO, Vec2::splat(512.0), wall);

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
        let baked = Sunlight::bake(Vec2::ZERO, Vec2::splat(256.0), pit);
        assert_eq!(baked.at(128.0, 128.0), NEVER_LIT);
    }
}
