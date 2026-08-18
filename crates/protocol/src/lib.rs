//! The wire between a server and its clients.
//!
//! The world crosses it. A server generates the ocean and hands it out a
//! chunk at a time; a client asks for chunks by coordinate and draws what
//! comes back, and it is told nothing else — not the seed, not the layout,
//! not which chunks are worth asking for. An answer is either open water,
//! which carries no data because the sea and its floor are two flat planes
//! anyone can draw, or ground, which arrives as a [`ground::ChunkPayload`]:
//! corner heights on a fixed grid, one palette entry per triangle, and — on
//! the minority of chunks that hold a lake — the level its water stands at.
//!
//! A deliberate inversion of how this started. A seed used to be a world —
//! every machine regenerating the same ocean bit for bit — which made the
//! client the second half of the generator, and porting it to another language
//! a promise to reproduce every noise octave. Sending the ground instead costs
//! bandwidth and buys a client anyone who can read this file could write.
//!
//! Determinism did not stop mattering, it moved: a seed must still mean the
//! same world wherever it is *hosted*, which now lives entirely in the `world`
//! crate and its digests.
//!
//! Like the world's layout, the wire is a *format*: the bytes each message
//! encodes to are pinned by tests, so that changing what a client receives is
//! something done on purpose rather than noticed later. [`PROTOCOL_VERSION`]
//! is the first thing a client says and the one thing a server may refuse,
//! but it is not bumped for every encoding change — there is no fleet of
//! older clients to turn away, only whatever was built from this checkout.

pub mod ground;
pub mod survey;

use std::io::{self, Read, Write};

use glam::{IVec2, Vec2};

pub use ground::{ChunkPayload, Shade, Surface, Tone};

/// The dialect spoken here. A client leads with it in [`ToServer::Hello`],
/// and a server that speaks a different one answers [`ToClient::Refused`]
/// and hangs up — which is the whole of version negotiation.
pub const PROTOCOL_VERSION: u16 = 14;

/// How long one turn of the world's day takes, in seconds — sunrise to
/// sunrise, ten minutes of it.
///
/// Part of the wire rather than each client's own idea, because a client
/// runs the clock itself between the server's tellings — see
/// [`ToClient::Daylight`] — and one running it at its own pace would drift
/// from the sky everyone else is under.
pub const DAY_SECONDS: f32 = 600.0;

/// The phase the last of the light has gone by, and [`DAYBREAK`] the phase
/// the first of it returns at — between them, the night.
///
/// Both ends need to agree on where the night is and neither can be talked
/// out of it: a server runs the clock fast through a night its players are
/// waiting out (see [`ToServer::WantDawn`]) and stops at [`DAYBREAK`], and a
/// client only offers to wait when there is a night to wait. Set a little
/// inside the dark on either side, so the night is the part with nothing to
/// look at rather than the whole of dusk and dawn.
pub const NIGHTFALL: f32 = 0.79;
/// Where the night ends — see [`NIGHTFALL`]. Ahead of the sunrise at 0.25,
/// so a night waited out ends looking at one.
pub const DAYBREAK: f32 = 0.22;

/// Whether a phase of the day (see [`ToClient::Daylight`]) falls in the
/// night — the span from [`NIGHTFALL`] round midnight to [`DAYBREAK`].
pub fn is_night(phase: f32) -> bool {
    !(DAYBREAK..NIGHTFALL).contains(&phase)
}

/// A phase of the day as a time on a twenty-four hour clock — `0.0` midnight,
/// `0.25` six in the morning.
///
/// Here rather than on either side because both ends say the hour out loud —
/// a server answering `time 18:00` at its console, a client's own readout —
/// and two spellings of it would have one session disagreeing with itself
/// about what time it is. What a phase *means* is this crate's, the same way
/// [`is_night`] is.
///
/// Rounded to the minute rather than truncated, and folded back into the day
/// after: an hour that is a hair under the minute it means — which is what a
/// phase written as a decimal usually is — should read as that minute.
pub fn clock(phase: f32) -> String {
    let minutes = (phase.rem_euclid(1.0) * 24.0 * 60.0).round() as u32 % (24 * 60);
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// The port a server listens on, and a client joins on, unless told
/// otherwise. Nothing else claims it, and it is easily remembered as the
/// powers of two run together.
pub const DEFAULT_PORT: u16 = 24816;

/// The most letters an island's name may run to — what a client stops taking
/// at, and the number [`NAME_BYTES`] is worked out from.
///
/// A chart has room for a real name and not for a sentence, and the cap is
/// what keeps one island's lettering off its neighbour's. It lives here rather
/// than on the sheet that draws it because a client that let somebody type
/// past it would be a client offering names the wire then refused, with
/// nothing on screen to say why — see the note on [`NAME_BYTES`] about which
/// of the two counts what.
pub const NAME_LETTERS: usize = 24;

/// The longest an island's name may be, in bytes — see [`island_name`].
///
/// Bytes rather than characters because bytes are what the wire counts and
/// what a frame is measured in, and the thing being bounded is what a hostile
/// client can make a server hold and hand on. Derived from [`NAME_LETTERS`] at
/// the four bytes a character costs in the worst case UTF-8 has, rather than
/// written down beside it: a name of two dozen letters fits whatever alphabet
/// it is written in, and the two numbers saying so were only ever kept in step
/// by a comment.
pub const NAME_BYTES: usize = NAME_LETTERS * 4;

/// A name for an island as the wire will carry it, or `None` for something
/// that is not a name at all.
///
/// The one place the rule lives, because both ends need it and neither may be
/// the one that decides: a client offers it while a player types, and a server
/// applies it again to whatever actually arrives — the client that sends a
/// name is the one thing here nobody controls.
///
/// Refused rather than repaired, in every case. A name trimmed to fit is half
/// of somebody's word standing on their island for good; a name with its
/// control characters filtered out is a different name from the one that was
/// typed; and an empty one is somebody asking for nothing, which is not the
/// same as asking for the name to be taken away. So the answer is either the
/// name or nothing, and a refusal leaves what was there before.
///
/// Control characters go because a name is a line of text in the end — the
/// world's own file writes one per line, and a chart draws it in one — and
/// because nothing typed at a keyboard has a newline in the middle of it.
pub fn island_name(raw: &str) -> Option<String> {
    let name = raw.trim();
    let carried = !name.is_empty()
        && name.len() <= NAME_BYTES
        && !name.chars().any(|letter| letter.is_control());
    carried.then(|| name.to_string())
}

/// The longest frame a server will accept from a client. Everything a client
/// says is a couple of dozen bytes — where it is, or which chunk it wants —
/// except a console line, which is as long as whatever was typed and gets the
/// room a sentence needs. The ceiling exists so that a corrupt length prefix
/// reads as corruption instead of as a request to buffer megabytes.
const MAX_CLIENT_FRAME: u16 = 512;

/// How many bytes of survey one [`ToClient::Surveyed`] carries.
///
/// A batch rather than a chunk at a time, because a returning player is told
/// back a whole voyage's worth of it at the door and a message each would be
/// thousands of messages — deeper than any outbox is, and the server's answer
/// to a client that far behind is to hang up on it.
///
/// A budget in bytes rather than a count of chunks, because chunks are not
/// alike: most of a voyage is open water, which is twelve bytes of "surveyed,
/// and nothing on it", while a chunk of broken coast is many times that.
/// Eight kilobytes is several hundred chunks of the common kind, so an hour's
/// sailing crosses in a handful of messages, and one island's coast in one.
pub const SURVEY_BATCH_BYTES: usize = 8 * 1024;

/// How many bytes one chunk's entry in a [`ToClient::Surveyed`] takes: where
/// it is, and what was found there.
///
/// Public because a server fills a batch to [`SURVEY_BATCH_BYTES`] by it, and
/// the budget being filled and the bytes that actually go had better be one
/// arithmetic.
pub fn surveyed_bytes(found: &survey::Soundings) -> usize {
    8 + found.bytes()
}

/// The longest frame a client will accept from a server: the larger of the
/// two answers big enough to be worth measuring, one ceiling having to cover
/// the whole direction.
///
/// One is a chunk of ground, and it is exactly that and not a byte more: its
/// tag, its coordinates, the flag that says what kind of answer this is, the
/// count of plants growing on it, and the payload — the largest kind, which
/// is ground with standing water on it and a full complement of plants.
///
/// The other is a batch of survey, filled to [`SURVEY_BATCH_BYTES`] — except
/// that one chunk's soundings are never split across two messages, so a chunk
/// more torn than the whole budget still has to travel whole. That is what
/// [`survey::SOUNDINGS_BYTES`] is for, and it is the term that wins: the
/// ceiling stands where a coast crossing every facet of a chunk would put it,
/// rather than where a real coast happens to.
///
/// Derived rather than picked, so that a message which outgrew it fails to
/// send here instead of arriving as garbage — and so that "how much can one
/// answer cost" has one answer, written down.
const MAX_SERVER_FRAME: u16 = {
    let ground = 1 + 8 + 1 + 1 + ground::payload_bytes(true, ground::MAX_PLANTS);
    let batch = if SURVEY_BATCH_BYTES > 8 + survey::SOUNDINGS_BYTES {
        SURVEY_BATCH_BYTES
    } else {
        8 + survey::SOUNDINGS_BYTES
    };
    let surveyed = 1 + 2 + batch;
    (if ground > surveyed { ground } else { surveyed }) as u16
};

/// A player, as the server counts them: dealt out in joining order, never
/// reused within a session, meaningless across sessions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PlayerId(pub u32);

impl std::fmt::Display for PlayerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "player {}", self.0)
    }
}

/// A world, as distinct from a seed: minted at random when a world is first
/// made, and carried by it for life — through saves, rehosts and changes of
/// address. Two worlds grown from one seed are two worlds with two histories,
/// and this is the fact that tells them apart.
///
/// It crosses the wire when the seed never does because a client needs to
/// know *which world this is* without being told how to make it: what a
/// client keeps of a world — its chart, its token — has to be keyed on
/// something, and the server's address cannot be it, since a world moved to
/// another host is meant to still be the same world.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WorldId(pub u64);

/// Spelled as sixteen hex digits — the form a client's files for a world are
/// named in, chosen because it is fixed-width and legal in a filename on
/// every platform.
impl std::fmt::Display for WorldId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

impl std::str::FromStr for WorldId {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        u64::from_str_radix(s, 16).map(Self)
    }
}

/// One player's standing in one world: dealt by the server on a first visit
/// (see [`ToClient::Welcome`]), presented on the next (see
/// [`ToServer::Papers`]), and the whole of how a returning player is known.
/// There are no accounts; holding the token *is* being that player, to that
/// world. A secret between one client's files and one world's, so it has no
/// `Display` — nothing anywhere should be printing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Token(pub u64);

/// A beast, as the server counts them: dealt out as players are — in order of
/// appearance, never reused within a session, meaningless across sessions.
///
/// A beast is the third kind of thing in a world, after the ground and the
/// players: a creature the server *means*. It earns the wire two ways. The
/// shark's way is consequence — a creature that will one day act on a player
/// has to be the same creature on every machine. The whale's is company: a
/// whale is a rare, pointable event, and "look, a whale!" only lands if the
/// player being shown one is under the same sea. What stays off the wire is
/// the texture nobody compares notes on — birds, which each client dreams up
/// for itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BeastId(pub u32);

impl std::fmt::Display for BeastId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "beast {}", self.0)
    }
}

/// What a beast is, which is the whole of what a client is told beyond where
/// it is and where it is going. What one looks like, how deep it swims, how
/// its body moves — all drawing, and all the client's business.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BeastKind {
    /// A shark: lives in the shallows around islands, cruises them endlessly,
    /// and is the reason beasts exist at all — the first creature that will
    /// one day act on a player rather than decorate their view.
    Shark,
    /// A pod of dolphins. One beast, several animals: the wire carries the
    /// pod as a single position and velocity, and a client draws the members
    /// arranged around it — how many and in what order they arc being
    /// drawing, though a client that wants its pod to match everyone else's
    /// can (and this workspace's does) deal those choices from the id, which
    /// every machine was told.
    Dolphins,
    /// A whale: deep water, a long back that barely clears the surface, and
    /// the sea's one event worth turning a boat for.
    Whale,
}

impl BeastKind {
    fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Shark),
            1 => Some(Self::Dolphins),
            2 => Some(Self::Whale),
            _ => None,
        }
    }

    fn byte(self) -> u8 {
        match self {
            Self::Shark => 0,
            Self::Dolphins => 1,
            Self::Whale => 2,
        }
    }
}

/// A boat, as the server counts them — and unlike a player's or a beast's,
/// a *lasting* name: minted when the boat first touches the water and
/// written in the world's file for the boat's whole life, because a
/// player's record says which boat they were last aboard and has to still
/// mean it next session.
///
/// A boat is a vehicle, not a part of any player. It outlives visits, lies
/// at anchor wherever its last helmsman left it — visible to everyone,
/// including while that player is offline — and its helm belongs to whoever
/// reaches it first. See [`ToClient::Boat`] for how one is told, and
/// [`ToServer::Board`] for how one changes hands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BoatId(pub u64);

impl std::fmt::Display for BoatId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "boat {:016x}", self.0)
    }
}

/// What a boat is, which — as with the beasts — is the whole of what a
/// client is told beyond where it stands and who is aboard. One kind today;
/// the lineup this is the seam for runs from a rowing boat to a
/// square-rigger, and each lands as a byte here when it becomes a vehicle
/// rather than a model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BoatKind {
    /// A small Bermuda sloop: the boat every player's story starts aboard.
    Sloop,
}

impl BoatKind {
    fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Sloop),
            _ => None,
        }
    }

    fn byte(self) -> u8 {
        match self {
            Self::Sloop => 0,
        }
    }
}

/// What a client may say.
///
/// Positions are metres on the world's ground plane, as everywhere else in
/// the workspace. Height is never sent: a player stands on the ground the
/// server sent them, so the server can put them back on it.
#[derive(Clone, Debug, PartialEq)]
pub enum ToServer {
    /// The first message on any connection, and never sent again.
    ///
    /// A server that speaks this version answers with which world this is
    /// ([`ToClient::World`]), waits for the client's [`ToServer::Papers`],
    /// and only then grants the session — so the handshake is four words
    /// long, two in each direction, and ends with [`ToClient::Welcome`].
    Hello { version: u16 },
    /// The client's papers for this world, shown once the server has said
    /// which world it is: the token dealt on an earlier visit, or nothing,
    /// which is what being new here looks like.
    ///
    /// Always the second thing a client says, and the server waits for it
    /// before any welcome — a returning player re-enters as themselves,
    /// where they left off, rather than as a stranger on the spawn. A token
    /// the server does not recognise is not an offence, just a stranger
    /// after all: the world's memory and the client's can part ways
    /// honestly, a world file lost or the world rebuilt, and the join
    /// simply starts them afresh.
    Papers { token: Option<Token> },
    /// Where the player now is, on their own feet. A player at a helm says
    /// [`ToServer::Helm`] instead — the server carries a rider with their
    /// vehicle, and a `Move` from somebody aboard would be a player walking
    /// away from a boat they are still steering.
    Move { position: Vec2 },
    /// Where this player has sailed the boat they occupy: its position and
    /// its heading, a yaw about the vertical. The boat and the player move
    /// together — the wire carries no separate word for the rider, a rider
    /// being wherever their vehicle is.
    ///
    /// Quietly ignored from a player occupying nothing, rather than fatal:
    /// a boarding the server refused can cross a helm report already on the
    /// wire, and neither end has done anything wrong.
    Helm { position: Vec2, heading: f32 },
    /// Asks for a boat's helm. Granted when the boat is unoccupied and the
    /// player is near it, and answered either way by a [`ToClient::Boat`]
    /// telling whose `occupant` says how it came out — there is no separate
    /// refusal, the boat's state being the whole of the answer. Whoever
    /// asks first is aboard: boats have keepers, not owners.
    Board { boat: BoatId },
    /// Steps off the occupied boat, landing at `position` — the walker's
    /// own spot, chosen by the client that judged the footing. The boat
    /// stays where it lies, at anchor for anyone.
    Disembark { position: Vec2 },
    /// Claims the island of this identity — see [`survey::Island::id`], which
    /// is what a client names it by.
    ///
    /// Granted to a player who is standing on that island having sailed the
    /// whole way round it, and to nobody else: the server asks
    /// [`survey::Survey::island_under`] of *its own* survey for this player,
    /// which answers both halves at once, and grants only if the island it
    /// finds is the one named and nobody has claimed it already. The claim is
    /// settled against the world's record of where this player has been, never
    /// against the client's assertion of it.
    ///
    /// A grant raises a cairn where the claimant stands, and everybody near
    /// enough to see it is told — see [`ToClient::Cairn`], which is the whole
    /// of the answer: what the asker hears is either the cairn they have just
    /// raised or the one that was already there. A claim refused for any other
    /// reason is answered by silence, the world being exactly as the asker
    /// last heard it.
    Claim { island: IVec2 },
    /// Christens a claimed island. Granted only to the holder of the claim,
    /// and the name then rides with the cairn for everyone who passes it.
    ///
    /// There is no such thing as a private name any more: a name is something
    /// the world carries, so it is earned the way the island was. A name that
    /// is empty, all whitespace, over-long or not text at all is a refusal
    /// rather than an erasure — see [`island_name`], which is the rule — and a
    /// refusal leaves the cairn saying whatever it said before.
    Name { island: IVec2, name: String },
    /// Ground, please — one chunk of it, named by its coordinate on the world
    /// grid of [`ground::CHUNK_METRES`] squares.
    ///
    /// A client asks for every chunk near its camera without knowing, or
    /// being able to know, which of them hold land. Answers come back as
    /// [`ToClient::Chunk`] and may arrive in any order: an island takes real
    /// time to generate and open water takes none, so a request for water
    /// posted after one for land will often be answered first.
    WantChunk { chunk: IVec2 },
    /// This player is lying to at anchor waiting the night out, and would
    /// like it over with.
    ///
    /// A standing request rather than an order: it says what this player
    /// wants *now*, and lapses within a beat of the client falling silent,
    /// so taking the helm again is simply a client that has stopped asking.
    /// What the server does with it is its own business — the night runs
    /// fast only while everyone in the world is asking, and a world where
    /// somebody is still sailing keeps its night. Repeat it while the wish
    /// stands; a client that sends it once and stops has changed its mind.
    WantDawn,
    /// A debug-console line for the server to interpret: whatever the player
    /// typed, verbatim.
    ///
    /// Deliberately opaque. The vocabulary — `spawn shark`, `time 6:00` —
    /// belongs to the *server* and may grow without this crate hearing about
    /// it: a client has no parsing to do and nothing to know, which keeps a
    /// client written in any language as capable as the newest server it
    /// talks to. `help` is the vocabulary's own index, and the server
    /// answers every line — the ones it did not understand included — with a
    /// [`ToClient::Reply`] to the asker alone.
    ///
    /// On the wire the line is a u16 byte count and that many bytes of
    /// UTF-8.
    Command { line: String },
}

/// What a server may say.
#[derive(Clone, Debug, PartialEq)]
pub enum ToClient {
    /// The session, granted: who the client is, where the world is entered,
    /// which way to look when it opens, and the token this player holds the
    /// world by.
    ///
    /// The seed is not here, and that is the point — a client has no use for
    /// one, having nothing to generate. `facing` is a ground point the view
    /// opens towards, so that a player arrives looking at the island they
    /// were put down beside rather than out to sea; a server with nothing in
    /// particular to look at sends the spawn itself, which names no direction.
    /// For a returning player (see [`ToServer::Papers`]) the spawn is not the
    /// world's but their own — wherever the world last saw them.
    ///
    /// `token` is the presented one if the server recognised it, and a fresh
    /// one dealt on the spot if not. A client keeps whatever arrives here,
    /// filed under the world's id: it is the name the next visit will be
    /// known by.
    ///
    /// `aboard` is the boat this player holds the helm of as they enter —
    /// the one the world minted for their arrival, or the one it seated
    /// them back into. `None` is a player entering on their own feet: they
    /// left ashore, and their boat lies wherever they left it, one telling
    /// among the [`ToClient::Boat`]s that follow the welcome.
    Welcome {
        id: PlayerId,
        spawn: Vec2,
        facing: Vec2,
        token: Token,
        aboard: Option<BoatId>,
    },
    /// The version the server speaks, sent instead of a welcome when the
    /// client's is not it. The connection closes after.
    Refused {
        version: u16,
    },
    /// Which world this is — the answer to a hello, and the question the
    /// papers answer. Sent before the welcome, not with it, because it is
    /// what the client needs in order to say who it is here: its token for
    /// this world, if it holds one, is filed under this id.
    World {
        id: WorldId,
    },
    /// Someone is in the world: sent once for each player already present
    /// when a client joins, and to everyone else when one arrives.
    Joined {
        id: PlayerId,
        position: Vec2,
    },
    /// A player moved. Never echoed back to the player who did.
    Moved {
        id: PlayerId,
        position: Vec2,
    },
    Left {
        id: PlayerId,
    },
    /// The ground a client asked for, or the absence of it: `None` is open
    /// water, which needs no data — the sea and the ocean floor are flat
    /// planes, and a client draws them whether or not anything is on top.
    ///
    /// Most answers are `None`. An island is laid out as a rectangle with a
    /// skirt of open water round it, and between islands there is nothing at
    /// all, so a camera's neighbourhood is mostly sea.
    ///
    /// Ground that carries a lake costs half as much again — see
    /// [`ChunkPayload::water`] — and is a small minority of the ground.
    Chunk {
        chunk: IVec2,
        ground: Option<ChunkPayload>,
    },
    /// What the weather is doing: the wind over the whole world, as a
    /// velocity — direction and metres per second in one vector, so there is
    /// no bearing convention to agree on and a calm is simply a short one.
    ///
    /// Sent once directly after [`ToClient::Welcome`], and again to everyone
    /// whenever it has changed enough to matter. The server is the one
    /// authority on it, exactly as with the ground: weather moves over a
    /// session, every player in a world is under the same sky, and what a
    /// client does with it — how a sea wears this much wind — is drawing,
    /// not simulation.
    Weather {
        wind: Vec2,
    },
    /// Where the world's day stands: the fraction of it since midnight, so
    /// 0.0 is midnight, 0.25 sunrise, 0.5 noon and 0.75 sunset. Always in
    /// `0.0..1.0`.
    ///
    /// Sent with the welcome and on a beat of a second or so after that, and
    /// the server is the authority on it exactly as it is on the weather:
    /// every player in a world is under the same sun, wherever they are and
    /// whatever their machine thinks the time is.
    ///
    /// A client is expected to run the clock itself between tellings, at
    /// [`DAY_SECONDS`] to the turn, and to ease onto each new word rather
    /// than snapping to it — the tellings are a beat apart and a sun that
    /// jumped every second would be a stuttering one. It never runs
    /// backwards: the two things that move it other than the clock are a
    /// night being waited out (see [`ToServer::WantDawn`]), which the server
    /// serves by running the same clock fast, and a console `time` command
    /// (see [`ToServer::Command`]), which the server serves by running the
    /// clock forward to the *next* occurrence of the asked-for hour. So a
    /// step may be most of a day at once, but it is always a day getting
    /// older — and since the phase wraps at midnight even when time is only
    /// passing, a client easing the shortest way round the circle already
    /// draws every step there is.
    Daylight {
        phase: f32,
    },
    /// A beast, wherever it has got to: one message is both the introduction
    /// and every movement after it, so the first word a client hears about a
    /// beast is enough to draw it and a later word only moves it. That is
    /// deliberately not the players' joined-then-moved pair — a beast needs no
    /// introducing beyond what every telling carries, so a client arriving
    /// mid-session, or wandering into a beast's waters, is caught up by the
    /// next beat with no one having to remember what it has been told.
    ///
    /// `velocity` is where it is going, as the weather gives the wind:
    /// bearing and metres per second in one vector, no convention to agree
    /// on. It is what a client draws between tellings — pointing the body,
    /// carrying it forward, working the tail at the pace of the water going
    /// by.
    ///
    /// A height is still never sent, as it is not for players: how deep a
    /// shark rides, and when its fin cuts the surface, is drawing. What
    /// `surfaced` carries is not a depth but a *decision* — whether this
    /// animal is currently showing itself. A whale that has gone down because
    /// a boat came near did that about a player, and behaviour about a player
    /// is the server's or it is a thing one client saw and another did not;
    /// the same flag is how a beast that has finished its life is under the
    /// water before it stops being told of, so nothing is ever watched
    /// blinking out. How far down "not showing" is, and how the animal gets
    /// there, remain a client's own business entirely.
    Beast {
        id: BeastId,
        kind: BeastKind,
        position: Vec2,
        velocity: Vec2,
        surfaced: bool,
    },
    /// The server has stopped minding a beast — everyone has left its waters,
    /// and there is nothing it could matter to. Not a death; the sea is
    /// simply emptier by one.
    BeastGone {
        id: BeastId,
    },
    /// A boat, wherever it lies: one message is both the introduction and
    /// every change after, as with the beasts — a client keys its hulls by
    /// id and redraws whatever a telling moves. Sent for every boat when a
    /// client joins, on every change of hands, and as an occupied boat's
    /// helm reports come in; an unoccupied boat is telling-quiet, lying
    /// exactly where its last telling left it.
    ///
    /// `heading` is a yaw about the vertical — how the hull is pointed,
    /// which is drawing the wire must carry: a client eases another
    /// player's boat between tellings, and a hull is not a capsule that
    /// looks the same from every side.
    Boat {
        id: BoatId,
        kind: BoatKind,
        position: Vec2,
        heading: f32,
        occupant: Option<PlayerId>,
    },
    /// The server's answer to a [`ToServer::Command`], sent to the player
    /// who typed it and nobody else: plain text for the console the line
    /// was typed into, whether the command was served or not understood.
    /// Never sent unasked, so a client that types nothing never hears one.
    ///
    /// On the wire the text is a u16 byte count and that many bytes of
    /// UTF-8, exactly as the command it answers travelled the other way.
    Reply {
        text: String,
    },
    /// The server's console vocabulary: the first word of every line its
    /// console serves, sent once after [`ToClient::Welcome`].
    ///
    /// Hints, not grammar. A console can offer these while a player types —
    /// completion is what this exists for — but a line still crosses the wire
    /// verbatim as [`ToServer::Command`] whether it starts with one of these
    /// or not, and the server still answers every line either way. A client
    /// that ignores this message loses nothing but the hinting, which is how
    /// the vocabulary stays the server's to grow.
    ///
    /// On the wire: a u8 count of verbs, then each verb as a u16 byte count
    /// and that many bytes of UTF-8.
    Vocabulary {
        verbs: Vec<String>,
    },
    /// Coast this player has surveyed: a batch of chunks, each with what one
    /// walk of its ground found — see [`survey`], which is what a survey *is*
    /// and is shared by both ends for the reason written there.
    ///
    /// The survey belongs to the world, not to the client. The server holds
    /// which chunks each player has been near enough to look at, works out
    /// what is on them, and says so: on joining, everything they had surveyed
    /// before, and after that a batch whenever sailing brings more within
    /// [`survey::SIGHT_RADIUS`]. A client records what it is told and draws
    /// it, and has no rule of its own about what counts as seen — which is
    /// what lets a claim be the server's to grant off the same coastline the
    /// player is looking at.
    ///
    /// An entry with no runs in it is not nothing: it is a chunk surveyed and
    /// found blank, which is most of an ocean and is worth saying, being the
    /// difference between water somebody has crossed and water nobody has.
    ///
    /// A batch because a voyage's worth arrives at once — see
    /// [`SURVEY_BATCH_BYTES`] for how much of it goes in a message and why it
    /// is measured in bytes. On the wire: a u16 count of chunks, then each as
    /// its coordinates and its soundings, the runs of the waterline and then
    /// of the shoal line, each run a flag saying whether it closes, a u16
    /// count of marks, and the marks as the two bytes they are.
    Surveyed {
        found: Vec<(IVec2, survey::Soundings)>,
    },
    /// A cairn: a heap of stones standing where somebody claimed an island,
    /// which is how anybody else finds out it is spoken for.
    ///
    /// One message is both the introduction and every change after, as with
    /// the beasts and the boats — a client keys cairns by the island they
    /// stand for and redraws whatever a telling says. Sent when one comes
    /// within sight of a player and again when they come near enough to read
    /// it, to everyone near enough when one is raised or renamed, to a joining
    /// player for every one they already knew of, and to a claim this server
    /// refused because the island was already somebody's, which is how an
    /// asker's picture corrects itself.
    ///
    /// `at` is where the claimant stood, in metres, and the cairn stands there
    /// for good: an island is claimed by a person in a place, not by a
    /// calculation about its middle.
    ///
    /// `name` is what the island is called, and empty for one nobody has
    /// christened yet — see [`ToServer::Name`]. It is **also** empty for one
    /// this hearer has not been near enough to read, and a client cannot tell
    /// those apart. That is the point rather than a shortcoming: knowledge here
    /// is gated on having been there, and being able to distinguish *there is a
    /// word here you may not read* from *there is no word here* would be
    /// knowing something about a place nobody has visited. Both are stones with
    /// nothing legible on them, which is what standing off a coast shows you.
    ///
    /// `yours` says whether this cairn is the hearer's own doing. It is a fact
    /// about the hearer rather than about the cairn, so the same cairn goes
    /// out with different answers to different players. What is *not* here is
    /// the [`Token`] that actually holds the claim: a token is a credential,
    /// and it goes to nobody but the client that holds it.
    Cairn {
        island: IVec2,
        at: Vec2,
        name: String,
        yours: bool,
    },
}

impl ToServer {
    /// Puts the message on the wire, whole: one frame, one write.
    pub fn write(&self, to: &mut impl Write) -> io::Result<()> {
        let mut payload = Vec::new();
        match self {
            Self::Hello { version } => {
                payload.push(0);
                put_u16(&mut payload, *version);
            }
            Self::Move { position } => {
                payload.push(1);
                put_vec2(&mut payload, *position);
            }
            Self::WantChunk { chunk } => {
                payload.push(2);
                put_ivec2(&mut payload, *chunk);
            }
            Self::WantDawn => payload.push(3),
            Self::Command { line } => {
                payload.push(4);
                put_str(&mut payload, line);
            }
            Self::Papers { token } => {
                payload.push(5);
                // A flag byte rather than a reserved value, because every
                // u64 is a token somebody could legitimately hold.
                match token {
                    None => payload.push(0),
                    Some(token) => {
                        payload.push(1);
                        put_u64(&mut payload, token.0);
                    }
                }
            }
            Self::Helm { position, heading } => {
                payload.push(6);
                put_vec2(&mut payload, *position);
                put_f32(&mut payload, *heading);
            }
            Self::Board { boat } => {
                payload.push(7);
                put_u64(&mut payload, boat.0);
            }
            Self::Disembark { position } => {
                payload.push(8);
                put_vec2(&mut payload, *position);
            }
            Self::Claim { island } => {
                payload.push(9);
                put_ivec2(&mut payload, *island);
            }
            Self::Name { island, name } => {
                payload.push(10);
                put_ivec2(&mut payload, *island);
                put_str(&mut payload, name);
            }
        }
        write_frame(to, &payload, MAX_CLIENT_FRAME)
    }

    /// Reads the next message, blocking until a whole frame has arrived.
    pub fn read(from: &mut impl Read) -> io::Result<Self> {
        let frame = read_frame(from, MAX_CLIENT_FRAME)?;
        let mut payload = Payload::over(&frame);
        let message = match payload.u8()? {
            0 => Self::Hello {
                version: payload.u16()?,
            },
            1 => Self::Move {
                position: payload.vec2()?,
            },
            2 => Self::WantChunk {
                chunk: payload.ivec2()?,
            },
            3 => Self::WantDawn,
            4 => Self::Command {
                line: payload.str()?,
            },
            5 => Self::Papers {
                token: match payload.u8()? {
                    0 => None,
                    1 => Some(Token(payload.u64()?)),
                    flag => return Err(corrupt(format!("papers flagged {flag}"))),
                },
            },
            6 => Self::Helm {
                position: payload.vec2()?,
                heading: payload.f32()?,
            },
            7 => Self::Board {
                boat: BoatId(payload.u64()?),
            },
            8 => Self::Disembark {
                position: payload.vec2()?,
            },
            9 => Self::Claim {
                island: payload.ivec2()?,
            },
            10 => Self::Name {
                island: payload.ivec2()?,
                // Read as whatever text it is, and judged where it is acted
                // on: what a name may say is the world's business — see
                // [`island_name`] — where this layer's business is only that
                // it arrived as text at all.
                name: payload.str()?,
            },
            tag => return Err(corrupt(format!("unknown client message tag {tag}"))),
        };
        payload.finish()?;
        Ok(message)
    }
}

impl ToClient {
    /// Puts the message on the wire, whole: one frame, one write.
    pub fn write(&self, to: &mut impl Write) -> io::Result<()> {
        let mut payload = Vec::new();
        match self {
            Self::Welcome {
                id,
                spawn,
                facing,
                token,
                aboard,
            } => {
                payload.push(0);
                put_u32(&mut payload, id.0);
                put_vec2(&mut payload, *spawn);
                put_vec2(&mut payload, *facing);
                put_u64(&mut payload, token.0);
                match aboard {
                    None => payload.push(0),
                    Some(boat) => {
                        payload.push(1);
                        put_u64(&mut payload, boat.0);
                    }
                }
            }
            Self::Refused { version } => {
                payload.push(1);
                put_u16(&mut payload, *version);
            }
            Self::World { id } => {
                payload.push(12);
                put_u64(&mut payload, id.0);
            }
            Self::Joined { id, position } => {
                payload.push(2);
                put_u32(&mut payload, id.0);
                put_vec2(&mut payload, *position);
            }
            Self::Moved { id, position } => {
                payload.push(3);
                put_u32(&mut payload, id.0);
                put_vec2(&mut payload, *position);
            }
            Self::Left { id } => {
                payload.push(4);
                put_u32(&mut payload, id.0);
            }
            Self::Weather { wind } => {
                payload.push(6);
                put_vec2(&mut payload, *wind);
            }
            Self::Daylight { phase } => {
                payload.push(7);
                put_f32(&mut payload, *phase);
            }
            Self::Beast {
                id,
                kind,
                position,
                velocity,
                surfaced,
            } => {
                payload.push(8);
                put_u32(&mut payload, id.0);
                payload.push(kind.byte());
                payload.push(u8::from(*surfaced));
                put_vec2(&mut payload, *position);
                put_vec2(&mut payload, *velocity);
            }
            Self::BeastGone { id } => {
                payload.push(9);
                put_u32(&mut payload, id.0);
            }
            Self::Boat {
                id,
                kind,
                position,
                heading,
                occupant,
            } => {
                payload.push(13);
                put_u64(&mut payload, id.0);
                payload.push(kind.byte());
                put_vec2(&mut payload, *position);
                put_f32(&mut payload, *heading);
                match occupant {
                    None => payload.push(0),
                    Some(player) => {
                        payload.push(1);
                        put_u32(&mut payload, player.0);
                    }
                }
            }
            Self::Reply { text } => {
                payload.push(10);
                put_str(&mut payload, text);
            }
            Self::Vocabulary { verbs } => {
                payload.push(11);
                payload.push(verbs.len() as u8);
                for verb in verbs {
                    put_str(&mut payload, verb);
                }
            }
            Self::Cairn {
                island,
                at,
                name,
                yours,
            } => {
                payload.push(15);
                put_ivec2(&mut payload, *island);
                put_vec2(&mut payload, *at);
                payload.push(u8::from(*yours));
                // Last, being the one field whose length is not the same for
                // every cairn — so everything a reader needs in order to read
                // it stands in front of it.
                put_str(&mut payload, name);
            }
            Self::Surveyed { found } => {
                payload.push(14);
                put_u16(&mut payload, found.len() as u16);
                for (chunk, soundings) in found {
                    put_ivec2(&mut payload, *chunk);
                    soundings.put(&mut payload);
                }
            }
            Self::Chunk { chunk, ground } => {
                payload.push(5);
                put_ivec2(&mut payload, *chunk);
                match ground {
                    // A flag byte rather than three tags, so that what kind of
                    // answer this is is one thing a reader tests and the
                    // coordinates are in the same place whichever it is. It
                    // says how long the rest of the message is, which is why
                    // ground with water on it is a kind of answer here rather
                    // than a detail inside the payload.
                    None => payload.push(0),
                    Some(ground) => {
                        payload.push(if ground.water.is_some() { 2 } else { 1 });
                        // Alongside the flag rather than inside the payload,
                        // and for the same reason: between them they say how
                        // long the rest of the message is, which is what a
                        // reader needs before it can read any of it.
                        payload.push(ground.plants.len() as u8);
                        ground.put(&mut payload);
                    }
                }
            }
        }
        write_frame(to, &payload, MAX_SERVER_FRAME)
    }

    /// Reads the next message, blocking until a whole frame has arrived.
    pub fn read(from: &mut impl Read) -> io::Result<Self> {
        let frame = read_frame(from, MAX_SERVER_FRAME)?;
        let mut payload = Payload::over(&frame);
        let message = match payload.u8()? {
            0 => Self::Welcome {
                id: PlayerId(payload.u32()?),
                spawn: payload.vec2()?,
                facing: payload.vec2()?,
                token: Token(payload.u64()?),
                aboard: match payload.u8()? {
                    0 => None,
                    1 => Some(BoatId(payload.u64()?)),
                    flag => return Err(corrupt(format!("a welcome aboard flagged {flag}"))),
                },
            },
            1 => Self::Refused {
                version: payload.u16()?,
            },
            2 => Self::Joined {
                id: PlayerId(payload.u32()?),
                position: payload.vec2()?,
            },
            3 => Self::Moved {
                id: PlayerId(payload.u32()?),
                position: payload.vec2()?,
            },
            4 => Self::Left {
                id: PlayerId(payload.u32()?),
            },
            5 => {
                let chunk = payload.ivec2()?;
                let ground = match payload.u8()? {
                    0 => None,
                    flag @ (1 | 2) => {
                        let plants = payload.u8()? as usize;
                        Some(payload.chunk_payload(flag == 2, plants)?)
                    }
                    flag => return Err(corrupt(format!("chunk {chunk} flagged {flag}"))),
                };
                Self::Chunk { chunk, ground }
            }
            6 => Self::Weather {
                wind: payload.vec2()?,
            },
            7 => Self::Daylight {
                phase: payload.f32()?,
            },
            8 => Self::Beast {
                id: BeastId(payload.u32()?),
                // Both ends of a session speak one version, so a kind this
                // build has never heard of is corruption, exactly as a chunk
                // painted in unknown colours is.
                kind: BeastKind::from_byte(payload.u8()?).ok_or_else(|| {
                    corrupt("a beast of a kind this build has never heard of".into())
                })?,
                surfaced: payload.u8()? != 0,
                position: payload.vec2()?,
                velocity: payload.vec2()?,
            },
            9 => Self::BeastGone {
                id: BeastId(payload.u32()?),
            },
            10 => Self::Reply {
                text: payload.str()?,
            },
            11 => Self::Vocabulary {
                verbs: (0..payload.u8()?)
                    .map(|_| payload.str())
                    .collect::<io::Result<_>>()?,
            },
            12 => Self::World {
                id: WorldId(payload.u64()?),
            },
            13 => Self::Boat {
                id: BoatId(payload.u64()?),
                kind: BoatKind::from_byte(payload.u8()?).ok_or_else(|| {
                    corrupt("a boat of a kind this build has never heard of".into())
                })?,
                position: payload.vec2()?,
                heading: payload.f32()?,
                occupant: match payload.u8()? {
                    0 => None,
                    1 => Some(PlayerId(payload.u32()?)),
                    flag => return Err(corrupt(format!("a boat occupied by flag {flag}"))),
                },
            },
            14 => {
                let mut found = Vec::new();
                for _ in 0..payload.u16()? {
                    let chunk = payload.ivec2()?;
                    found.push((chunk, payload.soundings()?));
                }
                Self::Surveyed { found }
            }
            15 => Self::Cairn {
                island: payload.ivec2()?,
                at: payload.vec2()?,
                yours: payload.u8()? != 0,
                name: payload.str()?,
            },
            tag => return Err(corrupt(format!("unknown server message tag {tag}"))),
        };
        payload.finish()?;
        Ok(message)
    }
}

// --- The encoding ------------------------------------------------------------
//
// A frame is a little-endian u16 length followed by that many bytes: a tag
// byte, then the message's fields, little-endian, floats as their IEEE 754
// bits. Nothing self-describing — the tag says what fields follow, and both
// ends are built from this file.

/// Frames a payload and writes it in one call, so a frame reaches the socket
/// whole and small messages travel as single packets.
fn write_frame(to: &mut impl Write, payload: &[u8], limit: u16) -> io::Result<()> {
    // Unreachable for the messages defined above, whose fields were counted
    // against their direction's ceiling; the check is for the message added
    // after this line was last read. Refusing to send beats a length prefix
    // that quietly wrapped in the `as u16` below, which would frame the whole
    // rest of the session as garbage — and only in release builds, where an
    // assertion is not there to say so.
    if payload.len() > limit as usize {
        return Err(corrupt(format!(
            "a {}-byte message does not fit a {limit}-byte frame",
            payload.len()
        )));
    }
    let mut frame = Vec::with_capacity(2 + payload.len());
    put_u16(&mut frame, payload.len() as u16);
    frame.extend_from_slice(payload);
    to.write_all(&frame)
}

/// Reads one frame's payload, blocking until all of it has arrived.
fn read_frame(from: &mut impl Read, limit: u16) -> io::Result<Vec<u8>> {
    let mut length = [0u8; 2];
    from.read_exact(&mut length)?;
    let length = u16::from_le_bytes(length);
    if length > limit {
        return Err(corrupt(format!(
            "a {length}-byte frame can only be garbage"
        )));
    }
    let mut payload = vec![0u8; length as usize];
    from.read_exact(&mut payload)?;
    Ok(payload)
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_f32(out: &mut Vec<u8>, value: f32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_vec2(out: &mut Vec<u8>, value: Vec2) {
    out.extend_from_slice(&value.x.to_le_bytes());
    out.extend_from_slice(&value.y.to_le_bytes());
}

fn put_ivec2(out: &mut Vec<u8>, value: IVec2) {
    out.extend_from_slice(&value.x.to_le_bytes());
    out.extend_from_slice(&value.y.to_le_bytes());
}

/// A string as a u16 byte count and its UTF-8 bytes. A count too big for the
/// u16 wraps, but the payload it miscounts cannot leave the machine: the
/// frame it belongs to is over its own ceiling by more, and is refused whole
/// by [`write_frame`].
fn put_str(out: &mut Vec<u8>, value: &str) {
    put_u16(out, value.len() as u16);
    out.extend_from_slice(value.as_bytes());
}

fn corrupt(what: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what)
}

/// A payload being decoded: hands out fields front to back, and insists at
/// the end that nothing is left — a message with spare bytes is as corrupt as
/// one that runs short.
struct Payload<'a> {
    bytes: &'a [u8],
}

impl<'a> Payload<'a> {
    fn over(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    fn take(&mut self, count: usize) -> io::Result<&'a [u8]> {
        if self.bytes.len() < count {
            return Err(corrupt("message ended mid-field".into()));
        }
        let (taken, rest) = self.bytes.split_at(count);
        self.bytes = rest;
        Ok(taken)
    }

    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> io::Result<u16> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().expect("2 bytes"),
        ))
    }

    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }

    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().expect("8 bytes"),
        ))
    }

    fn i32(&mut self) -> io::Result<i32> {
        Ok(i32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }

    fn f32(&mut self) -> io::Result<f32> {
        Ok(f32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }

    fn vec2(&mut self) -> io::Result<Vec2> {
        Ok(Vec2::new(self.f32()?, self.f32()?))
    }

    fn ivec2(&mut self) -> io::Result<IVec2> {
        Ok(IVec2::new(self.i32()?, self.i32()?))
    }

    fn str(&mut self) -> io::Result<String> {
        let count = self.u16()? as usize;
        let bytes = self.take(count)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|_| corrupt("text that is not UTF-8 is not text".into()))
    }

    /// One chunk's soundings, however long they turn out to be — a batch
    /// carries them one after another, so the reader is told where each ends
    /// rather than being told beforehand how long it will be.
    fn soundings(&mut self) -> io::Result<survey::Soundings> {
        let (found, rest) = survey::Soundings::take(self.bytes)
            .ok_or_else(|| corrupt("soundings that are not soundings".into()))?;
        self.bytes = rest;
        Ok(found)
    }

    fn chunk_payload(&mut self, water: bool, plants: usize) -> io::Result<ChunkPayload> {
        let bytes = self.take(ground::payload_bytes(water, plants))?;
        ChunkPayload::take(bytes, water, plants).ok_or_else(|| {
            corrupt("a chunk painted in colours this build has never heard of".into())
        })
    }

    fn finish(self) -> io::Result<()> {
        if !self.bytes.is_empty() {
            return Err(corrupt(format!(
                "{} bytes left after the message ended",
                self.bytes.len()
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::ground::{Shade, Surface, Tone, FACET_TRIS, FACET_VERTS};
    use super::survey::{Coast, Mark, Soundings};
    use super::*;

    /// A chunk's worth of ink whose every byte is a different one, so that
    /// anything which reordered the runs or the marks inside them shows.
    fn a_coast() -> Soundings {
        Soundings {
            coast: vec![
                Coast::new(vec![Mark::unpack([1, 2]), Mark::unpack([255, 0])], false),
                Coast::new(
                    vec![
                        Mark::unpack([10, 20]),
                        Mark::unpack([30, 40]),
                        Mark::unpack([50, 60]),
                    ],
                    true,
                ),
            ],
            shoal: vec![Coast::new(
                vec![Mark::unpack([7, 8]), Mark::unpack([9, 11])],
                false,
            )],
        }
    }

    fn bytes_of_client(message: &ToServer) -> Vec<u8> {
        let mut out = Vec::new();
        message.write(&mut out).expect("a Vec never fails to grow");
        out
    }

    fn bytes_of_server(message: &ToClient) -> Vec<u8> {
        let mut out = Vec::new();
        message.write(&mut out).expect("a Vec never fails to grow");
        out
    }

    /// A chunk of ground whose every byte is a function of where it is, so
    /// that anything which transposed, truncated or reordered the payload
    /// shows up rather than round-tripping perfectly.
    fn a_chunk() -> ChunkPayload {
        ChunkPayload {
            heights: (0..FACET_VERTS * FACET_VERTS)
                .map(|i| (i * 601 % 65_521) as u16)
                .collect(),
            surfaces: (0..FACET_TRIS)
                .map(|i| {
                    Surface::new(
                        [
                            Tone::Seabed,
                            Tone::Sand,
                            Tone::Forest,
                            Tone::Fell,
                            Tone::Marsh,
                        ][i % 5],
                        [Shade::Dark, Shade::Plain, Shade::Light][i % 3],
                    )
                })
                .collect(),
            water: None,
            plants: Vec::new(),
        }
    }

    /// The same chunk with a lake on it, its levels a different function of
    /// position from the heights — so a payload that sent one grid twice, or
    /// read the water out of the heights, fails rather than agreeing with
    /// itself.
    fn a_chunk_with_a_lake() -> ChunkPayload {
        ChunkPayload {
            water: Some(
                (0..FACET_VERTS * FACET_VERTS)
                    .map(|i| (i * 907 % 65_519) as u16)
                    .collect(),
            ),
            ..a_chunk()
        }
    }

    #[test]
    fn every_message_survives_the_round_trip() {
        let at = Vec2::new(1234.5, -6789.25);
        for message in [
            ToServer::Hello {
                version: PROTOCOL_VERSION,
            },
            ToServer::Move { position: at },
            ToServer::WantChunk {
                chunk: IVec2::new(-9, 4),
            },
            ToServer::WantDawn,
            ToServer::Command {
                line: "spawn shark".to_string(),
            },
            ToServer::Papers { token: None },
            ToServer::Papers {
                token: Some(Token(0x0102_0304_0506_0708)),
            },
            ToServer::Helm {
                position: at,
                heading: 1.25,
            },
            ToServer::Board {
                boat: BoatId(0x0102_0304_0506_0708),
            },
            ToServer::Disembark { position: at },
            ToServer::Claim {
                island: IVec2::new(-1_234, 5_678),
            },
            ToServer::Name {
                island: IVec2::new(-1_234, 5_678),
                name: "Windward Reach".to_string(),
            },
            // An island christened in an alphabet that costs more than a byte
            // a letter, which is the case the wire's cap is counted in bytes
            // for.
            ToServer::Name {
                island: IVec2::ZERO,
                name: "Ilha do Príncipe".to_string(),
            },
        ] {
            let bytes = bytes_of_client(&message);
            assert_eq!(ToServer::read(&mut bytes.as_slice()).unwrap(), message);
        }

        for message in [
            ToClient::Welcome {
                id: PlayerId(3),
                spawn: at,
                facing: Vec2::new(-1.0, 2.0),
                token: Token(0x0102_0304_0506_0708),
                aboard: Some(BoatId(0x0807_0605_0403_0201)),
            },
            ToClient::Welcome {
                id: PlayerId(3),
                spawn: at,
                facing: Vec2::new(-1.0, 2.0),
                token: Token(0x0102_0304_0506_0708),
                aboard: None,
            },
            ToClient::Refused { version: 9 },
            ToClient::World {
                id: WorldId(0x0807_0605_0403_0201),
            },
            ToClient::Boat {
                id: BoatId(12),
                kind: BoatKind::Sloop,
                position: at,
                heading: -0.5,
                occupant: Some(PlayerId(3)),
            },
            ToClient::Boat {
                id: BoatId(13),
                kind: BoatKind::Sloop,
                position: at,
                heading: 2.0,
                occupant: None,
            },
            ToClient::Joined {
                id: PlayerId(1),
                position: at,
            },
            ToClient::Moved {
                id: PlayerId(2),
                position: at,
            },
            ToClient::Left { id: PlayerId(4) },
            ToClient::Weather {
                wind: Vec2::new(-3.25, 8.5),
            },
            ToClient::Daylight { phase: 0.125 },
            ToClient::Beast {
                id: BeastId(12),
                kind: BeastKind::Shark,
                position: at,
                velocity: Vec2::new(-1.0, 0.5),
                surfaced: true,
            },
            ToClient::Beast {
                id: BeastId(13),
                kind: BeastKind::Dolphins,
                position: at,
                velocity: Vec2::new(2.0, -1.5),
                surfaced: true,
            },
            ToClient::Beast {
                id: BeastId(14),
                kind: BeastKind::Whale,
                position: at,
                velocity: Vec2::new(-0.5, -1.0),
                surfaced: false,
            },
            ToClient::BeastGone { id: BeastId(12) },
            ToClient::Reply {
                text: "the clock stands at 06:00".to_string(),
            },
            ToClient::Vocabulary {
                verbs: vec!["help".to_string(), "spawn".to_string()],
            },
            ToClient::Vocabulary { verbs: Vec::new() },
            ToClient::Chunk {
                chunk: IVec2::new(3, -8),
                ground: None,
            },
            ToClient::Chunk {
                chunk: IVec2::new(-2, 7),
                ground: Some(a_chunk()),
            },
            ToClient::Chunk {
                chunk: IVec2::new(6, -1),
                ground: Some(a_chunk_with_a_lake()),
            },
            ToClient::Surveyed {
                found: vec![
                    (IVec2::new(-2, 7), a_coast()),
                    // Surveyed and blank, which is most of an ocean.
                    (IVec2::new(-1, 7), Soundings::default()),
                ],
            },
            ToClient::Surveyed { found: Vec::new() },
            ToClient::Cairn {
                island: IVec2::new(-1_234, 5_678),
                at,
                name: "Windward Reach".to_string(),
                yours: true,
            },
            // And one nobody has christened, standing for somebody else.
            ToClient::Cairn {
                island: IVec2::new(7, -7),
                at,
                name: String::new(),
                yours: false,
            },
        ] {
            let bytes = bytes_of_server(&message);
            assert_eq!(ToClient::read(&mut bytes.as_slice()).unwrap(), message);
        }
    }

    #[test]
    fn the_wire_is_a_format() {
        // The exact bytes, pinned the way the world pins its digests. Nothing
        // old is left running to be broken, so re-recording is the right
        // answer to a deliberate change — the pin is here to make sure the
        // change was deliberate, and that both ends are rebuilt together.
        //
        // Every variant of both enums appears, because the round trip above
        // cannot see any of the ways this format could move while still
        // agreeing with itself: renumbered tags, fields swapped within a
        // message, a length counted wrong. All of those read back perfectly
        // and would still leave two builds unable to talk.
        //
        // The point (1.5, -2.0) is shared by every message that carries one:
        // both halves are exact in binary, and the two differ in every byte
        // that matters, so a pair of axes that swapped places would show.
        assert_eq!(
            bytes_of_client(&ToServer::Hello { version: 3 }),
            [3, 0, 0, 3, 0],
            "hello: length 3, tag 0, version LE"
        );
        assert_eq!(
            bytes_of_client(&ToServer::Move {
                position: Vec2::new(1.5, -2.0),
            }),
            [
                9, 0, // length
                1, // tag
                0, 0, 0xC0, 0x3F, // x = 1.5
                0, 0, 0, 0xC0, // y = -2.0
            ],
        );
        assert_eq!(
            bytes_of_client(&ToServer::WantChunk {
                chunk: IVec2::new(5, -3),
            }),
            [
                9, 0, // length
                2, // tag
                5, 0, 0, 0, // x = 5
                0xFD, 0xFF, 0xFF, 0xFF, // z = -3, two's complement LE
            ],
        );
        assert_eq!(
            bytes_of_client(&ToServer::WantDawn),
            [1, 0, 3],
            "want dawn: length 1, tag 3, and nothing to say"
        );
        assert_eq!(
            bytes_of_client(&ToServer::Command {
                line: "hi".to_string(),
            }),
            [
                5, 0, // length
                4, // tag
                2, 0, // the line's own byte count, LE
                0x68, 0x69, // "hi", as the UTF-8 it already was
            ],
        );
        assert_eq!(
            bytes_of_client(&ToServer::Papers { token: None }),
            [2, 0, 5, 0],
            "empty papers: length 2, tag 5, and the flag saying so"
        );
        assert_eq!(
            bytes_of_client(&ToServer::Papers {
                token: Some(Token(0x0102_0304_0506_0708)),
            }),
            [
                10, 0, // length
                5, // tag
                1, // a token follows
                8, 7, 6, 5, 4, 3, 2, 1, // the token, LE
            ],
        );
        assert_eq!(
            bytes_of_client(&ToServer::Helm {
                position: Vec2::new(1.5, -2.0),
                heading: 0.75,
            }),
            [
                13, 0, // length
                6, // tag
                0, 0, 0xC0, 0x3F, // x = 1.5
                0, 0, 0, 0xC0, // y = -2.0
                0, 0, 0x40, 0x3F, // heading = 0.75
            ],
        );
        assert_eq!(
            bytes_of_client(&ToServer::Board {
                boat: BoatId(0x0102_0304_0506_0708),
            }),
            [
                9, 0, // length
                7, // tag
                8, 7, 6, 5, 4, 3, 2, 1, // the boat, LE
            ],
        );
        assert_eq!(
            bytes_of_client(&ToServer::Disembark {
                position: Vec2::new(1.5, -2.0),
            }),
            [
                9, 0, // length
                8, // tag
                0, 0, 0xC0, 0x3F, // x = 1.5
                0, 0, 0, 0xC0, // y = -2.0
            ],
        );
        assert_eq!(
            bytes_of_client(&ToServer::Claim {
                island: IVec2::new(5, -3),
            }),
            [
                9, 0, // length
                9, // tag
                5, 0, 0, 0, // x = 5
                0xFD, 0xFF, 0xFF, 0xFF, // z = -3, two's complement LE
            ],
        );
        assert_eq!(
            bytes_of_client(&ToServer::Name {
                island: IVec2::new(5, -3),
                name: "hi".to_string(),
            }),
            [
                13, 0,  // length
                10, // tag
                5, 0, 0, 0, // x = 5
                0xFD, 0xFF, 0xFF, 0xFF, // z = -3
                2, 0, // the name's own byte count, LE
                0x68, 0x69, // "hi"
            ],
        );

        assert_eq!(
            bytes_of_server(&ToClient::Welcome {
                id: PlayerId(7),
                spawn: Vec2::new(1.5, -2.0),
                facing: Vec2::new(-2.0, 1.5),
                token: Token(0x0102_0304_0506_0708),
                aboard: Some(BoatId(0x0807_0605_0403_0201)),
            }),
            [
                38, 0, // length
                0, // tag
                7, 0, 0, 0, // id
                0, 0, 0xC0, 0x3F, // spawn x = 1.5
                0, 0, 0, 0xC0, // spawn z = -2.0
                0, 0, 0, 0xC0, // facing x = -2.0 — the spawn's axes swapped,
                0, 0, 0xC0, 0x3F, // facing z = 1.5, so a confused pair shows
                8, 7, 6, 5, 4, 3, 2, 1, // the token dealt, LE
                1, // at a helm...
                1, 2, 3, 4, 5, 6, 7, 8, // ...of this boat, LE
            ],
        );
        // Entering on foot differs in exactly the flag and the boat's absence.
        assert_eq!(
            bytes_of_server(&ToClient::Welcome {
                id: PlayerId(7),
                spawn: Vec2::new(1.5, -2.0),
                facing: Vec2::new(-2.0, 1.5),
                token: Token(0x0102_0304_0506_0708),
                aboard: None,
            })[..],
            [
                &[30u8, 0][..],
                &bytes_of_server(&ToClient::Welcome {
                    id: PlayerId(7),
                    spawn: Vec2::new(1.5, -2.0),
                    facing: Vec2::new(-2.0, 1.5),
                    token: Token(0x0102_0304_0506_0708),
                    aboard: Some(BoatId(0x0807_0605_0403_0201)),
                })[2..31],
                &[0u8][..],
            ]
            .concat()[..],
        );
        assert_eq!(
            bytes_of_server(&ToClient::World {
                id: WorldId(0x0807_0605_0403_0201),
            }),
            [
                9, 0,  // length
                12, // tag
                1, 2, 3, 4, 5, 6, 7, 8, // the id, LE
            ],
        );
        assert_eq!(
            bytes_of_server(&ToClient::Boat {
                id: BoatId(7),
                kind: BoatKind::Sloop,
                position: Vec2::new(1.5, -2.0),
                heading: 0.75,
                occupant: Some(PlayerId(9)),
            }),
            [
                27, 0,  // length
                13, // tag
                7, 0, 0, 0, 0, 0, 0, 0, // the boat, LE
                0, // kind: sloop
                0, 0, 0xC0, 0x3F, // x = 1.5
                0, 0, 0, 0xC0, // y = -2.0
                0, 0, 0x40, 0x3F, // heading = 0.75
                1,    // somebody at the helm...
                9, 0, 0, 0, // ...this player, LE
            ],
        );
        // A boat lying empty differs in exactly the flag and the missing hand.
        let occupied = bytes_of_server(&ToClient::Boat {
            id: BoatId(7),
            kind: BoatKind::Sloop,
            position: Vec2::new(1.5, -2.0),
            heading: 0.75,
            occupant: Some(PlayerId(9)),
        });
        let empty = bytes_of_server(&ToClient::Boat {
            id: BoatId(7),
            kind: BoatKind::Sloop,
            position: Vec2::new(1.5, -2.0),
            heading: 0.75,
            occupant: None,
        });
        assert_eq!(empty[..2], [23, 0], "an empty boat is shorter by its hand");
        assert_eq!(empty[2..24], occupied[2..24], "emptiness moved the fields");
        assert_eq!(empty[24], 0, "nobody at the helm is flag 0");
        assert_eq!(
            bytes_of_server(&ToClient::Refused { version: 9 }),
            [3, 0, 1, 9, 0],
            "refused: length 3, tag 1, version LE"
        );
        assert_eq!(
            bytes_of_server(&ToClient::Joined {
                id: PlayerId(7),
                position: Vec2::new(1.5, -2.0),
            }),
            [
                13, 0, // length
                2, // tag
                7, 0, 0, 0, // id
                0, 0, 0xC0, 0x3F, // x = 1.5
                0, 0, 0, 0xC0, // y = -2.0
            ],
        );
        assert_eq!(
            bytes_of_server(&ToClient::Moved {
                id: PlayerId(7),
                position: Vec2::new(1.5, -2.0),
            }),
            [
                13, 0, // length
                3, // tag — Joined's twin, and only the tag tells them apart
                7, 0, 0, 0, // id
                0, 0, 0xC0, 0x3F, // x = 1.5
                0, 0, 0, 0xC0, // y = -2.0
            ],
        );
        assert_eq!(
            bytes_of_server(&ToClient::Left { id: PlayerId(7) }),
            [
                5, 0, // length
                4, // tag
                7, 0, 0, 0, // id
            ],
        );
        assert_eq!(
            bytes_of_server(&ToClient::Weather {
                wind: Vec2::new(1.5, -2.0),
            }),
            [
                9, 0, // length
                6, // tag
                0, 0, 0xC0, 0x3F, // x = 1.5
                0, 0, 0, 0xC0, // y = -2.0
            ],
        );
        assert_eq!(
            bytes_of_server(&ToClient::Daylight { phase: 0.75 }),
            [
                5, 0, // length
                7, // tag
                0, 0, 0x40, 0x3F, // phase = 0.75, sunset
            ],
        );
        assert_eq!(
            bytes_of_server(&ToClient::Beast {
                id: BeastId(7),
                kind: BeastKind::Shark,
                position: Vec2::new(1.5, -2.0),
                velocity: Vec2::new(-2.0, 1.5),
                surfaced: true,
            }),
            [
                23, 0, // length
                8, // tag
                7, 0, 0, 0, // id
                0, // kind: shark
                1, // surfaced
                0, 0, 0xC0, 0x3F, // position x = 1.5
                0, 0, 0, 0xC0, // position z = -2.0
                0, 0, 0, 0xC0, // velocity x = -2.0 — the position's axes
                0, 0, 0xC0, 0x3F, // velocity z = 1.5, so a confused pair shows
            ],
        );
        // The other kinds differ from the shark's message in exactly the one
        // byte that says what the beast is, and a sounded one in exactly the
        // byte after it.
        let shark = bytes_of_server(&ToClient::Beast {
            id: BeastId(7),
            kind: BeastKind::Shark,
            position: Vec2::new(1.5, -2.0),
            velocity: Vec2::new(-2.0, 1.5),
            surfaced: true,
        });
        for (kind, byte) in [(BeastKind::Dolphins, 1u8), (BeastKind::Whale, 2)] {
            let told = bytes_of_server(&ToClient::Beast {
                id: BeastId(7),
                kind,
                position: Vec2::new(1.5, -2.0),
                velocity: Vec2::new(-2.0, 1.5),
                surfaced: true,
            });
            assert_eq!(told[7], byte, "{kind:?} is not kind byte {byte}");
            assert_eq!(told[..7], shark[..7], "{kind:?} moved the head");
            assert_eq!(told[8..], shark[8..], "{kind:?} moved the fields");
        }
        let sounded = bytes_of_server(&ToClient::Beast {
            id: BeastId(7),
            kind: BeastKind::Shark,
            position: Vec2::new(1.5, -2.0),
            velocity: Vec2::new(-2.0, 1.5),
            surfaced: false,
        });
        assert_eq!(sounded[8], 0, "a sounded beast is not flagged 0");
        assert_eq!(sounded[..8], shark[..8], "sounding moved the head");
        assert_eq!(sounded[9..], shark[9..], "sounding moved the fields");
        assert_eq!(
            bytes_of_server(&ToClient::BeastGone { id: BeastId(7) }),
            [
                5, 0, // length
                9, // tag
                7, 0, 0, 0, // id
            ],
        );
        assert_eq!(
            bytes_of_server(&ToClient::Reply {
                text: "hi".to_string(),
            }),
            [
                5, 0,  // length
                10, // tag
                2, 0, // the text's own byte count, LE
                0x68, 0x69, // "hi"
            ],
        );
        assert_eq!(
            bytes_of_server(&ToClient::Vocabulary {
                verbs: vec!["hi".to_string(), "yo".to_string()],
            }),
            [
                10, 0,  // length
                11, // tag
                2,  // two verbs
                2, 0, 0x68, 0x69, // "hi", counted then spelled
                2, 0, 0x79, 0x6F, // "yo"
            ],
        );
        assert_eq!(
            bytes_of_server(&ToClient::Cairn {
                island: IVec2::new(5, -3),
                at: Vec2::new(1.5, -2.0),
                name: "hi".to_string(),
                yours: true,
            }),
            [
                22, 0,  // length
                15, // tag
                5, 0, 0, 0, // the island's x = 5
                0xFD, 0xFF, 0xFF, 0xFF, // and z = -3
                0, 0, 0xC0, 0x3F, // where it stands: x = 1.5
                0, 0, 0, 0xC0, // z = -2.0
                1,    // the hearer's own doing
                2, 0, 0x68, 0x69, // "hi", counted then spelled
            ],
        );
        // Somebody else's, and unchristened: the flag and an empty count, and
        // nothing else moves.
        let mine = bytes_of_server(&ToClient::Cairn {
            island: IVec2::new(5, -3),
            at: Vec2::new(1.5, -2.0),
            name: "hi".to_string(),
            yours: true,
        });
        let theirs = bytes_of_server(&ToClient::Cairn {
            island: IVec2::new(5, -3),
            at: Vec2::new(1.5, -2.0),
            name: String::new(),
            yours: false,
        });
        assert_eq!(
            theirs[..2],
            [20, 0],
            "a nameless cairn is shorter by a name"
        );
        assert_eq!(theirs[2..19], mine[2..19], "whose it is moved the fields");
        assert_eq!(theirs[19], 0, "somebody else's cairn is not flagged 0");
        assert_eq!(theirs[20..], [0, 0], "an unchristened cairn says nothing");

        // A survey: two chunks, one with a single open run on its waterline
        // and one surveyed and blank. Written out whole, since between them
        // they carry every field the encoding has.
        assert_eq!(
            bytes_of_server(&ToClient::Surveyed {
                found: vec![
                    (
                        IVec2::new(5, -3),
                        Soundings {
                            coast: vec![Coast::new(
                                vec![Mark::unpack([1, 2]), Mark::unpack([255, 0])],
                                false,
                            )],
                            shoal: Vec::new(),
                        },
                    ),
                    (IVec2::ZERO, Soundings::default()),
                ],
            }),
            [
                34, 0,  // length
                14, // tag
                2, 0, // two chunks
                5, 0, 0, 0, // x = 5
                0xFD, 0xFF, 0xFF, 0xFF, // z = -3
                1, 0, // one run of waterline...
                0, 0, // ...and no shoal line
                0, // the run does not close
                2, 0, // two marks
                1, 2, // the first, x then z
                255, 0, // the second, on the chunk's own edge
                0, 0, 0, 0, // the second chunk: x = 0
                0, 0, 0, 0, // z = 0
                0, 0, 0, 0, // surveyed, and nothing found on it
            ],
        );

        // Open water: the whole message, since there is nothing in it.
        assert_eq!(
            bytes_of_server(&ToClient::Chunk {
                chunk: IVec2::new(5, -3),
                ground: None,
            }),
            [
                10, 0, // length
                5, // tag
                5, 0, 0, 0, // x = 5
                0xFD, 0xFF, 0xFF, 0xFF, // z = -3
                0,    // no ground here
            ],
        );

        // Ground: too long to write out, so the head, the length and a
        // handful of interior bytes at known offsets. Between them they pin
        // the layout — where the heights start, that they are little-endian
        // pairs, where the surfaces start, and how a surface packs.
        let ground = bytes_of_server(&ToClient::Chunk {
            chunk: IVec2::new(5, -3),
            ground: Some(a_chunk()),
        });
        let framed = 1 + 8 + 1 + 1 + ground::PAYLOAD_BYTES;
        assert_eq!(ground.len(), 2 + framed);
        assert_eq!(
            ground[..11],
            [
                (framed & 0xFF) as u8,
                (framed >> 8) as u8, // length
                5,                   // tag
                5,
                0,
                0,
                0, // x = 5
                0xFD,
                0xFF,
                0xFF,
                0xFF, // z = -3
            ],
            "the head of a ground answer"
        );
        assert_eq!(ground[11], 1, "the flag says there is dry ground");
        assert_eq!(ground[12], 0, "and the count says nothing grows on it");

        // Heights start at 13. Corner 0 is 0, corner 1 is 601, corner 2 is
        // 1202 — little-endian pairs.
        assert_eq!(ground[13..19], [0, 0, 0x59, 0x02, 0xB2, 0x04]);

        // Surfaces start once the heights are done. The first is Seabed dark
        // — tone 0 in the high bits, shade 0 in the low two — and the second
        // Sand plain: tone 2, shade 1.
        let surfaces = 13 + FACET_VERTS * FACET_VERTS * 2;
        assert_eq!(
            ground[surfaces..surfaces + 3],
            [0b0000_0000, 0b0000_1001, 0b0001_0010],
            "seabed/dark, sand/plain, forest/light"
        );

        // And the same chunk with a lake on it. The heights and the surfaces
        // must land at exactly the offsets they land at above — water is
        // something a chunk carries in addition, not a rearrangement of what
        // it carried already — so the two answers agree byte for byte up to
        // the end of the surfaces and differ only in the flag and the tail.
        let lake = bytes_of_server(&ToClient::Chunk {
            chunk: IVec2::new(5, -3),
            ground: Some(a_chunk_with_a_lake()),
        });
        let wet = 1 + 8 + 1 + 1 + ground::payload_bytes(true, 0);
        assert_eq!(lake.len(), 2 + wet);
        assert_eq!(
            lake[..2],
            [(wet & 0xFF) as u8, (wet >> 8) as u8],
            "a watered chunk is longer by exactly its water grid"
        );
        assert_eq!(
            lake[2..11],
            ground[2..11],
            "the head is the same either way"
        );
        assert_eq!(lake[11], 2, "the flag says there is water on this ground");
        assert_eq!(
            lake[12..surfaces + FACET_TRIS],
            ground[12..surfaces + FACET_TRIS],
            "the water moved the heights or the surfaces"
        );

        // The water grid starts once the surfaces are done, little-endian
        // pairs like the heights: level 0 is 0, level 1 is 907, level 2 is
        // 1814.
        let water = surfaces + FACET_TRIS;
        assert_eq!(lake[water..water + 6], [0, 0, 0x8B, 0x03, 0x16, 0x07]);

        // And a plant, which goes on the end of everything else: its kind
        // first, then its position as a pair of little-endian sixteenths of a
        // chunk, then its bearing and its size as single bytes. The kind
        // leads because the size cannot be read without it — a size is a step
        // through the range that kind is drawn at.
        let mut planted = a_chunk();
        planted.plants = vec![ground::Plant {
            kind: ground::Kind::Palm,
            at: Vec2::new(2.0, 96.0),
            yaw: std::f32::consts::FRAC_PI_2,
            scale: ground::Kind::Palm.scale().1,
        }];
        let stand = bytes_of_server(&ToClient::Chunk {
            chunk: IVec2::new(5, -3),
            ground: Some(planted),
        });
        assert_eq!(stand.len(), ground.len() + ground::PLANT_BYTES);
        assert_eq!(stand[12], 1, "the count says one thing grows on it");
        let plant = 13 + ground::PAYLOAD_BYTES;
        assert_eq!(
            stand[plant..plant + ground::PLANT_BYTES],
            [
                0, // a palm
                0x00, 0x04, // x = 2 m of 128
                0xFF, 0xBF, // z = 96 m of 128
                64,   // a quarter turn
                255,  // the largest a palm is drawn
            ],
            "one palm, on the end of the ground it stands on"
        );
    }

    #[test]
    fn a_name_is_either_carried_or_refused() {
        // What a person types, with the whitespace they leaned on either side
        // of it taken off — the name is the word, not the typing.
        assert_eq!(
            island_name("  Windward Reach "),
            Some("Windward Reach".to_string())
        );
        // A name in an alphabet that costs more than a byte a letter fits,
        // which is what counting to [`NAME_BYTES`] rather than to twenty-four
        // is for.
        assert_eq!(
            island_name("Ilha do Príncipe").as_deref(),
            Some("Ilha do Príncipe")
        );

        for (raw, what) in [
            ("", "nothing at all"),
            ("   ", "a name that is only the space bar"),
            ("Windward\nReach", "a name with a line break in it"),
            ("Windward\tReach", "a name with a tab in it"),
        ] {
            assert!(island_name(raw).is_none(), "the wire carried {what}");
        }
        // And one past the cap, which is refused whole rather than trimmed to
        // fit: half of somebody's word would stand on their island for good.
        let overlong = "a".repeat(NAME_BYTES + 1);
        assert_eq!(island_name(&overlong), None);
        assert!(island_name(&"a".repeat(NAME_BYTES)).is_some());
    }

    #[test]
    fn a_message_too_big_to_frame_is_refused() {
        // The client's ceiling is far below any message defined here, so this
        // asks the framing directly. The length prefix is a u16 and the
        // ceiling is far below one, so an oversized payload written anyway
        // would arrive as a plausible short frame followed by the rest of it
        // read as messages.
        let mut wire = Vec::new();
        assert!(write_frame(
            &mut wire,
            &vec![0u8; MAX_CLIENT_FRAME as usize + 1],
            MAX_CLIENT_FRAME
        )
        .is_err());
        assert!(wire.is_empty(), "half a frame reached the wire");

        // And the server's ceiling covers both of the answers big enough to
        // measure. The largest chunk of ground there is — with water on it
        // and as many plants as one may carry — fits, and so does a batch of
        // survey filled to its budget with the most torn chunk there could
        // be on the end of it, which is what the ceiling actually stands at.
        assert!(usize::from(MAX_SERVER_FRAME) >= 3 + SURVEY_BATCH_BYTES);
        assert!(usize::from(MAX_SERVER_FRAME) >= 3 + 8 + survey::SOUNDINGS_BYTES);

        let mut most = a_chunk_with_a_lake();
        most.plants = vec![
            ground::Plant {
                kind: ground::Kind::Palm,
                at: Vec2::ZERO,
                yaw: 0.0,
                scale: ground::Kind::Palm.scale().0,
            };
            ground::MAX_PLANTS
        ];
        let biggest = bytes_of_server(&ToClient::Chunk {
            chunk: IVec2::ZERO,
            ground: Some(most),
        });
        assert_eq!(
            biggest.len() - 2,
            1 + 8 + 1 + 1 + ground::payload_bytes(true, ground::MAX_PLANTS),
            "a watered chunk under a full stand of plants costs what it costs"
        );
        assert!(biggest.len() - 2 <= MAX_SERVER_FRAME as usize);
        let lake = bytes_of_server(&ToClient::Chunk {
            chunk: IVec2::ZERO,
            ground: Some(a_chunk_with_a_lake()),
        });
        let ground = bytes_of_server(&ToClient::Chunk {
            chunk: IVec2::ZERO,
            ground: Some(a_chunk()),
        });
        assert_eq!(lake.len() - ground.len(), ground::WATER_BYTES);
        assert_eq!(
            biggest.len() - lake.len(),
            ground::MAX_PLANTS * ground::PLANT_BYTES
        );
    }

    #[test]
    fn a_batch_of_survey_costs_what_its_chunks_cost() {
        // What a server fills a message to its budget by. The count and the
        // encoding are two pieces of arithmetic about one thing, and a batch
        // built to fit a frame only fits while they agree.
        let found = vec![
            (IVec2::new(-2, 7), a_coast()),
            (IVec2::ZERO, Soundings::default()),
        ];
        let spent: usize = found.iter().map(|(_, ink)| surveyed_bytes(ink)).sum();
        let bytes = bytes_of_server(&ToClient::Surveyed { found });
        assert_eq!(
            bytes.len(),
            2 + 1 + 2 + spent,
            "the length and the count part"
        );
    }

    #[test]
    fn a_client_cannot_be_asked_to_buffer_a_chunk() {
        // The two directions have different ceilings, and the small one is
        // what protects a server from a client claiming to have a great deal
        // to say. A length that would be perfectly legal coming the other way
        // is corruption coming this way.
        let mut wire = Vec::new();
        put_u16(&mut wire, MAX_CLIENT_FRAME + 1);
        wire.extend(std::iter::repeat_n(0, MAX_CLIENT_FRAME as usize + 1));
        assert!(ToServer::read(&mut wire.as_slice()).is_err());
    }

    #[test]
    fn messages_stream_back_to_back() {
        let mut wire = Vec::new();
        let first = ToServer::Hello { version: 2 };
        let second = ToServer::Move {
            position: Vec2::new(8.0, -4.0),
        };
        let third = ToServer::WantChunk {
            chunk: IVec2::new(1, 1),
        };
        first.write(&mut wire).unwrap();
        second.write(&mut wire).unwrap();
        third.write(&mut wire).unwrap();

        let mut reading = wire.as_slice();
        assert_eq!(ToServer::read(&mut reading).unwrap(), first);
        assert_eq!(ToServer::read(&mut reading).unwrap(), second);
        assert_eq!(ToServer::read(&mut reading).unwrap(), third);
        assert!(reading.is_empty());
    }

    /// Ground answers are the one thing on this wire big enough to be split
    /// across packets, so a reader that assumed a frame arrives whole would
    /// pass every test above and fail on a real socket.
    #[test]
    fn a_chunk_split_across_reads_still_arrives() {
        struct Dribble<'a> {
            bytes: &'a [u8],
        }
        impl Read for Dribble<'_> {
            fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
                let n = out.len().min(self.bytes.len()).min(1000);
                out[..n].copy_from_slice(&self.bytes[..n]);
                self.bytes = &self.bytes[n..];
                Ok(n)
            }
        }

        // The watered chunk, being the largest answer there is and so the
        // most split.
        let message = ToClient::Chunk {
            chunk: IVec2::new(-2, 7),
            ground: Some(a_chunk_with_a_lake()),
        };
        let wire = bytes_of_server(&message);
        let mut dribble = Dribble { bytes: &wire };
        assert_eq!(ToClient::read(&mut dribble).unwrap(), message);
    }

    #[test]
    fn garbage_is_refused_not_believed() {
        // A length past the ceiling is corruption, however patient the reader.
        let oversized = [0xFF, 0xFF];
        assert!(ToServer::read(&mut oversized.as_slice()).is_err());

        // An unknown tag, in an otherwise well-formed frame.
        let unknown = [1, 0, 200];
        assert!(ToServer::read(&mut unknown.as_slice()).is_err());
        assert!(ToClient::read(&mut unknown.as_slice()).is_err());

        // A frame that ends mid-field, and one that runs past its meaning.
        let short = [2, 0, 0, 1];
        assert!(ToServer::read(&mut short.as_slice()).is_err());
        let long = [4, 0, 0, 1, 0, 99];
        assert!(ToServer::read(&mut long.as_slice()).is_err());

        // A beast of a kind this build has never heard of, in an otherwise
        // well-formed frame — a kind is meaning, not framing, and both ends
        // of a session speak one version.
        let mut unknown_beast = vec![22, 0, 8, 7, 0, 0, 0, 200];
        unknown_beast.extend([0; 16]);
        assert!(ToClient::read(&mut unknown_beast.as_slice()).is_err());

        // Papers whose flag byte is neither kind of answer.
        let bad_papers = [10, 0, 5, 2, 0, 0, 0, 0, 0, 0, 0, 0];
        assert!(ToServer::read(&mut bad_papers.as_slice()).is_err());

        // A chunk whose flag byte is none of the three kinds of answer.
        let mut bad_flag = vec![10, 0, 5];
        bad_flag.extend([0; 8]);
        bad_flag.push(3);
        assert!(ToClient::read(&mut bad_flag.as_slice()).is_err());

        // And one that claims water and then ends where dry ground would
        // have: the flag is what says how long the payload is, so a frame
        // that disagrees with its own flag is corrupt rather than a chunk
        // with a short lake.
        let mut short_lake = Vec::new();
        put_u16(&mut short_lake, (1 + 8 + 1 + ground::PAYLOAD_BYTES) as u16);
        short_lake.push(5);
        short_lake.extend([0; 8]);
        short_lake.push(2);
        short_lake.extend(std::iter::repeat_n(0, ground::PAYLOAD_BYTES));
        assert!(ToClient::read(&mut short_lake.as_slice()).is_err());

        // A survey promising more chunks than it carries: the counts inside a
        // batch are a reader's to believe only as far as the bytes go.
        let short_survey = [3, 0, 14, 2, 0];
        assert!(ToClient::read(&mut short_survey.as_slice()).is_err());

        // And a wire that simply ends is an ordinary end-of-file error.
        assert!(ToServer::read(&mut [].as_slice()).is_err());
    }
}
