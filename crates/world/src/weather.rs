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
//! The wind is a point wandering a 2D noise field, read off as a velocity:
//! its bearing is the point's bearing from the origin and its strength grows
//! from the point's distance out. That one construction buys the behaviour
//! wanted for its own sake — the wind veers smoothly, and every so often the
//! walk passes near the origin, where the strength dies and the bearing
//! swings freely: a calm, out of which the wind returns from somewhere new.
//! Nothing here decides "now a storm"; storms are the far excursions of the
//! same walk.
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

/// Seconds across one cell of the walk's noise, which sets how fast the
/// weather has ideas: with the octaves in [`wind`], the wind's character
/// drifts over a few minutes and turns over entirely in ten or twenty —
/// long enough for a calm or a blow to be a *spell* somebody sails through,
/// short enough that a session sees more than one sky.
const WEATHER_PACE: f32 = 240.0;

/// The hardest the wind blows, in metres per second — a near gale, reached
/// only at the walk's farthest excursions. The shaping in [`wind`] keeps the
/// middle of the range common and both ends occasional.
const MAX_WIND: f32 = 16.0;

/// The wind over the whole world at a moment, as a velocity in metres per
/// second: its length is the wind's strength, its bearing the way the air is
/// moving, and a calm is simply a short vector. `elapsed` is seconds since
/// the world was opened — the server's clock, whose zero is the session's.
pub fn wind(seed: u32, elapsed: f32) -> Vec2 {
    let noise = Noise::new(seed ^ WEATHER_SEED);
    let t = elapsed / WEATHER_PACE;

    // Two independent channels of the same field — far-apart lanes, so the
    // walk's x and y never correlate. Three octaves: the slowest sets the
    // spells, the fastest puts a little restlessness on top.
    let walk = Vec2::new(noise.fbm(t, 7.3, 3), noise.fbm(t, 41.9, 3));

    // Three octaves of fbm keep the walk mostly within half a cell of the
    // origin, so 0.55 out is a far excursion: strength rises with the square
    // of the distance — slow to leave a calm, quick through the top of the
    // range — and is capped where the walk outruns its usual bounds, so the
    // rare wilder wander is a hard blow rather than an impossible one.
    let out = walk.length();
    let strength = MAX_WIND * (out / 0.55).min(1.0) * (out / 0.55).min(1.0);

    if out < 1e-4 {
        // The walk is passing through the origin: a flat calm, with no
        // bearing worth inventing — and no division by nearly nothing.
        return Vec2::ZERO;
    }
    walk * (strength / out)
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
        assert_eq!(got, 0x8E15_4D11_6D79_9FCD);
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
        // A sky that never changes, or never calms, is the failure this
        // module exists to avoid. Over a few hours of one seed the wind must
        // visit real weather — spells below a light air and spells above a
        // fresh breeze — and must not be the same twice in a row when
        // sampled minutes apart.
        let over_a_day: Vec<Vec2> = (0..500).map(|i| wind(7, i as f32 * 60.0)).collect();
        let calmest = over_a_day
            .iter()
            .map(|w| w.length())
            .fold(f32::MAX, f32::min);
        let hardest = over_a_day.iter().map(|w| w.length()).fold(0.0, f32::max);
        assert!(calmest < 2.0, "seed 7's wind never dropped below {calmest}");
        assert!(hardest > 8.0, "seed 7's wind never rose above {hardest}");
        assert_ne!(over_a_day[10], over_a_day[11], "the wind froze");
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
