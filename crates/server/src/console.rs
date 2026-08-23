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
//! of the wire, under a `client` prefix this parser will never see. Anyone in
//! a session may command; [`interpret`] is told who asked so that a later
//! gate has somewhere to stand, and the host's log names the asker either
//! way.
//!
//! Two shapes of line, and the first word says which. `goto`, `grant` and
//! `spawn` act, and answer with what happened. `world` is the dials the
//! world itself stands on — the hour, the wind — in the three forms `client`
//! has on the other side: the bare word lists them, a dial alone reads it, a
//! dial and a value turns it. Reading is the half that was missing while
//! these were verbs: a console could order a gale but never ask whether one
//! was still ordered, and `natural` could be given back to a sky that
//! already had it.

use glam::Vec2;
use protocol::{clock, BeastKind, BoatId, BoatKind, PlayerId, ToClient};
use world::archipelago::{Archipelago, SOUNDING, SPAWN_OFFSHORE};

use crate::{
    aimed, beasts, broadcast, broadcast_all, keeper, post, reachable, BoatState, Held, Shared,
};

/// What `help` says. One line per command, in the imperative the commands
/// themselves are written in, and the blank line between the two kinds — the
/// acts above, the dials below.
const HELP: &str = "goto <x> <z> — be taken to a place, however you are travelling\n\
                    grant sloop|rowboat — a hull of that kind put in the water for you\n\
                    spawn shark|dolphins|whale [count] — raise beasts in your waters\n\
                    \n\
                    world — every dial as it stands; a dial alone reads that one\n\
                    world time <hh:mm> — run the clock forward to that hour\n\
                    world weather calm|breeze|gale|natural — order the wind, or give it back";

/// Every line [`interpret`] serves, as far as its words are fixed — taught to
/// each client on joining as [`ToClient::Vocabulary`], so a console can
/// complete them as a player types. Whole phrases and not first words alone,
/// because `world` is a shelf and a client that knew only the shelf would
/// complete a player into a dead end.
///
/// The grammar's index, not the grammar: `interpret` never reads this, and a
/// test holds the two to agreement.
pub(crate) const PHRASES: [&str; 6] = [
    "goto",
    "grant",
    "help",
    "spawn",
    "world time",
    "world weather",
];

/// The hulls `grant` deals, and the words that ask for them. The naming is
/// the console's own: [`BoatKind`] carries none, nothing else in the world
/// being asked for by name, and the prose here calls a rowing boat a tender
/// or a dinghy depending on what it is doing — none of which is a word to
/// make somebody guess at.
///
/// An index of the grammar and not the grammar: a test holds it to what
/// `help` advertises and to what [`grant`] will actually deal.
const HULLS: [(&str, BoatKind); 2] = [("sloop", BoatKind::Sloop), ("rowboat", BoatKind::Rowboat)];

/// The dials `world` turns, in the order a bare `world` reads them out.
/// Named here once because three places want them: the listing, the reading,
/// and what a miss is told to try instead.
const TIME: &str = "time";
const WEATHER: &str = "weather";
const DIALS: [&str; 2] = [TIME, WEATHER];

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
        ["grant", rest @ ..] => grant(shared, from, rest).into(),
        ["spawn", rest @ ..] => spawn(shared, from, rest).into(),
        ["world", rest @ ..] => world(shared, rest).into(),
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

/// `goto <x> <z>`: the asker taken to a point of the world, however they
/// happen to be travelling.
///
/// A hull is the only thing here a point can be wrong for: it cannot sit on
/// a hillside. Somebody on their own feet is at home anywhere the world has
/// — they walk it, wade it or swim it — so the ground under the point
/// decides nothing for them, and the answer says which case it was:
///
/// - On foot, anywhere: they are put down on the spot they named, and what
///   to do about it is theirs. Whatever boats they have stay where they
///   were, because walking away from a boat is already how a player parts
///   from one, and this is a long walk — or a long swim. This command deals
///   nobody a hull: being somewhere and having a boat are two asks, and
///   [`grant`] is the other one.
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

    // Whether the point is land is the whole of what a hull's placement
    // turns on, and asking costs the island under it being generated if it
    // never has been — the tens to hundreds of milliseconds this command is
    // worth, and paid before any lock is taken. The offing goes with it, and
    // both are wasted on a walker, who is put down wherever they asked. Only
    // the roster knows which this asker is, and reading it first to save the
    // sixteen rays would mean either sounding the sea under its lock — every
    // other connection held out of the world while it happened — or taking
    // the lock twice around a walk that is heights already in hand.
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
    let aboard = player.aboard;

    let (at, heading, told) = match aboard {
        // At a helm: the hull goes, and lies off the shore when the shore is
        // what was asked for. Facing it, in that case — a hull anchored
        // stern-on to the island it was brought to see is no use to whoever
        // asked for it.
        Some(boat) => {
            let mut boats = shared.boats.held();
            let at = berth.map_or(asked, |(off, _)| off);
            let state = boats.get_mut(&boat).expect("a boat once boarded exists");
            state.position = at;
            if berth.is_some() {
                state.heading = aimed(at, asked);
            }
            (at, Some(state.heading), Some(state.told(boat)))
        }
        // Afoot: the point itself, whatever is under it, and which way they
        // face is their own business — see [`ToClient::PutDown`], whose
        // `None` heading this is.
        None => (asked, None, None),
    };
    player.position = at;

    match told {
        // A word about a hull is news to everyone but the client steering
        // it, which is the authority on its own hull and is told where it
        // stands by the put down instead.
        Some(telling) => broadcast(&players, from, telling),
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

    let reply = match berth {
        // Measured to the shore rather than to the point that was asked
        // for: what a player wants to know from a deck is how far off the
        // beach they are lying, and the place they named is somewhere
        // inland of it. The coordinates are the berth either way — where
        // they are is where they are.
        Some((_, shore)) if aboard.is_some() => format!(
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

/// `grant <kind>`: a hull of that kind put in the water for the asker, at
/// anchor and with nobody aboard.
///
/// It deals a boat and stops there. Boarding is walking up to a free helm
/// and taking it — the one way anybody gets aboard anything — and a command
/// that seated its asker would be the console doing by fiat what the world
/// already has a rule for. Being somewhere and having a boat are two asks:
/// [`goto`] is the first of them and this is the second.
///
/// Where they stand, if that is water. Off the nearest shore if it is not —
/// [`standing_off`], the offing a world is entered on — because a hull dealt
/// onto a hillside is the one thing the game itself never does, so a grant
/// asked for from a summit is answered with a walk and a swim.
///
/// Their own hull of that kind comes to them if one is lying free anywhere
/// in the world, and one is minted only if none is. Theirs first for the
/// reason the door prefers a spare to a mint — see
/// [`crate::BoatState::keeper`] and `fresh_hull` in the join path. An
/// evening of this would otherwise leave a sloop adrift at every place its
/// asker had stood, each one filed in the world and posted to every future
/// joiner.
fn grant(shared: &Shared, from: PlayerId, args: &[&str]) -> String {
    let [named] = args else {
        return format!("`grant` wants a kind of boat — {}", kinds());
    };
    let Some(&(_, kind)) = HULLS.iter().find(|(name, _)| name == named) else {
        return format!("no boat called `{named}` — {}", kinds());
    };

    let who = {
        let players = shared.players.held();
        players
            .get(&from)
            .map(|player| (player.token, player.position))
    };
    let Some((token, asker)) = who else {
        // Unreachable from a served connection, whose player is on the
        // roster for as long as it can speak — the same nobody-by-that-id
        // case `spawn` answers, and it needs words here too.
        return "you are nowhere a boat could reach you".to_string();
    };

    // Sounded before the boats are locked, for the reason `goto` says at
    // length: the ground under a point may have to be generated to answer
    // this, and that is not a wait to hold the world through.
    let offing = (shared.world.height(asker.x, asker.y) >= 0.0)
        .then(|| standing_off(&shared.world, asker).0);
    let at = offing.unwrap_or(asker);

    let (minted, telling) = {
        let mut boats = shared.boats.held();
        let theirs = boats
            .iter()
            .find(|(_, boat)| {
                boat.kind == kind && boat.occupant.is_none() && boat.keeper == Some(token)
            })
            .map(|(&boat, _)| boat);
        match theirs {
            Some(boat) => {
                let state = boats.get_mut(&boat).expect("looked up a breath ago");
                state.position = at;
                (false, state.told(boat))
            }
            None => {
                let boat = BoatId(keeper::mint());
                // Written whole, on the same reasoning `Lower` gives for
                // writing its tender that way: a hull's fields are settled in
                // one place or they drift apart. The keeper above all — this
                // one is theirs from the moment it touches the water, which
                // is what makes the next `grant` bring it back rather than
                // mint another.
                let state = BoatState {
                    kind,
                    position: at,
                    heading: 0.0,
                    occupant: None,
                    keeper: Some(token),
                };
                let telling = state.told(boat);
                boats.insert(boat, state);
                (true, telling)
            }
        }
    };
    // Everyone, the asker included: no client is steering this hull, so
    // nobody here is the authority on it that a helmsman would be.
    broadcast_all(&shared.players.held(), telling);

    let whose = if minted {
        format!("a {named} is")
    } else {
        format!("your {named} is")
    };
    match offing {
        // How far they have to go to reach it, which is the whole of what a
        // grant answered from dry land has to tell somebody.
        Some(off) => format!(
            "{whose} in the water {} m off, at {} {}",
            round(asker.distance(off)),
            round(at.x),
            round(at.y)
        ),
        None => format!("{whose} here, at {} {}", round(at.x), round(at.y)),
    }
}

/// The kinds a `grant` will take, as its refusals list them.
fn kinds() -> String {
    HULLS
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(" or ")
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

/// The `world` grammar: `world` reads every dial, `world <dial>` reads one,
/// `world <dial> <value>` turns one. The same three forms the client's own
/// `client` lines have, for the same reason — a dial nobody can read is a
/// switch you have to remember the state of.
fn world(shared: &Shared, args: &[&str]) -> String {
    match args {
        [] => DIALS
            .iter()
            .filter_map(|dial| reading(shared, dial))
            .collect::<Vec<_>>()
            .join(" / "),
        [dial] => reading(shared, dial).unwrap_or_else(|| no_such(dial)),
        [dial, value] => match *dial {
            TIME => time(shared, value),
            WEATHER => weather(shared, value),
            _ => no_such(dial),
        },
        _ => "one dial, one value — `world time 6:30`".to_string(),
    }
}

/// What one dial reads as, or `None` for a name that is not a dial at all.
///
/// A reading names the value that could be written back — `gale`, an hour —
/// so nothing has to be remembered to know what a dial would take. The
/// weather's reading is word for word what its write answered with, since
/// there is nothing more to say about an order than that it stands; the
/// clock's differs because a write there also says the day *moved*, and
/// forward.
fn reading(shared: &Shared, dial: &str) -> Option<String> {
    Some(match dial {
        TIME => format!("the day stands at {}", clock(shared.phase())),
        WEATHER => match *shared.commanded_wind.held() {
            Some((name, _)) => ordered(name),
            None => "the weather is the world's own".to_string(),
        },
        _ => return None,
    })
}

/// What a name that is not a dial is told, which is every dial there is —
/// the same answer whether it was read or written to.
fn no_such(dial: &str) -> String {
    format!(
        "the world has no dial called `{dial}` — {}",
        DIALS.join(" or ")
    )
}

/// `world time <hh:mm>`: the world's clock run forward to the next time it
/// reads that hour — never backwards, which is [`ToClient::Daylight`]'s
/// promise — and everyone told at once rather than on the sky thread's next
/// beat, because the one who asked is watching for it.
fn time(shared: &Shared, given: &str) -> String {
    let Some(target) = parse_clock(given) else {
        return format!("`{given}` is not an hour on a 24-hour clock — `world time 6:30`");
    };

    let phase = shared.wind_forward_to(target);
    {
        let players = shared.players.held();
        broadcast_all(&players, ToClient::Daylight { phase });
    }
    format!("the day has run on to {}", clock(phase))
}

/// `world weather <wind>`: the sky taken in hand for everyone, or —
/// `natural` — given back to the world. No word is sent from here: the sky
/// thread notices the wind moving and tells the roster, exactly as it does
/// when the real weather turns, so an ordered gale arrives the way any gale
/// does.
fn weather(shared: &Shared, named: &str) -> String {
    if named == "natural" {
        shared.command_wind(None);
        return "the weather is the world's own again".to_string();
    }
    match WINDS.iter().find(|(name, _)| *name == named) {
        Some(&(name, wind)) => {
            shared.command_wind(Some((name, wind)));
            ordered(name)
        }
        None => format!("no wind called `{named}` — calm, breeze, gale or natural"),
    }
}

/// How a standing order reads, said in one place because the write and the
/// read both say it.
fn ordered(name: &str) -> String {
    format!("the wind is ordered {name}")
}

/// `hh:mm` on a 24-hour clock as a phase of the day, or a bare hour — `world
/// time 6` is a morning nobody should have to punctuate. `None` for anything
/// that is not a time of some day.
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
        let reply = answer(&shared, PlayerId(1), "world time 6:00");
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
        let refused = answer(&shared, PlayerId(1), "world time dusk");
        assert!(refused.contains("dusk"), "unhelpful: {refused}");
        // And a refused hour moved nothing.
        assert_eq!(*shared.skipped.held(), 0.0);
    }

    #[test]
    fn weather_is_ordered_and_given_back() {
        let shared = a_world(0.5);

        answer(&shared, PlayerId(1), "world weather gale");
        assert_eq!(shared.wind(), Vec2::new(11.31, -11.31));

        answer(&shared, PlayerId(1), "world weather calm");
        assert_eq!(shared.wind(), Vec2::ZERO);

        // Given back, the wind is the world's own function of the clock
        // again — whatever that is right now, it is not held anywhere.
        answer(&shared, PlayerId(1), "world weather natural");
        assert_eq!(*shared.commanded_wind.held(), None);

        let refused = answer(&shared, PlayerId(1), "world weather sirocco");
        assert!(refused.contains("sirocco"), "unhelpful: {refused}");
    }

    /// Every dial reads, and reads back the words its own write answered
    /// with — the half that was missing while these were verbs. Held to
    /// [`DIALS`] rather than to a list here, so a dial added without a
    /// reading fails rather than going quietly missing from a bare `world`.
    ///
    /// The round trip is shown on the weather and not on the clock, whose
    /// reading has moved on by the next line: a day is
    /// [`protocol::DAY_SECONDS`] long, so the hour keeps changing while the
    /// test runs, and an equality between two readings of it would be a
    /// stopwatch dressed as an assertion.
    #[test]
    fn every_dial_reads_and_reads_back_what_could_be_written() {
        let shared = a_world(0.5);
        for dial in DIALS {
            let read = answer(&shared, PlayerId(1), &format!("world {dial}"));
            assert!(
                !read.contains("no dial called"),
                "`{dial}` is listed but does not read: {read}"
            );
        }

        assert_eq!(
            answer(&shared, PlayerId(1), "world weather"),
            "the weather is the world's own"
        );
        let ordered = answer(&shared, PlayerId(1), "world weather gale");
        assert_eq!(answer(&shared, PlayerId(1), "world weather"), ordered);

        // And a bare `world` is the dials together.
        let listed = answer(&shared, PlayerId(1), "world");
        assert!(
            listed.contains(&ordered) && listed.contains("the day stands at"),
            "a bare `world` said `{listed}`"
        );
    }

    #[test]
    fn a_dial_the_world_does_not_have_is_answered() {
        let shared = a_world(0.5);
        for asked in ["world tide", "world tide high"] {
            let refused = answer(&shared, PlayerId(1), asked);
            assert!(
                refused.contains("`tide`") && refused.contains("weather"),
                "`{asked}` was answered `{refused}`, which teaches nothing"
            );
        }
        let too_many = answer(&shared, PlayerId(1), "world time 6:00 sharp");
        assert!(too_many.contains("one dial"), "unhelpful: {too_many}");
        assert_eq!(*shared.skipped.held(), 0.0, "a refused line moved the day");
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
    fn every_hull_the_grant_names_can_be_asked_for() {
        let shared = a_world(0.5);
        for (named, _) in HULLS {
            assert!(
                HELP.contains(named),
                "`grant` deals a {named} and `help` never says so"
            );
            // Nobody is on this world's roster, so the answer is the one
            // about there being no such asker — which is already past the
            // parsing, and that is what this is asking about.
            let answered = answer(&shared, PlayerId(1), &format!("grant {named}"));
            assert!(
                !answered.contains("no boat called"),
                "`{named}` is advertised and not known: {answered}"
            );
        }

        for asked in ["grant", "grant frigate", "grant sloop rowboat"] {
            let refused = answer(&shared, PlayerId(1), asked);
            assert!(
                HULLS.iter().all(|(named, _)| refused.contains(named)),
                "`{asked}` was answered `{refused}`, which teaches nothing"
            );
        }
    }

    #[test]
    fn the_vocabulary_is_the_grammar_it_advertises() {
        // Every advertised verb is served: alone it may earn a usage
        // complaint, but never the not-a-command answer — a client is going
        // to complete these under players' fingers, and a taught word the
        // server then disowns would make the completion a lie.
        let shared = a_world(0.5);
        for phrase in PHRASES {
            let reply = answer(&shared, PlayerId(1), phrase);
            assert!(
                !reply.contains("is not a command") && !reply.contains("no dial called"),
                "`{phrase}` is advertised but not served: {reply}"
            );
        }
    }

    #[test]
    fn every_line_gets_an_answer() {
        let shared = a_world(0.5);
        let unknown = answer(&shared, PlayerId(1), "dance");
        assert!(unknown.contains("`dance`") && unknown.contains("help"));

        // Every phrase but `help` itself, which is what was typed to get
        // this far and needs no line of its own.
        let help = answer(&shared, PlayerId(1), "help");
        for phrase in PHRASES.iter().filter(|it| **it != "help") {
            assert!(help.contains(phrase), "`help` does not mention {phrase}");
        }
    }
}
