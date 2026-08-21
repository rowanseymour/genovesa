//! The server's half of the debug console: what a [`ToServer::Command`]
//! line means here, and the [`ToClient::Reply`] it earns.
//!
//! The vocabulary lives on this side of the wire on purpose. A client
//! forwards whatever was typed, verbatim, and draws whatever text comes back
//! — so a command added here reaches every client ever written, and `help`
//! is the one piece of documentation a player needs. The protocol crate says
//! the same thing from the other side.
//!
//! Everything here changes the *world*, for everyone in it — the game's own
//! console keeps purely local switches (what one machine draws) on its side
//! of the wire, under a `set` prefix this parser will never see. Anyone in a
//! session may command; [`interpret`] is told who asked so that a later gate
//! has somewhere to stand, and the host's log names the asker either way.

use std::collections::HashMap;

use glam::Vec2;
use protocol::{clock, BeastKind, BoatId, BoatKind, PlayerId, ToClient, Token};
use world::archipelago::{Archipelago, SOUNDING, SPAWN_OFFSHORE};

use crate::{
    aimed, beasts, broadcast, broadcast_all, keeper, post, reachable, BoatState, Held, Shared,
};

/// What `help` says. One line per command, in the imperative the commands
/// themselves are written in.
const HELP: &str = "goto <x> <z> — be taken to a place, in whatever can be there\n\
                    spawn shark|dolphins|whale [count] — raise beasts in your waters\n\
                    time <hh:mm> — run the world's clock forward to that hour\n\
                    weather calm|breeze|gale|natural — order the wind, or give it back";

/// The first word of every line [`interpret`] serves — taught to each client
/// on joining as [`ToClient::Vocabulary`], so a console can complete them as
/// a player types. The grammar's index, not the grammar: `interpret` never
/// reads this, and a test holds the two to agreement.
pub(crate) const VERBS: [&str; 5] = ["goto", "help", "spawn", "time", "weather"];

/// The winds the console can order up. The strengths are the sea's landmarks
/// rather than round numbers: a flat calm, the reference breeze the wave
/// amplitudes are written for, and the hardest gale an honest sky can blow.
/// The bearings swing wide between neighbours on purpose, so ordering one
/// after another marches every wave train through its re-aim dance — half of
/// what the command exists to watch, the other half being how the sea wears
/// each strength without waiting for the real sky to happen to visit it.
const WINDS: [(&str, Vec2); 3] = [
    ("calm", Vec2::ZERO),
    ("breeze", Vec2::new(-4.95, -4.95)),
    ("gale", Vec2::new(11.31, -11.31)),
];

/// What serving a line came to: the text the asker is owed, and — for the one
/// command that moves them — where the world has put them down.
///
/// The second is not the console's to act on, and that is why it is handed
/// back rather than dealt with here. A player set down somewhere else has
/// *arrived*, and arriving is the survey's business: the connection that
/// took the line reopens its wake there, exactly as the door does for
/// somebody joining. Without that the chart spends the next kilometre paying
/// off a voyage nobody made — see [`crate::Wake`], which is a connection
/// thread's own and which nothing in this module can reach.
pub(crate) struct Served {
    pub(crate) reply: String,
    pub(crate) put: Option<Vec2>,
}

impl From<String> for Served {
    /// Every command but one moves nobody, and says so by saying nothing.
    fn from(reply: String) -> Self {
        Self { reply, put: None }
    }
}

/// Serves one console line and says what came of it — every line gets an
/// answer, the ones nothing here recognises included, because silence at a
/// console reads as a hang.
pub(crate) fn interpret(shared: &Shared, from: PlayerId, line: &str) -> Served {
    let words: Vec<&str> = line.split_whitespace().collect();
    match words.as_slice() {
        [] | ["help"] => HELP.to_string().into(),
        ["goto", rest @ ..] => goto(shared, from, rest),
        ["spawn", rest @ ..] => spawn(shared, from, rest).into(),
        ["time", rest @ ..] => time(shared, rest).into(),
        ["weather", rest @ ..] => weather(shared, rest).into(),
        [verb, ..] => format!("`{verb}` is not a command here — `help` lists what is").into(),
    }
}

/// How many bearings [`standing_off`] tries at each radius as it looks
/// outward for water. Sixteen is a point of the compass every 22°, which
/// finds the sea from anywhere on the islands the generator draws — and
/// finding *the* nearest water is not the promise anyway: what a hull wants
/// is somewhere it can lie off the shore that was asked for, and any water
/// within a few strides of the nearest is that.
const BEARINGS: usize = 16;

/// `goto <x> <z>`: the asker taken to a point of the world, in whatever can
/// be there.
///
/// Nobody in this world is anywhere on their own — they are aboard a hull or
/// on their own feet, and a point that suits one of those suits the other
/// badly. So the ground under the point decides, and the answer says which
/// it was:
///
/// - Dry land, on foot: they stand on it. Whatever boats they have stay
///   where they were, because walking away from a boat is already how a
///   player parts from one, and this is a long walk.
/// - Dry land, at a helm: the hull cannot be there, so it lies off the
///   nearest shore to it — [`standing_off`], which is the offing a world is
///   entered on. Their crew keeps the helm and sails in, exactly as an
///   arrival does: the hull is set down at rest and facing the shore, on the
///   put down's own terms — see [`ToClient::PutDown`], where why a jump
///   never arrives under way is written out. A hull put down on a hillside
///   would be the one thing this command could do that the game itself
///   never does.
/// - Water, at a helm: the hull goes, and its crew with it, and lies where
///   it is put until somebody sets sail again.
/// - Water, on foot: a hull is put under them and they take its helm — their
///   own if one is lying free, minted if not. There is no swimming in this
///   world, so a walker set down at sea would be standing on the water.
///
/// Which leaves nothing to refuse but a point the world does not have. That
/// is deliberate, this being a debugging command: what it is for is being
/// taken to a place to look at it, and *no, not like that* is what sailing
/// there would have said.
fn goto(shared: &Shared, from: PlayerId, args: &[&str]) -> Served {
    let Some(asked) = point(args) else {
        return "`goto` wants a place to be taken to — `goto 480 -1200`"
            .to_string()
            .into();
    };
    if !reachable(asked) {
        return format!("`{} {}` is outside the world", asked.x, asked.y).into();
    }

    // Whether the point is land is the whole of what the placement turns on,
    // and asking costs the island under it being generated if it never has
    // been — the tens to hundreds of milliseconds this command is worth, and
    // paid before any lock is taken. The offing goes with it for the same
    // reason and not because it is always wanted: a walker sent to land
    // reads nothing off `berth` but its being `Some`, and the sixteen rays
    // are thrown away. Walking them under the roster's lock to save that
    // would hold every other connection out of the world while it happened,
    // which is a worse thing to spend than a walk over heights already in
    // hand.
    let dry = shared.world.height(asked.x, asked.y) >= 0.0;
    let berth = dry.then(|| standing_off(&shared.world, asked));

    let mut players = shared.players.held();
    let Some(player) = players.get_mut(&from) else {
        // Unreachable from a served connection, whose player is on the roster
        // for as long as it can speak — the same nobody-by-that-id case
        // `spawn` answers, and it needs words here too.
        return "you are nowhere the world could take you from"
            .to_string()
            .into();
    };
    let (token, aboard) = (player.token, player.aboard);

    let (at, heading, told, said) = {
        let mut boats = shared.boats.held();
        match (aboard, berth) {
            // At a helm: the hull goes, and lies off the shore when the
            // shore is what was asked for. Facing it, in that case — a hull
            // anchored stern-on to the island it was brought to see is no
            // use to whoever asked for it.
            (Some(boat), berth) => {
                let at = berth.map_or(asked, |(off, _)| off);
                let state = boats.get_mut(&boat).expect("a boat once boarded exists");
                state.position = at;
                if berth.is_some() {
                    state.heading = aimed(at, asked);
                }
                let heading = state.heading;
                (at, Some(heading), Some((boat, state.told(boat))), None)
            }
            // Afoot, and the point is ground to stand on: nothing but the
            // walker moves, and which way they face is their own business —
            // see [`ToClient::PutDown`], whose `None` heading this is.
            (None, Some(_)) => (asked, None, None, None),
            // Afoot at sea: a hull under them, and the helm of it.
            (None, None) => {
                let (boat, minted) = a_hull_for(&mut boats, token, asked);
                let state = boats.get_mut(&boat).expect("dealt a breath ago");
                state.occupant = Some(from);
                (
                    asked,
                    Some(state.heading),
                    Some((boat, state.told(boat))),
                    Some(if minted {
                        "a ship is here for you"
                    } else {
                        "your ship is here"
                    }),
                )
            }
        }
    };
    // Whether this jump seated somebody who was on their own feet, which is
    // what decides both halves of what happens next.
    let seating = aboard.is_none() && told.is_some();
    if let Some((boat, _)) = told.as_ref().filter(|_| seating) {
        // The roster's half of it, which is only ever written with the
        // boat's — see [`crate::Player::aboard`].
        player.aboard = Some(*boat);
    }
    player.position = at;

    match told {
        // A seating goes to the whole roster, the asker included, exactly as
        // a boarding grant does: theirs is the client it seats.
        Some((_, telling)) if seating => broadcast_all(&players, telling),
        // Any other word about a hull is news to everyone but the client
        // steering it, which is the authority on its own hull and is told
        // where it stands by the put down instead.
        Some((_, telling)) => broadcast(&players, from, telling),
        None => broadcast(
            &players,
            from,
            ToClient::Moved {
                id: from,
                position: at,
            },
        ),
    }
    if let Some(player) = players.get(&from) {
        post(
            player,
            ToClient::PutDown {
                position: at,
                heading,
            },
        );
    }
    drop(players);

    let reply = match (said, berth) {
        (Some(said), _) => format!("{said}, at {} {}", round(at.x), round(at.y)),
        // Measured to the shore rather than to the point that was asked
        // for: what a player wants to know from a deck is how far off the
        // beach they are lying, and the place they named is somewhere
        // inland of it. The coordinates are the berth either way — where
        // they are is where they are.
        (None, Some((_, shore))) if aboard.is_some() => format!(
            "the shore is {} m off the bow, at {} {}",
            round(at.distance(shore)),
            round(at.x),
            round(at.y)
        ),
        _ => format!("you are at {} {}", round(at.x), round(at.y)),
    };
    Served {
        reply,
        put: Some(at),
    }
}

/// A place, as two numbers: `goto 480 -1200`.
fn point(args: &[&str]) -> Option<Vec2> {
    match args {
        [x, z] => Some(Vec2::new(x.parse().ok()?, z.parse().ok()?)),
        _ => None,
    }
}

/// Where a hull lies for somebody who asked to be taken to dry land: off the
/// nearest shore to the point they named, on the offing a world is entered
/// on — see `world::archipelago::Archipelago::spawn`, whose walk this is,
/// worked outward from a point on the land instead of inward from the sea.
///
/// Sounded on [`BEARINGS`] rays at widening radii, and the first water any
/// of them finds is the shore taken. Bounded by the island's own frame,
/// which is both an honest limit — land never stands outside it, so water is
/// always within a frame's diagonal of anywhere on the island — and what
/// keeps the sampling off the neighbours: a height asked inside another
/// island's frame generates that island, and this command has already paid
/// for one.
///
/// Answered as the berth and the shore it stands off, the second being what
/// the reply is written in: a player at a deck wants to know how far off the
/// beach they are lying, not how far from the point they typed.
///
/// A point the walk finds no water for at all leaves the hull where it was
/// asked to be, aground on the ground it asked for. That is the least bad of
/// the answers available: the client sails out of it, every way down to the
/// sea being downhill, which is the same thing the door relies on when it
/// mints a hull under a player who logged off inland.
fn standing_off(world: &Archipelago, asked: Vec2) -> (Vec2, Vec2) {
    let bound = world
        .island_at(asked.x, asked.y)
        .map_or(SPAWN_OFFSHORE, |spec| spec.extent().length());
    let steps = (bound / SOUNDING) as usize;

    let bearings: Vec<Vec2> = (0..BEARINGS)
        .map(|ray| {
            let bearing = ray as f32 / BEARINGS as f32 * std::f32::consts::TAU;
            Vec2::new(bearing.cos(), bearing.sin())
        })
        .collect();

    for step in 1..=steps {
        for out in &bearings {
            let shore = asked + *out * (step as f32 * SOUNDING);
            if world.height(shore.x, shore.y) >= 0.0 {
                continue;
            }
            // Water: stand off it, and further out still if the offing is
            // somehow dry — a spit beside the ray, an islet just past the
            // beach. Bounded, because a ray long enough leaves this island
            // and the next thing it finds is another one's business.
            let mut off = shore + *out * SPAWN_OFFSHORE;
            for _ in 0..steps {
                if world.height(off.x, off.y) < 0.0 {
                    return (off, shore);
                }
                off += *out * SOUNDING;
            }
            return (shore, shore);
        }
    }
    (asked, asked)
}

/// The hull a walker at sea is put aboard: one of their own lying free
/// anywhere in the world, brought to them, or a new one where they stand.
///
/// Theirs first for the reason the door prefers a spare to a mint — see
/// [`crate::BoatState::keeper`] and `fresh_hull` in the join path. An evening
/// of `goto` would otherwise leave a sloop adrift at every place its asker
/// had stood, each one filed in the world and posted to every future joiner.
/// Unlike the door's, this search has no berth about it: the point of the
/// command is being taken somewhere, so the hull comes to the player rather
/// than the player being handed one that happens to be near.
///
/// A sloop either way. A dinghy somebody beached is left on its beach, where
/// they will want it, and an arrival's story starts at a ship's helm.
fn a_hull_for(boats: &mut HashMap<BoatId, BoatState>, token: Token, at: Vec2) -> (BoatId, bool) {
    let theirs = boats
        .iter()
        .find(|(_, boat)| {
            boat.kind == BoatKind::Sloop && boat.occupant.is_none() && boat.keeper == Some(token)
        })
        .map(|(&boat, _)| boat);
    if let Some(boat) = theirs {
        let state = boats.get_mut(&boat).expect("looked up a breath ago");
        state.position = at;
        return (boat, false);
    }

    let boat = BoatId(keeper::mint());
    boats.insert(
        boat,
        BoatState {
            kind: BoatKind::Sloop,
            position: at,
            heading: 0.0,
            occupant: None,
            keeper: Some(token),
        },
    );
    (boat, true)
}

/// A distance or a coordinate as the console says it: whole metres, which is
/// as fine as anybody reading a reply cares about and finer than they can
/// steer.
fn round(metres: f32) -> i32 {
    metres.round() as i32
}

/// The most beasts one `spawn` will raise. High enough to be a silly number
/// of animals on one screen, which is what it is for: the beasts are the only
/// thing in the world drawn per-creature and told about per-beat, so a hundred
/// of them at once is the measurement neither the wire nor the client gets
/// asked for by ordinary play.
const MOST: usize = 100;

/// `spawn <kind> [count]`: beasts raised in the asker's waters, near enough to
/// be seen — the summoning is meant to put the animal in front of whoever
/// typed it, unlike the warden's own raising, which sends everything in from
/// deep water precisely so it is never watched appearing.
///
/// It is deliberately generous about *where*. The kind's own water is
/// preferred and any water will do, so `spawn whale` in a lagoon gives you a
/// whale rather than a refusal: what the command is for is putting something
/// on the screen — one to look at, or a hundred to time the frame with — and
/// an animal in the wrong water sorts itself out within a few beats anyway,
/// its dwelling being to hold to its own band. Only water it could not be in
/// at all is refused, which is a summons on dry land.
fn spawn(shared: &Shared, from: PlayerId, args: &[&str]) -> String {
    let (named, count) = match args {
        [named] => (named, 1usize),
        [named, count] => match count.parse::<usize>() {
            Ok(count) if (1..=MOST).contains(&count) => (named, count),
            _ => return format!("`spawn` will raise 1 to {MOST} of a kind, not `{count}`"),
        },
        _ => return "`spawn` wants a kind of beast — shark, dolphins or whale".to_string(),
    };
    let kind = match *named {
        "shark" => BeastKind::Shark,
        "dolphins" => BeastKind::Dolphins,
        "whale" => BeastKind::Whale,
        _ => return format!("no beast called `{named}` — shark, dolphins or whale"),
    };

    let near = {
        let players = shared.players.held();
        players.get(&from).map(|player| player.position)
    };
    let Some(near) = near else {
        // Unreachable from a served connection, whose player is on the
        // roster for as long as it can speak — but this function cannot know
        // who calls it, and "nothing happened" needs words either way.
        return "you are nowhere a beast could join you".to_string();
    };

    // Decorative randomness, exactly as the beasts' own: nothing about a
    // summons needs to agree with anything, so the clock's leftover nanos
    // are entropy enough to keep repeated summonses off one spot — and each
    // of a batch is dealt its own, so a hundred whales are a hundred places
    // rather than one place a hundred times.
    let entropy = shared.started.elapsed().subsec_nanos();
    let raised: Vec<(BeastKind, Vec2)> = (0..count)
        .filter_map(|which| {
            beasts::conjure(
                shared,
                kind,
                near,
                entropy.wrapping_add(which as u32 * 0x9E37),
            )
            .map(|spot| (kind, spot))
        })
        .collect();
    if raised.is_empty() {
        return format!("no water a {named} could be in near here");
    }

    let nearest = raised
        .iter()
        .map(|(_, spot)| near.distance(*spot))
        .fold(f32::INFINITY, f32::min);
    // Said in the words a player would use for what is now out there, which is
    // not the word they typed: `dolphins` is one pod of them, so several are
    // pods rather than "3 dolphins".
    let announced = match (kind, raised.len()) {
        (BeastKind::Shark, 1) => "a shark rises".to_string(),
        (BeastKind::Dolphins, 1) => "dolphins surface".to_string(),
        (BeastKind::Whale, 1) => "a whale surfaces".to_string(),
        (BeastKind::Shark, many) => format!("{many} sharks rise"),
        (BeastKind::Dolphins, many) => format!("{many} pods surface"),
        (BeastKind::Whale, many) => format!("{many} whales surface"),
    };
    shared.summoned.held().extend(raised);
    format!("{announced}, the nearest {nearest:.0} m away")
}

/// `time <hh:mm>`: the world's clock run forward to the next time it reads
/// that hour — never backwards, which is [`ToClient::Daylight`]'s promise —
/// and everyone told at once rather than on the sky thread's next beat,
/// because the one who asked is watching for it.
fn time(shared: &Shared, args: &[&str]) -> String {
    let [given] = args else {
        return "`time` wants an hour to make it — `time 6:30`".to_string();
    };
    let Some(target) = parse_clock(given) else {
        return format!("`{given}` is not an hour on a 24-hour clock — `time 6:30`");
    };

    let phase = shared.wind_forward_to(target);
    {
        let players = shared.players.held();
        broadcast_all(&players, ToClient::Daylight { phase });
    }
    format!("the day has run on to {}", clock(phase))
}

/// `weather <wind>`: the sky taken in hand for everyone, or — `natural` —
/// given back to the world. No word is sent from here: the sky thread
/// notices the wind moving and tells the roster, exactly as it does when the
/// real weather turns, so an ordered gale arrives the way any gale does.
fn weather(shared: &Shared, args: &[&str]) -> String {
    let [named] = args else {
        return "`weather` wants a wind — calm, breeze, gale or natural".to_string();
    };
    if *named == "natural" {
        shared.command_wind(None);
        return "the weather is the world's own again".to_string();
    }
    match WINDS.iter().find(|(name, _)| name == named) {
        Some((name, wind)) => {
            shared.command_wind(Some(*wind));
            format!("the wind is ordered {name}")
        }
        None => format!("no wind called `{named}` — calm, breeze, gale or natural"),
    }
}

/// `hh:mm` on a 24-hour clock as a phase of the day, or a bare hour — `time
/// 6` is a morning nobody should have to punctuate. `None` for anything that
/// is not a time of some day.
fn parse_clock(given: &str) -> Option<f32> {
    let (hours, minutes) = match given.split_once(':') {
        Some((h, m)) => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        None => (given.parse::<u32>().ok()?, 0),
    };
    (hours < 24 && minutes < 60).then(|| (hours * 60 + minutes) as f32 / (24.0 * 60.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Server, WorldConfig};

    /// What a line is answered with, for the tests that care only about the
    /// words — [`Served`]'s other half is the connection's business, and
    /// [`the_console_takes_you_places`] is where it is looked at.
    fn answer(shared: &Shared, from: PlayerId, line: &str) -> String {
        interpret(shared, from, line).reply
    }

    /// A world to command, never served: `interpret` works on the shared
    /// state alone, so nothing here needs a socket.
    fn a_world(opening: f32) -> std::sync::Arc<Shared> {
        Server::bind(("127.0.0.1", 0), WorldConfig { seed: 20_040_112 })
            .expect("a server should bind")
            .opening_at(opening)
            .shared
    }

    #[test]
    fn the_clock_parses_and_prints() {
        assert_eq!(parse_clock("6:30"), Some(6.5 / 24.0));
        assert_eq!(parse_clock("18"), Some(0.75));
        assert_eq!(parse_clock("0:00"), Some(0.0));
        assert_eq!(parse_clock("24:00"), None);
        assert_eq!(parse_clock("12:60"), None);
        assert_eq!(parse_clock("noon"), None);
        assert_eq!(clock(0.75), "18:00");
    }

    #[test]
    fn time_runs_the_day_forward_never_back() {
        // A world in its afternoon, asked for a morning: the only honest way
        // there is through the night — the clock may never run backwards.
        let shared = a_world(0.5);
        let reply = answer(&shared, PlayerId(1), "time 6:00");
        assert_eq!(reply, "the day has run on to 06:00");
        let phase = shared.phase();
        assert!(
            (phase - 0.25).abs() < 1e-3,
            "the day should stand at morning, not {phase}"
        );
        // And three quarters of a day were skipped to get there, not a
        // quarter unwound.
        let skipped = *shared.skipped.held();
        assert!(
            (skipped - 0.75 * protocol::DAY_SECONDS).abs() < 1.0,
            "the way to an earlier hour is forward through {} seconds, not {skipped}",
            0.75 * protocol::DAY_SECONDS
        );
    }

    #[test]
    fn time_wants_an_hour_it_can_read() {
        let shared = a_world(0.5);
        assert!(answer(&shared, PlayerId(1), "time").contains("6:30"));
        assert!(answer(&shared, PlayerId(1), "time dusk").contains("dusk"));
        // And a refused hour moved nothing.
        assert_eq!(*shared.skipped.held(), 0.0);
    }

    #[test]
    fn weather_is_ordered_and_given_back() {
        let shared = a_world(0.5);

        answer(&shared, PlayerId(1), "weather gale");
        assert_eq!(shared.wind(), Vec2::new(11.31, -11.31));

        answer(&shared, PlayerId(1), "weather calm");
        assert_eq!(shared.wind(), Vec2::ZERO);

        // Given back, the wind is the world's own function of the clock
        // again — whatever that is right now, it is not held anywhere.
        answer(&shared, PlayerId(1), "weather natural");
        assert_eq!(*shared.commanded_wind.held(), None);

        let refused = answer(&shared, PlayerId(1), "weather sirocco");
        assert!(refused.contains("sirocco"), "unhelpful: {refused}");
    }

    #[test]
    fn a_summons_needs_a_player_and_a_kind() {
        let shared = a_world(0.5);
        // Nobody by this id is in the world, so nothing can be near them.
        let reply = answer(&shared, PlayerId(9), "spawn shark");
        assert_eq!(reply, "you are nowhere a beast could join you");

        let refused = answer(&shared, PlayerId(9), "spawn kraken");
        assert!(refused.contains("kraken"), "unhelpful: {refused}");
        assert!(
            shared.summoned.held().is_empty(),
            "something was summoned anyway"
        );
    }

    #[test]
    fn a_summons_will_raise_a_silly_number_but_not_a_nonsense_one() {
        // The count is what makes this command a way of measuring the client
        // rather than only a way of looking at an animal — see `MOST`. What
        // it will not do is guess at a count it cannot read, since a typo
        // that quietly raised one beast would be a measurement of nothing.
        let shared = a_world(0.5);
        for asked in ["spawn shark 0", "spawn shark 101", "spawn shark lots"] {
            let refused = answer(&shared, PlayerId(1), asked);
            assert!(
                refused.contains(&MOST.to_string()),
                "`{asked}` was answered `{refused}`, which says nothing about the range"
            );
        }
        // A count it can read gets as far as looking for the player, which is
        // where the nobody-by-that-id answer comes from.
        assert_eq!(
            answer(&shared, PlayerId(9), "spawn shark 40"),
            "you are nowhere a beast could join you"
        );
    }

    #[test]
    fn a_place_is_two_numbers() {
        let there = Vec2::new(98.0, -317.0);
        assert_eq!(point(&["98", "-317"]), Some(there));
        assert_eq!(point(&["98.5", "-317.25"]), Some(Vec2::new(98.5, -317.25)));
        // And nothing else is a place. The comma form went with the socket's
        // `focus`, which is what it was ever written for.
        assert_eq!(point(&["98,-317"]), None);
        assert_eq!(point(&[]), None);
        assert_eq!(point(&["98"]), None);
        assert_eq!(point(&["98", "-317", "12"]), None);
        assert_eq!(point(&["north", "a bit"]), None);
    }

    #[test]
    fn a_hull_is_stood_off_the_shore_that_was_asked_for() {
        // Somewhere on the entry island's land — found the way the entry
        // point itself is found, by walking in from the water until the
        // ground comes up.
        let shared = a_world(0.5);
        let spawn = shared.world.spawn().expect("a world has islands in it");
        let inland = spawn.island.centre();
        assert!(
            shared.world.height(inland.x, inland.y) >= 0.0,
            "the middle of an island should be land to be stood off from"
        );

        let (off, shore) = standing_off(&shared.world, inland);
        assert!(
            shared.world.height(shore.x, shore.y) < 0.0,
            "the shore reported at {shore} is not the water's edge"
        );
        assert!(
            shared.world.height(off.x, off.y) < 0.0,
            "a hull was left standing off dry land at {off}"
        );
        // Off *that* shore, not off the far side of the world: the island is
        // its own bound on how far the walk can have gone.
        let reach = spawn.island.extent().length() + SPAWN_OFFSHORE;
        assert!(
            off.distance(inland) <= reach,
            "the shore found for {inland} was {off}, {} m away",
            off.distance(inland)
        );
        // And clear of the beach rather than on it — an arrival stands off.
        assert!(
            off.distance(inland) >= SPAWN_OFFSHORE,
            "a hull was anchored within a stride of the land it asked for"
        );
    }

    #[test]
    fn a_place_the_world_does_not_have_is_refused_and_nothing_else_is() {
        let shared = a_world(0.5);
        let refused = answer(&shared, PlayerId(1), "goto 1e12 0");
        assert!(
            refused.contains("outside the world"),
            "unhelpful: {refused}"
        );

        for asked in ["goto", "goto 12", "goto north"] {
            let refused = answer(&shared, PlayerId(1), asked);
            assert!(
                refused.contains("goto 480 -1200"),
                "`{asked}` was answered `{refused}`, which teaches nothing"
            );
        }

        // A place the world does have, asked for by nobody the world knows —
        // as far as this can get without a roster, and the answer says so
        // rather than pretending somebody moved.
        let nobody = answer(&shared, PlayerId(9), "goto 0 0");
        assert!(nobody.contains("nowhere"), "unhelpful: {nobody}");
    }

    #[test]
    fn the_vocabulary_is_the_grammar_it_advertises() {
        // Every advertised verb is served: alone it may earn a usage
        // complaint, but never the not-a-command answer — a client is going
        // to complete these under players' fingers, and a taught word the
        // server then disowns would make the completion a lie.
        let shared = a_world(0.5);
        for verb in VERBS {
            let reply = answer(&shared, PlayerId(1), verb);
            assert!(
                !reply.contains("is not a command"),
                "`{verb}` is advertised but not served: {reply}"
            );
        }
    }

    #[test]
    fn every_line_gets_an_answer() {
        let shared = a_world(0.5);
        let unknown = answer(&shared, PlayerId(1), "dance");
        assert!(unknown.contains("`dance`") && unknown.contains("help"));

        let help = answer(&shared, PlayerId(1), "help");
        for verb in ["goto", "spawn", "time", "weather"] {
            assert!(help.contains(verb), "`help` does not mention {verb}");
        }
    }
}
