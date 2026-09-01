//! What the wind does behind land: the island-wide bake behind the shelter
//! lattice [`protocol::ToClient::Chunk`] carries.
//!
//! A headland takes the wind out of the water for some way downwind of
//! itself, and how far is a question about terrain that may be a kilometre
//! upwind of the bay being calmed — so, exactly like [`crate::sunlight`],
//! only whoever holds the whole island can answer it, and the answer
//! travels. The wire's doc says what a client does with the lattice; this
//! module is how a generator arrives at it.
//!
//! **A wake is a shadow cast by a very low sun.** Downwind of an obstacle
//! the sheltered air stands as high as the obstacle and recovers with
//! distance, which is the sweep the sunlight bake runs: walk the raster
//! away from the source carrying a height that each point either ducks
//! under or clears, dropping it a fixed amount per metre. Sunlight's drop is
//! the tangent of the sun's altitude; this one's is [`RECOVERY`]. So the
//! walk is [`crate::raster::sweep`], shared, given the quarter the wind
//! blows *from* as its source. What comes back is not a shadow but a depth
//! of shelter — how far the wake stands above the water — which [`cover`]
//! turns into the fraction of the wind that survives.
//!
//! The sweep runs once per compass point of [`protocol::ground::BEARINGS`]
//! and the results are stored per point rather than resolved against a wind
//! here, because the wind veers and a chunk is handed out once. Resolving
//! here would mean re-sending every chunk in the world each time the weather
//! turned, or a sky that could not turn.
//!
//! **The lee dies inside the island's own box.** An island answers for the
//! wind only where it answers for the ground — its frame and one skirt
//! chunk — and past that line nobody does: the next chunk is open sea to
//! every client, or the edge of a neighbour that has never heard of this
//! island's hills. A wake still standing at that line would step to nothing
//! across it, which is exactly the seam the lattice's shared corners exist
//! to rule out. So the cover is walked back to nothing across the skirt —
//! see [`Shelter::bake`]'s `fade` — and the outermost ring of every island's
//! lattice reads open, agreeing with whatever lies beyond it. What that
//! costs is the far end of a long lee: a tall headland's wake would run
//! further than its island's box, and here it runs to the box. The near end,
//! where a bay actually is, is untouched.
//!
//! This bake reads terrain four times coarser than the sunlight bake does,
//! decimated from the same raster rather than sampled again — see
//! [`STEP_METRES`] for why coarser is right here and wrong there.

use protocol::ground::{bearings, Exposure, BEARINGS, CELL_METRES, EXPOSED, LEAST_EXPOSURE};

use crate::raster::{sweep, Grid, Raster};

/// Metres between the raster points this bake sweeps — four times
/// [`CELL_METRES`], and deliberately not the wire's own grid the way
/// [`crate::sunlight`]'s raster is.
///
/// That module went the other way for a reason that does not carry: a shadow
/// has a hard edge, so a two-metre crag falling between samples loses a
/// terminator that stands tens of metres from where it should. A wake has no
/// edge. Air does not pour off the top of a crag and resume a hundred metres
/// later; it closes back in behind anything small, and what shelters a bay is
/// a headland or a ridge, measured in hundreds of metres. Sweeping sixteen
/// times the points would be paying for crags that shelter nothing.
const STEP_METRES: f32 = 4.0 * CELL_METRES;

/// How fast a wake recovers with distance downwind, as a slope: the sheltered
/// air loses this much height per metre travelled, so an obstacle standing
/// `h` above the water shelters for roughly `h / RECOVERY` metres behind
/// itself before the wind has closed back in.
///
/// One in twelve, which puts a forty-metre headland's cover about five
/// hundred metres downwind — the scale of a bay rather than of a whole
/// island's lee. Real air recovers faster over water than this and much
/// slower in a valley; a single slope is the simplification the whole model
/// rests on, and the thing to reach for first if a lee ever feels the wrong
/// size. Past an island's own box the lee is faded out regardless — see the
/// module header — so a longer reach buys a bay more depth, not more length.
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

/// One island's baked shelter: for every raster point, how much of the wind
/// survives from each of [`BEARINGS`].
pub struct Shelter {
    grid: Grid,
    /// The exposures, row-major — see [`Exposure`].
    exposure: Vec<Exposure>,
}

/// What a wake standing `above` metres over the water leaves of the wind, as
/// the wire's byte — [`EXPOSED`] where nothing stands over it at all, and
/// never below [`LEAST_EXPOSURE`]. `window` is how much of the cover to
/// keep, `1.0` inside the island and falling to nothing at its box's edge.
///
/// The window scales the *cover* rather than the height, so a saturated lee
/// fades linearly across the whole band rather than holding full depth until
/// the last few metres of it.
fn cover(above: f32, window: f32) -> u8 {
    let deep = (above / FULL_COVER).clamp(0.0, 1.0) * window;
    let surviving = 1.0 - (1.0 - LEAST_EXPOSURE) * deep;
    (surviving * EXPOSED as f32).round() as u8
}

impl Shelter {
    /// Bakes the shelter over the surface the wind crosses — the same raster
    /// [`crate::sunlight`] is given, read at every [`STEP_METRES`]. `fade` is
    /// the width of the band inside the raster's edge over which the cover
    /// is walked back to nothing, for the reason the module header gives:
    /// the skirt's width, so the lattice's outermost ring agrees with the
    /// open sea past it.
    pub fn bake(ground: &Raster, fade: f32) -> Self {
        let by = (STEP_METRES / ground.grid.step).round() as usize;
        debug_assert!(by >= 1 && (ground.grid.columns - 1).is_multiple_of(by));
        let ground = ground.decimated(by);
        let grid = ground.grid;
        let points = grid.points();

        // A sweep per bearing, each writing only its own slot of every
        // point's exposures — one thread apiece, nothing to merge and nothing
        // to order, since a bearing's answer depends on the immutable raster
        // and on nothing another bearing did.
        let mut exposure = vec![[EXPOSED; BEARINGS]; points];
        std::thread::scope(|scope| {
            let mut swept = Vec::new();
            for (slot, toward) in bearings().into_iter().enumerate() {
                let ground = &ground;
                swept.push(scope.spawn(move || {
                    let mut wake = vec![0.0f32; points];
                    // The source of a wake is the quarter the wind blows
                    // *from*, which for a velocity is the way it is not going.
                    sweep(&mut wake, ground, -toward, RECOVERY);
                    let covers: Vec<u8> = (0..points)
                        .map(|at| {
                            let (ix, iz) = (at % grid.columns, at / grid.columns);
                            let window = (grid.inset(ix, iz) / fade).clamp(0.0, 1.0);
                            cover(wake[at] - ground.values[at], window)
                        })
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

        Self { grid, exposure }
    }

    /// How exposed a world point is to each bearing — or wide open past the
    /// raster, where there is nothing but sea for the wind to cross. The
    /// nearest raster point's own exposures, for the reason [`Grid::nearest`]
    /// gives; whoever needs an answer *between* lattice points blends there —
    /// [`protocol::ground::shelter_across`].
    pub fn at(&self, wx: f32, wz: f32) -> Exposure {
        self.grid
            .nearest(wx, wz)
            .map_or([EXPOSED; BEARINGS], |at| self.exposure[at])
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec2;

    use super::*;

    /// The slot a compass point is stored in, for tests that want to name one.
    const N: usize = 0;
    const E: usize = 2;
    const S: usize = 4;
    const W: usize = 6;

    /// A bake over a surface, on the bake's own step and with no fade at the
    /// edge — a fixture wall's lee is the thing under test, not the box's.
    fn baked(extent: f32, surface: impl Fn(f32, f32) -> f32 + Sync) -> Shelter {
        Shelter::bake(
            &Raster::sample(Vec2::ZERO, Vec2::splat(extent), STEP_METRES, surface, 2),
            0.0,
        )
    }

    #[test]
    fn open_water_is_exposed_from_every_quarter() {
        let flat = baked(256.0, |_, _| 0.0);
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
        // would catch the sweep given the wrong end of the wind as its source.
        let ridge = |_wx: f32, wz: f32| if (wz - 128.0).abs() < 8.0 { 40.0 } else { 0.0 };
        let baked = baked(1024.0, ridge);

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
        let baked = baked(512.0, wall);
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
        let low = baked(512.0, hill(10.0));
        let high = baked(512.0, hill(40.0));
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
        // still there — the wire's floor, LEAST_EXPOSURE, and see
        // `protocol::sheltered` for the floor in metres per second behind it.
        let mountain = |_wx: f32, wz: f32| {
            if (wz - 128.0).abs() < 32.0 {
                400.0
            } else {
                0.0
            }
        };
        let baked = baked(512.0, mountain);
        let deepest = baked.at(256.0, 168.0)[S];
        assert_eq!(
            deepest,
            (LEAST_EXPOSURE * 255.0).round() as u8,
            "the deepest lee on the island is not the wire's floor"
        );
    }

    #[test]
    fn the_lee_is_walked_back_to_nothing_at_the_edge_of_the_box() {
        // A wall right against the raster's northern edge, its lee running
        // south across the whole raster and out of the box. With a fade the
        // outermost ring reads open — what lies past the box is open sea to
        // everyone, and this is what stops the wind stepping across the line
        // — while the water a fade's width inside is as sheltered as ever.
        let wall = |_wx: f32, wz: f32| if wz < 8.0 { 60.0 } else { 0.0 };
        let fade = 128.0;
        let ground = Raster::sample(Vec2::ZERO, Vec2::splat(512.0), STEP_METRES, wall, 2);
        let faded = Shelter::bake(&ground, fade);
        let sharp = Shelter::bake(&ground, 0.0);

        // Without the fade the lee reaches the southern edge at full depth,
        // which is the seam the fade exists to remove.
        assert!(
            sharp.at(256.0, 512.0)[S] < 80,
            "a sixty-metre wall's lee should still be deep 512 m on"
        );
        for wx in [0.0, 128.0, 256.0, 384.0, 512.0] {
            assert_eq!(
                faded.at(wx, 512.0),
                [EXPOSED; BEARINGS],
                "the southern edge at x = {wx} is not open"
            );
            assert_eq!(faded.at(wx, 0.0), [EXPOSED; BEARINGS], "the northern edge");
        }
        for wz in [0.0, 256.0, 512.0] {
            assert_eq!(faded.at(0.0, wz), [EXPOSED; BEARINGS], "the western edge");
            assert_eq!(faded.at(512.0, wz), [EXPOSED; BEARINGS], "the eastern edge");
        }
        // Inside the band the fade eases in rather than switching: a point
        // half a fade in sits between the edge and the interior.
        let (edge, half, inside) = (
            faded.at(256.0, 512.0)[S],
            faded.at(256.0, 512.0 - fade / 2.0)[S],
            faded.at(256.0, 512.0 - fade)[S],
        );
        assert!(
            inside < half && half < edge,
            "the fade does not ease: {inside} inside, {half} halfway, {edge} at the edge"
        );
        assert_eq!(
            inside,
            sharp.at(256.0, 512.0 - fade)[S],
            "the fade reached water a whole band inside the edge"
        );
    }
}
