//! The server's half of the debug console: what a
//! [`protocol::ToServer::Command`] line means here, and the
//! [`ToClient::Reply`] it earns.
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
//! Three shapes of line, and the first word says which. `goto`, `grant` and
//! `spawn` act, and answer with what happened. `world` is the dials the
//! world itself stands on — the hour, the wind, the seed — in the three
//! forms `client` has on the other side: the bare word lists them, a dial
//! alone reads it, a dial and a value turns it. Reading is the half that was
//! missing while these were verbs: a console could order a gale but never
//! ask whether one was still ordered, and `natural` could be given back to a
//! sky that already had it. And `where` reads the asker's own situation,
//! which is the world's to answer rather than the client's for the reason
//! [`whereabouts`] gives; `islands` reads the layout around them, which is
//! the world's alone.
//!
//! The acts stay acts. Giving `goto` a second, argumentless meaning would
//! hide a reading behind a verb, which is what `where` is there for.
//!
//! Every command is a row of [`COMMANDS`]: a word, the `help` lines it
//! answers for, the fixed words that may follow it, and one function of
//! [`Asked`] returning what the player is told. That last is the whole
//! contract — words in, words out, and refusals told apart from answers only
//! so `?` can be written and a test can ask. Effects a command has it has
//! through `shared`, like everything else in the crate. What the shape buys
//! is that `help` and the completion a client is taught are folds over the
//! table rather than listings beside it, so neither can advertise a word the
//! world would then disown.

use glam::Vec2;
use protocol::{clock, BeastKind, BoatId, BoatKind, PlayerId, ToClient, Underway};
use world::archipelago::{berth_off, Archipelago, BERTH_OFFING, SOUNDING};

use crate::{
    aimed, astern, beasts, broadcast, broadcast_all, keeper, post, reachable, BoatState, Held,
    Shared,
};

/// One command, as the table has it: the word that reaches it, what `help`
/// says about it, the fixed words that may follow it, and what it does.
///
/// A table because the alternatives were listings — a `help` string, a phrase
/// list for completion, and a match — each of which had to be taught the same
/// word, and none of which could be made to. What holds them together now is
/// that there is only one of them: [`help`] and [`phrases`] are folds over
/// this, so a command missing from the listing is a command that does not
/// exist rather than one a client completes a player into and the world then
/// disowns.
struct Command {
    /// The first word of every line that reaches it.
    word: &'static str,
    /// What `help` says, each line printed after the word — several where a
    /// command has several forms, as a shelf does.
    usage: &'static [&'static str],
    /// The words that may follow, for a client completing a line as a player
    /// types — see [`phrases`]. Empty where what follows is the asker's own:
    /// a place, an hour, a count.
    tails: fn() -> Vec<String>,
    /// What it does, and what the asker is told for it. [`Err`] is a refusal,
    /// and the asker reads it exactly as they read an answer — the wire
    /// carries one kind of reply. The distinction is for this side: nothing
    /// here has to remember which of its own strings were the sorry ones, and
    /// a test can ask whether a line was refused instead of grepping the
    /// prose for the apology.
    run: fn(Asked) -> Result<String, String>,
}

/// Every command there is, in the order `help` prints them: the acts first,
/// then the shelf the world's own dials stand on.
const COMMANDS: [Command; 7] = [
    Command {
        word: "goto",
        usage: &["<x> <z> — be taken to a place, however you are travelling"],
        tails: none,
        run: goto,
    },
    Command {
        word: "grant",
        usage: &["sloop|rowboat — a hull of that kind put in the water for you"],
        tails: || names(&HULLS),
        run: grant,
    },
    Command {
        word: "spawn",
        usage: &["shark|dolphins|whale [count] — raise beasts in your waters"],
        tails: || names(&BEASTS),
        run: spawn,
    },
    Command {
        word: "where",
        usage: &["— where you stand, what you are on or keep, and the water under you"],
        tails: none,
        run: whereabouts,
    },
    Command {
        word: "islands",
        usage: &[
            "— the nearest islands: the middle of each, a place for `goto`, its size and how far",
        ],
        tails: none,
        run: islands,
    },
    Command {
        word: "help",
        usage: &["— every command here, and what it takes"],
        tails: none,
        run: |_| Ok(help()),
    },
    Command {
        word: "world",
        usage: &[
            "— every dial as it stands; a dial alone reads that one",
            "time <hh:mm> — run the clock forward to that hour",
            "weather calm|breeze|gale|natural — order the wind, or give it back",
            "seed — the number this world was raised on",
        ],
        tails: dials,
        run: world,
    },
];

/// What `help` says: every form of every command, the word and then the line
/// that form is written as. Read off [`COMMANDS`] rather than kept beside it,
/// so the listing cannot advertise a word the table has never heard of.
fn help() -> String {
    COMMANDS
        .iter()
        .flat_map(|command| {
            command
                .usage
                .iter()
                .map(move |line| format!("{} {line}", command.word))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every phrase [`interpret`] serves, as far as its words are fixed — taught
/// to each client on joining as [`ToClient::Vocabulary`], so a console can
/// complete them as a player types.
///
/// Whole phrases and not first words alone, because `world` is a shelf and a
/// client that knew only the shelf would complete a player into a dead end.
/// The same argument runs one word further out wherever the argument is a
/// fixed few — the hulls, the beasts, the winds — and stops at the ones that
/// are the asker's own: there is no completing an hour or a place, and a tab
/// that guessed at one would be inventing rather than teaching.
pub(crate) fn phrases() -> Vec<String> {
    COMMANDS
        .iter()
        .flat_map(|command| {
            std::iter::once(command.word.to_string()).chain(
                (command.tails)()
                    .into_iter()
                    .map(|tail| format!("{} {tail}", command.word)),
            )
        })
        .collect()
}

/// The tails of a command whose arguments are the asker's own — no word to
/// offer, which is not the same as a word nobody has got round to listing.
fn none() -> Vec<String> {
    Vec::new()
}

/// The words a table of named things answers to, which is what a client is
/// taught to complete and what a refusal offers instead. Read off the table
/// that serves them, so the two cannot disagree.
fn names<T>(table: &[(&'static str, T)]) -> Vec<String> {
    table.iter().map(|(name, _)| name.to_string()).collect()
}

/// A list as a sentence says it: `time, weather or seed`.
fn listed(words: &[String]) -> String {
    match words.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// The hulls `grant` deals, and the words that ask for them. The naming is
/// the console's own: [`BoatKind`] carries none, nothing else in the world
/// being asked for by name, and the prose here calls a rowing boat a tender
/// or a dinghy depending on what it is doing — none of which is a word to
/// make somebody guess at.
///
/// [`grant`] reads this listing and so does the completion a client is
/// taught, so neither can disagree with it. What can is the `help` line,
/// which is prose — a test holds the two to each other, both ways round.
const HULLS: [(&str, BoatKind); 2] = [("sloop", BoatKind::Sloop), ("rowboat", BoatKind::Rowboat)];

/// The beasts `spawn` raises, and the words that ask for them — the same
/// table [`HULLS`] is, and for the same reasons.
const BEASTS: [(&str, BeastKind); 3] = [
    ("shark", BeastKind::Shark),
    ("dolphins", BeastKind::Dolphins),
    ("whale", BeastKind::Whale),
];

/// A dial the world stands on: what it is called, what it reads as, and —
/// where it turns at all — what turns it and what turning it will take.
///
/// The turn is an [`Option`] because reading and writing are separate powers
/// and one dial has only the first. A world is raised on its seed and keeps
/// it: there is a number to ask for and nothing to set. Before this was a
/// table that distinction had nowhere to live, and a read-only dial would
/// have had to be answered by the shelf's own miss — telling a player that
/// the world has no dial called `seed` a line after printing its value.
struct Dial {
    name: &'static str,
    read: fn(&Shared) -> String,
    turn: Option<Turn>,
    /// The values the turn takes, where they are a fixed few — empty for the
    /// clock, every hour there is being no listing at all.
    values: fn() -> Vec<String>,
}

/// What turning a dial takes, and what it answers with. Named because the
/// type is two deep where [`Dial`] holds it and reads better with a word on
/// it.
type Turn = fn(&Shared, &str) -> Result<String, String>;

/// The dials the world stands on, in the order a bare `world` reads them out.
///
/// A reading names the value that could be written back — `gale`, an hour —
/// so nothing has to be remembered to know what a dial would take. The
/// weather's reading is word for word what its write answered with, since
/// there is nothing more to say about an order than that it stands; the
/// clock's differs because a write there also says the day *moved*, and
/// forward.
const DIALS: [Dial; 3] = [
    Dial {
        name: "time",
        read: |shared| format!("the day stands at {}", clock(shared.phase())),
        turn: Some(time),
        values: none,
    },
    Dial {
        name: "weather",
        read: |shared| match *shared.commanded_wind.held() {
            Some((name, _)) => ordered(name),
            None => "the weather is the world's own".to_string(),
        },
        turn: Some(weather),
        values: || {
            names(&WINDS)
                .into_iter()
                .chain([NATURAL.to_string()])
                .collect()
        },
    },
    Dial {
        name: "seed",
        read: |shared| format!("this world was raised on seed {}", shared.world.seed()),
        turn: None,
        values: none,
    },
];

/// The phrases that stand behind `world`: every dial, and every value a dial
/// takes a fixed few of.
fn dials() -> Vec<String> {
    DIALS
        .iter()
        .flat_map(|dial| {
            std::iter::once(dial.name.to_string()).chain(
                (dial.values)()
                    .into_iter()
                    .map(|value| format!("{} {value}", dial.name)),
            )
        })
        .collect()
}

/// The word that gives the sky back to the world, which is not a wind and so
/// is not in [`WINDS`] — named once because the turn and the completion both
/// want it.
const NATURAL: &str = "natural";

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

/// What a command is handed: the world it works on, who asked, and the words
/// after the verb.
///
/// The one thing a command hands back that is not words is here too, rather
/// than in the return type. Commands have effects — a hull dealt, a day run
/// on, a beast raised — and they work every one of those through `shared`,
/// like anything else in the crate. The put down is the exception because
/// what it acts on is the *caller* rather than the world, for the reason
/// [`Served::put`] gives. Kept here it costs the answer nothing: every
/// command still answers in words alone.
pub(crate) struct Asked<'a> {
    shared: &'a Shared,
    from: PlayerId,
    args: &'a [&'a str],
    /// Where the world has put the asker — see [`Served::put`], which is
    /// where this comes out.
    put: &'a mut Option<Vec2>,
}

/// Serves one console line and says what came of it — every line gets an
/// answer, the ones nothing here recognises included, because silence at a
/// console reads as a hang.
///
/// An empty line is `help`, which is the friendliest reading of somebody
/// pressing return at a prompt they have just found.
pub(crate) fn interpret(shared: &Shared, from: PlayerId, line: &str) -> Served {
    let mut put = None;
    // An answer and a refusal are both what the asker reads, and the wire
    // has one kind of reply to carry them in. The two part company only on
    // this side of it — see [`serve`].
    let (Ok(reply) | Err(reply)) = serve(shared, from, line, &mut put);
    Served { reply, put }
}

/// The table's answer to a line, refusals still told apart from answers.
///
/// Split from [`interpret`] because the difference is worth something here
/// even though the wire flattens it: a test can ask whether a line was
/// refused rather than grep the prose for an apology, and a command that
/// wants to refuse says so with `?` rather than by remembering to return
/// early.
fn serve(
    shared: &Shared,
    from: PlayerId,
    line: &str,
    put: &mut Option<Vec2>,
) -> Result<String, String> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let (word, args) = words
        .split_first()
        .map_or(("help", &[][..]), |(word, args)| (*word, args));

    let command = COMMANDS
        .iter()
        .find(|command| command.word == word)
        .ok_or_else(|| format!("`{word}` is not a command here — `help` lists what is"))?;
    (command.run)(Asked {
        shared,
        from,
        args,
        put,
    })
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
/// It only writes. Asking where somebody *is* is a client's own question —
/// `client position`, answered off the picture this machine is drawing — and
/// giving `goto` a second, argumentless meaning would put the reading in the
/// one place a reader has to know a verb to look.
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
fn goto(asked: Asked) -> Result<String, String> {
    let Asked {
        shared,
        from,
        args,
        put,
    } = asked;

    let Some(wanted) = point(args) else {
        return Err("`goto` wants a place to be taken to — `goto 480 -1200`".to_string());
    };
    if !reachable(wanted) {
        return Err(format!("`{} {}` is outside the world", wanted.x, wanted.y));
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
    let dry = shared.world.height(wanted.x, wanted.y) >= 0.0;
    let berth = dry.then(|| standing_off(&shared.world, wanted));

    let mut players = shared.players.held();
    let Some(player) = players.get_mut(&from) else {
        // Unreachable from a served connection, whose player is on the roster
        // for as long as it can speak — the same nobody-by-that-id case
        // `spawn` answers, and it needs words here too.
        return Err("you are nowhere the world could take you from".to_string());
    };
    let aboard = player.aboard;

    let (at, heading, told) = match aboard {
        // At a helm: the hull goes, and lies off the shore when the shore is
        // what was asked for. Facing it, in that case — a hull anchored
        // stern-on to the island it was brought to see is no use to whoever
        // asked for it.
        Some(boat) => {
            let mut boats = shared.boats.held();
            let at = berth.map_or(wanted, |(off, _)| off);
            let state = boats.get_mut(&boat).expect("a boat once boarded exists");
            // At rest, which is [`ToClient::PutDown`]'s own rule and now the
            // server's word rather than a thing each client does for itself:
            // a hull that kept its way across a `goto` would sail on from
            // wherever it was set down, and nobody was sailing it there.
            let heading = if berth.is_some() {
                aimed(at, wanted)
            } else {
                state.hull.heading
            };
            state.hull = Underway::lying(at, heading);
            let mut told = vec![state.told(boat)];
            // And the boat on its painter with it, a painter's length
            // astern as the sea lays one behind a ship it moves: the jump
            // is the world's, so the world's own book has to say where the
            // tow lies rather than wait for the client to report it.
            for (id, tow) in boats.iter_mut().filter(|(_, s)| s.towed_by == Some(boat)) {
                tow.hull = Underway::lying(astern(at, heading, protocol::TENDER_ASTERN), heading);
                told.push(tow.told(*id));
            }
            (at, Some(heading), told)
        }
        // Afoot: the point itself, whatever is under it, and which way they
        // face is their own business — see [`ToClient::PutDown`], whose
        // `None` heading this is.
        None => (wanted, None, Vec::new()),
    };
    player.position = at;

    if told.is_empty() {
        broadcast(
            &players,
            from,
            ToClient::Moved {
                id: from,
                position: at,
            },
        );
    }
    // A word about a hull is news to everyone but the client steering it,
    // which is the authority on its own hull and is told where it stands by
    // the put down instead.
    for telling in told {
        broadcast(&players, from, telling);
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
    *put = Some(at);
    Ok(reply)
}

/// Where the asker stands, or nothing at all for an id the roster has never
/// heard of — unreachable from a served connection, whose player is on the
/// roster for as long as it can speak, but every caller needs words for it.
fn stands_at(shared: &Shared, from: PlayerId) -> Option<Vec2> {
    let players = shared.players.held();
    players.get(&from).map(|player| player.position)
}

/// A place, as two numbers: `goto 480 -1200`.
fn point(args: &[&str]) -> Option<Vec2> {
    match args {
        [x, z] => Some(Vec2::new(x.parse().ok()?, z.parse().ok()?)),
        _ => None,
    }
}

/// Where a hull lies for somebody who asked to be taken to dry land: off the
/// nearest shore to the point they named, on [`BERTH_OFFING`].
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
        .map_or(BERTH_OFFING, |spec| spec.extent().length());
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
            // Water: the hull is put down at a berth off it — see
            // [`berth_off`] — so a driven ship is left in water its crew can
            // step off it in wherever the coast allows.
            return (berth_off(shore, *out, |x, z| world.height(x, z)), shore);
        }
    }
    (asked, asked)
}

/// `grant <kind>`: a hull of that kind put in the water for the asker, with
/// nobody aboard — at anchor where the water lets an anchor hold, and
/// otherwise adrift from the moment it is dealt, which is what any empty
/// hull in that water is; see [`crate::sea`].
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
/// Minted every time, a hull being nobody's to bring back: what `grant`
/// adds to the world stays in it, filed and posted to every future joiner,
/// which is the rate the join itself runs at.
fn grant(asked: Asked) -> Result<String, String> {
    let Asked {
        shared, from, args, ..
    } = asked;

    let [named] = args else {
        return Err(format!("`grant` wants a kind of boat — {}", kinds()));
    };
    let Some(&(_, kind)) = HULLS.iter().find(|(name, _)| name == named) else {
        return Err(format!("no boat called `{named}` — {}", kinds()));
    };

    // Where they are, peeked at and given back before the sounding below,
    // which cannot happen under a lock — see what `goto` says about paying
    // for an island. The dealing then takes the roster again and keeps it,
    // so a player who sails on in the moment between is dealt a hull where
    // they asked from rather than where they now are, which the answer's own
    // coordinates own up to.
    let Some(asker) = stands_at(shared, from) else {
        return Err("you are nowhere a boat could reach you".to_string());
    };

    // Sounded before either lock is taken, for the reason `goto` says at
    // length: the ground under a point may have to be generated to answer
    // this, and that is not a wait to hold the world through.
    let offing = (shared.world.height(asker.x, asker.y) >= 0.0)
        .then(|| standing_off(&shared.world, asker).0);
    let at = offing.unwrap_or(asker);
    // Anchored on the terms an asker's own hull is — see `drop_anchor` —
    // where the water allows it, so a dealt hull waits to be boarded rather
    // than leaving on the wind.
    let anchor = protocol::ground::anchor_holds(shared.world.height(at.x, at.y)).then_some(at);
    let now = shared.age();

    {
        // The roster first and the boats under it — the nesting the two
        // locks allow — so that the hull's new state and the telling of it
        // leave together. A telling let go of first is one another
        // connection's boarding can overtake, and then every client but the
        // one steering the hull holds a helm the world has already given
        // away.
        let players = shared.players.held();
        let telling = {
            let mut boats = shared.boats.held();
            let boat = BoatId(keeper::mint());
            let state = BoatState {
                kind,
                hull: Underway::lying(at, 0.0),
                occupant: None,
                towed_by: None,
                anchor,
                vacated: now,
            };
            let telling = state.told(boat);
            boats.insert(boat, state);
            telling
        };
        // Everyone, the asker included: no client is steering this hull, so
        // nobody here is the authority on it that a helmsman would be.
        broadcast_all(&players, telling);
    }

    let lies = match offing {
        // How far they have to go to reach it, which is the whole of what a
        // grant answered from dry land has to tell somebody.
        Some(off) => format!(
            "a {named} is in the water {} m off, at {} {}",
            round(asker.distance(off)),
            round(at.x),
            round(at.y)
        ),
        None => format!("a {named} is here, at {} {}", round(at.x), round(at.y)),
    };
    Ok(match anchor {
        Some(_) => format!("{lies}, at anchor"),
        None => format!("{lies}, adrift in water too deep to anchor"),
    })
}

/// What a hull is called when the world brings it up unasked — a `where`
/// reporting one, where every other mention is the asker's own word handed
/// back.
///
/// A match rather than a lookup in [`HULLS`] so that a kind added to the
/// lineup cannot compile without a word here; that the two agree on the
/// words they do share is `hull_words_match_the_ones_grant_takes`'s.
fn named(kind: BoatKind) -> &'static str {
    match kind {
        BoatKind::Sloop => "sloop",
        BoatKind::Rowboat => "rowboat",
    }
}

/// The kinds a `grant` will take, as its refusals list them.
fn kinds() -> String {
    listed(&names(&HULLS))
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
fn spawn(asked: Asked) -> Result<String, String> {
    let Asked {
        shared, from, args, ..
    } = asked;

    let (named, count) = match args {
        [named] => (named, 1usize),
        [named, count] => match count.parse::<usize>() {
            Ok(count) if (1..=MOST).contains(&count) => (named, count),
            _ => {
                return Err(format!(
                    "`spawn` will raise 1 to {MOST} of a kind, not `{count}`"
                ))
            }
        },
        _ => {
            return Err(format!(
                "`spawn` wants a kind of beast — {}",
                listed(&names(&BEASTS))
            ))
        }
    };
    let Some(&(_, kind)) = BEASTS.iter().find(|(name, _)| name == named) else {
        return Err(format!(
            "no beast called `{named}` — {}",
            listed(&names(&BEASTS))
        ));
    };

    let Some(near) = stands_at(shared, from) else {
        return Err("you are nowhere a beast could join you".to_string());
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
        return Err(format!("no water a {named} could be in near here"));
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
    Ok(format!("{announced}, the nearest {nearest:.0} m away"))
}

/// `where`: the asker's own situation — where they stand, the hull they are
/// at the helm of and what it has in tow, and what the water is doing under
/// them.
///
/// One reading and not three because they are one question. What it gets
/// asked in the middle of is *why will this hull not do what I asked* — a
/// helm that cannot be left, a boat that will not come in tow — and every
/// one of those turns on the three together: which hull, where, and how much
/// water. Three commands would be three round trips and three chances for
/// the world to move between them.
///
/// A reading of the world rather than of the client, because the numbers a
/// client could answer from are the ones it was *sent* — quantised, and only
/// for chunks that have arrived. This side is where
/// [`protocol::ground::ANCHOR_DEPTH`] is actually weighed — in
/// `drop_anchor`, which is what refuses an anchor — so a reading taken
/// anywhere else could disagree with the refusal it is being used to
/// explain. Whether the hook is down is the world's word too, the same one
/// the hull's telling carries.
fn whereabouts(asked: Asked) -> Result<String, String> {
    let Asked { shared, from, .. } = asked;

    // Everything off the roster and the boats in one hold, in the order the
    // crate keeps them, and let go before the world is asked: sounding may
    // mean generating an island, which is the wait `goto` explains and which
    // no lock may be held across.
    let (at, helm, towing) = {
        let players = shared.players.held();
        let Some(player) = players.get(&from) else {
            // Unreachable from a served connection — see [`stands_at`].
            return Err("you are nowhere the world could find you".to_string());
        };
        let (at, aboard) = (player.position, player.aboard);
        let boats = shared.boats.held();
        let helm = aboard.and_then(|boat| {
            boats
                .get(&boat)
                .map(|state| (state.kind, state.anchor.is_some()))
        });
        let towing = aboard.and_then(|boat| {
            boats
                .values()
                .find(|state| state.towed_by == Some(boat))
                .map(|state| (state.kind, state.hull.at))
        });
        (at, helm, towing)
    };
    let height = shared.world.height(at.x, at.y);

    let mut lines = vec![match helm {
        Some((kind, _)) => format!(
            "at the helm of a {} at {} {}",
            named(kind),
            round(at.x),
            round(at.y)
        ),
        None => format!("afoot at {} {}", round(at.x), round(at.y)),
    }];
    // A tenth of a metre where the rest of the console rounds to whole ones:
    // what this line is read for is which side of a mark a hull is on, and
    // both marks it can answer about — the waterline and the anchor's reach
    // — sit close enough together on a shelving coast that a metre of
    // rounding would put the reading on the wrong side of one.
    lines.push(match (helm, height >= 0.0) {
        (Some(_), true) => format!("aground, with {height:.1} m of it out of the water"),
        (Some((_, true)), false) => format!("afloat in {:.1} m, riding at anchor", -height),
        (Some((_, false)), false) if protocol::ground::anchor_holds(height) => {
            format!("afloat in {:.1} m, and an anchor holds here", -height)
        }
        (Some(_), false) => format!("afloat in {:.1} m, too deep to anchor", -height),
        (None, true) => format!("ashore, {height:.1} m above the water"),
        (None, false) => format!("in {:.1} m of water", -height),
    });
    if let Some((kind, lies)) = towing {
        lines.push(format!(
            "towing a {} {} m astern, at {} {}",
            named(kind),
            round(at.distance(lies)),
            round(lies.x),
            round(lies.y)
        ));
    }
    Ok(lines.join("\n"))
}

/// How many islands `islands` names. Enough to pick a shape or a size from,
/// few enough to read at a glance.
const NEAREST_ISLANDS: usize = 8;

/// `islands`: the nearest islands to the asker, nearest first — see
/// [`nearby`]. What a tester needs to look at a seed's islands: the layout
/// is the server's, so the console is the one place the places can be read
/// from, and `goto` takes what this says.
fn islands(asked: Asked) -> Result<String, String> {
    let Asked { shared, from, .. } = asked;
    let Some(at) = stands_at(shared, from) else {
        // Unreachable from a served connection — see [`stands_at`].
        return Err("you are nowhere the world could look out from".to_string());
    };
    let lines = nearby(&shared.world, at);
    if lines.is_empty() {
        return Err("no island anywhere near".to_string());
    }
    Ok(lines.join("\n"))
}

/// One line per island of the [`NEAREST_ISLANDS`] nearest `at`, nearest
/// first: the middle of it as the two numbers `goto` takes, its size, and
/// how far off its frame stands — the frame rather than the shore, that
/// being the honest bound the layout can give without generating anything,
/// see `IslandSpec::frame_point`.
fn nearby(world: &Archipelago, at: Vec2) -> Vec<String> {
    world
        .nearest(at, NEAREST_ISLANDS)
        .iter()
        .map(|spec| {
            let (middle, size) = (spec.centre(), spec.extent());
            format!(
                "{} {} — {} by {} m, {} m to its frame",
                round(middle.x),
                round(middle.y),
                round(size.x),
                round(size.y),
                round(spec.frame_point(at).distance(at))
            )
        })
        .collect()
}

/// The `world` grammar: `world` reads every dial, `world <dial>` reads one,
/// `world <dial> <value>` turns one. The same three forms the client's own
/// `client` lines have, for the same reason — a dial nobody can read is a
/// switch you have to remember the state of.
fn world(asked: Asked) -> Result<String, String> {
    let Asked { shared, args, .. } = asked;
    match args {
        [] => Ok(DIALS
            .iter()
            .map(|dial| (dial.read)(shared))
            .collect::<Vec<_>>()
            .join(" / ")),
        [name] => Ok((dial(name)?.read)(shared)),
        [name, value] => match dial(name)?.turn {
            Some(turn) => turn(shared, value),
            // A dial with a reading and no turn, which is the seed: saying
            // so is the whole reason the turn is an [`Option`] rather than a
            // name the shelf has simply never been told about.
            None => Err(format!("`{name}` is a dial that reads and does not turn")),
        },
        _ => Err("one dial, one value — `world time 6:30`".to_string()),
    }
}

/// The dial a name asks for, or the refusal a name that is not one earns —
/// which is every dial there is, the same answer whether it was read or
/// written to.
fn dial(name: &str) -> Result<&'static Dial, String> {
    DIALS.iter().find(|dial| dial.name == name).ok_or_else(|| {
        let every: Vec<String> = DIALS.iter().map(|dial| dial.name.to_string()).collect();
        format!("the world has no dial called `{name}` — {}", listed(&every))
    })
}

/// `world time <hh:mm>`: the world's clock run forward to the next time it
/// reads that hour — never backwards, which is [`ToClient::Daylight`]'s
/// promise — and everyone told at once rather than on the sky thread's next
/// beat, because the one who asked is watching for it.
fn time(shared: &Shared, given: &str) -> Result<String, String> {
    let Some(target) = parse_clock(given) else {
        return Err(format!(
            "`{given}` is not an hour on a 24-hour clock — `world time 6:30`"
        ));
    };

    let phase = shared.wind_forward_to(target);
    {
        let players = shared.players.held();
        broadcast_all(&players, ToClient::Daylight { phase });
    }
    Ok(format!("the day has run on to {}", clock(phase)))
}

/// `world weather <wind>`: the sky taken in hand for everyone, or —
/// `natural` — given back to the world. No word is sent from here: the sky
/// thread notices the wind moving and tells the roster, exactly as it does
/// when the real weather turns, so an ordered gale arrives the way any gale
/// does.
fn weather(shared: &Shared, given: &str) -> Result<String, String> {
    if given == NATURAL {
        shared.command_wind(None);
        return Ok("the weather is the world's own again".to_string());
    }
    match WINDS.iter().find(|(name, _)| *name == given) {
        Some(&(name, wind)) => {
            shared.command_wind(Some((name, wind)));
            Ok(ordered(name))
        }
        None => {
            let every: Vec<String> = names(&WINDS)
                .into_iter()
                .chain([NATURAL.to_string()])
                .collect();
            Err(format!("no wind called `{given}` — {}", listed(&every)))
        }
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
    use crate::Server;

    /// What a line is answered with, for the tests that care only about the
    /// words — [`Served`]'s other half is the connection's business, and
    /// `goto_takes_a_player_to_a_place_however_they_are_travelling`, over in
    /// the session tests, is where it is looked at.
    fn answer(shared: &Shared, from: PlayerId, line: &str) -> String {
        interpret(shared, from, line).reply
    }

    /// A line's answer as [`serve`] has it, refusal and all — for the tests
    /// that care which of the two a line earned.
    fn ask(shared: &Shared, from: PlayerId, line: &str) -> Result<String, String> {
        serve(shared, from, line, &mut None)
    }

    /// A world to command, never served: `interpret` works on the shared
    /// state alone, so nothing here needs a socket.
    fn a_world(opening: f32) -> std::sync::Arc<Shared> {
        Server::bind(("127.0.0.1", 0), 20_040_112)
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
        for dial in &DIALS {
            let name = dial.name;
            let read = ask(&shared, PlayerId(1), &format!("world {name}"));
            assert!(
                read.is_ok(),
                "`{name}` is listed but does not read: {read:?}"
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

    /// [`named`] is a match and [`HULLS`] a table, and they name the same
    /// hulls: a kind granted as one word and reported as another would read
    /// as two boats.
    #[test]
    fn hull_words_match_the_ones_grant_takes() {
        for (word, kind) in HULLS {
            assert_eq!(named(kind), word, "{kind:?} is granted and reported apart");
        }
    }

    #[test]
    fn where_wants_somebody_to_be() {
        let shared = a_world(0.5);
        // Nobody by this id is in the world, so there is nowhere to sound.
        let reply = answer(&shared, PlayerId(9), "where");
        assert_eq!(reply, "you are nowhere the world could find you");
    }

    #[test]
    fn islands_wants_somebody_to_be() {
        let shared = a_world(0.5);
        let reply = answer(&shared, PlayerId(9), "islands");
        assert_eq!(reply, "you are nowhere the world could look out from");
    }

    #[test]
    fn the_nearest_islands_are_named_as_places_goto_takes() {
        let shared = a_world(0.5);
        let lines = nearby(&shared.world, world::archipelago::ENTRY);
        assert_eq!(lines.len(), NEAREST_ISLANDS);

        // The first line is the first land, the island the welcome faces,
        // and every line leads with a place that is that island's own —
        // the two numbers `goto` wants, cut off the front of the line.
        let first = shared
            .world
            .first_land()
            .expect("a world has islands in it");
        let places: Vec<Vec2> = lines
            .iter()
            .map(|line| {
                let words: Vec<&str> = line.split_whitespace().collect();
                point(&words[..2]).unwrap_or_else(|| panic!("no place leads `{line}`"))
            })
            .collect();
        assert_eq!(places[0], first.centre().round());
        for (place, line) in places.iter().zip(&lines) {
            let island = shared.world.island_at(place.x, place.y);
            let Some(spec) = island.filter(|spec| spec.centre().round() == *place) else {
                panic!("`{line}` names a place that is no island's middle");
            };
            // And the rest of the line is what `help` says it is: the
            // island's size, and how far off its frame stands.
            let words: Vec<&str> = line.split_whitespace().collect();
            let number = |i: usize| -> i32 {
                words[i]
                    .trim_end_matches(',')
                    .parse()
                    .unwrap_or_else(|_| panic!("`{line}` has no number where one was promised"))
            };
            let (size, apart) = (
                spec.extent(),
                spec.frame_point(world::archipelago::ENTRY).length(),
            );
            assert_eq!(
                (number(3), words[4], number(5), words[6]),
                (round(size.x), "by", round(size.y), "m,"),
                "`{line}` does not give the size `help` promises"
            );
            assert_eq!(
                (number(7), &words[8..]),
                (round(apart), &["m", "to", "its", "frame"][..]),
                "`{line}` does not say how far off `help` promises"
            );
        }
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
        // Somewhere on the first island's land: its middle.
        let shared = a_world(0.5);
        let island = shared
            .world
            .first_land()
            .expect("a world has islands in it");
        let inland = island.centre();
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
        let reach = island.extent().length() + BERTH_OFFING;
        assert!(
            off.distance(inland) <= reach,
            "the shore found for {inland} was {off}, {} m away",
            off.distance(inland)
        );
        // And where a hull is anchored it has room to swing before the
        // beach — [`berth_off`]'s own promise, which the sounding line's
        // shore is the point to measure from.
        let depth = shared.world.height(off.x, off.y);
        assert!(
            !world::archipelago::a_berth(depth)
                || off.distance(shore) >= protocol::ground::ANCHOR_SWING,
            "a hull was berthed {} m off the shore, inside a cable's swing",
            off.distance(shore)
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

    /// The alternatives a `help` line offers for a word — `sloop|rowboat`
    /// read off the prose, which is the one part of a command that a table
    /// cannot generate and so the one part that can still drift.
    fn advertised(word: &str) -> Vec<String> {
        help()
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{word} ")))
            .and_then(|line| line.split_whitespace().next())
            .unwrap_or_else(|| panic!("`help` should have a line for `{word}`"))
            .split('|')
            .map(String::from)
            .collect()
    }

    /// Every alternative a `help` line names is one the command serves, and
    /// every one it serves is named — held as sequences rather than by
    /// containment, so an order that reads oddly fails too.
    ///
    /// The direction that can actually break is a word `help` advertises and
    /// the table has never heard of: a client completes a player into it and
    /// the world answers that there is no such thing. Completion itself
    /// cannot drift this way any more — it is the table — which leaves the
    /// prose, and this.
    #[test]
    fn every_word_the_usage_names_is_a_word_that_is_served() {
        assert_eq!(advertised("grant"), names(&HULLS));
        assert_eq!(advertised("spawn"), names(&BEASTS));
        assert_eq!(
            advertised("world weather"),
            names(&WINDS)
                .into_iter()
                .chain([NATURAL.to_string()])
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn every_hull_the_grant_names_can_be_asked_for() {
        let shared = a_world(0.5);
        for asked in ["grant", "grant frigate", "grant sloop rowboat"] {
            let refused = answer(&shared, PlayerId(1), asked);
            assert!(
                HULLS.iter().all(|(named, _)| refused.contains(named)),
                "`{asked}` was answered `{refused}`, which teaches nothing"
            );
        }
    }

    /// The seed reads and cannot be turned, which is the whole of what a
    /// [`Dial`] with no `turn` is for. What it must not answer is that there
    /// is no such dial — the shelf's own miss, and a lie a line after it
    /// printed the number.
    #[test]
    fn the_seed_reads_and_does_not_turn() {
        let shared = a_world(0.5);
        let read = ask(&shared, PlayerId(1), "world seed").expect("the seed should read");
        assert!(
            read.contains(&shared.world.seed().to_string()),
            "the seed read as `{read}`"
        );
        // And a bare `world` carries it, so one line is the whole console's
        // answer to what world this is.
        assert!(answer(&shared, PlayerId(1), "world").contains(&read));

        let refused = ask(&shared, PlayerId(1), "world seed 20040112")
            .expect_err("a seed should not be settable");
        assert!(
            refused.contains("does not turn") && !refused.contains("no dial called"),
            "unhelpful: {refused}"
        );
    }

    /// The tails a client is taught are the words the commands themselves
    /// serve, and they stop where the argument becomes the asker's own.
    #[test]
    fn the_phrases_carry_the_words_and_no_more() {
        let taught = phrases();
        for wanted in [
            "goto",
            "grant sloop",
            "spawn dolphins",
            "world seed",
            "world weather gale",
            "world weather natural",
        ] {
            assert!(taught.iter().any(|it| it == wanted), "`{wanted}` untaught");
        }
        // An hour, a place and a count are nobody's to offer: every phrase is
        // words the console itself knows.
        for phrase in &taught {
            assert!(
                !phrase.contains('<') && !phrase.contains('['),
                "`{phrase}` offers to complete an argument"
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
        for phrase in phrases() {
            let reply = answer(&shared, PlayerId(1), &phrase);
            assert!(
                !reply.contains("is not a command")
                    && !reply.contains("no dial called")
                    && !reply.contains("no boat called")
                    && !reply.contains("no beast called")
                    && !reply.contains("no wind called"),
                "`{phrase}` is advertised but not served: {reply}"
            );
        }
    }

    #[test]
    fn every_line_gets_an_answer() {
        let shared = a_world(0.5);
        let unknown = answer(&shared, PlayerId(1), "dance");
        assert!(unknown.contains("`dance`") && unknown.contains("help"));

        // And every word a client is taught is a word `help` accounts for,
        // both being folds over the one table.
        let help = answer(&shared, PlayerId(1), "help");
        for phrase in phrases() {
            let word = phrase
                .split_whitespace()
                .next()
                .expect("a phrase has a word");
            assert!(help.contains(word), "`help` does not mention {word}");
        }

        // A shelf needs more than its own word said, because a dial is
        // taught as a phrase and completed into: `world tide` offered to
        // every client while `help` says only `world` is exactly the drift
        // the two being folds over one table is supposed to rule out, and
        // the shelf is the one place the table cannot rule it out by itself
        // — a dial's `help` line is prose in the `world` row.
        for dial in &DIALS {
            let advertised = format!("world {}", dial.name);
            assert!(
                help.contains(&advertised),
                "`{advertised}` is taught to clients and `help` does not mention it"
            );
        }
    }
}
