//! What the wind does behind land: the island-wide bake behind
//! [`protocol::ToClient::Chunk`]'s shelter lattice.
//!
//! A headland takes the wind out of the water for some way downwind of
//! itself, and how far is a question about terrain that may be a kilometre
//! upwind of the bay being calmed — so, exactly like [`crate::sunlight`],
//! only whoever holds the whole island can answer it, and the answer
//! travels. The wire's doc says what a client does with the lattice; this
//! module is how a generator arrives at it.
//!
//! **A wake is a shadow cast by a very low sun.** That is not a metaphor
//! reached for after the fact — it is why this file is as short as it is.
//! Downwind of an obstacle the sheltered air stands as high as the obstacle
//! and recovers with distance, which is the same sweep the sunlight bake
//! already runs: walk the raster away from the source carrying a height that
//! each point either ducks under or clears, dropping it a fixed amount per
//! metre walked. Sunlight's drop is the tangent of the sun's altitude; this
//! one's is [`RECOVERY`], and everything else about the two passes is the
//! same shape. What comes back is not a shadow but a *depth of shelter* —
//! how far the wake stands above the water — which [`cover`] turns into the
//! fraction of the wind that survives.
//!
//! The sweep is run once per compass point of
//! [`protocol::ground::BEARINGS`], and the results stored per point rather
//! than resolved against a wind here, because the wind veers and a chunk is
//! handed out once. Resolving here would mean either re-sending every chunk
//! in the world each time the weather turned, or a sky that could not turn.
//!
//! Unlike the sunlight bake this reads terrain on a raster four times
//! coarser than the wire's grid, and the reason the two differ is the reason
//! [`STEP_METRES`] is worth a comment rather than a number.

use glam::Vec2;

use protocol::ground::{Exposure, BEARINGS, EXPOSED};

/// How many threads the sweeps split across — the machine's, less one for
/// whoever asked, and never none. As with the sunlight bake only the *speed*
/// rides on this: each bearing is swept whole by one thread into its own
/// slot, so no two threads ever touch the same byte and the answer is the
/// same at any count.
fn hands() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(1).max(1))
        .unwrap_or(1)
}

/// Metres between raster points — four times [`protocol::ground::CELL_METRES`],
/// and deliberately not the wire's own grid the way [`crate::sunlight`]'s
/// raster is.
///
/// That module went the other way for a reason that does not carry: a shadow
/// has a hard edge, so a two-metre crag falling between samples loses a
/// terminator that stands tens of metres from where it should. A wake has no
/// edge. Air does not pour off the top of a crag and resume a hundred metres
/// later; it closes back in behind anything small, and what shelters a bay is
/// a headland or a ridge, measured in hundreds of metres. Sampling the
/// *casting* side more finely than that would be paying sixteen times over
/// for crags that shelter nothing.
const STEP_METRES: f32 = 4.0 * protocol::ground::CELL_METRES;

/// How fast a wake recovers with distance downwind, as a slope: the sheltered
/// air loses this much height per metre travelled, so an obstacle standing
/// `h` above the water shelters for roughly `h / RECOVERY` metres behind
/// itself before the wind has closed back in.
///
/// One in twelve, which puts a forty-metre headland's cover about five
/// hundred metres downwind — the scale of a bay rather than of a whole
/// island's lee, and short enough that rounding a point is something that
/// happens over a minute of sailing rather than an afternoon of it. Real air
/// recovers faster over water than this and much slower in a valley; a single
/// slope is the simplification the whole model rests on, and the thing to
/// reach for first if a lee ever feels the wrong size.
const RECOVERY: f32 = 1.0 / 12.0;

/// How far a wake must stand above the water before the point under it is as
/// sheltered as this model ever gets, in metres. Below it the cover comes on
/// proportionally.
///
/// Deliberately smaller than the islands: a bay behind a hill of any real
/// size should be *fully* in its lee rather than a fraction of the way there,
/// and what varies from bay to bay is then how far the shelter reaches rather
/// than how deep it goes. Set as high as the summits and only the biggest
/// islands would shelter anything at all.
const FULL_COVER: f32 = 12.0;

/// The least of the wind that any lee leaves standing, as a fraction.
///
/// The floor that matters is [`protocol::LIGHT_AIR`], which
/// [`protocol::sheltered`] applies in metres per second and which is what
/// actually keeps a becalmed boat from existing. This one is separate and
/// smaller-scale: it stops the *deepest* lee reading as a hole in the
/// weather, so a gale behind a mountain is a quiet corner of a gale rather
/// than a different day. A boat sailing out of one should feel the wind come
/// on, not switch on.
const MOST_COVER: f32 = 0.18;

/// One island's baked shelter: for every raster point, how much of the wind
/// survives from each of [`BEARINGS`].
pub struct Shelter {
    /// World position of raster point `(0, 0)`.
    min: Vec2,
    /// Raster points along x and z.
    columns: usize,
    rows: usize,
    /// The exposures, row-major — see [`Exposure`].
    exposure: Vec<Exposure>,
}

/// The eight compass points as unit vectors, in the order the wire stores
/// them: north first and clockwise round the card, matching
/// [`protocol::ground::BEARINGS`]' own doc.
///
/// Built out of a square root rather than a sine, for the reason
/// [`crate::weather`] gives at length: `sin` is not correctly-rounded and
/// drifts between platforms, and this bake is under the digest tests' promise
/// that a seed means the same island everywhere.
fn bearings() -> [Vec2; BEARINGS] {
    let d = 0.5f32.sqrt();
    [
        Vec2::new(0.0, -1.0),
        Vec2::new(d, -d),
        Vec2::new(1.0, 0.0),
        Vec2::new(d, d),
        Vec2::new(0.0, 1.0),
        Vec2::new(-d, d),
        Vec2::new(-1.0, 0.0),
        Vec2::new(-d, -d),
    ]
}

/// What a wake standing `above` metres over the water leaves of the wind, as
/// the wire's byte — [`EXPOSED`] where nothing stands over it at all.
fn cover(above: f32) -> u8 {
    let deep = (above / FULL_COVER).clamp(0.0, 1.0);
    let surviving = 1.0 - (1.0 - MOST_COVER) * deep;
    (surviving * EXPOSED as f32).round() as u8
}

impl Shelter {
    /// Bakes the shelter over `min..=max`, reading the surface the wind
    /// actually blows over — ground, or the water standing on it — from
    /// `surface`, which is the same surface [`crate::sunlight`] is given.
    pub fn bake(min: Vec2, max: Vec2, surface: impl Fn(f32, f32) -> f32 + Sync) -> Self {
        Self::baked_across(min, max, surface, hands())
    }

    /// [`Shelter::bake`] over a named number of threads, which only a test
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

        // A sweep per bearing, each writing only its own slot of every
        // point's exposures. Nothing to merge and nothing to order: a
        // bearing's answer depends on the immutable ground raster and on
        // nothing another bearing did, which is what makes the thread count
        // unreadable from the result.
        let mut exposure = vec![[EXPOSED; BEARINGS]; points];
        std::thread::scope(|scope| {
            let mut swept = Vec::new();
            for (slot, toward) in bearings().into_iter().enumerate() {
                let ground = &ground;
                swept.push(scope.spawn(move || {
                    let mut wake = vec![0.0f32; points];
                    sweep(&mut wake, ground, columns, rows, toward);
                    let covers: Vec<u8> = wake
                        .iter()
                        .zip(ground)
                        .map(|(wake, surface)| cover(wake - surface))
                        .collect();
                    (slot, covers)
                }));
            }
            for done in swept {
                let (slot, covers) = done.join().expect("a bearing's sweep");
                for (point, cover) in exposure.iter_mut().zip(covers) {
                    point[slot] = cover;
                }
            }
        });

        Self {
            min,
            columns,
            rows,
            exposure,
        }
    }

    /// How exposed a world point is to each bearing — or wide open past the
    /// raster, where there is nothing but sea for the wind to cross.
    ///
    /// The nearest raster point's own exposures, not a blend of the ones
    /// around it: [`STEP_METRES`] divides [`protocol::ground::SHELTER_METRES`]
    /// exactly, so every lattice point a payload asks about lands on a raster
    /// point and there is nothing to blend. Whoever needs an answer *between*
    /// lattice points blends there — [`protocol::ground::shelter_across`].
    pub fn at(&self, wx: f32, wz: f32) -> Exposure {
        let gx = (wx - self.min.x) / STEP_METRES;
        let gz = (wz - self.min.y) / STEP_METRES;
        if gx < 0.0 || gz < 0.0 || gx > (self.columns - 1) as f32 || gz > (self.rows - 1) as f32 {
            return [EXPOSED; BEARINGS];
        }
        let x = (gx.round() as usize).min(self.columns - 1);
        let z = (gz.round() as usize).min(self.rows - 1);
        self.exposure[z * self.columns + x]
    }
}

/// One pass over the raster for one bearing: afterwards `wake[p]` is the
/// height the sheltered air stands at over point `p` — its own surface where
/// nothing upwind of it reaches higher.
///
/// The twin of [`crate::sunlight`]'s own sweep, and deliberately the same
/// walk: away from where the wind comes from, so each point needs only the
/// line before it — the wake there, read between the two nearest points,
/// dropped by what [`RECOVERY`] costs over one step, against the point's own
/// surface. Beyond the raster lies open sea, whose wake is its own surface at
/// zero, which is why the first line seeds from nothing.
///
/// `toward` is the way the wind is blowing, matching the wire's convention
/// that a wind is a velocity — so the walk runs *along* it, from the upwind
/// edge of the raster to the downwind one.
fn sweep(wake: &mut [f32], ground: &[f32], columns: usize, rows: usize, toward: Vec2) {
    let x_major = toward.x.abs() >= toward.y.abs();
    let (majors, minors, along, cross) = if x_major {
        (columns, rows, toward.x, toward.y)
    } else {
        (rows, columns, toward.y, toward.x)
    };
    let at = |major: usize, minor: usize| {
        if x_major {
            minor * columns + major
        } else {
            major * columns + minor
        }
    };
    // The line downwind of this one is one major step on, sliding this much
    // along the minor axis — a fraction, since the major axis is by
    // construction the direction's larger component.
    let slide = cross / along.abs();
    let drop = RECOVERY * (1.0 + slide * slide).sqrt() * STEP_METRES;

    for k in 0..majors {
        // From the raster's upwind side inward, so the line the wind reached
        // before this one is already swept.
        let (major, upwind) = if along > 0.0 {
            (k, k.wrapping_sub(1))
        } else {
            (majors - 1 - k, majors - k)
        };
        for minor in 0..minors {
            let carried = if upwind >= majors {
                0.0
            } else {
                let m = (minor as f32 - slide).clamp(0.0, (minors - 1) as f32);
                let m0 = m.floor() as usize;
                let m1 = (m0 + 1).min(minors - 1);
                let t = m - m0 as f32;
                wake[at(upwind, m0)] * (1.0 - t) + wake[at(upwind, m1)] * t
            };
            let here = at(major, minor);
            wake[here] = ground[here].max(carried - drop);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The slot a compass point is stored in, for tests that want to name one.
    const N: usize = 0;
    const E: usize = 2;
    const S: usize = 4;
    const W: usize = 6;

    #[test]
    fn the_bake_is_the_same_however_many_threads_run_it() {
        // The seed digests hold one answer per seed, so a bake that drifted
        // with the host's core count would make a seed mean somewhere else on
        // a different machine — the failure the whole crate is arranged
        // against. A ridge across open water, so the raster carries real
        // shelter rather than one flat number.
        let relief = |wx: f32, wz: f32| {
            if (wx - 96.0).abs() < 12.0 {
                40.0
            } else {
                (wz / 40.0).sin().max(0.0) * 2.0
            }
        };
        let one = Shelter::baked_across(Vec2::ZERO, Vec2::splat(512.0), relief, 1);
        for hands in [2, 3, 5, 16] {
            let many = Shelter::baked_across(Vec2::ZERO, Vec2::splat(512.0), relief, hands);
            assert_eq!(
                one.exposure, many.exposure,
                "{hands} threads baked a different lee than one did"
            );
        }
        assert!(
            one.exposure
                .iter()
                .any(|point| *point != [EXPOSED; BEARINGS]),
            "a ridge across the raster and nothing lay in its lee"
        );
    }

    #[test]
    fn open_water_is_exposed_from_every_quarter() {
        let flat = Shelter::bake(Vec2::ZERO, Vec2::splat(256.0), |_, _| 0.0);
        assert_eq!(flat.at(128.0, 128.0), [EXPOSED; BEARINGS]);
        // And past the raster there is only sea, which shelters nothing.
        assert_eq!(flat.at(-1000.0, 40.0), [EXPOSED; BEARINGS]);
    }

    #[test]
    fn a_ridge_shelters_the_water_behind_it_and_not_in_front() {
        // A forty-metre ridge along the raster's middle latitude. Water south
        // of it is in its lee when the wind blows from the north — that is,
        // toward the south — and wide open when the wind blows back the other
        // way. Which is the whole model in one assertion, and the one that
        // would catch the sweep walking the wrong way down its own axis.
        let ridge = |_wx: f32, wz: f32| if (wz - 128.0).abs() < 8.0 { 40.0 } else { 0.0 };
        let baked = Shelter::bake(Vec2::ZERO, Vec2::splat(1024.0), ridge);

        let behind = baked.at(512.0, 160.0);
        assert!(
            behind[S] < 80,
            "water just south of the ridge read {} to a northerly",
            behind[S]
        );
        assert_eq!(
            behind[N], EXPOSED,
            "the same water read sheltered from the open sea to its south"
        );

        // The lee fades with distance rather than stopping. Forty metres of
        // ridge at one in twelve stands full cover for a third of a
        // kilometre and is gone by half of one, so these three walk the near
        // edge of the fade, the middle of it, and the water past its end.
        let (held, fading, clear) = (
            baked.at(512.0, 400.0)[S],
            baked.at(512.0, 540.0)[S],
            baked.at(512.0, 640.0)[S],
        );
        assert!(
            held < fading && fading < clear && clear == EXPOSED,
            "the lee does not fade downwind: {held}, then {fading}, then {clear}"
        );
    }

    #[test]
    fn a_lee_runs_downwind_whichever_way_the_wind_blows() {
        // The same wall read from all four cardinals, which is what catches a
        // sweep whose x-major and z-major branches disagree about which way
        // is upwind — an error one bearing alone cannot see.
        let wall = |wx: f32, wz: f32| {
            if (wx - 256.0).abs() < 8.0 && (wz - 256.0).abs() < 8.0 {
                40.0
            } else {
                0.0
            }
        };
        let baked = Shelter::bake(Vec2::ZERO, Vec2::splat(512.0), wall);
        for (slot, at, quarter) in [
            (S, Vec2::new(256.0, 300.0), "south"),
            (N, Vec2::new(256.0, 212.0), "north"),
            (E, Vec2::new(300.0, 256.0), "east"),
            (W, Vec2::new(212.0, 256.0), "west"),
        ] {
            let read = baked.at(at.x, at.y)[slot];
            assert!(
                read < 120,
                "the water {quarter} of the wall read {read} to a wind blowing that way"
            );
        }
    }

    #[test]
    fn high_ground_shelters_further_than_low() {
        // The reach is the obstacle's height over RECOVERY, so doubling the
        // hill roughly doubles the lee. Pinned as an ordering rather than a
        // distance: the constants are meant to be tuned, and a test that
        // fixed the metres would have to be re-recorded every time they were.
        let hill = |height: f32| {
            move |_wx: f32, wz: f32| {
                if (wz - 128.0).abs() < 8.0 {
                    height
                } else {
                    0.0
                }
            }
        };
        let low = Shelter::bake(Vec2::ZERO, Vec2::splat(512.0), hill(10.0));
        let high = Shelter::bake(Vec2::ZERO, Vec2::splat(512.0), hill(40.0));
        let downwind = 300.0;
        assert_eq!(
            low.at(256.0, downwind)[S],
            EXPOSED,
            "a ten-metre hill sheltered water most of a kilometre downwind"
        );
        assert!(
            high.at(256.0, downwind)[S] < EXPOSED,
            "a forty-metre hill sheltered nothing at the same distance"
        );
    }

    #[test]
    fn nothing_is_ever_sheltered_out_of_the_weather_entirely() {
        // A mountain, read right in its lee: the wind is cut hard and is
        // still there — see MOST_COVER, and see `protocol::sheltered` for the
        // floor in metres per second that backs this one up.
        let mountain = |_wx: f32, wz: f32| {
            if (wz - 128.0).abs() < 32.0 {
                400.0
            } else {
                0.0
            }
        };
        let baked = Shelter::bake(Vec2::ZERO, Vec2::splat(512.0), mountain);
        let deepest = baked.at(256.0, 168.0)[S];
        assert!(
            deepest > 0,
            "the deepest lee on the island is a hole in the weather"
        );
        assert!(
            deepest < 60,
            "a mountain's own lee read {deepest}, which is barely sheltered"
        );
    }
}
