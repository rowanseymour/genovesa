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

use glam::Vec2;
use protocol::{clock, BeastKind, PlayerId, ToClient};

use crate::{beasts, broadcast_all, Held, Shared};

/// What `help` says. One line per command, in the imperative the commands
/// themselves are written in.
const HELP: &str = "spawn shark|dolphins|whale [count] — raise beasts in your waters\n\
                    time <hh:mm> — run the world's clock forward to that hour\n\
                    weather calm|breeze|gale|natural — order the wind, or give it back";

/// The first word of every line [`interpret`] serves — taught to each client
/// on joining as [`ToClient::Vocabulary`], so a console can complete them as
/// a player types. The grammar's index, not the grammar: `interpret` never
/// reads this, and a test holds the two to agreement.
pub(crate) const VERBS: [&str; 4] = ["help", "spawn", "time", "weather"];

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

/// Serves one console line and says what came of it — every line gets an
/// answer, the ones nothing here recognises included, because silence at a
/// console reads as a hang.
pub(crate) fn interpret(shared: &Shared, from: PlayerId, line: &str) -> String {
    let words: Vec<&str> = line.split_whitespace().collect();
    match words.as_slice() {
        [] | ["help"] => HELP.to_string(),
        ["spawn", rest @ ..] => spawn(shared, from, rest),
        ["time", rest @ ..] => time(shared, rest),
        ["weather", rest @ ..] => weather(shared, rest),
        [verb, ..] => format!("`{verb}` is not a command here — `help` lists what is"),
    }
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
        let reply = interpret(&shared, PlayerId(1), "time 6:00");
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
        assert!(interpret(&shared, PlayerId(1), "time").contains("6:30"));
        assert!(interpret(&shared, PlayerId(1), "time dusk").contains("dusk"));
        // And a refused hour moved nothing.
        assert_eq!(*shared.skipped.held(), 0.0);
    }

    #[test]
    fn weather_is_ordered_and_given_back() {
        let shared = a_world(0.5);

        interpret(&shared, PlayerId(1), "weather gale");
        assert_eq!(shared.wind(), Vec2::new(11.31, -11.31));

        interpret(&shared, PlayerId(1), "weather calm");
        assert_eq!(shared.wind(), Vec2::ZERO);

        // Given back, the wind is the world's own function of the clock
        // again — whatever that is right now, it is not held anywhere.
        interpret(&shared, PlayerId(1), "weather natural");
        assert_eq!(*shared.commanded_wind.held(), None);

        let refused = interpret(&shared, PlayerId(1), "weather sirocco");
        assert!(refused.contains("sirocco"), "unhelpful: {refused}");
    }

    #[test]
    fn a_summons_needs_a_player_and_a_kind() {
        let shared = a_world(0.5);
        // Nobody by this id is in the world, so nothing can be near them.
        let reply = interpret(&shared, PlayerId(9), "spawn shark");
        assert_eq!(reply, "you are nowhere a beast could join you");

        let refused = interpret(&shared, PlayerId(9), "spawn kraken");
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
            let refused = interpret(&shared, PlayerId(1), asked);
            assert!(
                refused.contains(&MOST.to_string()),
                "`{asked}` was answered `{refused}`, which says nothing about the range"
            );
        }
        // A count it can read gets as far as looking for the player, which is
        // where the nobody-by-that-id answer comes from.
        assert_eq!(
            interpret(&shared, PlayerId(9), "spawn shark 40"),
            "you are nowhere a beast could join you"
        );
    }

    #[test]
    fn the_vocabulary_is_the_grammar_it_advertises() {
        // Every advertised verb is served: alone it may earn a usage
        // complaint, but never the not-a-command answer — a client is going
        // to complete these under players' fingers, and a taught word the
        // server then disowns would make the completion a lie.
        let shared = a_world(0.5);
        for verb in VERBS {
            let reply = interpret(&shared, PlayerId(1), verb);
            assert!(
                !reply.contains("is not a command"),
                "`{verb}` is advertised but not served: {reply}"
            );
        }
    }

    #[test]
    fn every_line_gets_an_answer() {
        let shared = a_world(0.5);
        let unknown = interpret(&shared, PlayerId(1), "dance");
        assert!(unknown.contains("`dance`") && unknown.contains("help"));

        let help = interpret(&shared, PlayerId(1), "help");
        for verb in ["spawn", "time", "weather"] {
            assert!(help.contains(verb), "`help` does not mention {verb}");
        }
    }
}
