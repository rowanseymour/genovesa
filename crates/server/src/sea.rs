//! The sea's hand on the hulls nobody is aboard.
//!
//! A boat with somebody at its helm is that client's to move and to report —
//! see [`crate::take_the_helm`] — and one on a ship's painter goes where the
//! ship goes. Every other hull is nobody's to answer for, and used to lie
//! exactly where its last telling left it whatever the water under it: a
//! ship stepped off in the open sea sat there like a mooring buoy. Now the
//! sea moves it. An empty hull afloat drifts on the wind unless it is
//! anchored, an anchored one swings to lie downwind of its hook, and one
//! lying aground stays aground.
//!
//! The server does this rather than a client because it is the one machine
//! that always has the wind and the bed — [`Shared::wind`] and the world's
//! own height — and because where a boat ends up is consequential: it decides
//! whether a player finds their ship where they left it, and nothing
//! consequential is decided at a client. A client near enough to be made the
//! authority would also be a client that can hang up, and a hull whose mover
//! has hung up is exactly the hull this is for. The one client's word that
//! still moves an empty hull is a shove — see [`crate::shove`] — which lands
//! between beats and is taken as read: the next beat carries the hull on
//! from wherever the shove put it.
//!
//! Only the hulls near somebody, on the beasts' terms: a hull nobody is
//! within [`MINDED`] of lies as it was until somebody comes near, and a
//! world with nobody in it moves nothing. What that buys is that the bed is
//! only ever sounded where players are — a hull adrift a hundred kilometres
//! from anyone would otherwise have its island generated every beat and
//! dropped every sweep — and that the tellings go where there is somebody
//! to draw them. What it costs is a hull that stops moving when the last
//! player sails out of reach of it, which nobody is there to see.
//!
//! Told on the wire as ordinary [`protocol::ToClient::Boat`] tellings, way
//! and all, so a client carries a drifting hull forward between beats
//! exactly as it carries another player's: there is no second kind of
//! moving boat for the wire to know about. A hull the sea is not moving —
//! aground, or riding to its anchor in a steady wind — is told nothing.
//!
//! A helmsman who hangs up sleeps aboard, and the world carries them with
//! the hull: a remembered player whose record names a hull the sea moves is
//! moved with it, so that resuming — see [`crate::seat_the_arrival`], which
//! seats a returner only into a boat still lying where they left it — works
//! for a boat the sea moved exactly as for one nothing moved. Somebody else
//! sailing it away is a different matter, and stays one.
//!
//! Nothing here is under the cross-machine promise the generator keeps: there
//! is one server, and the tellings are the agreement.

use std::collections::HashMap;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use glam::Vec2;
use protocol::ground::ANCHOR_SWING;
use protocol::{swing_to, BoatId, Underway, TENDER_ASTERN};

use crate::{aimed, astern, broadcast_all, carry_the_sleepers, Held, Shared};

/// How often the sea moves the empty hulls, and tells of it. A client
/// reckons a telling forward for about half a second before it stops
/// believing it, so a beat well inside that keeps a drifting hull moving
/// smoothly at the far end rather than in steps.
const SEA_TICK: Duration = Duration::from_millis(250);

/// The most a beat may be reckoned as, in seconds. A beat that stalled
/// sounding an island the world had to generate is still one beat, and a
/// hull must not lurch a minute's drift to make up for it.
const LONGEST_BEAT: f32 = 1.0;

/// How near a player a hull has to lie for the sea to mind it, in metres.
///
/// Twice what a client draws around its camera, so a hull drifting at the
/// edge of what anybody can see is moving, and well inside how far from a
/// player the world keeps an island — see `ISLAND_CACHE_RADIUS` — so sounding
/// the bed under a minded hull never grows an island the next sweep drops.
const MINDED: f32 = 2_048.0;

/// What share of the wind's speed a bare hull drifts at.
///
/// A real hull makes a few per cent; this is more, because a drift that takes
/// an hour to be seen is a drift a player would reasonably deny. In a fresh
/// breeze of eight metres a second a hull left adrift makes about half a
/// metre a second — plainly going somewhere within the minute it takes to row
/// ashore and look back — and in the lightest air the sky ever blows it still
/// creeps a few centimetres a second, which over a night is a bay.
const LEEWAY: f32 = 0.06;

/// How fast an anchored hull takes up its cable when the wind shifts, in
/// metres per second: a boat sheering round to a new wind rather than a boat
/// sailing to a new spot.
const SWING_PACE: f32 = 0.5;

/// How fast an empty hull comes round, in radians per second — a drifting
/// hull to lie beam-on to the wind, an anchored one to ride bow to it. Slow:
/// nobody is at the helm.
const COMING_ROUND: f32 = 0.2;

/// The deepest bed a hull lies aground on, in metres below the sea: where the
/// bed is shallower than this the hull is on the bottom, and the sea does not
/// move it.
///
/// One number for both kinds rather than each hull's own draft, and a
/// generous one. A rowing boat rowed onto a beach and stepped out of is
/// left with its keel on the sand in a hand's breadth of water; what it is
/// here is *not adrift*, and a rule sharp enough to float it off in exactly
/// the shallows it was left in would be wrong exactly there. A ship, drawing
/// most of a metre, grounds where its client's own keel probe would have
/// stopped it anyway.
const AGROUND: f32 = 1.0;

/// How close to where it should be lying an anchored hull has to be, in
/// metres, and how close to its bearing any empty hull has to be, in
/// radians, before the sea stops moving it. Under both, the hull is told
/// nothing: a boat riding to its anchor in a steady wind is telling-quiet.
const SETTLED_OFF: f32 = 0.05;
const SETTLED_BEARING: f32 = 0.01;

/// Minds the empty hulls for the life of the session: see the module doc.
pub(crate) fn mind_the_hulls(shared: &Arc<Shared>) {
    let shared = shared.clone();
    thread::spawn(move || {
        let mut last = Instant::now();
        loop {
            if shared.stopping.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            thread::sleep(SEA_TICK);
            let dt = last.elapsed().as_secs_f32().min(LONGEST_BEAT);
            last = Instant::now();
            beat(&shared, dt);
        }
    });
}

/// One beat of the sea over every hull nobody is aboard and nobody tows.
///
/// Three holds of the boats' lock rather than one, and the world sounded
/// between the first two: the bed under a hull may be an island the world has
/// yet to generate, and no lock is held across that — the order every path
/// into the generator keeps. A hull somebody boarded or shoved between the
/// snapshot and the writing back is left as they left it: the sea's move was
/// worked out from a picture that has stopped being true, and the next beat
/// works from the new one.
fn beat(shared: &Shared, dt: f32) {
    let wind = shared.wind();
    // Where everyone is, taken and let go before the boats are looked at:
    // the roster's lock nests over the boats', never under it.
    let near: Vec<Vec2> = {
        let players = shared.players.held();
        players.values().map(|player| player.position).collect()
    };
    if near.is_empty() {
        return;
    }
    let free: Vec<(BoatId, Underway, Option<Vec2>)> = {
        let boats = shared.boats.held();
        boats
            .iter()
            .filter(|(_, state)| state.occupant.is_none() && state.towed_by.is_none())
            .filter(|(_, state)| near.iter().any(|at| at.distance(state.hull.at) <= MINDED))
            .map(|(id, state)| (*id, state.hull, state.anchor))
            .collect()
    };
    let moves: Vec<(BoatId, Underway, Underway)> = free
        .into_iter()
        .filter_map(|(id, hull, anchor)| {
            let to = moved(hull, anchor, wind, dt, |at| shared.world.height(at.x, at.y))?;
            Some((id, hull, to))
        })
        .collect();
    if moves.is_empty() {
        return;
    }

    let mut news = Vec::new();
    let mut carried: Vec<(BoatId, Vec2, Vec2)> = Vec::new();
    {
        let mut boats = shared.boats.held();
        let mut went: HashMap<BoatId, Underway> = HashMap::new();
        for (id, from, to) in moves {
            let Some(state) = boats.get_mut(&id) else {
                continue;
            };
            if state.occupant.is_some() || state.towed_by.is_some() || state.hull != from {
                continue;
            }
            state.hull = to;
            news.push(state.told(id));
            carried.push((id, from.at, to.at));
            went.insert(id, to);
        }
        // The boat on a moved ship's painter goes with it, a painter's
        // length astern of wherever the ship now lies, exactly where a
        // client at that helm would have put it. Told after its ship, as a
        // join tells them — see [`crate::welcome_aboard`].
        for (id, state) in boats.iter_mut() {
            let Some(ship) = state.towed_by.and_then(|ship| went.get(&ship)) else {
                continue;
            };
            if state.occupant.is_none() {
                state.hull = Underway {
                    at: astern(ship.at, ship.heading, TENDER_ASTERN),
                    ..*ship
                };
                news.push(state.told(*id));
            }
        }
    }

    carry_the_sleepers(shared, &carried);

    let players = shared.players.held();
    for word in news {
        broadcast_all(&players, word);
    }
}

/// Where the sea takes one empty hull in `dt` seconds of this wind, over a
/// bed `sounded` gives the height of — or `None` for a hull it leaves lying
/// as it is, which is what makes such a hull telling-quiet.
///
/// The hull is asked about twice at most: where it lies, and where it would
/// go. A hull aground where it lies is not the sea's to move at all, anchor
/// or no anchor. One that would go aground fetches up instead, still afloat
/// and still turning, and lies there until the wind takes it off again.
fn moved(
    hull: Underway,
    anchor: Option<Vec2>,
    wind: Vec2,
    dt: f32,
    sounded: impl Fn(Vec2) -> f32,
) -> Option<Underway> {
    let aground = |at: Vec2| sounded(at) >= -AGROUND;
    if aground(hull.at) {
        return None;
    }
    let downwind = wind.normalize_or_zero();

    let at = match anchor {
        // Riding to the anchor: drawn to the end of its cable downwind of
        // the hook. In a calm the cable hangs slack and the hull drifts
        // back over the hook.
        Some(hook) => {
            let toward = hook + downwind * ANCHOR_SWING - hull.at;
            if toward.length() <= SETTLED_OFF {
                hull.at
            } else {
                hull.at + toward.clamp_length_max(SWING_PACE * dt)
            }
        }
        // Adrift: carried down the wind.
        None => hull.at + wind * LEEWAY * dt,
    };
    // A hull that would go aground fetches up instead — afloat where it
    // was, and still coming round. Sounded only where it would move.
    let at = if at == hull.at || !aground(at) {
        at
    } else {
        hull.at
    };

    // How far the hull has to come round: bow to the wind at anchor, and
    // across it adrift, whichever beam is nearer. The sky never goes slack,
    // but the console can order a flat calm, and a calm names no bearing:
    // the hull keeps the one it has.
    let turn = if downwind == Vec2::ZERO {
        0.0
    } else {
        // The yaw that points a bow into the wind — the client's own
        // convention, see [`aimed`].
        let upwind = aimed(hull.at, hull.at - downwind);
        match anchor {
            Some(_) => swing_to(upwind, hull.heading),
            None => {
                let half = std::f32::consts::FRAC_PI_2;
                let port = swing_to(upwind + half, hull.heading);
                let starboard = swing_to(upwind - half, hull.heading);
                if port.abs() <= starboard.abs() {
                    port
                } else {
                    starboard
                }
            }
        }
    };
    let turned = if turn.abs() > SETTLED_BEARING {
        turn.clamp(-COMING_ROUND * dt, COMING_ROUND * dt)
    } else {
        0.0
    };

    if at == hull.at && turned == 0.0 && hull.way == Vec2::ZERO && hull.swinging == 0.0 {
        return None;
    }
    // The motion the beat made, rather than a difference of endpoints: the
    // heading is kept wrapped, and a difference across the wrap would be a
    // whole turn a second on the wire.
    Some(Underway {
        at,
        heading: (hull.heading + turned).rem_euclid(std::f32::consts::TAU),
        way: (at - hull.at) / dt,
        swinging: turned / dt,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bed flat at the ocean's floor everywhere: open water.
    fn open_sea(_: Vec2) -> f32 {
        -protocol::ground::OCEAN_DEPTH
    }

    /// Where a hull pointed `heading` is pointing, as a unit vector — the
    /// client's own convention, the one [`aimed`] is written in.
    fn ahead(heading: f32) -> Vec2 {
        Vec2::new(-heading.sin(), -heading.cos())
    }

    #[test]
    fn an_empty_hull_drifts_downwind_and_comes_to_lie_across_the_wind() {
        let wind = Vec2::new(8.0, 0.0);
        let mut hull = Underway::lying(Vec2::ZERO, 0.0);
        for _ in 0..40 {
            hull = moved(hull, None, wind, 0.25, open_sea).expect("adrift");
        }
        // Ten seconds of a fresh breeze: carried down the wind at the
        // leeway's share of it, and told the way it is making.
        let expected = wind * LEEWAY * 10.0;
        assert!(
            hull.at.distance(expected) < 1e-3,
            "drifted to {} rather than {expected}",
            hull.at
        );
        assert!(
            hull.way.distance(wind * LEEWAY) < 1e-3,
            "told a way of {} in a wind of {wind}",
            hull.way
        );
        // And lying beam-on to it: the bow has come round from north to
        // point across an easterly wind, whichever way was the shorter.
        let across = ahead(hull.heading).dot(wind.normalize()).abs();
        assert!(
            across < 0.05,
            "the hull lies {across} along the wind rather than across it"
        );
    }

    #[test]
    fn an_anchored_hull_swings_to_lie_downwind_of_its_hook_bow_to_wind() {
        let hook = Vec2::new(100.0, 100.0);
        let wind = Vec2::new(0.0, 6.0);
        // Left where the hook was let go, pointing east: the wind swings it
        // out to the end of its cable and round to face the wind.
        let mut hull = Underway::lying(hook, std::f32::consts::FRAC_PI_2);
        let mut beats = 0;
        while let Some(on) = moved(hull, Some(hook), wind, 0.25, open_sea) {
            hull = on;
            beats += 1;
            assert!(beats < 400, "the hull never settled at its anchor");
        }
        let lie = hook + wind.normalize() * ANCHOR_SWING;
        assert!(
            hull.at.distance(lie) <= SETTLED_OFF,
            "settled at {} rather than {lie}, downwind of the hook",
            hull.at
        );
        let into = ahead(hull.heading).dot(-wind.normalize());
        assert!(
            into > 0.999,
            "the hull rides {into} to the wind rather than bow-on"
        );
        // And, settled, is the sea's to leave alone.
        assert!(moved(hull, Some(hook), wind, 0.25, open_sea).is_none());
    }

    #[test]
    fn an_anchored_hull_never_leaves_its_cable() {
        let hook = Vec2::ZERO;
        let mut hull = Underway::lying(hook, 0.0);
        // A wind that boxes the compass over a minute.
        for beat in 0..240 {
            let angle = beat as f32 * 0.03;
            let wind = Vec2::new(angle.cos(), angle.sin()) * 9.0;
            if let Some(on) = moved(hull, Some(hook), wind, 0.25, open_sea) {
                hull = on;
            }
            assert!(
                hull.at.distance(hook) <= ANCHOR_SWING + 1e-3,
                "beat {beat}: the hull is {} m from a hook on {ANCHOR_SWING} m of cable",
                hull.at.distance(hook)
            );
        }
    }

    #[test]
    fn a_hull_aground_is_not_the_seas_to_move() {
        let beach = |_: Vec2| -0.3;
        let wind = Vec2::new(12.0, 0.0);
        let hull = Underway::lying(Vec2::new(5.0, 5.0), 1.0);
        assert!(
            moved(hull, None, wind, 0.25, beach).is_none(),
            "a beached hull drifted"
        );
        assert!(
            moved(hull, Some(Vec2::ZERO), wind, 0.25, beach).is_none(),
            "a beached hull swung to its anchor"
        );
    }

    #[test]
    fn a_hull_drifting_onto_a_shore_fetches_up_on_it() {
        // The bed shelves up to the east of x = 10: a hull blown that way
        // stops afloat at the edge of the shallows, still coming round,
        // and once it lies across the wind is told nothing more.
        let shelf = |at: Vec2| if at.x > 10.0 { -0.5 } else { -9.0 };
        let wind = Vec2::new(8.0, 0.0);
        let mut hull = Underway::lying(Vec2::new(9.0, 0.0), std::f32::consts::FRAC_PI_2);
        let mut beats = 0;
        while let Some(on) = moved(hull, None, wind, 0.25, shelf) {
            hull = on;
            beats += 1;
            assert!(beats < 400, "the hull never fetched up");
        }
        assert!(
            hull.at.x <= 10.0,
            "the hull was carried into the shallows, to {}",
            hull.at
        );
        assert_eq!(hull.way, Vec2::ZERO, "fetched up and still told a way");
    }

    #[test]
    fn a_hull_lying_still_where_it_should_is_telling_quiet() {
        let wind = Vec2::new(0.0, 6.0);
        let hook = Vec2::ZERO;
        let riding = Underway::lying(
            hook + wind.normalize() * ANCHOR_SWING,
            aimed(Vec2::ZERO, -wind),
        );
        assert!(moved(riding, Some(hook), wind, 0.25, open_sea).is_none());
    }

    #[test]
    fn a_calm_moves_nothing_and_turns_nothing() {
        // The console's flat calm: no bearing to come round to, adrift or
        // at anchor, so a hull lying still is left lying still — and one
        // riding downwind of its hook drifts back over it on a slack cable.
        let calm = Vec2::ZERO;
        let still = Underway::lying(Vec2::new(3.0, 4.0), -0.5);
        assert!(
            moved(still, None, calm, 0.25, open_sea).is_none(),
            "a calm turned a drifting hull"
        );
        let hook = Vec2::ZERO;
        let out = Underway::lying(Vec2::new(0.0, ANCHOR_SWING), 0.0);
        let mut hull = out;
        let mut beats = 0;
        while let Some(on) = moved(hull, Some(hook), calm, 0.25, open_sea) {
            assert_eq!(on.heading, hull.heading, "a calm turned a hull at anchor");
            hull = on;
            beats += 1;
            assert!(beats < 400, "the hull never came back over its hook");
        }
        assert!(hull.at.distance(hook) <= SETTLED_OFF);
    }

    #[test]
    fn a_hull_told_a_negative_yaw_is_never_told_a_whole_turn_of_swing() {
        // A client's yaw comes off atan2 and is as often negative as not;
        // the sea keeps its own headings wrapped, and the swing it tells is
        // the turn it made, never the difference across the wrap.
        let wind = Vec2::new(0.0, -6.0);
        let mut hull = Underway::lying(Vec2::ZERO, -0.5);
        for _ in 0..200 {
            let Some(on) = moved(hull, Some(Vec2::ZERO), wind, 0.25, open_sea) else {
                break;
            };
            assert!(
                on.swinging.abs() <= COMING_ROUND + 1e-5,
                "told a swing of {} rad/s from a heading of {}",
                on.swinging,
                hull.heading
            );
            hull = on;
        }
    }
}
