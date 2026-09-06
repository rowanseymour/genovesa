//! The weather over a world: what the wind is doing, when.
//!
//! One function, and deliberately a *pure* one: [`wind`] maps a seed and a
//! moment to a wind, and holds no state between calls. That is what lets
//! weather survive everything the terrain survives — a server asked twice
//! gives the same answer, a re-hosted world under the same elapsed clock
//! blows the same gale, and there is nothing to persist, replay or get out
//! of sync. The trade is that weather follows the *server's* clock rather
//! than any world-wide calendar: rehost a world after an hour down and its
//! weather starts from the beginning, which for weather — unlike ground —
//! reads as weather.
//!
//! The wind is two points wandering 2D noise fields, at two paces. The
//! strength is one point's distance from the origin, read on a walk brisk
//! enough that a session sees calms and blows come and go; the bearing is the
//! *other* point's bearing from the origin, on a walk slow enough that a
//! passage holds its wind — see [`BEARING_PACE`] for the figure and the test
//! that holds it. A calm is a spell of light air, never a dead sky — see
//! [`MIN_WIND`]. Nothing here decides "now a storm"; storms are the far
//! excursions of the strength's walk.
//!
//! They used to be one point, the bearing and the strength read off the same
//! walk. That coupled them the wrong way round for a sailor: the walk sits
//! mostly near the origin, where a small step is a large swing, so the bearing
//! turned over in about three minutes — two islets' worth of sailing — and one
//! crossing in four ended with its destination in the no-go zone. Reading the
//! bearing from a walk of its own is what lets a fresh breeze hold its
//! direction while the strength stays restless. What it gives up is the old
//! design's one nicety, a calm out of which the wind came back from somewhere
//! new: now a lull may lift with the wind where it was.
//!
//! No trigonometry anywhere, and that is a constraint rather than a style:
//! `sin` and `cos` are not correctly-rounded and drift between platforms,
//! and this file is under the same cross-machine promise as the terrain —
//! see `the_weather_is_part_of_the_seed`. Additions, multiplications and
//! square roots are exact, and the bearing is born as a vector so no angle
//! is ever taken.

use glam::Vec2;

use crate::noise::Noise;

/// Stirred into the seed so the weather's noise is not the terrain's: the
/// same permutation serving both would tie the sky to the ground in ways no
/// one intended.
const WEATHER_SEED: u32 = 0x57EA_7E12;

/// Seconds across one cell of the strength's walk, which sets how fast the
/// weather has ideas: with the octaves in [`wind`], the strength drifts over
/// a few minutes and turns over entirely in ten or twenty — long enough for
/// a calm or a blow to be a *spell* somebody sails through, short enough
/// that a session sees more than one sky.
const WEATHER_PACE: f32 = 240.0;

/// Seconds across one cell of the bearing's walk — a single octave, so the
/// figure is the whole of the wind's rate of veer. Tuned to a target rather
/// than a feel: a boat that sets off on a beam reach for the next island
/// must still fetch it, four times in five, over the five minutes a big
/// island's neighbour is away — which `a_passage_holds_its_wind` holds it
/// to. Measured over nine seeds this pace fails one crossing in eight, a
/// pace of 960 one in four, and the strength's own pace one in four at any
/// window at all. Over twenty minutes the bearing is as good as new either
/// way: the walk spends most of its time near the origin, where one cell's
/// travel is a full turn.
const BEARING_PACE: f32 = 1_440.0;

/// The hardest the wind blows, in metres per second — a near gale, reached
/// only at the walk's farthest excursions. The shaping in [`wind`] keeps the
/// middle of the range common and both ends occasional.
///
/// A ceiling clients calibrate hulls against, having no other statement of how
/// hard the weather can get. Lowering it does not break anything here, and can
/// quietly put a heading or a hull out of a client's reach — worth a look over
/// the other side before it moves.
const MAX_WIND: f32 = 16.0;

/// The lightest the sky ever blows — [`protocol::LIGHT_AIR`], which is where
/// the number lives now that a lee has to honour it too.
///
/// The walk used to run strengths straight down to zero, and judged across
/// seeds the sky spent about a minute in seven below half a metre a second,
/// mostly in spells of half a minute, occasionally in ones several minutes
/// long — long enough to be a spell somebody is stuck inside rather than a
/// lull they coast through. The client's answer then was a floor that drove
/// the boat on any heading, eye of the wind included, which read as a boat
/// ignoring the weather; killing the dead sky here is what let that hatch
/// close. The strength is *rescaled* into `MIN..MAX` rather than clamped, so
/// a lull still breathes instead of sitting pinned at the floor.
const MIN_WIND: f32 = protocol::LIGHT_AIR;

/// The wind over the whole world at a moment, as a velocity in metres per
/// second: its length is the wind's strength, its bearing the way the air is
/// moving, and a calm is simply a short vector. `elapsed` is seconds since
/// the world was opened — the server's clock, whose zero is the session's.
pub fn wind(seed: u32, elapsed: f32) -> Vec2 {
    let noise = Noise::new(seed ^ WEATHER_SEED);

    // Two independent channels of the same field — far-apart lanes, so the
    // walk's x and y never correlate. Three octaves: the slowest sets the
    // spells, the fastest puts a little restlessness on top.
    let t = elapsed / WEATHER_PACE;
    let walk = Vec2::new(noise.fbm(t, 7.3, 3), noise.fbm(t, 41.9, 3));

    // Three octaves of fbm keep the walk mostly within half a cell of the
    // origin, so 0.55 out is a far excursion: strength rises with the square
    // of the distance — slow to leave a lull, quick through the top of the
    // range — and is capped where the walk outruns its usual bounds, so the
    // rare wilder wander is a hard blow rather than an impossible one.
    let reach = (walk.length() / 0.55).min(1.0);
    let strength = MIN_WIND + (MAX_WIND - MIN_WIND) * reach * reach;

    // The bearing's own walk, on lanes of its own and one octave: any
    // restlessness laid on top would be laid on the bearing, which is the
    // one thing this walk exists to keep still.
    let b = elapsed / BEARING_PACE;
    let heading = Vec2::new(noise.fbm(b, 83.7, 1), noise.fbm(b, 127.1, 1));

    let out = heading.length();
    if out == 0.0 {
        // The walk is standing exactly on the origin, where it has no bearing
        // to read and nothing at all to divide by. Only the exact zero needs
        // saying: `length` is a square root, so it comes back either zero or
        // a good deal larger than the smallest float — there is no denormal
        // `out` for `strength / out` to overflow through, and every walk that
        // rounds to a positive length still points somewhere honest. Guarding
        // a whole neighbourhood instead would snap the bearing due south
        // across a boundary the walk crosses far more often than it lands
        // on. A moment of measure zero: hand the strength an arbitrary fixed
        // bearing and let the client's easing swallow it.
        return Vec2::new(0.0, -strength);
    }
    heading * (strength / out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{digest, floats};

    #[test]
    fn the_weather_is_part_of_the_seed() {
        // The same promise the terrain digests make, for the sky: hand this
        // seed to any machine and the wind at these moments must come out
        // bit for bit the same, or a shared world's players are sailing
        // under different weather. Re-record with `--nocapture` if the
        // *generator* changed on purpose; a red run that didn't touch it
        // means a platform has stopped agreeing, which is a bug.
        let samples = (0..48).flat_map(|i| {
            let wind = wind(20040112, i as f32 * 97.0);
            [wind.x, wind.y]
        });
        let got = digest(floats(samples));
        println!("weather digests to {got:#018X}");
        assert_eq!(got, 0x9C1C_70DC_72B4_233E);
    }

    #[test]
    fn the_wind_stays_inside_the_gale() {
        // The strength shaping is a cap, not a hope: nothing the walk does
        // may put more than MAX_WIND on the water, at any seed or hour.
        for seed in [0, 7, 20040112, u32::MAX] {
            for i in 0..2_000 {
                let wind = wind(seed, i as f32 * 13.7);
                assert!(
                    wind.length() <= MAX_WIND + 1e-3,
                    "seed {seed} blows {} m/s at t={}",
                    wind.length(),
                    i as f32 * 13.7
                );
            }
        }
    }

    #[test]
    fn the_wind_moves_and_rests() {
        // A sky that never changes, or never lulls, is the failure this
        // module exists to avoid. Over a few hours of one seed the wind must
        // visit real weather — spells down near the light-air floor and
        // spells above a fresh breeze — and must not be the same twice in a
        // row when sampled minutes apart.
        let over_a_day: Vec<Vec2> = (0..500).map(|i| wind(7, i as f32 * 60.0)).collect();
        let calmest = over_a_day
            .iter()
            .map(|w| w.length())
            .fold(f32::MAX, f32::min);
        let hardest = over_a_day.iter().map(|w| w.length()).fold(0.0, f32::max);
        assert!(
            calmest < MIN_WIND + 0.5,
            "seed 7's wind never dropped below {calmest}"
        );
        assert!(hardest > 8.0, "seed 7's wind never rose above {hardest}");
        assert_ne!(over_a_day[10], over_a_day[11], "the wind froze");
    }

    #[test]
    fn the_sky_never_goes_slack() {
        // The guarantee the client sails on: the weather always names a wind
        // of at least a light air, at any seed or hour, so a boat that only
        // moves under canvas is never parked by the sky. The other half of
        // [`the_wind_stays_inside_the_gale`]'s promise.
        for seed in [0, 7, 20040112, u32::MAX] {
            for i in 0..2_000 {
                let wind = wind(seed, i as f32 * 13.7);
                assert!(
                    wind.length() >= MIN_WIND - 1e-3,
                    "seed {seed} slackens to {} m/s at t={}",
                    wind.length(),
                    i as f32 * 13.7
                );
            }
        }
    }

    #[test]
    fn a_passage_holds_its_wind() {
        // What BEARING_PACE is tuned to, held so it cannot drift back: a boat
        // that sets off on a beam reach — the first point of sail to earn full
        // drive, a right angle off the wind — and sails for five minutes must
        // find, four times in five, that the wind has not swung her course
        // into the no-go zone. Every hop between islets is shorter than this
        // and a big island's nearest neighbour is about this far, so it is the
        // crossing a player commits to. No trigonometry: the beam-reach course
        // is the wind's perpendicular, and "inside the no-go zone" is the
        // course's dot with where the air comes from exceeding cos 45°.
        let crossing = 300.0;
        let (mut foul, mut total) = (0, 0);
        for seed in [1, 7, 42, 20040112] {
            for i in 0..1_500 {
                let at = i as f32 * 10.0;
                let set_off = wind(seed, at);
                let arrived = wind(seed, at + crossing);
                let course = Vec2::new(-set_off.y, set_off.x).normalize();
                let eye = -arrived.normalize();
                total += 1;
                if course.dot(eye) > std::f32::consts::FRAC_1_SQRT_2 {
                    foul += 1;
                }
            }
        }
        let percent = 100.0 * foul as f32 / total as f32;
        assert!(
            percent < 20.0,
            "the wind fouled {percent:.0}% of five-minute beam reaches"
        );
    }

    #[test]
    fn seeds_get_their_own_sky() {
        // Two seeds under one clock must not share a forecast, or every
        // world would be having the same day.
        let (a, b): (Vec<Vec2>, Vec<Vec2>) = (0..20)
            .map(|i| (wind(1, i as f32 * 300.0), wind(2, i as f32 * 300.0)))
            .unzip();
        assert_ne!(a, b);
    }
}
