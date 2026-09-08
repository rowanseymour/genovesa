//! A server and its clients talking over real sockets: the handshake, the
//! introductions, the relay, the ground, and leaving.
//!
//! The night tests read the clock's pace off the clock's own steps, never off
//! a stopwatch, because how long a tick takes in real seconds is the
//! machine's business: on a loaded one the asking waits seconds behind the
//! survey of the asker's own arrival before the first tick is wound at all,
//! and a stopwatch cannot tell that wait from a night that never ran. Ten
//! seconds of one failed on it, and six hundred milliseconds before that
//! failed on a tick that did not fit.

use std::collections::HashMap;
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use glam::{IVec2, Vec2};
use protocol::ground::{dequantize, CHUNK_METRES};
use protocol::survey::{in_sight, in_sight_along, Soundings, Standing, Survey, SIGHT_RADIUS};
use protocol::{
    BeastId, BeastKind, BoatKind, PlayerId, ToClient, ToServer, Token, Underway, PROTOCOL_VERSION,
};
use server::{Host, Server};
use world::archipelago::{Archipelago, IslandSpec, WorldConfig, ENTRY};

/// How long a reader waits for the message it is after before failing the
/// test. Long enough that a loaded machine is not mistaken for a session that
/// has stopped talking, short enough that a wait which will never end still
/// ends.
///
/// The loaded machine to size it for is a CI runner, and the gap is wider
/// than a slow core: raising an island fans out across every core the machine
/// has, so a step that takes the server most of a second on a laptop — a
/// full survey sweep worked on the session's own thread — has a dozen cores
/// behind it there and, on a four-vCPU runner running this whole file at
/// once, about one. Ten seconds lost that step; so did thirty. Only a test
/// that was going to fail pays the whole of this, so it can afford to be
/// generous.
const PATIENCE: Duration = Duration::from_secs(120);

/// The same, for the survey, which is worked out as somebody walks rather
/// than sent in one burst.
const SURVEY_PATIENCE: Duration = Duration::from_secs(240);

/// The least the day may have moved between two tellings of the time, over
/// and above what the wall clock between them moved it, that can only be a
/// wound tick — see [`Client::ask_for_dawn_by`]. A wound tick runs the day
/// on by a fiftieth of it; the day passing on its own moves it by the wall
/// clock and nothing more, however long the sky thread took over the
/// telling, so any telling is plainly one or the other.
const PLAINLY_WOUND: f32 = 0.01;

/// How far the sea may have carried a hull nobody is aboard — and the
/// sleeper the world remembers aboard it — in the seconds a test takes, in
/// metres. A bound on the drift and not a measure of it: the tests that leave
/// a hull unanchored in open water and expect it, or its helmsman, back
/// compare against this rather than the exact spot. What the sea does with an
/// empty hull is its own tests' business, below.
const ADRIFT: f32 = 8.0;

/// What a seed's world is, to a test that is allowed to know. A client never
/// gets one of these — that is the whole point of the arrangement — so these
/// are here to say what the server's answers *should* have been.
fn behind_the_curtain(seed: u32) -> Archipelago {
    Archipelago::new(&WorldConfig { seed })
}

/// A newcomer taken by the console from the entry to a berth off the nearest
/// land — water the anchor holds in, which the open sea a world is entered
/// on is not. Where every test of a hull at rest starts, and the same door a
/// tester goes through to look at an island.
fn berthed_off_the_first_land(client: &Client, seed: u32) -> Vec2 {
    let inland = behind_the_curtain(seed)
        .first_land()
        .expect("a world has land to berth off")
        .centre();
    // The joining chatter first, so that the put down is the only word about
    // this hull still to come — see `Client::hear_put_down_alone`.
    client.caught_up();
    client.say(ToServer::Command {
        line: format!("goto {} {}", inland.x, inland.y),
    });
    let (berth, _) = client.hear_put_down_alone();
    let _ = client.hear_reply();
    berth
}

/// The chunk a world point stands in.
fn chunk_at(point: Vec2) -> IVec2 {
    (point / CHUNK_METRES).floor().as_ivec2()
}

/// A hosted world on a loopback port of the machine's choosing, running until
/// the test process ends. Most of what is tested here is a conversation, not a
/// lifetime — the tests that are about the lifetime host their own.
fn host(seed: u32) -> SocketAddr {
    forever(Server::bind(("127.0.0.1", 0), seed).expect("bind"))
}

/// The same, opened at a chosen hour of its day, for the tests that are
/// about the night — which nothing else can reach, a world's clock running
/// at ten minutes to the day from whenever it was bound.
fn host_at(seed: u32, opening: f32) -> SocketAddr {
    forever(
        Server::bind(("127.0.0.1", 0), seed)
            .expect("bind")
            .opening_at(opening),
    )
}

/// Serves a world that nothing ever stops, and says where.
///
/// The handle is deliberately let go of without being dropped: dropping it is
/// what ends a world, and these are the worlds meant to outlast the tests
/// talking to them. What it leaks is one thread and one world for the length
/// of a test binary that is about to exit anyway.
fn forever(server: Server) -> SocketAddr {
    let host = server.spawn().expect("spawn");
    let addr = host.addr();
    std::mem::forget(host);
    addr
}

/// The same, on a thread the test can end — what a game hosting a world for
/// its own player holds.
fn spawn_host(seed: u32) -> Host {
    Server::bind(("127.0.0.1", 0), seed)
        .expect("bind")
        .spawn()
        .expect("spawn")
}

/// Whether an error is the read's own clock running out rather than anything
/// the session did. Unix answers a socket timeout with `WouldBlock` and
/// Windows with `TimedOut`, and this runs on both.
fn ran_out_of_time(why: &std::io::Error) -> bool {
    matches!(
        why.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

/// A test client: a socket that speaks the protocol, every read of it bounded
/// so a message that never comes fails the test instead of hanging it.
struct Client(TcpStream);

impl Client {
    fn connect(addr: SocketAddr) -> Self {
        Self(TcpStream::connect(addr).expect("connect"))
    }

    /// The next message of any kind, or the test failing at `deadline` saying
    /// it never heard `awaited`.
    fn hear_by(&self, deadline: Instant, awaited: &str) -> ToClient {
        self.heard_by(deadline)
            .unwrap_or_else(|why| panic!("waited out {awaited}: {why}"))
    }

    /// The next message of any kind, or the error once `deadline` has passed
    /// with nothing said — for the tests that listen to what the sea says of
    /// a hull for a while, where silence is an answer rather than a failure.
    ///
    /// The socket's timeout is set to what is left of the wait, one read at a
    /// time, rather than the socket keeping a short one of its own. That is
    /// the whole of this: a frame half-read cannot be asked for again —
    /// [`ToClient::read`] takes a length and then a body, and a timeout
    /// between them has already eaten bytes, so a second attempt resyncs
    /// inside a frame. One read given the whole remaining wait puts the
    /// failure on the deadline, where a caller can say what never came.
    ///
    /// It used to be a fixed five seconds on the socket, which is why the
    /// deadlines the readers below carry could never fire while a session was
    /// quiet: the socket always gave up first, with a `WouldBlock` naming
    /// neither the message nor the bound.
    fn heard_by(&self, deadline: Instant) -> std::io::Result<ToClient> {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "the deadline passed",
            ));
        }
        self.bound_the_next_read(left);
        ToClient::read(&mut &self.0)
    }

    /// Bounds the next read at `left` — or leaves it unbounded, on a socket
    /// the platform will not let us bound.
    ///
    /// macOS answers `setsockopt` with `EINVAL` on a socket that can neither
    /// send nor receive any more, which is where a line the server has hung
    /// up on ends up. Accepting that refusal costs nothing: such a socket is
    /// the one whose reads cannot block — whatever is still buffered comes
    /// back at once, and then the end of the line. So the read is worth
    /// making anyway, and it is the read that answers the caller. Failing on
    /// the bookkeeping call instead would report how a timeout went to a
    /// test that asked how a session ended.
    fn bound_the_next_read(&self, left: Duration) {
        let _ = self.0.set_read_timeout(Some(left));
    }

    fn join(addr: SocketAddr) -> (Self, PlayerId, Vec2, Vec2) {
        let (client, id, spawn, facing, _token) = Self::join_presenting(addr, None);
        (client, id, spawn, facing)
    }

    /// The whole handshake, papers and all: hello, hear which world, present
    /// the token (or none), and take the welcome — with the token it dealt,
    /// which is what a returning join presents next time.
    fn join_presenting(
        addr: SocketAddr,
        presenting: Option<Token>,
    ) -> (Self, PlayerId, Vec2, Vec2, Token) {
        let client = Self::connect(addr);
        client.say(ToServer::Hello {
            version: PROTOCOL_VERSION,
        });
        match client.hear() {
            ToClient::World { .. } => {}
            other => panic!("expected to hear which world, heard {other:?}"),
        }
        client.say(ToServer::Papers { token: presenting });
        match client.hear() {
            ToClient::Welcome {
                id,
                spawn,
                facing,
                token,
                ..
            } => (client, id, spawn, facing, token),
            other => panic!("expected a welcome, heard {other:?}"),
        }
    }

    /// The same handshake, keeping the whole welcome — for the tests that
    /// are about the boats a player enters with.
    fn join_aboard(
        addr: SocketAddr,
        presenting: Option<Token>,
    ) -> (Self, PlayerId, Vec2, Token, Option<protocol::BoatId>) {
        let client = Self::connect(addr);
        client.say(ToServer::Hello {
            version: PROTOCOL_VERSION,
        });
        match client.hear() {
            ToClient::World { .. } => {}
            other => panic!("expected to hear which world, heard {other:?}"),
        }
        client.say(ToServer::Papers { token: presenting });
        match client.hear() {
            ToClient::Welcome {
                id,
                spawn,
                token,
                aboard,
                ..
            } => (client, id, spawn, token, aboard),
            other => panic!("expected a welcome, heard {other:?}"),
        }
    }

    /// The next word about a boat, ignoring everything else — bounded like
    /// the beasts' reader, the session chattering on regardless.
    fn hear_a_boat(&self) -> (protocol::BoatId, Vec2, f32, Option<PlayerId>) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let ToClient::Boat {
                id, hull, occupant, ..
            } = self.hear_by(deadline, "word of any boat")
            {
                return (id, hull.at, hull.heading, occupant);
            }
        }
    }

    /// The next word about a boat, kind and all — for the tests about what a
    /// hull *is*, where [`Client::hear_a_boat`] only cares where it lies and
    /// whose it is.
    fn hear_a_boat_kinded(&self) -> (protocol::BoatId, BoatKind, Vec2, f32, Option<PlayerId>) {
        self.hear_a_boat_kinded_by(Instant::now() + PATIENCE, "word of any boat")
    }

    /// The same, on a deadline a caller already started and in the caller's
    /// own words — so that a reader looping over this is bounded as a whole
    /// rather than granting a fresh wait to every hull that is not the one it
    /// wants, and says what *it* was waiting for when the whole runs out.
    /// Hulls keep arriving while such a reader waits, so its own "word of any
    /// boat" is the one thing that did not fail.
    fn hear_a_boat_kinded_by(
        &self,
        deadline: Instant,
        awaited: &str,
    ) -> (protocol::BoatId, BoatKind, Vec2, f32, Option<PlayerId>) {
        loop {
            if let ToClient::Boat {
                id,
                kind,
                hull,
                occupant,
                ..
            } = self.hear_by(deadline, awaited)
            {
                return (id, kind, hull.at, hull.heading, occupant);
            }
        }
    }

    /// Reads on until the world says this hull is in the hands named.
    ///
    /// Which is how a test waits out an ask it is about to hang up behind.
    /// Dropping a socket with the session's chatter still unread *resets* the
    /// connection rather than closing it, and a reset throws away whatever the
    /// server had not got round to reading — so a client that says three
    /// things and drops can lose the third. Hearing the answer to the last of
    /// them is the proof that nothing is still in flight to be thrown away.
    fn boat_changed_hands(&self, boat: protocol::BoatId, to: Option<PlayerId>) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let (told, _kind, _at, _heading, occupant) =
                self.hear_a_boat_kinded_by(deadline, "that hull changing hands");
            if told == boat && occupant == to {
                return;
            }
        }
    }

    /// The next word about a boat as a tow reads it: which hull, whose
    /// hands are on it, and whose painter it is on.
    fn hear_a_painter(&self) -> (protocol::BoatId, Option<PlayerId>, Option<protocol::BoatId>) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let ToClient::Boat {
                id,
                occupant,
                towed_by,
                ..
            } = self.hear_by(deadline, "word of any boat")
            {
                return (id, occupant, towed_by);
            }
        }
    }

    /// Reads on until the world says this hull lies at `at`, and says whose
    /// painter it was on when it did — how an onlooker watches a tender go
    /// where its ship's reports put it.
    fn boat_came_to(&self, boat: protocol::BoatId, at: Vec2) -> Option<protocol::BoatId> {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let ToClient::Boat {
                id, hull, towed_by, ..
            } = self.hear_by(deadline, "that hull arriving")
            {
                if id == boat && hull.at == at {
                    return towed_by;
                }
            }
        }
    }

    /// The next word about a rowing boat, ignoring everything else.
    ///
    /// Which is how a test learns the name of a hull it asked the console
    /// for — the telling is the only place the world says it — and reading
    /// for the kind rather than off the top of the inbox is what keeps
    /// somebody else's sloop arriving in the same breath from being mistaken
    /// for it.
    fn hear_a_rowboat(&self) -> protocol::BoatId {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let (told, kind, ..) =
                self.hear_a_boat_kinded_by(deadline, "anything going over the side");
            if kind == BoatKind::Rowboat {
                return told;
            }
        }
    }

    /// Says a word the world always answers and reads until the answer comes
    /// back, which waits out everything said before it.
    ///
    /// The same proof as [`Client::boat_changed_hands`] for the asks that have
    /// no telling of their own to wait on: a move and a helm report are
    /// broadcast to everybody *but* the player who made them, so there is
    /// nothing of theirs coming back to read. One connection is read in order,
    /// so a reply to a later word is a reply after an earlier one was acted
    /// on.
    fn caught_up(&self) {
        self.say(ToServer::Command {
            line: "help".to_string(),
        });
        let deadline = Instant::now() + PATIENCE;
        loop {
            if matches!(
                self.hear_by(deadline, "a word back from the world"),
                ToClient::Reply { .. }
            ) {
                return;
            }
        }
    }

    /// The next word about a boat, whole: which hull, what it is, how it
    /// lies, whose hands are on it and whose painter it is on.
    fn hear_a_hull(
        &self,
    ) -> (
        protocol::BoatId,
        BoatKind,
        Underway,
        Option<PlayerId>,
        Option<protocol::BoatId>,
    ) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let ToClient::Boat {
                id,
                kind,
                hull,
                occupant,
                towed_by,
                ..
            } = self.hear_by(deadline, "word of any boat")
            {
                return (id, kind, hull, occupant, towed_by);
            }
        }
    }

    /// The next word about one hull in particular, whole: where it lies and
    /// how it is going, whose hands are on it, whose painter it is on, and
    /// where its hook lies. Everything else said meanwhile is let go by.
    fn hear_of(
        &self,
        boat: protocol::BoatId,
        deadline: Instant,
        awaited: &str,
    ) -> (
        Underway,
        Option<PlayerId>,
        Option<protocol::BoatId>,
        Option<Vec2>,
    ) {
        loop {
            if let ToClient::Boat {
                id,
                hull,
                occupant,
                towed_by,
                anchor,
                ..
            } = self.hear_by(deadline, awaited)
            {
                if id == boat {
                    return (hull, occupant, towed_by, anchor);
                }
            }
        }
    }

    /// A console line, said and answered.
    fn order(&self, line: &str) -> String {
        self.say(ToServer::Command {
            line: line.to_string(),
        });
        self.hear_reply()
    }

    /// The boat on a ship's painter, as the world tells it: reads on until
    /// a hull towed by that ship is told, and says which and where it lies.
    /// How a newcomer learns the name of the boat they entered with.
    fn hear_the_tender_of(&self, ship: protocol::BoatId) -> (protocol::BoatId, Vec2) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let ToClient::Boat {
                id, hull, towed_by, ..
            } = self.hear_by(deadline, "the boat on that ship's painter")
            {
                if towed_by == Some(ship) {
                    return (id, hull.at);
                }
            }
        }
    }

    /// Asks for one chunk and takes the answer to it. Nothing else is in
    /// flight in the tests that use this, so the next message is the answer.
    fn ask_for(&self, chunk: IVec2) -> Option<protocol::ChunkPayload> {
        self.say(ToServer::WantChunk { chunk });
        match self.hear() {
            ToClient::Chunk {
                chunk: answered,
                ground,
                ..
            } => {
                assert_eq!(answered, chunk, "an answer about the wrong chunk");
                ground
            }
            other => panic!("expected ground, heard {other:?}"),
        }
    }

    fn say(&self, message: ToServer) {
        message.write(&mut &self.0).expect("write");
    }

    /// The next message that is *about* something. The sky is skipped — both
    /// the weather and the time of day — and so are the beasts: each is sent
    /// on joining and again on a clock nothing in a test controls, so any
    /// assertion about message order would be flaky against them. The tests
    /// that *are* about the sky or the beasts read for what they want with
    /// [`Client::hear_the_time`] and [`Client::hear_a_beast`]. The console
    /// vocabulary is skipped with them, being part of the same joining
    /// chatter; the newcomer test reads it raw. The survey too — every step
    /// anybody takes may put more ink on their own chart, and the tests that
    /// are about that read for it with [`Client::hear_the_survey`].
    fn hear(&self) -> ToClient {
        let deadline = Instant::now() + PATIENCE;
        loop {
            match self.hear_by(deadline, "a message about anything") {
                ToClient::Weather { .. }
                | ToClient::Daylight { .. }
                | ToClient::Beast { .. }
                | ToClient::BeastGone { .. }
                | ToClient::Boat { .. }
                | ToClient::BoatGone { .. }
                | ToClient::Surveyed { .. }
                | ToClient::Cairn { .. }
                | ToClient::Uncharted
                | ToClient::Vocabulary { .. } => continue,
                message => return message,
            }
        }
    }

    /// Every chunk this client has been told it has surveyed, gathered onto
    /// `charted` until `enough` is happy with them — bounded like the beasts'
    /// reader, since the rest of the session chatters on regardless and a
    /// survey that never arrives must fail a test rather than hang it.
    ///
    /// Gathered *onto* what a test already holds, because a chunk is told
    /// once: a survey read in two goes is two halves of one chart, and the
    /// second go alone would look like ground that had gone missing.
    fn hear_the_survey(
        &self,
        mut charted: HashMap<IVec2, Soundings>,
        enough: impl Fn(&HashMap<IVec2, Soundings>) -> bool,
    ) -> HashMap<IVec2, Soundings> {
        let deadline = Instant::now() + SURVEY_PATIENCE;
        if enough(&charted) {
            return charted;
        }
        loop {
            let awaited = format!("a survey past its {} chunks", charted.len());
            if let ToClient::Surveyed { found } = self.hear_by(deadline, &awaited) {
                charted.extend(found);
                if enough(&charted) {
                    return charted;
                }
            }
        }
    }

    /// The next word about a cairn, ignoring everything else — bounded like
    /// the beasts' reader, the session chattering on regardless.
    fn hear_a_cairn(&self) -> (IVec2, Vec2, String, bool) {
        self.hear_a_cairn_by(Instant::now() + PATIENCE, "word of any cairn")
    }

    /// The same, on a caller's deadline and in a caller's words — see
    /// [`Client::hear_a_boat_kinded_by`].
    fn hear_a_cairn_by(&self, deadline: Instant, awaited: &str) -> (IVec2, Vec2, String, bool) {
        let (island, at, _covers, name, yours) = self.hear_a_cairn_whole_by(deadline, awaited);
        (island, at, name, yours)
    }

    /// The same, with the claim's reach — for the tests that are about what a
    /// cairn says it covers.
    fn hear_a_cairn_whole(&self) -> (IVec2, Vec2, (Vec2, Vec2), String, bool) {
        self.hear_a_cairn_whole_by(Instant::now() + PATIENCE, "word of any cairn")
    }

    #[allow(clippy::type_complexity)]
    fn hear_a_cairn_whole_by(
        &self,
        deadline: Instant,
        awaited: &str,
    ) -> (IVec2, Vec2, (Vec2, Vec2), String, bool) {
        loop {
            if let ToClient::Cairn {
                island,
                at,
                covers,
                name,
                yours,
            } = self.hear_by(deadline, awaited)
            {
                return (island, at, covers, name, yours);
            }
        }
    }

    /// The next answer that there is more coast here than the asker has
    /// surveyed — the one refusal with something to say. A cairn arriving
    /// instead is a grant that should not have happened, and fails loudly.
    fn hear_uncharted(&self) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            match self.hear_by(deadline, "word that the survey was unfinished") {
                ToClient::Uncharted => return,
                ToClient::Cairn { .. } => {
                    panic!("a cairn was raised on an unfinished survey")
                }
                _ => {}
            }
        }
    }

    /// The next word about a cairn whose name `wanted` is happy with, ignoring
    /// the words before it.
    ///
    /// What a client hears about one cairn is a sequence and not an event: the
    /// stones are told when they come into sight and told again when they are
    /// walked up to and read. A test about the second would otherwise be a test
    /// about how many of the first happened to have arrived first, which
    /// depends on where the world put somebody down.
    fn hear_a_cairn_saying(&self, wanted: impl Fn(&str) -> bool) -> (IVec2, Vec2, String, bool) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let told = self.hear_a_cairn_by(deadline, "a cairn saying the wanted thing");
            if wanted(&told.2) {
                return told;
            }
        }
    }

    /// Whether the session said nothing about any cairn — which is what a
    /// refusal with no state to show sounds like.
    ///
    /// Silence cannot be waited for, so it is bracketed instead: `help` is
    /// answered to the asker alone, and one client's words are read in the
    /// order it says them, so a reply to a line typed after the ask is proof
    /// that whatever the ask had to say has been said already.
    fn nothing_was_said_about_a_cairn(&self) -> bool {
        self.say(ToServer::Command {
            line: "help".to_string(),
        });
        let deadline = Instant::now() + PATIENCE;
        loop {
            match self.hear_by(deadline, "the reply that brackets the ask") {
                ToClient::Cairn { .. } => return false,
                ToClient::Reply { .. } => return true,
                _ => {}
            }
        }
    }

    /// Whether the session said nothing about any boat — which is what a
    /// refusal with no state to show sounds like on this half of the wire:
    /// a crossing that was not granted, or an ask after a hull that is not
    /// there to answer.
    ///
    /// Bracketed rather than waited for, exactly as
    /// [`Client::nothing_was_said_about_a_cairn`] is, and for the same
    /// reason: silence has no arrival to wait for, so a reply to a line
    /// typed *after* the ask is the proof that whatever the ask had to say
    /// has been said already. Word of a hull in `known` is not a hull
    /// dealt: the sea says its piece about the empty ones whenever it moves
    /// them, a jump being as good a moment as any.
    fn no_hull_was_dealt(&self, known: &[protocol::BoatId]) -> bool {
        self.say(ToServer::Command {
            line: "help".to_string(),
        });
        let deadline = Instant::now() + PATIENCE;
        loop {
            match self.hear_by(deadline, "the reply that brackets the ask") {
                ToClient::Boat { id, .. } if !known.contains(&id) => return false,
                ToClient::Reply { .. } => return true,
                _ => {}
            }
        }
    }

    /// The next word about a hull not in `known` — the one a `grant` deals,
    /// read past the sea's word about the hulls this client already knows,
    /// which it turns bow to wind and tells as they turn.
    fn hear_a_hull_dealt(
        &self,
        known: &[protocol::BoatId],
    ) -> (protocol::BoatId, BoatKind, Vec2, f32, Option<PlayerId>) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let heard = self.hear_a_boat_kinded_by(deadline, "a hull dealt");
            if !known.contains(&heard.0) {
                return heard;
            }
        }
    }

    /// Every boat this client was introduced to on joining, in the order
    /// told — which hull, what it is, and whose painter it is on: what a
    /// fresh arrival finds already floating in the world.
    ///
    /// Bracketed rather than counted out one by one, on the same reasoning as
    /// [`Client::no_hull_was_dealt`]: a `help` typed after the
    /// welcome is answered after everything the joining itself had to say, so
    /// the reply is the end of the burst. Each hull once, as it was first
    /// told: the sea may say more about an empty one before the reply lands.
    fn boats_introduced(&self) -> Vec<(protocol::BoatId, BoatKind, Option<protocol::BoatId>)> {
        self.say(ToServer::Command {
            line: "help".to_string(),
        });
        let mut told: Vec<(protocol::BoatId, BoatKind, Option<protocol::BoatId>)> = Vec::new();
        let deadline = Instant::now() + PATIENCE;
        loop {
            match self.hear_by(deadline, "the reply that ends the welcome") {
                ToClient::Boat {
                    id, kind, towed_by, ..
                } => {
                    if !told.iter().any(|(known, ..)| *known == id) {
                        told.push((id, kind, towed_by));
                    }
                }
                ToClient::Reply { .. } => return told,
                _ => {}
            }
        }
    }

    /// The next `how_many` hulls the world takes back, in the order told,
    /// ignoring everything else — bounded like every other reader here.
    fn hear_hulls_gone(&self, how_many: usize) -> Vec<protocol::BoatId> {
        let deadline = Instant::now() + PATIENCE;
        let mut gone = Vec::new();
        while gone.len() < how_many {
            if let ToClient::BoatGone { id } = self.hear_by(
                deadline,
                &format!("{how_many} hulls taken out of the world"),
            ) {
                gone.push(id);
            }
        }
        gone
    }

    /// Whether the world has taken back no hull this client has not already
    /// heard of — bracketed by a `help`, on
    /// [`Client::nothing_was_said_about_a_boat`]'s reasoning.
    fn no_hull_was_retired(&self) -> bool {
        self.say(ToServer::Command {
            line: "help".to_string(),
        });
        let deadline = Instant::now() + PATIENCE;
        loop {
            match self.hear_by(deadline, "the reply that brackets the ask") {
                ToClient::BoatGone { .. } => return false,
                ToClient::Reply { .. } => return true,
                _ => {}
            }
        }
    }

    /// Runs the world's day on by at least one whole day, through the
    /// console — the one way a test moves the world's clock other than
    /// waiting for it. A hull's grace is a day of that clock, so after this
    /// every hull that lay empty before it has lain empty long enough.
    fn wind_on_a_day(&self) {
        // Twice: the first ask reaches the next noon, which may be moments
        // away, and the second is then a whole day on from it.
        for _ in 0..2 {
            self.say(ToServer::Command {
                line: "world time 12".to_string(),
            });
            let answer = self.hear_reply();
            assert!(answer.contains("12:00"), "the day did not run on: {answer}");
        }
    }

    /// The next word about a beast of the wanted kind, ignoring everything
    /// else — bounded, because the rest of the session chatters on
    /// regardless and a world that raises no such beast would otherwise keep
    /// this reading forever.
    fn hear_a_beast(&self, wanted: BeastKind) -> (BeastId, Vec2, Vec2) {
        let deadline = Instant::now() + PATIENCE;
        let awaited = format!("a {wanted:?} in these waters");
        loop {
            if let ToClient::Beast {
                id,
                kind,
                position,
                velocity,
                ..
            } = self.hear_by(deadline, &awaited)
            {
                if kind == wanted {
                    return (id, position, velocity);
                }
            }
        }
    }

    /// Every beast there is, as one beat tells them.
    ///
    /// A beat says the whole flock in one burst, under the roster's own lock,
    /// so a client either sees all of a beat or none of it — which makes an id
    /// coming round a second time the end of the first complete beat, and this
    /// reads exactly that far. Bounded like the reader above, and for the same
    /// reason.
    ///
    /// This rather than [`Client::hear_a_beast`] wherever a test means *the
    /// animals*, plural: waters that have had a whale summoned into them hold
    /// that whale and the one they were going to raise anyway, and "the next
    /// whale" is then a question with two right answers.
    fn hear_the_flock(&self) -> Vec<(BeastId, BeastKind, Vec2)> {
        let deadline = Instant::now() + PATIENCE;
        let mut beat: Vec<(BeastId, BeastKind, Vec2)> = Vec::new();
        loop {
            if let ToClient::Beast {
                id, kind, position, ..
            } = self.hear_by(deadline, "a whole beat of the beasts")
            {
                if beat.iter().any(|(told, ..)| *told == id) {
                    return beat;
                }
                beat.push((id, kind, position));
            }
        }
    }

    /// The next word putting this player down somewhere, with nothing said
    /// about any occupied boat on the way to it.
    ///
    /// The two are one reading because the order is what makes them one
    /// question. A jump at a helm moves a hull whose own client is the
    /// authority on it, so the world tells everybody *but* the asker — and a
    /// telling that wrongly went to the asker too would go out *before* the
    /// put down, since the broadcast comes first. Read for the put down
    /// alone, skipping whatever came before it, and the mistake would
    /// already be behind the reader by the time anything looked. Only an
    /// *occupied* hull is the mistake: the sea tells of the empty ones
    /// whenever it moves them, and a jump is as good a moment as any.
    fn hear_put_down_alone(&self) -> (Vec2, Option<f32>) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            match self.hear_by(deadline, "anybody being put down anywhere") {
                ToClient::PutDown { position, heading } => return (position, heading),
                ToClient::Boat {
                    id,
                    occupant: Some(_),
                    ..
                } => {
                    panic!("the world told this client where its own {id:?} is")
                }
                _ => {}
            }
        }
    }

    /// The next answer to a console line, ignoring everything else — the
    /// session goes on introducing players and telling the sky around a
    /// command, and none of that is what a reply is about.
    fn hear_reply(&self) -> String {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let ToClient::Reply { text } = self.hear_by(deadline, "an answer to a console line")
            {
                return text;
            }
        }
    }

    /// The next word on what time it is, ignoring everything else.
    fn hear_the_time(&self) -> f32 {
        self.hear_the_time_by(Instant::now() + PATIENCE, "the time of day")
    }

    /// The same, on a caller's deadline and in a caller's words — see
    /// [`Client::hear_a_boat_kinded_by`].
    fn hear_the_time_by(&self, deadline: Instant, awaited: &str) -> f32 {
        loop {
            if let ToClient::Daylight { phase } = self.hear_by(deadline, awaited) {
                return phase;
            }
        }
    }

    /// Asks for the night to be over and reads the next telling of the time:
    /// the hour told, and how far the day moved on from `since` over and
    /// above what the wall clock in between moved it anyway — a wound tick's
    /// worth or nothing, which is what [`PLAINLY_WOUND`] tells apart.
    fn ask_for_dawn_by(&self, since: f32, deadline: Instant, awaited: &str) -> (f32, f32) {
        let asked = Instant::now();
        self.say(ToServer::WantDawn);
        let told = self.hear_the_time_by(deadline, awaited);
        let step = (told - since).rem_euclid(1.0);
        let own = asked.elapsed().as_secs_f32() / protocol::DAY_SECONDS;
        (told, step - own)
    }

    /// Reads until the line gives out, and says how. Whatever the session had
    /// already said is drained first: a client that has not been listening is
    /// still owed its messages, and the question here is only how the
    /// conversation ends.
    ///
    /// A read that merely ran out of time is not the line giving out, and is
    /// the one error this must not return: a session still perfectly open
    /// would otherwise be reported as having hung up, and every test asking
    /// *how* it ended would be handed a `WouldBlock` to answer with.
    fn until_hung_up(&self) -> std::io::Error {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "the session never hung up");
            self.bound_the_next_read(left);
            match ToClient::read(&mut &self.0) {
                Ok(_) => {}
                Err(why) if ran_out_of_time(&why) => panic!("the session never hung up"),
                Err(why) => return why,
            }
        }
    }

    /// Which world stands at this address — the handshake as far as the
    /// server naming itself, and no further.
    fn which_world(addr: SocketAddr) -> protocol::WorldId {
        let client = Self::connect(addr);
        client.say(ToServer::Hello {
            version: PROTOCOL_VERSION,
        });
        match client.hear() {
            ToClient::World { id } => id,
            other => panic!("expected to hear which world, heard {other:?}"),
        }
    }
}

/// A directory of this test's own under the system's temporary space, so
/// parallel tests cannot see each other's worlds.
///
/// Emptied on the way in, which matters more than it looks: the name carries
/// the process id, nothing ever sweeps these up, and a system that has run
/// these tests a few hundred times will hand a pid back out again eventually.
/// A test that found the last run's world file still sitting there would open
/// *that* world instead of making its own — and would fail with something
/// about papers or a hour rather than anything to do with what it was testing,
/// on a machine that had done nothing wrong except run the suite often enough.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir()
        .join("genovesa-session-tests")
        .join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp space");
    dir
}

#[test]
fn a_client_is_welcomed_with_somewhere_to_stand_and_something_to_look_at() {
    let addr = host(7);
    let (_client, _id, spawn, facing) = Client::join(addr);

    // The welcome no longer names a seed — a client has nothing to generate,
    // so the world it is in is a place rather than a number. What identifies
    // it is where the player was put down: players enter on the world's
    // entry, the open water around the origin, scattered a few boat-lengths
    // so arrivals don't stack.
    assert!(
        spawn.distance(ENTRY) <= server::SPAWN_SCATTER,
        "{spawn} is not the patch of water players enter on"
    );

    // And the view opens towards the nearest land, which the client could
    // not have worked out for itself.
    let first = behind_the_curtain(7)
        .first_land()
        .expect("the seed offers land to face");
    assert_eq!(facing, first.centre());
}

#[test]
fn a_newcomer_is_told_the_sky_before_anything_else_happens() {
    // The sky arrives straight after the welcome, before the client has said
    // or asked anything: a client draws the sea and the light from its first
    // frame, and either of them drawn under an assumed sky would visibly
    // change its mind moments in. Read raw rather than through `hear`, which
    // exists to skip exactly these messages everywhere else.
    let addr = host(7);
    let client = Client::connect(addr);
    client.say(ToServer::Hello {
        version: PROTOCOL_VERSION,
    });
    match client.hear_by(Instant::now() + PATIENCE, "which world") {
        ToClient::World { .. } => {}
        other => panic!("expected to hear which world, heard {other:?}"),
    }
    client.say(ToServer::Papers { token: None });
    match client.hear_by(Instant::now() + PATIENCE, "a welcome") {
        ToClient::Welcome { .. } => {}
        other => panic!("expected a welcome, heard {other:?}"),
    }
    match client.hear_by(Instant::now() + PATIENCE, "the weather") {
        ToClient::Weather { wind } => {
            assert!(wind.is_finite(), "the wind blows {wind}");
            // The same answer the pure function gives for this seed at the
            // server's age — no exact pin, the server's clock not being the
            // test's to read, but a session seconds old is in its first
            // moments of weather and the wind must be a plausible one.
            assert!(
                wind.length() <= 16.0,
                "{} m/s is past the gale",
                wind.length()
            );
        }
        other => panic!("expected the weather, heard {other:?}"),
    }
    // And the hour, for the same reason: a world drawn at an assumed midday
    // would correct itself to a night moments after the player arrived in it.
    match client.hear_by(Instant::now() + PATIENCE, "the time of day") {
        ToClient::Daylight { phase } => assert!(
            (phase - server::OPENING).abs() < 0.01,
            "a world seconds old opened at {phase} rather than its morning"
        ),
        other => panic!("expected the time of day, heard {other:?}"),
    }
    // Then the console's words, before the client has asked anything —
    // completion is only worth having from the first line typed.
    match client.hear_by(Instant::now() + PATIENCE, "the console's vocabulary") {
        ToClient::Vocabulary { phrases } => {
            assert!(
                phrases.iter().any(|phrase| phrase == "help"),
                "no `help` among {phrases:?}"
            );
        }
        other => panic!("expected the vocabulary, heard {other:?}"),
    }
}

#[test]
fn ground_is_asked_for_by_chunk_and_answered_either_way() {
    let addr = host(7);
    let (client, _id, spawn, facing) = Client::join(addr);

    // The middle of the island the player was put down beside is ground, and
    // it arrives as heights and surfaces rather than as anything about how it
    // was made.
    let land = client
        .ask_for(chunk_at(facing))
        .expect("the middle of an island should be ground");
    assert!(land.well_formed());
    let highest = land
        .heights
        .iter()
        .map(|h| dequantize(*h))
        .fold(f32::MIN, f32::max);
    assert!(
        highest > 0.0,
        "an island's middle came back entirely under water, at {highest} m"
    );

    // Open water carries nothing. Walked out from the spawn until the test's
    // own copy of the world says there is no island answerable for a chunk —
    // which is exactly what the server will find when it looks.
    let world = behind_the_curtain(7);
    let ocean = (1..200)
        .map(|i| chunk_at(spawn) + IVec2::splat(i))
        .find(|chunk| {
            let middle = (chunk.as_vec2() + 0.5) * CHUNK_METRES;
            world.island_at(middle.x, middle.y).is_none()
        })
        .expect("some open ocean within a few kilometres");
    assert_eq!(client.ask_for(ocean), None, "open water carried a payload");
}

#[test]
fn ground_can_be_asked_for_out_of_order_and_comes_back_labelled() {
    // Answers are labelled with the chunk they are about because they need
    // not arrive in the order they were asked for: open water costs nothing
    // and an island costs hundreds of milliseconds, so a later request is
    // often the first one answered. A client keys everything by coordinate,
    // and this is what lets it.
    let addr = host(3);
    let (client, _id, spawn, _facing) = Client::join(addr);
    let asked: Vec<IVec2> = (0..8)
        .map(|i| chunk_at(spawn) + IVec2::new(i, -i))
        .collect();

    for chunk in &asked {
        client.say(ToServer::WantChunk { chunk: *chunk });
    }

    let mut answered = Vec::new();
    for _ in 0..asked.len() {
        match client.hear() {
            ToClient::Chunk { chunk, .. } => answered.push(chunk),
            other => panic!("expected ground, heard {other:?}"),
        }
    }
    answered.sort_by_key(|chunk| (chunk.x, chunk.y));
    let mut expected = asked.clone();
    expected.sort_by_key(|chunk| (chunk.x, chunk.y));
    assert_eq!(answered, expected, "every request is answered exactly once");
}

#[test]
fn two_clients_asking_for_one_chunk_get_the_same_ground() {
    // One world behind both of them, not two generations of it that merely
    // ought to agree — which is what makes a session one place.
    let addr = host(7);
    let (alice, _a, _spawn, facing) = Client::join(addr);
    let (bob, _b, _spawn, _facing) = Client::join(addr);
    // The introductions each is owed, out of the way, so that the next thing
    // either hears is the ground it asked for.
    let _ = alice.hear();
    let _ = bob.hear();

    let chunk = chunk_at(facing);
    assert_eq!(alice.ask_for(chunk), bob.ask_for(chunk));
}

#[test]
fn a_chunk_no_player_could_stand_in_ends_the_session() {
    // The twin of the position check below: coordinates past where the world
    // resolves are a broken or hostile client, and answering would put a
    // worker to work on ground made of arithmetic that has run out.
    let addr = host(1);
    let (alice, _a, _, _) = Client::join(addr);
    let (bob, b, _, _) = Client::join(addr);
    let _ = alice.hear(); // Bob's arrival

    bob.say(ToServer::WantChunk {
        chunk: IVec2::new(i32::MAX, 0),
    });
    assert_eq!(alice.hear(), ToClient::Left { id: b });
}

#[test]
fn two_players_are_never_put_down_in_the_same_spot() {
    // Otherwise the first thing a joined session shows is one marker where
    // there are two players.
    let addr = host(1);
    let (_alice, _a, first, _) = Client::join(addr);
    let (_bob, _b, second, _) = Client::join(addr);
    assert_ne!(first, second, "two players' markers would stack");
}

#[test]
fn a_position_no_player_could_reach_ends_the_session() {
    let addr = host(1);
    let (alice, _a, _, _) = Client::join(addr);
    let (bob, b, _, _) = Client::join(addr);
    let _ = alice.hear(); // Bob's arrival

    // Far enough out that `f32` has stopped resolving the ground, which no
    // client walks to and every client would be dragged towards.
    bob.say(ToServer::Move {
        position: Vec2::new(1e30, 0.0),
    });
    assert_eq!(alice.hear(), ToClient::Left { id: b });
}

#[test]
fn the_wrong_dialect_is_refused() {
    let addr = host(1);
    let client = Client::connect(addr);
    client.say(ToServer::Hello {
        version: PROTOCOL_VERSION + 1,
    });

    // The refusal names the version the server wanted, and then the line is
    // closed — there is nothing left to negotiate.
    assert_eq!(
        client.hear(),
        ToClient::Refused {
            version: PROTOCOL_VERSION
        }
    );
    client.until_hung_up();
}

#[test]
fn players_meet_move_and_part() {
    let addr = host(1);
    let (alice, a, alices_spawn, _) = Client::join(addr);
    let (bob, b, bobs_spawn, _) = Client::join(addr);
    assert_ne!(a, b, "two players were dealt one id");

    // Introductions both ways: the newcomer hears who was already here, and
    // whoever is here hears the newcomer — each at their own spawn, arrivals
    // being scattered around the island rather than stacked on its centre.
    assert_eq!(
        bob.hear(),
        ToClient::Joined {
            id: a,
            position: alices_spawn
        }
    );
    assert_eq!(
        alice.hear(),
        ToClient::Joined {
            id: b,
            position: bobs_spawn
        }
    );

    // Bob steps ashore first — an arrival is seated at a helm, and a
    // helmsman's `Move` is quietly ignored: one can honestly cross a
    // boarding grant on the wire, and believing it would walk him off a deck
    // everyone else can see him standing on. So the wander below changes
    // nothing anybody hears, and the step off — which is itself a move
    // everyone hears — is the first word about him, from where he really is.
    bob.say(ToServer::Move {
        position: bobs_spawn + Vec2::new(300.0, 300.0),
    });
    bob.say(ToServer::Disembark {
        position: bobs_spawn,
    });
    assert_eq!(
        alice.hear(),
        ToClient::Moved {
            id: b,
            position: bobs_spawn
        }
    );

    // A move reaches the other player, and is not echoed back — Alice hears
    // it, and Bob's next message below is about Carol, not himself.
    let out = Vec2::new(64.0, -32.0);
    bob.say(ToServer::Move { position: out });
    assert_eq!(
        alice.hear(),
        ToClient::Moved {
            id: b,
            position: out
        }
    );

    // A latecomer is introduced to everyone at their current positions.
    // Alice having heard the move is what guarantees the server had processed
    // it before Carol connected. The roster iterates in no particular order,
    // so sort what she hears before pinning it.
    let (carol, c, carols_spawn, _) = Client::join(addr);
    assert_eq!(
        alice.hear(),
        ToClient::Joined {
            id: c,
            position: carols_spawn
        }
    );
    assert_eq!(
        bob.hear(),
        ToClient::Joined {
            id: c,
            position: carols_spawn
        }
    );
    let mut introductions = [carol.hear(), carol.hear()];
    introductions.sort_by_key(|message| match message {
        ToClient::Joined { id, .. } => id.0,
        other => panic!("expected an introduction, heard {other:?}"),
    });
    assert_eq!(
        introductions,
        [
            ToClient::Joined {
                id: a,
                position: alices_spawn
            },
            ToClient::Joined {
                id: b,
                position: out
            },
        ]
    );

    // Hanging up is leaving: everyone else hears it.
    drop(bob);
    assert_eq!(alice.hear(), ToClient::Left { id: b });
    assert_eq!(carol.hear(), ToClient::Left { id: b });
}

#[test]
fn a_spawned_host_serves_the_same_world() {
    // Hosting on a thread is the same session, only reachable from a game that
    // is drawing frames alongside it.
    let host = spawn_host(7);
    let (_client, _id, spawn, _facing) = Client::join(host.addr());
    assert!(spawn.distance(ENTRY) <= server::SPAWN_SCATTER);
}

#[test]
fn dropping_the_host_hangs_up_on_everybody() {
    // A player leaving a world they hosted ends it for the guests too. They
    // are each blocked in a read at the time, so nothing but the host reaching
    // in and closing their sockets can tell them.
    let host = spawn_host(1);
    let (alice, _a, _, _) = Client::join(host.addr());
    let (bob, _b, _, _) = Client::join(host.addr());

    drop(host);

    // End of file rather than a timeout, which is what the read timeout on a
    // test client turns a session that was merely abandoned into.
    for (who, client) in [("alice", &alice), ("bob", &bob)] {
        let error = client.until_hung_up();
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::UnexpectedEof,
            "{who} was left holding a line to a world that has ended: {error}"
        );
    }
}

#[test]
fn a_line_finished_at_both_ends_is_still_read_out() {
    // The test above meets a socket finished in both directions only under
    // load, a few runs in a hundred, and macOS will not set a read timeout on
    // one of those — `EINVAL` from the bookkeeping call, on precisely the
    // socket the reader was called to hear out. So reach that state on
    // purpose: the client hangs up its own sending half, the server hangs up
    // in return, and the line is read to its end before the question is put.
    // What is asked is what the tests above ask, and the answer must be the
    // same — how the session ended, not how the timeout went.
    use std::io::Read;

    let host = spawn_host(1);
    let (client, _id, _, _) = Client::join(host.addr());
    client
        .0
        .shutdown(std::net::Shutdown::Write)
        .expect("hang up the sending half");

    // Read out whatever the session had already said, and the end of the
    // line after it, so that nothing below is waiting on the network.
    let mut sink = [0u8; 1024];
    while (&client.0).read(&mut sink).expect("the end of the line") > 0 {}

    let error = client.until_hung_up();
    assert_eq!(
        error.kind(),
        std::io::ErrorKind::UnexpectedEof,
        "a line finished at both ends was reported as {error}"
    );
}

#[test]
fn a_port_can_be_hosted_again_once_the_host_is_dropped() {
    // Hosting, leaving, and hosting again is an ordinary evening: a new seed
    // from the menu is a new world on the same port. The listener has to be
    // properly gone by the time the drop returns — and so must the connections
    // it was serving, which is the harder half: a guest is still connected
    // when the host leaves, so the host is the end that closes first, and a
    // socket closed from this end is the one that lingers.
    let server = Server::bind(("127.0.0.1", 0), 1).expect("bind");
    let addr = server.local_addr().expect("addr");
    let host = server.spawn().expect("spawn");
    let (guest, _id, _, _) = Client::join(addr);
    drop(host);
    assert_eq!(
        guest.until_hung_up().kind(),
        std::io::ErrorKind::UnexpectedEof
    );

    let again = Server::bind(addr, 2)
        .expect("the port is still held")
        .spawn()
        .expect("spawn");
    let (_client, _id, spawn, facing) = Client::join(again.addr());
    let first = behind_the_curtain(2).first_land().expect("land to face");
    assert!(
        spawn.distance(ENTRY) <= server::SPAWN_SCATTER && facing == first.centre(),
        "the second world is not the one being served"
    );
}

#[test]
fn a_corrupt_client_is_dropped() {
    use std::io::Write;

    let addr = host(1);
    let (alice, _a, _, _) = Client::join(addr);
    let (bob, b, _, _) = Client::join(addr);
    let _ = alice.hear(); // Bob's arrival

    // Bob's stream degenerates into noise: an oversized frame length. The
    // server must give up on him rather than trying to read past it.
    (&bob.0).write_all(&[0xFF, 0xFF, 1, 2, 3]).expect("write");
    assert_eq!(alice.hear(), ToClient::Left { id: b });
}

#[test]
fn a_world_says_what_time_it_is_and_goes_on_saying_so() {
    // Opened mid-afternoon: the first word is that hour, and the clock is
    // running — a later word is a later hour, without anybody asking.
    let addr = host_at(3, 0.6);
    let (client, _id, _, _) = Client::join(addr);

    let opened = client.hear_the_time();
    assert!(
        (opened - 0.6).abs() < 0.01,
        "the world opened at {opened} rather than the hour it was given"
    );

    let later = client.hear_the_time();
    assert!(later > opened, "the day stood still: {opened} then {later}");
}

#[test]
fn a_night_everybody_is_waiting_out_runs_off_to_daybreak() {
    // Opened in the small hours, with one player in the world asking for it
    // to be over. The night has to pass at a pace nobody sits through, and
    // stop at daybreak rather than carrying the morning away with it.
    let addr = host_at(3, 0.10);
    let (client, _id, _, _) = Client::join(addr);

    // Asked until daybreak, the wall clock only a hang guard: the pace is
    // read off the clock's own steps, see `PLAINLY_WOUND`. What is proved is
    // that the night was run off rather than sat through — once it was
    // plainly running, the day passing on its own while the asking lapsed
    // counts for less than the wound ticks do. Against them rather than
    // against a fixed share of the night, because a loaded machine lapses
    // the asking for as long as it likes, and what that must not do is
    // outweigh a run that still happened. A night crawled through unwound
    // ends the loop the same way, having never plainly run at all.
    let opened = client.hear_the_time();
    let deadline = Instant::now() + PATIENCE;
    let mut phase = opened;
    let mut wound = 0.0;
    let mut crawled = 0.0;
    while protocol::is_night(phase) {
        let (told, beyond) = client.ask_for_dawn_by(
            phase,
            deadline,
            &format!("the night to run off from {opened}, at {phase}"),
        );
        if beyond >= PLAINLY_WOUND {
            wound += beyond;
        } else if wound > 0.0 && protocol::is_night(told) {
            // The step onto daybreak is left out: it is the run capped at
            // what was left of the night, which the check below is about.
            crawled += (told - phase).rem_euclid(1.0);
        }
        phase = told;
    }
    assert!(
        wound > 0.0,
        "the night passed at the day's own pace from {opened} to {phase}"
    );
    assert!(
        crawled < wound,
        "the day passed on its own for {crawled} against {wound} run off, with everybody waiting"
    );
    // Landed on daybreak, not somewhere past it: a fast clock that overshot
    // would take the sunrise with it. Within a quarter of a wound tick, so
    // that a run left uncapped could not land inside the tolerance from
    // anywhere in the night.
    assert!(
        phase < protocol::DAYBREAK + 0.005,
        "the night ran past daybreak to {phase}"
    );
}

#[test]
fn a_night_keeps_its_pace_while_somebody_is_still_sailing() {
    // Two in the world and one of them asking. The sky is one sky, so the
    // night stays a night: the player who is still out there does not have
    // it whipped away.
    let addr = host_at(3, 0.10);
    let (alice, _a, _, _) = Client::join(addr);
    let (_bob, _b, _, _) = Client::join(addr);

    let opened = alice.hear_the_time();
    let until = std::time::Instant::now() + Duration::from_secs(1);
    let mut phase = opened;
    while std::time::Instant::now() < until {
        alice.say(ToServer::WantDawn);
        phase = alice.hear_the_time();
    }

    // About a second of a ten-minute day has passed, which is under a
    // hundredth of it; a second of a night running off would be a tenth.
    assert!(
        phase - opened < 0.02,
        "the night ran on from {opened} to {phase} with somebody still sailing"
    );
}

#[test]
fn a_night_stops_running_off_once_the_asking_stops() {
    // The asking is a wish that lapses rather than a switch that is thrown —
    // see `WAIT_LAPSE` — so a client that has gone quiet is one that is no
    // longer asking, whether it has taken the helm again or stopped talking
    // altogether. What it costs to get this wrong is a night that goes on
    // racing under somebody who is sailing again.
    //
    // Opened at the first of the night rather than the last of it, so that
    // what is left to run off outlasts both the asking below and the lapse
    // after it: a night that had already reached daybreak would say nothing
    // about either.
    let addr = host_at(3, 0.82);
    let (client, _id, _, _) = Client::join(addr);

    // Asked until the night is plainly running — one step of the clock that
    // could only be a wound tick, see `PLAINLY_WOUND` — rather than for a
    // fixed while and then asked whether that was long enough. The deadline
    // is only what a night that never started running looks like.
    let opened = client.hear_the_time();
    let giving_up = Instant::now() + PATIENCE;
    let mut phase = opened;
    loop {
        let (told, beyond) = client.ask_for_dawn_by(
            phase,
            giving_up,
            &format!("the night to start running: {opened} then {phase}"),
        );
        phase = told;
        if beyond >= PLAINLY_WOUND {
            break;
        }
    }

    // Then quiet — listened to throughout rather than slept through, so that
    // nothing piles up in the socket for the readings below to mistake for
    // the present. Long enough past the lapse that the last of the fast
    // clock is over well before the first of those readings.
    let quiet = std::time::Instant::now();
    let mut before = phase;
    while quiet.elapsed() < Duration::from_millis(2_500) {
        before = client.hear_the_time();
    }
    assert!(
        protocol::is_night(before),
        "the night ran itself out with nobody asking for it: {before}"
    );

    // And now the day moves at the pace of a day: a couple of real seconds
    // of a ten-minute one is a few thousandths, where a night still running
    // off would be a fifth. Bounded loosely on purpose — what is being told
    // apart here is two paces sixty times apart, and a loaded machine that
    // took a few seconds over this is still nowhere near.
    let measured = std::time::Instant::now();
    let mut after = before;
    while measured.elapsed() < Duration::from_secs(2) {
        after = client.hear_the_time();
    }
    let moved = (after - before).rem_euclid(1.0);
    assert!(
        moved < 0.02,
        "the day moved {moved} from {before} to {after} with nobody asking"
    );
}

#[test]
fn a_stranger_and_a_token_nobody_dealt_both_enter_fresh() {
    // A token this world never dealt is not an offence, just a stranger
    // after all: the join starts them on the spawn with fresh papers, as if
    // they had presented nothing.
    let addr = host(7);
    let (_client, _id, spawn, _facing, dealt) = Client::join_presenting(addr, Some(Token(12_345)));
    assert_ne!(dealt, Token(12_345), "a token nobody dealt was believed");
    assert!(
        spawn.distance(ENTRY) <= server::SPAWN_SCATTER,
        "a stranger was put down somewhere other than the spawn"
    );
}

#[test]
fn two_worlds_from_one_seed_are_two_worlds() {
    // The seed is the geography, not the identity: a client's memory of one
    // world must not be answered by another that merely shares its islands.
    let one = Client::which_world(host(7));
    let other = Client::which_world(host(7));
    assert_ne!(one, other);
}

#[test]
fn leaving_and_rejoining_with_papers_resumes_in_place() {
    // Within one session, no file involved: the world remembers where it
    // last saw each token it dealt, so leaving and coming back is re-entry,
    // not arrival.
    let addr = host(1);
    let (alice, ..) = Client::join(addr);
    let (bob, b, _, _, bobs_token) = Client::join_presenting(addr, None);
    let _ = alice.hear(); // Bob's arrival

    // Bob sails out — he arrived seated at a helm, so the boat is how he
    // goes anywhere. Waited out before the hang-up: a drop with the sailing
    // still unread would throw it away, see [`Client::caught_up`].
    let out = Vec2::new(640.0, -320.0);
    bob.say(ToServer::Helm {
        hull: Underway::lying(out, 0.0),
        tender: None,
    });
    bob.caught_up();
    drop(bob);
    // Alice hearing the departure is what guarantees it is filed: the world
    // remembers a leaver before anyone is told they left.
    assert_eq!(alice.hear(), ToClient::Left { id: b });

    // Where the world saw him, give or take what the sea did with his hull
    // while nobody was aboard — see [`ADRIFT`].
    let (_bob, _id, spawn, facing, dealt) = Client::join_presenting(addr, Some(bobs_token));
    assert!(
        spawn.distance(out) < ADRIFT,
        "Bob was not put back where the world saw him: {spawn} for {out}"
    );
    assert_eq!(
        facing - spawn,
        Vec2::new(0.0, -64.0),
        "a keeper resumed at their helm opens looking past their own bow"
    );
    assert_eq!(dealt, bobs_token, "recognised papers were re-dealt");

    // The same papers presented while their holder is aboard are somebody's
    // copied file: the second arrival enters as a stranger rather than
    // being refused — or worse, being resumed onto the first one's spot.
    let (_copy, _c, elsewhere, _f, fresh) = Client::join_presenting(addr, Some(bobs_token));
    assert_ne!(fresh, bobs_token, "one token was aboard twice");
    assert_ne!(elsewhere, out, "the copy was resumed onto the original");
}

#[test]
fn a_world_is_named_at_birth_and_keeps_the_name_it_was_given() {
    let path = scratch("named").join("one.world");
    // Trimmed and stripped of what a line of the file could not carry —
    // the file's own rule, applied before the name is ever written.
    let first = Server::bind(("127.0.0.1", 0), 7)
        .expect("bind")
        .named("  Windward\nReach ")
        .keeping_at(path.clone())
        .expect("keeping");
    drop(first);
    let listed = server::kept_worlds(path.parent().expect("dir"));
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "WindwardReach");

    // A reopened world already has its name, and a naming on the way back
    // in is a naming of the wrong world.
    let again = Server::reopen(("127.0.0.1", 0), &path)
        .expect("reopen")
        .named("Somewhere Else");
    // Opened and closed, which is what writes the world back down.
    drop(again.spawn().expect("spawn"));
    assert_eq!(
        server::kept_worlds(path.parent().expect("dir"))[0].name,
        "WindwardReach"
    );
}

#[test]
fn a_kept_world_reopens_where_it_left_off() {
    // The whole story: a world kept to a file, sailed, left, reopened — and
    // it is the same world, at the same hour, with the player where the
    // world last saw them.
    let path = scratch("kept").join("one.world");
    let first = Server::bind(("127.0.0.1", 0), 7)
        .expect("bind")
        .opening_at(0.5)
        .keeping_at(path.clone())
        .expect("keeping");
    let addr = first.local_addr().expect("addr");
    let host = first.spawn().expect("spawn");
    let world_before = Client::which_world(addr);

    let (client, _id, _spawn, _facing, token) = Client::join_presenting(addr, None);
    let out = Vec2::new(2_048.0, -512.0);
    client.say(ToServer::Helm {
        hull: Underway::lying(out, 0.0),
        tender: None,
    });
    // An answered chunk is proof the sailing was processed: one connection,
    // read in order. (Hearing the time would prove nothing — the sky is
    // another thread's telling.)
    let _ = client.ask_for(IVec2::new(5_000, 5_000));
    drop(client);
    // Dropping the host is leaving the world: the closing save has happened
    // by the time the drop returns.
    drop(host);

    let again = Server::reopen(("127.0.0.1", 0), &path).expect("reopen");
    assert_eq!(again.seed(), 7, "the file forgot which world it keeps");
    let addr = again.local_addr().expect("addr");
    let _host = again.spawn().expect("spawn");

    // Same world — the id a client keys its own files by survives the
    // reopening...
    assert_eq!(Client::which_world(addr), world_before);

    // ...same player, back where the world last saw them — give or take the
    // sea's hand on a hull left unanchored, see [`ADRIFT`]...
    let (client, _id, spawn, _facing, dealt) = Client::join_presenting(addr, Some(token));
    assert!(
        spawn.distance(out) < ADRIFT,
        "the world forgot where it last saw its player: {spawn} for {out}"
    );
    assert_eq!(dealt, token, "kept papers were re-dealt");

    // ...and the same afternoon: the clock stands where it stood, moved only
    // by the seconds the world was actually open. While it was closed, no
    // time passed at all — which is what makes quitting mid-storm and
    // reloading land back in the storm.
    let hour = client.hear_the_time();
    assert!(
        (0.5..0.53).contains(&hour),
        "the world reopened at {hour} rather than the afternoon it closed on"
    );
}

#[test]
fn a_newcomer_enters_at_the_helm_of_a_minted_ship_with_its_boat_astern() {
    let addr = host(7);
    let (client, id, spawn, _token, aboard) = Client::join_aboard(addr, None);
    let ship = aboard.expect("a newcomer's story starts aboard");

    // The hull is introduced to its own helmsman like any other: standing
    // where they stand, with their hands on the helm — and before the boat
    // on its painter, which a client only takes in tow once it holds the
    // helm the telling says it is behind.
    let (told, kind, hull, occupant, towed_by) = client.hear_a_hull();
    assert_eq!(told, ship, "introduced to somebody else's boat first");
    assert_eq!(kind, BoatKind::Sloop);
    assert_eq!(hull.at, spawn, "the minted hull is not under its helmsman");
    assert_eq!(occupant, Some(id), "the newcomer is not at their own helm");
    assert_eq!(towed_by, None, "a ship on a painter");

    // Then its boat: a painter's length astern, on the painter, empty.
    let (tender, kind, boat, occupant, towed_by) = client.hear_a_hull();
    assert_ne!(tender, ship);
    assert_eq!(kind, BoatKind::Rowboat);
    assert_eq!(occupant, None, "somebody is in the boat astern");
    assert_eq!(
        towed_by,
        Some(ship),
        "the boat is not on its ship's painter"
    );
    let ahead = Vec2::new(-hull.heading.sin(), -hull.heading.cos());
    assert!(
        (boat.at - spawn).dot(ahead) < 0.0,
        "the boat lies at {} ahead of a ship at {spawn} pointing {ahead}",
        boat.at
    );
    assert!(
        ((boat.at - spawn).length() - protocol::TENDER_ASTERN).abs() < 1e-3,
        "the boat lies {} m from its ship, not a painter's length",
        (boat.at - spawn).length()
    );
}

#[test]
fn a_boat_left_at_anchor_is_anyones_within_reach() {
    let addr = host(1);
    let (alice, a, alices_spawn, _t, a_boat) = Client::join_aboard(addr, None);
    let a_boat = a_boat.expect("aboard");
    let (bob, b, bobs_spawn, _t2, _b_boat) = Client::join_aboard(addr, None);
    // Bob steps off his own boat first: a helm is only granted to somebody
    // on their own feet, there being no stepping across decks.
    bob.say(ToServer::Disembark {
        position: bobs_spawn,
    });

    // Alice sails off and steps ashore — the boat stays where she left it,
    // nobody's — and then leaves the world entirely. Heard out of the boat
    // before the drop, and this is the one place that mattered most: the
    // departure frees a leaver's helm whether or not they stepped off it, so
    // losing the disembark to a reset would leave the boat exactly as this
    // test wants to find it and prove nothing at all.
    let far = alices_spawn + Vec2::new(600.0, 0.0);
    alice.say(ToServer::Helm {
        hull: Underway::lying(far, 1.0),
        tender: None,
    });
    alice.say(ToServer::Disembark {
        position: far + Vec2::new(2.0, 0.0),
    });
    alice.boat_changed_hands(a_boat, None);
    drop(alice);

    // Bob hears the helm empty out where she left it.
    let deadline = Instant::now() + PATIENCE;
    let at = loop {
        let (hull, occupant, _, _) = bob.hear_of(a_boat, deadline, "the abandoned helm told empty");
        if occupant.is_none() {
            break hull.at;
        }
    };
    assert_eq!(at, far, "the boat did not stay where its helmsman left it");

    // Asking for the helm from across the water is refused — the answer is
    // the boat's state, unchanged...
    bob.say(ToServer::Board { boat: a_boat });
    let (_, _, _, occupant) = {
        let mut heard = bob.hear_a_boat();
        while heard.0 != a_boat {
            heard = bob.hear_a_boat();
        }
        heard
    };
    assert_eq!(occupant, None, "a helm was granted from across the water");

    // ...and asking from alongside is granted: a boat is whoever's is in
    // it, and Alice's is Bob's now.
    bob.say(ToServer::Move {
        position: far + Vec2::new(1.0, 0.0),
    });
    bob.say(ToServer::Board { boat: a_boat });
    bob.boat_changed_hands(a_boat, Some(b));
    let _ = a; // Alice's id has no further part; the boat outlived her visit.
}

#[test]
fn a_ships_crew_steps_down_into_the_boat_on_its_painter_and_into_nothing_else() {
    let addr = host(7);
    let (client, id, spawn, _token, aboard) = Client::join_aboard(addr, None);
    let ship = aboard.expect("a newcomer's story starts aboard");
    let (tender, _) = client.hear_the_tender_of(ship);

    // A second rowing boat lying free alongside, dealt by the console: a
    // boat that is nobody's and in reach, and still not one a ship's crew
    // can step down into, the deck leading only to the boat on its own
    // painter. The refusal is the boat's state, unchanged.
    client.say(ToServer::Command {
        line: "grant rowboat".to_string(),
    });
    let loose = client.hear_a_rowboat();
    assert_ne!(loose, tender);
    client.caught_up();
    // Read by the hull each answer is about, the dealt boat lying at anchor
    // being the sea's to turn bow to wind meanwhile, and told as it turns.
    client.say(ToServer::Board { boat: loose });
    let (_, occupant, _, _) = client.hear_of(loose, Instant::now() + PATIENCE, "the refusal");
    assert_eq!(
        occupant, None,
        "a ship's crew stepped into a boat off the painter"
    );

    // Down into the boat on the painter: the boat first, seated and cut
    // free — that telling is what seats the asker — and then the ship, left
    // for anyone, where it lay.
    client.say(ToServer::Board { boat: tender });
    let (_, occupant, towed_by, _) = client.hear_of(tender, Instant::now() + PATIENCE, "the boat");
    assert_eq!(occupant, Some(id), "the asker was not seated in the boat");
    assert_eq!(towed_by, None, "seated in a boat still on the painter");
    let (hull, occupant, _, _) = client.hear_of(ship, Instant::now() + PATIENCE, "the ship");
    assert_eq!(hull.at, spawn, "stepping down moved the ship");
    assert_eq!(occupant, None, "the ship was not left for anyone");

    // And from the thwarts nobody steps into another rowing boat, however
    // close it lies.
    client.say(ToServer::Board { boat: loose });
    let (_, occupant, _, _) = client.hear_of(loose, Instant::now() + PATIENCE, "the refusal");
    assert_eq!(occupant, None, "a rowing boat's crew crossed into another");
}

#[test]
fn a_tender_stays_on_its_painter_when_its_helmsman_leaves_the_world() {
    // A ship nobody is aboard goes where the sea takes it, and the boat
    // behind it goes too: the painter outlasts the session that tied it,
    // and the helmsman resuming — or anybody taking the helm — finds the
    // tow astern.
    let addr = host(7);
    let (alice, a, _spawn, token, aboard) = Client::join_aboard(addr, None);
    let ship = aboard.expect("a newcomer's story starts aboard");
    let (tender, _) = alice.hear_the_tender_of(ship);
    // An onlooker, so that the departure can be watched happening rather
    // than waited out: a dropped socket is noticed when it is noticed.
    let (bob, ..) = Client::join_aboard(addr, None);
    bob.caught_up();
    alice.caught_up();
    drop(alice);

    // Bob hears her go and her helm freed, and nothing about the boat
    // behind it: nothing about it changed.
    let deadline = Instant::now() + PATIENCE;
    while bob.hear_by(deadline, "anybody leaving the world") != (ToClient::Left { id: a }) {}
    let (told, _, _, occupant, _) = bob.hear_a_hull();
    assert_eq!(
        (told, occupant),
        (ship, None),
        "the leaver's helm was not freed"
    );

    // A fresh arrival is introduced to the tow as it stands...
    let (carol, ..) = Client::join_aboard(addr, None);
    let fleet = carol.boats_introduced();
    assert!(
        fleet.contains(&(tender, BoatKind::Rowboat, Some(ship))),
        "the tender came off the painter while its helmsman was away: {fleet:?}"
    );

    // ...and Alice, back at her helm, is told the ship before the boat
    // behind it, whatever order the world keeps its hulls in — a client
    // takes a boat in tow only behind a helm it already holds.
    let (alice, _, _spawn, _t, aboard) = Client::join_aboard(addr, Some(token));
    assert_eq!(aboard, Some(ship), "seated at some other helm");
    let fleet = alice.boats_introduced();
    let place = |boat| fleet.iter().position(|(id, ..)| *id == boat);
    assert!(
        place(ship) < place(tender),
        "the tender was told before its ship: {fleet:?}"
    );
}

#[test]
fn boarding_the_ship_from_the_tender_takes_it_in_tow() {
    let addr = host(7);
    let (client, id, spawn, _token, aboard) = Client::join_aboard(addr, None);
    let ship = aboard.expect("a newcomer's story starts aboard");
    let (tender, _) = client.hear_the_tender_of(ship);
    // An onlooker, who only ever hears where the tender is from the ship's
    // own reports of it.
    let (bob, ..) = Client::join_aboard(addr, None);

    // Down into the boat, which cuts the painter — by the ship's own
    // emptying, the onlooker's arrival having put his sloop in this inbox
    // ahead of the seating.
    client.say(ToServer::Board { boat: tender });
    client.boat_changed_hands(ship, None);

    // Rowed off and back — the tender is a boat like any other under way.
    let alongside = spawn + Vec2::new(3.0, 0.0);
    client.say(ToServer::Helm {
        hull: Underway::lying(spawn + Vec2::new(40.0, 0.0), 0.5),
        tender: None,
    });
    client.say(ToServer::Helm {
        hull: Underway::lying(alongside, 0.5),
        tender: None,
    });

    // Laid alongside again, the ship's helm is granted — and the tender
    // stays in the water on the ship's painter, told to everyone with
    // nobody aboard and the ship named, after the telling that seats its
    // crew.
    client.say(ToServer::Board { boat: ship });
    client.boat_changed_hands(ship, Some(id));
    assert_eq!(
        client.hear_a_painter(),
        (tender, None, Some(ship)),
        "the tender was not left on the ship's painter"
    );

    // It goes where the ship goes: one report moves both, and the onlooker
    // hears the tender wherever the ship's client put it.
    let away = spawn + Vec2::new(40.0, 0.0);
    let astern = away + Vec2::new(-8.0, 0.0);
    client.say(ToServer::Helm {
        hull: Underway::lying(away, 0.5),
        tender: Some(Underway::lying(astern, 0.5)),
    });
    assert_eq!(
        bob.boat_came_to(tender, astern),
        Some(ship),
        "the tender came off the painter on the way"
    );

    // Stepping down from a towing ship is into the boat on the painter,
    // wherever it lies: the same hull, cut free, with the asker seated in
    // it — and the onlooker hears the painter cut with the seating.
    client.say(ToServer::Board { boat: tender });
    let (again, kind, hull, occupant, towed_by) = client.hear_a_hull();
    assert_eq!(again, tender, "a second hull was minted behind the first");
    assert_eq!(kind, BoatKind::Rowboat);
    assert_eq!(
        hull.at, astern,
        "the tender was not stepped into where it lay"
    );
    assert_eq!(occupant, Some(id), "the asker was not seated in the boat");
    assert_eq!(towed_by, None, "seated in a boat still on a painter");
    let (told, ..) = client.hear_a_boat_kinded();
    assert_eq!(told, ship, "the ship was not left at anchor");
    let deadline = Instant::now() + PATIENCE;
    loop {
        let (told, occupant, towed_by) = bob.hear_a_painter();
        if told == tender && occupant == Some(id) {
            assert_eq!(towed_by, None, "seated in a boat still on a painter");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the onlooker never heard the seating"
        );
    }
}

#[test]
fn boarding_a_tender_off_a_painter_cuts_it_free() {
    // A dinghy in the water is anyone's who can reach it, on a painter or off
    // one. Bob swims out to the boat Alice is towing and takes it — and her
    // ship's reports of it from then on say nothing, the hull having stopped
    // being hers to move.
    let addr = host(7);
    let (alice, _a, spawn, _t, a_ship) = Client::join_aboard(addr, None);
    let a_ship = a_ship.expect("a newcomer's story starts aboard");
    let (tender, astern) = alice.hear_the_tender_of(a_ship);
    let (bob, b, bobs_spawn, _t2, _b_ship) = Client::join_aboard(addr, None);

    // Bob steps off his own deck into the water beside it and takes it.
    bob.say(ToServer::Disembark {
        position: bobs_spawn,
    });
    bob.say(ToServer::Move {
        position: astern + Vec2::new(1.0, 0.0),
    });
    bob.say(ToServer::Board { boat: tender });
    bob.boat_changed_hands(tender, Some(b));
    // Alice hears the same: her tender is somebody's, and off her painter.
    let deadline = Instant::now() + PATIENCE;
    loop {
        let (told, occupant, towed_by) = alice.hear_a_painter();
        if told == tender && occupant == Some(b) {
            assert_eq!(towed_by, None, "boarded, and still on the painter");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Alice never heard her tender taken"
        );
    }

    // Her next report carries a tender that is no longer hers, and the world
    // moves nothing by it: Bob, sitting in the hull, hears only her ship.
    let away = spawn + Vec2::new(40.0, 0.0);
    alice.say(ToServer::Helm {
        hull: Underway::lying(away, 0.0),
        tender: Some(Underway::lying(away + Vec2::new(-8.0, 0.0), 0.0)),
    });
    bob.say(ToServer::Command {
        line: "help".to_string(),
    });
    let deadline = Instant::now() + PATIENCE;
    loop {
        match bob.hear_by(deadline, "the reply that brackets the report") {
            ToClient::Boat { id, .. } if id == tender => {
                panic!("a cut painter still moved the boat on it")
            }
            ToClient::Reply { .. } => break,
            _ => {}
        }
    }
}

#[test]
fn a_ships_helm_is_taken_from_a_tender_and_never_from_another_deck() {
    let addr = host(1);
    let (alice, a, _alices_spawn, _t, a_boat) = Client::join_aboard(addr, None);
    let a_boat = a_boat.expect("aboard");
    let (a_tender, _) = alice.hear_the_tender_of(a_boat);
    let (bob, b, bobs_spawn, _t2, b_boat) = Client::join_aboard(addr, None);
    let b_boat = b_boat.expect("aboard");

    // Bob steps ashore, leaving his sloop free where he entered — with his
    // own boat still on its painter.
    bob.say(ToServer::Disembark {
        position: bobs_spawn,
    });

    // Alice's inbox so far, in order: Bob's ship arriving, the boat behind
    // it, and Bob's emptying out.
    let (told, _k, _at, _h, occupant) = alice.hear_a_boat_kinded();
    assert_eq!((told, occupant), (b_boat, Some(b)));
    let (_b_tender, _) = alice.hear_the_tender_of(b_boat);
    let (told, _k, _at, _h, occupant) = alice.hear_a_boat_kinded();
    assert_eq!((told, occupant), (b_boat, None));

    // Alice lays her ship alongside Bob's and asks for its helm from her own
    // deck: refused — ships do not board ships — and the answer is the
    // boat's state, unchanged.
    let alongside = bobs_spawn + Vec2::new(3.0, 0.0);
    alice.say(ToServer::Helm {
        hull: Underway::lying(alongside, 0.0),
        tender: Some(Underway::lying(alongside + Vec2::new(0.0, 8.0), 0.0)),
    });
    alice.say(ToServer::Board { boat: b_boat });
    let (told, _k, _at, _h, occupant) = alice.hear_a_boat_kinded();
    assert_eq!(told, b_boat, "a boat other than the asked-for one answered");
    assert_eq!(
        occupant, None,
        "a helm was granted from another ship's deck"
    );

    // From the thwarts of her tender the same ask is granted — a boat is
    // anyone's who reaches it, so the tender goes aboard a hull it never
    // came off. Bob's ship has a boat on its painter already, though, so
    // hers is left lying where she stepped out of it, free.
    alice.say(ToServer::Board { boat: a_tender });
    alice.boat_changed_hands(a_tender, Some(a));
    alice.boat_changed_hands(a_boat, None);
    alice.say(ToServer::Helm {
        hull: Underway::lying(bobs_spawn + Vec2::new(5.0, 0.0), 0.0),
        tender: None,
    });
    alice.say(ToServer::Board { boat: b_boat });
    let (told, _k, _at, _h, occupant) = alice.hear_a_boat_kinded();
    assert_eq!((told, occupant), (b_boat, Some(a)));
    assert_eq!(
        alice.hear_a_painter(),
        (a_tender, None, None),
        "a second boat was taken in tow behind one already there"
    );
}

#[test]
fn hanging_up_in_the_boat_puts_you_back_in_it() {
    // The way an ordinary player leaves most: mid passage, oars in the
    // water, close the game. They come back to the boat they were rowing —
    // a rowing boat is a helm like any other — and nothing about it costs
    // the world a hull. Four turns of it is what this counts.
    let addr = host(7);
    let (mut alice, mut a, _spawn, token, aboard) = Client::join_aboard(addr, None);
    let ship = aboard.expect("a newcomer's story starts aboard");
    let (tender, _) = alice.hear_the_tender_of(ship);
    // An onlooker, so that each hang-up can be watched being filed rather
    // than raced: a rejoin that got in front of it would be met as a
    // stranger and dealt a hull for that reason instead of this one.
    let watcher = Client::join(addr).0;

    for turn in 0..4 {
        if turn > 0 {
            // Back aboard the ship, which takes the boat in tow again.
            alice.say(ToServer::Board { boat: ship });
            alice.boat_changed_hands(ship, Some(a));
        }
        // Down into the boat, and the line goes dead with her still in it.
        // Hearing the seating is what proves the ask landed, see
        // [`Client::caught_up`].
        alice.say(ToServer::Board { boat: tender });
        alice.boat_changed_hands(tender, Some(a));
        drop(alice);
        let deadline = Instant::now() + PATIENCE;
        while watcher.hear_by(deadline, "anybody leaving the world") != (ToClient::Left { id: a }) {
        }

        let (back, id, _spawn, dealt, seat) = Client::join_aboard(addr, Some(token));
        assert_eq!(dealt, token, "the papers were not the ones handed over");
        assert_eq!(
            seat,
            Some(tender),
            "turn {turn}: somebody who hung up mid-row was not put back in the boat"
        );
        alice = back;
        a = id;
    }

    // Three ships and three boats, whatever the count of handshakes: one
    // pair each for Alice, the onlooker, and the counting client.
    let (carol, ..) = Client::join_aboard(addr, None);
    let fleet = carol.boats_introduced();
    let sloops = fleet
        .iter()
        .filter(|(_, kind, _)| *kind == BoatKind::Sloop)
        .count();
    let dinghies = fleet
        .iter()
        .filter(|(_, kind, _)| *kind == BoatKind::Rowboat)
        .count();
    assert_eq!(sloops, 3, "a handshake minted a ship: {fleet:?}");
    assert_eq!(dinghies, 3, "a handshake left a dinghy behind: {fleet:?}");
}

#[test]
fn a_returner_seated_in_a_towed_boat_comes_off_the_painter() {
    // A boat is whoever's is in it, painter or no painter. Alice hangs up in
    // her tender; while she is away Bob ties it astern of a ship; and her
    // rejoining seats her in it and cuts the rope. Left tied it would be a
    // hull steered from two machines — her own reports of it, and Bob's of
    // the boat he believes he is towing — so the towing helm hears the
    // painter cut in the very telling that seats her.
    let addr = host(7);
    let (alice, a, _spawn, token, ship) = Client::join_aboard(addr, None);
    let ship = ship.expect("a newcomer's story starts aboard");
    let (tender, astern) = alice.hear_the_tender_of(ship);
    let (bob, b, bobs_spawn, _t2, _b_ship) = Client::join_aboard(addr, None);

    // Down into her boat, which cuts her own painter and leaves her ship
    // free and towing nothing — and the line goes dead with her still in it.
    alice.say(ToServer::Board { boat: tender });
    alice.boat_changed_hands(tender, Some(a));
    drop(alice);
    let deadline = Instant::now() + PATIENCE;
    while bob.hear_by(deadline, "anybody leaving the world") != (ToClient::Left { id: a }) {}

    // Bob steps off his own deck, swims across to the boat she left, and
    // takes the helm of her ship from its thwarts — which puts her boat on
    // that ship's painter, the ship having nothing there.
    bob.say(ToServer::Disembark {
        position: bobs_spawn,
    });
    bob.say(ToServer::Move {
        position: astern + Vec2::new(1.0, 0.0),
    });
    bob.say(ToServer::Board { boat: tender });
    bob.boat_changed_hands(tender, Some(b));
    bob.say(ToServer::Board { boat: ship });
    bob.boat_changed_hands(ship, Some(b));
    let deadline = Instant::now() + PATIENCE;
    loop {
        let (told, occupant, towed_by) = bob.hear_a_painter();
        if told == tender && occupant.is_none() {
            assert_eq!(towed_by, Some(ship), "the boat was not taken in tow");
            break;
        }
        assert!(Instant::now() < deadline, "the tow was never tied");
    }

    // Back with her papers, Alice is seated in the boat where it lies —
    // hers again the moment she is in it, and off the painter.
    let (_alice, a, _spawn, _token, aboard) = Client::join_aboard(addr, Some(token));
    assert_eq!(
        aboard,
        Some(tender),
        "somebody who hung up in a boat was not put back in it"
    );
    let deadline = Instant::now() + PATIENCE;
    loop {
        let (told, occupant, towed_by) = bob.hear_a_painter();
        if told == tender && occupant == Some(a) {
            assert_eq!(towed_by, None, "the returner was seated on a painter");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the towing helm never heard the painter cut"
        );
    }
}

#[test]
fn a_returning_helmsman_is_seated_back_at_their_helm() {
    // The single-player story: stop the world at a helm somewhere, reopen
    // it, and be exactly there, aboard exactly that boat.
    let path = scratch("helm").join("one.world");
    let first = Server::bind(("127.0.0.1", 0), 7)
        .expect("bind")
        .keeping_at(path.clone())
        .expect("keeping");
    let addr = first.local_addr().expect("addr");
    let host = first.spawn().expect("spawn");

    let (client, _id, spawn, token, aboard) = Client::join_aboard(addr, None);
    let boat = aboard.expect("aboard");
    let out = spawn + Vec2::new(900.0, -250.0);
    client.say(ToServer::Helm {
        hull: Underway::lying(out, 2.0),
        tender: None,
    });
    // An answered chunk proves the helm report was processed.
    let _ = client.ask_for(IVec2::new(5_000, 5_000));
    drop(client);
    drop(host);

    let again = Server::reopen(("127.0.0.1", 0), &path).expect("reopen");
    let addr = again.local_addr().expect("addr");
    let _host = again.spawn().expect("spawn");
    let (_client, _id, spawn, _token, aboard) = Client::join_aboard(addr, Some(token));
    assert_eq!(aboard, Some(boat), "seated at some other helm");
    // Give or take the beat or two the sea had at the hull between the
    // hang-up and the world stopping — see [`ADRIFT`]. While the world was
    // stopped, nothing moved it.
    assert!(
        spawn.distance(out) < ADRIFT,
        "the boat moved while the world was stopped: {spawn} for {out}"
    );
}

#[test]
fn a_kept_world_reopens_with_its_tows_tied() {
    // The painter is a fact about two hulls, and the file keeps it: a world
    // closed with a boat astern of a ship reopens with the boat still there.
    let path = scratch("painter").join("one.world");
    let first = Server::bind(("127.0.0.1", 0), 7)
        .expect("bind")
        .keeping_at(path.clone())
        .expect("keeping");
    let addr = first.local_addr().expect("addr");
    let host = first.spawn().expect("spawn");

    let (client, _id, _spawn, _token, aboard) = Client::join_aboard(addr, None);
    let ship = aboard.expect("aboard");
    let (tender, _) = client.hear_the_tender_of(ship);
    drop(client);
    drop(host);

    let again = Server::reopen(("127.0.0.1", 0), &path).expect("reopen");
    let addr = again.local_addr().expect("addr");
    let _host = again.spawn().expect("spawn");
    let (carol, ..) = Client::join_aboard(addr, None);
    let fleet = carol.boats_introduced();
    assert!(
        fleet.contains(&(tender, BoatKind::Rowboat, Some(ship))),
        "the painter was not kept: {fleet:?}"
    );
}

#[test]
fn a_taken_boat_is_not_resumed_into() {
    // Alice leaves at a helm; Bob takes the boat while she is away. Her
    // memory of being aboard is a memory, not a hold: she returns where she
    // was, in a fresh hull the world provides, and Bob keeps what he took.
    let addr = host(1);
    let (alice, _a, alices_spawn, alices_token, a_boat) = Client::join_aboard(addr, None);
    let a_boat = a_boat.expect("aboard");
    let far = alices_spawn + Vec2::new(600.0, 0.0);
    alice.say(ToServer::Helm {
        hull: Underway::lying(far, 1.0),
        tender: None,
    });
    let _ = alice.ask_for(IVec2::new(5_000, 5_000));
    drop(alice);

    let (bob, b, bobs_spawn, ..) = Client::join_aboard(addr, None);
    bob.say(ToServer::Disembark {
        position: bobs_spawn,
    });
    bob.say(ToServer::Move {
        position: far + Vec2::new(1.0, 0.0),
    });
    bob.say(ToServer::Board { boat: a_boat });
    bob.boat_changed_hands(a_boat, Some(b));

    // Where she was, give or take what the sea did with the hull before Bob
    // took it — see [`ADRIFT`].
    let (_alice, _id, spawn, _t, aboard) = Client::join_aboard(addr, Some(alices_token));
    assert!(
        spawn.distance(far) < ADRIFT,
        "Alice did not return where she was: {spawn} for {far}"
    );
    assert_ne!(aboard, Some(a_boat), "one helm held two players");
    assert!(aboard.is_some(), "Alice was left standing on open water");
}

#[test]
fn a_boat_sailed_away_and_left_free_is_not_resumed_into_either() {
    // The same memory, and a boat that is nobody's again by the time she
    // comes back — but lying somewhere else. Being seated back into it would
    // teleport her across the water to wherever a stranger abandoned it, so
    // the world puts her down where she stood, in a hull of her own.
    let addr = host(1);
    let (alice, a, alices_spawn, alices_token, a_boat) = Client::join_aboard(addr, None);
    let a_boat = a_boat.expect("aboard");
    let (bob, b, bobs_spawn, _t, _b_boat) = Client::join_aboard(addr, None);

    // Alice sails out and hangs up at the helm. Bob hearing her leave is
    // what says the helm is free before he asks for it.
    let far = alices_spawn + Vec2::new(600.0, 0.0);
    alice.say(ToServer::Helm {
        hull: Underway::lying(far, 1.0),
        tender: None,
    });
    // Where the boat ends up is the whole of this test, so the sailing is
    // waited out rather than raced against the drop — see
    // [`Client::caught_up`].
    alice.caught_up();
    drop(alice);
    // Bob was introduced to her on arriving, and her helm report is answered
    // by now, so hearing the leaving after that is hearing both.
    assert!(matches!(bob.hear(), ToClient::Joined { id, .. } if id == a));
    assert_eq!(bob.hear(), ToClient::Left { id: a });

    // Bob takes her boat, sails it well clear of where she left it, and
    // steps off — leaving it free, and nowhere near her memory of it.
    bob.say(ToServer::Disembark {
        position: bobs_spawn,
    });
    bob.say(ToServer::Move {
        position: far + Vec2::new(1.0, 0.0),
    });
    bob.say(ToServer::Board { boat: a_boat });
    let moored = far + Vec2::new(0.0, 400.0);
    bob.say(ToServer::Helm {
        hull: Underway::lying(moored, 2.0),
        tender: None,
    });
    bob.say(ToServer::Disembark {
        position: moored + Vec2::new(2.0, 0.0),
    });
    let deadline = Instant::now() + PATIENCE;
    loop {
        let (hull, occupant, _, _) =
            bob.hear_of(a_boat, deadline, "the boat left free somewhere else");
        if occupant.is_none() && hull.at == moored {
            break;
        }
    }
    let _ = b;

    let (_alice, _id, spawn, _t, aboard) = Client::join_aboard(addr, Some(alices_token));
    assert!(
        spawn.distance(far) < ADRIFT,
        "Alice did not return where she was: {spawn} for {far}"
    );
    assert_ne!(
        aboard,
        Some(a_boat),
        "Alice was dragged to where her old boat had got to"
    );
    assert!(aboard.is_some(), "Alice was left standing on open water");
}

#[test]
fn a_boat_taken_up_and_left_where_it_lay_is_resumed_into() {
    // The same memory, and a hull that never went anywhere — somebody still
    // in the world took it up while she was away and stepped off it again.
    // A boat is whoever's is in it and nobody's otherwise, so a helm lying
    // free where she left it is hers to resume at, whoever sat in it since.
    let addr = host(1);
    let (alice, a, alices_spawn, alices_token, a_boat) = Client::join_aboard(addr, None);
    let a_boat = a_boat.expect("aboard");
    let (bob, b, bobs_spawn, _t, _bobs) = Client::join_aboard(addr, None);

    // Alice hangs up at the helm without ever sailing. Bob hearing her leave
    // is what says the helm is free before he asks for it.
    drop(alice);
    assert!(matches!(bob.hear(), ToClient::Joined { id, .. } if id == a));
    assert_eq!(bob.hear(), ToClient::Left { id: a });

    // Bob takes her boat — on his own feet, a helm being granted only to
    // somebody not already at one — and steps straight off it again, where it
    // lies and while staying in the world.
    bob.say(ToServer::Disembark {
        position: bobs_spawn,
    });
    bob.say(ToServer::Move {
        position: alices_spawn,
    });
    bob.say(ToServer::Board { boat: a_boat });
    bob.boat_changed_hands(a_boat, Some(b));
    bob.say(ToServer::Disembark {
        position: alices_spawn,
    });
    bob.boat_changed_hands(a_boat, None);

    let (_alice, _id, spawn, _t, aboard) = Client::join_aboard(addr, Some(alices_token));
    assert!(
        spawn.distance(alices_spawn) < ADRIFT,
        "Alice did not return where she was: {spawn} for {alices_spawn}"
    );
    assert_eq!(
        aboard,
        Some(a_boat),
        "Alice was not seated back into a hull lying free where she left it"
    );
}

#[test]
fn every_arrival_is_minted_a_hull_of_their_own() {
    // A free hull lying by the spawn is somebody's parked boat, whether they
    // are here or gone, and a newcomer's story does not start in one: every
    // arrival is dealt a ship of their own.
    let addr = host(1);
    // A watcher, so the leaving can be *heard* to have been dealt with
    // before the next arrival knocks.
    let (watcher, _w, _spawn, _t, watchers) = Client::join_aboard(addr, None);
    let watchers = watchers.expect("aboard");

    let (alice, a, _spawn, _t, first) = Client::join_aboard(addr, None);
    let first = first.expect("a newcomer's story starts aboard");
    assert_ne!(first, watchers, "two players were dealt one hull");
    drop(alice);
    // Her arrival, then her departure — the watcher's own hull's tellings
    // are not among these, [`Client::hear`] passing over the boats.
    assert!(matches!(watcher.hear(), ToClient::Joined { id, .. } if id == a));
    assert_eq!(watcher.hear(), ToClient::Left { id: a });

    let (_bob, _b, _spawn, _t, second) = Client::join_aboard(addr, None);
    assert!(
        second.is_some_and(|second| second != first && second != watchers),
        "the world handed a newcomer a hull somebody had left: {second:?}"
    );
}

#[test]
fn hulls_stepped_out_of_and_left_behind_leave_the_world_and_its_file() {
    // The leak #143 was about, closed: a client that joins, steps ashore and
    // hangs up in a loop leaves a ship and a boat behind every time round,
    // and once they have lain unused for a day with nobody near, the world
    // takes every one of them back — told to whoever is left, and gone from
    // the file. The boat on a painter whose ship somebody is still sailing
    // is the one hull as old and as far off that stays.
    let path = scratch("hulls").join("one.world");
    let world = Server::bind(("127.0.0.1", 0), 7)
        .expect("bind")
        .keeping_at(path.clone())
        .expect("keeping");
    let addr = world.local_addr().expect("addr");
    let host = world.spawn().expect("spawn");

    // Somebody who stays, so that each leaving is heard to have been dealt
    // with before the next arrival knocks — and who will sail out of range
    // of the lot of them when the loop is done.
    let (watcher, _w, spawn, _token, watchers) = Client::join_aboard(addr, None);
    let watchers = watchers.expect("aboard");
    let (watchers_boat, _) = watcher.hear_the_tender_of(watchers);

    let mut ships = Vec::new();
    for _ in 0..20 {
        let (client, id, spawn, _token, aboard) = Client::join_aboard(addr, None);
        ships.push(aboard.expect("a newcomer's story starts aboard"));
        client.say(ToServer::Disembark { position: spawn });
        drop(client);
        while !matches!(watcher.hear(), ToClient::Left { id: gone } if gone == id) {}
    }

    // Out of range of the spawn, with the ship's own boat left lying there
    // on the painter — `tender: None` being a report that says nothing about
    // it, so it stays exactly where it was minted.
    watcher.say(ToServer::Helm {
        hull: Underway::lying(spawn + Vec2::new(3_000.0, 0.0), 0.0),
        tender: None,
    });
    watcher.wind_on_a_day();

    let gone = watcher.hear_hulls_gone(2 * ships.len());
    for ship in &ships {
        assert!(
            gone.contains(ship),
            "a ship stepped out of was kept: {ship}"
        );
    }
    assert!(
        !gone.contains(&watchers) && !gone.contains(&watchers_boat),
        "the watcher's own ship or the boat on its painter was taken: {gone:?}"
    );
    assert!(
        watcher.no_hull_was_retired(),
        "more than the leavers' hulls were taken"
    );

    drop(watcher);
    drop(host);
    let kept = std::fs::read_to_string(&path).expect("the kept world");
    let hulls: Vec<&str> = kept
        .lines()
        .filter(|line| line.starts_with("boat "))
        .collect();
    assert_eq!(
        hulls.len(),
        2,
        "the file should hold the watcher's ship and its boat and nothing else:\n{}",
        hulls.join("\n")
    );
}

#[test]
fn a_hull_stays_while_anybody_is_near_it_and_goes_once_nobody_is() {
    // Two leavers step ashore and hang up, one on the spawn and one three
    // kilometres off; the watcher stands on the spawn. A day on, the far
    // hulls are taken and the near ones are not — and once the watcher sails
    // out of their range, those go too.
    let addr = host(7);
    let (watcher, _w, spawn, _t, _aboard) = Client::join_aboard(addr, None);

    let leave = |where_to: Option<Vec2>| {
        let (client, id, spawn, _token, aboard) = Client::join_aboard(addr, None);
        let ship = aboard.expect("aboard");
        let ashore = where_to.unwrap_or(spawn);
        if let Some(out) = where_to {
            client.say(ToServer::Helm {
                hull: Underway::lying(out, 0.0),
                tender: Some(Underway::lying(out + Vec2::new(0.0, 10.0), 0.0)),
            });
        }
        client.say(ToServer::Disembark { position: ashore });
        client.boat_changed_hands(ship, None);
        drop(client);
        while !matches!(watcher.hear(), ToClient::Left { id: gone } if gone == id) {}
        ship
    };
    let near = leave(None);
    let far = leave(Some(spawn + Vec2::new(3_000.0, 0.0)));

    watcher.wind_on_a_day();
    let gone = watcher.hear_hulls_gone(2);
    assert!(gone.contains(&far), "the far ship was kept: {gone:?}");
    assert!(
        !gone.contains(&near),
        "the ship beside the watcher was taken"
    );
    assert!(
        watcher.no_hull_was_retired(),
        "a hull beside the watcher was taken"
    );

    watcher.say(ToServer::Helm {
        hull: Underway::lying(spawn + Vec2::new(-3_000.0, 0.0), 0.0),
        tender: None,
    });
    let gone = watcher.hear_hulls_gone(2);
    assert!(
        gone.contains(&near),
        "the ship the watcher sailed away from was kept: {gone:?}"
    );
}

#[test]
fn a_helm_hung_up_at_is_kept_for_its_helmsman() {
    // Hanging up at a helm is a promise the world makes: the boat is where
    // it was left, for as long as it lies free. So a ship and its tender
    // abandoned three kilometres from anyone for a day are kept all the
    // same when somebody's papers name the helm — and, for contrast, a
    // leaver who stepped ashore beside them loses theirs.
    let addr = host(7);
    let (watcher, _w, spawn, ..) = Client::join_aboard(addr, None);
    let out = spawn + Vec2::new(3_000.0, 0.0);

    let (alice, a, _, token, aboard) = Client::join_aboard(addr, None);
    let alices = aboard.expect("aboard");
    let (alices_boat, _) = alice.hear_the_tender_of(alices);
    alice.say(ToServer::Helm {
        hull: Underway::lying(out, 1.0),
        tender: Some(Underway::lying(out + Vec2::new(0.0, 10.0), 1.0)),
    });
    // An answered chunk proves the helm report was processed.
    let _ = alice.ask_for(IVec2::new(5_000, 5_000));
    drop(alice);
    while !matches!(watcher.hear(), ToClient::Left { id } if id == a) {}

    // With her boat in tow: a tender left lying on the spawn would be
    // beside the watcher, and a ship stays for the boat on its painter as
    // much as the other way about.
    let (carol, c, _, _, aboard) = Client::join_aboard(addr, None);
    let carols = aboard.expect("aboard");
    let carols_way = out + Vec2::new(100.0, 0.0);
    carol.say(ToServer::Helm {
        hull: Underway::lying(carols_way, 0.0),
        tender: Some(Underway::lying(carols_way + Vec2::new(0.0, 10.0), 0.0)),
    });
    carol.say(ToServer::Disembark {
        position: carols_way,
    });
    carol.boat_changed_hands(carols, None);
    drop(carol);
    while !matches!(watcher.hear(), ToClient::Left { id } if id == c) {}

    watcher.wind_on_a_day();
    let gone = watcher.hear_hulls_gone(2);
    assert!(
        gone.contains(&carols),
        "the ship stepped out of was kept: {gone:?}"
    );
    assert!(
        watcher.no_hull_was_retired(),
        "a helm somebody hung up at was taken"
    );

    // And the promise honoured: seated back at that helm, the tender still
    // on its painter.
    let (alice, _, _, _, aboard) = Client::join_aboard(addr, Some(token));
    assert_eq!(aboard, Some(alices), "seated at some other helm");
    let introduced = alice.boats_introduced();
    assert!(
        introduced
            .iter()
            .any(|(id, _, towed_by)| *id == alices_boat && *towed_by == Some(alices)),
        "the tender was not on the painter: {introduced:?}"
    );
}

#[test]
fn a_kept_world_reopens_with_its_beasts_where_they_were() {
    // The shark scenario, by proxy of a whole sea: the animals alive when a
    // world closes are in its file, and reopening finds each of them where it
    // stood — a beast with consequence cannot be escaped by relogging, any
    // more than a gale can.
    let path = scratch("beasts").join("one.world");
    let first = Server::bind(("127.0.0.1", 0), 7)
        .expect("bind")
        .keeping_at(path.clone())
        .expect("keeping");
    let addr = first.local_addr().expect("addr");
    let host = first.spawn().expect("spawn");

    let (client, _id, _spawn, _facing, token) = Client::join_presenting(addr, None);
    client.say(ToServer::Command {
        line: "spawn whale".to_string(),
    });
    let summons = client.hear_reply();
    assert!(
        summons.starts_with("a whale"),
        "the summons came to: {summons}"
    );

    // The whole flock rather than the next whale, because by now there are
    // two of those — the summoned one, and the one these waters were going to
    // raise for a player anyway — and neither is the one the test means.
    //
    // Read on until a whale is in the beat: a beat already in flight when the
    // summons landed has yet to hear of it. Once it is told of, it is in the
    // ledger a save reads too, that being rewritten before each beat's
    // tellings go out.
    let before = loop {
        let beat = client.hear_the_flock();
        if beat.iter().any(|(_, kind, _)| *kind == BeastKind::Whale) {
            break beat;
        }
    };
    drop(client);
    drop(host);

    let again = Server::reopen(("127.0.0.1", 0), &path).expect("reopen");
    let addr = again.local_addr().expect("addr");
    let _host = again.spawn().expect("spawn");
    let (client, ..) = Client::join_presenting(addr, Some(token));
    let after = client.hear_the_flock();

    // Every animal that was there is there again, one for one and near where
    // it was — they wander at their own pace, and no world time passed while
    // the world was closed, so the slack only covers the few real seconds
    // either side of the reopening. Matched off as they are found, so two
    // whales that came back as one are still a whale lost. Animals the
    // reopened world raised on top of these are its own business: the promise
    // is that none was lost, not that the sea stood still.
    let mut unclaimed = after.clone();
    for (_, kind, was) in &before {
        let same = unclaimed
            .iter()
            .position(|(_, found, at)| found == kind && at.distance(*was) < 300.0);
        let Some(same) = same else {
            panic!("a {kind:?} was at {was} and the reopened world has only {after:?}")
        };
        unclaimed.remove(same);
    }
}

#[test]
fn a_kept_world_cannot_be_hosted_twice_at_once() {
    // Two processes writing one file would be two histories under one name;
    // the world's lock makes the second host an error instead.
    let path = scratch("locked").join("one.world");
    let holding = Server::bind(("127.0.0.1", 0), 1)
        .expect("bind")
        .keeping_at(path.clone())
        .expect("keeping");
    assert!(
        Server::reopen(("127.0.0.1", 0), &path).is_err(),
        "one world came to be hosted twice"
    );
    drop(holding);
    assert!(
        Server::reopen(("127.0.0.1", 0), &path).is_ok(),
        "the lock outlived the session holding it"
    );
}

/// Every chunk within sight of a point, worked out the way the survey's own
/// rule does — what a test expects to be told when a player stands still.
fn in_sight_of(at: Vec2) -> Vec<IVec2> {
    let reach = (SIGHT_RADIUS / CHUNK_METRES).ceil() as i32 + 2;
    let home = chunk_at(at);
    (-reach..=reach)
        .flat_map(|dz| (-reach..=reach).map(move |dx| home + IVec2::new(dx, dz)))
        .filter(|chunk| in_sight(*chunk, at))
        .collect()
}

#[test]
fn a_player_is_told_the_survey_of_where_they_are_put_down() {
    // The survey is the world's: nobody has to look at anything to have it,
    // and arriving is already having looked. Exactly the water within sight
    // of the spawn, and no more — every coast is theirs to sail for.
    let addr = host(7);
    let (client, _id, spawn, _facing) = Client::join(addr);
    let wanted = in_sight_of(spawn);

    let charted = client.hear_the_survey(HashMap::new(), |charted| charted.len() >= wanted.len());
    for chunk in &wanted {
        assert!(charted.contains_key(chunk), "{chunk} was never surveyed");
    }
    for chunk in charted.keys() {
        assert!(
            in_sight(*chunk, spawn),
            "{chunk} was surveyed from {spawn}, which cannot see it"
        );
    }
}

#[test]
fn the_two_ends_survey_one_chunk_the_same_way() {
    // The whole reason the server goes through a chunk's *payload* to survey
    // it rather than asking the generator for its own heights. A client
    // surveys what it was sent — heights the wire has rounded to sixteen bits
    // — and a waterline traced across those is not quite the one traced
    // across the numbers before them. Two ends that disagree about where a
    // coast runs disagree about whether it closes, and so about whether
    // anybody has been round it.
    let addr = host(7);
    let (client, _id, _spawn, _facing) = Client::join(addr);
    // Off a coast, the entry being open sea with none in sight.
    let berth = berthed_off_the_first_land(&client, 7);
    let charted = client.hear_the_survey(HashMap::new(), |charted| {
        charted.len() >= in_sight_of(berth).len()
            && charted.values().any(|ink| !ink.coast.is_empty())
    });

    let (chunk, told) = charted
        .iter()
        .find(|(_, ink)| !ink.coast.is_empty())
        .expect("some coast within sight of the berth");
    let ground = client.ask_for(*chunk).expect("ground under a coast");
    let heights: Vec<f32> = ground.heights.iter().copied().map(dequantize).collect();
    assert_eq!(
        &protocol::survey::survey(&heights),
        told,
        "the server's {chunk} and a client's are two different coasts"
    );
}

#[test]
fn sailing_past_a_coast_earns_the_ground_it_passes() {
    // The live half: a survey grows by going somewhere. What arrives is the
    // ground the way brought into sight and nothing else — a voyage does not
    // hand anybody the sea it did not cross.
    let addr = host(7);
    let (client, _id, spawn, _facing) = Client::join(addr);
    let at_first = client.hear_the_survey(HashMap::new(), |charted| {
        charted.len() >= in_sight_of(spawn).len()
    });

    // Half a kilometre along, which is several chunks of new water and, at
    // the entry island's own coast, some new shore.
    let out = spawn + Vec2::new(512.0, 0.0);
    client.say(ToServer::Helm {
        hull: Underway::lying(out, 0.0),
        tender: None,
    });

    let wanted = in_sight_of(out);
    let charted = client.hear_the_survey(at_first.clone(), |charted| {
        wanted.iter().all(|chunk| charted.contains_key(chunk))
    });
    assert!(
        charted.len() > at_first.len(),
        "sailing half a kilometre earned nothing"
    );
    for chunk in charted.keys() {
        assert!(
            in_sight_along(*chunk, spawn, out),
            "{chunk} is nowhere near the way from {spawn} to {out}"
        );
    }
}

#[test]
fn a_crossing_between_two_reports_leaves_no_hole() {
    // A hull under sail covers ground between one position report and the
    // next, and the survey follows the way rather than its ends — or a fast
    // boat would leave a stripe of unsurveyed water down the middle of its
    // own wake.
    let addr = host(7);
    let (client, _id, spawn, _facing) = Client::join(addr);
    let at_first = client.hear_the_survey(HashMap::new(), |charted| {
        charted.len() >= in_sight_of(spawn).len()
    });

    let out = spawn + Vec2::new(1_536.0, 0.0);
    let midway = chunk_at(spawn + (out - spawn) / 2.0);
    assert!(
        !in_sight(midway, spawn) && !in_sight(midway, out),
        "the test's midpoint is visible from an end, and proves nothing"
    );
    client.say(ToServer::Helm {
        hull: Underway::lying(out, 0.0),
        tender: None,
    });

    let charted = client.hear_the_survey(at_first, |charted| charted.contains_key(&midway));
    assert!(charted.contains_key(&midway));
}

#[test]
fn a_returning_player_is_told_back_the_survey_they_left_with() {
    // The world's memory of where somebody has been, kept to a file and
    // handed back: the same chunks, and the same coast on them, worked out
    // afresh from ground the world grew again. Only the coordinates are in
    // the file — the ink being derived — so this is also the test that the
    // deriving is stable.
    let path = scratch("surveyed").join("one.world");
    let first = Server::bind(("127.0.0.1", 0), 7)
        .expect("bind")
        .keeping_at(path.clone())
        .expect("keeping");
    let addr = first.local_addr().expect("addr");
    let host = first.spawn().expect("spawn");

    let (client, _id, spawn, _facing, token) = Client::join_presenting(addr, None);
    let out = spawn + Vec2::new(768.0, 0.0);
    client.say(ToServer::Helm {
        hull: Underway::lying(out, 0.0),
        tender: None,
    });
    let wanted = in_sight_of(out);
    let sailed = client.hear_the_survey(HashMap::new(), |charted| {
        wanted.iter().all(|chunk| charted.contains_key(chunk))
    });
    drop(client);
    // Dropping the host is leaving the world: the closing save has happened
    // by the time the drop returns.
    drop(host);

    let again = Server::reopen(("127.0.0.1", 0), &path).expect("reopen");
    let addr = again.local_addr().expect("addr");
    let _host = again.spawn().expect("spawn");

    let (client, _id, _spawn, _facing, dealt) = Client::join_presenting(addr, Some(token));
    assert_eq!(dealt, token, "kept papers were re-dealt");
    let told_back = client.hear_the_survey(HashMap::new(), |charted| {
        sailed.keys().all(|chunk| charted.contains_key(chunk))
    });
    for (chunk, ink) in &sailed {
        assert_eq!(
            told_back.get(chunk),
            Some(ink),
            "{chunk} came back as a different coast"
        );
    }
}

#[test]
fn a_jump_no_hull_could_make_is_followed_only_so_far() {
    // Nobody sails two kilometres between two position reports, and the
    // survey does not pretend otherwise: what a client says it has done is
    // followed at the pace somebody could have done it, and the rest of the
    // way waits. Without that a client that never asks for a chunk can order
    // a hundred of them worked out per twelve-byte report, as fast as it can
    // write, on the connection's own thread and outside every bound the
    // chunk path has.
    let addr = host(7);
    let (client, _id, spawn, _facing) = Client::join(addr);
    let at_first = client.hear_the_survey(HashMap::new(), |charted| {
        charted.len() >= in_sight_of(spawn).len()
    });

    // Twice as far as anyone is believed to have got.
    let claimed = spawn + Vec2::new(2.0 * server::SURVEY_SWEEP, 0.0);
    let believed = spawn + Vec2::new(server::SURVEY_SWEEP, 0.0);
    client.say(ToServer::Helm {
        hull: Underway::lying(claimed, 0.0),
        tender: None,
    });

    // The way as far as it was believed is inked — this is the chunk a
    // report short of the end of the allowance...
    let reached = chunk_at(believed - Vec2::new(2.0 * CHUNK_METRES, 0.0));
    let charted = client.hear_the_survey(at_first, |charted| charted.contains_key(&reached));
    // ...and nothing beyond it is, the far end being water this client
    // merely claimed to have crossed.
    assert!(
        !charted.contains_key(&chunk_at(claimed)),
        "the survey followed a jump no hull could make all the way to {claimed}"
    );
    for chunk in charted.keys() {
        assert!(
            in_sight_along(*chunk, spawn, believed),
            "{chunk} is off the way from {spawn} to as far as anyone got"
        );
    }
}

#[test]
fn a_world_sailed_to_its_own_edge_opens_again() {
    // The survey is only ever allowed to hold ground the world's own file
    // will take back. A player standing just inside the edge of the
    // coordinates has chunks within sight whose corners are past it — a
    // legal position, an honest report — and a survey that recorded those
    // would write a world file that this build then refuses to load, for
    // ever, taking the backup with it at the next save.
    let path = scratch("edge").join("one.world");
    let first = Server::bind(("127.0.0.1", 0), 7)
        .expect("bind")
        .keeping_at(path.clone())
        .expect("keeping");
    let addr = first.local_addr().expect("addr");
    let host = first.spawn().expect("spawn");

    // Out to where the world stops resolving. The survey does not follow the
    // whole way — nobody sails that in a report — but the player is there,
    // and the world files them there.
    let brink = Vec2::new(1_279_999.0, 0.0);
    let (client, _id, _spawn, _facing, token) = Client::join_presenting(addr, None);
    client.say(ToServer::Helm {
        hull: Underway::lying(brink, 0.0),
        tender: None,
    });
    // Asked and answered before hanging up, which is what says the helm word
    // was read: the session reads in order, so ground answered after it is
    // ground answered after the position was taken.
    client.ask_for(chunk_at(brink));
    drop(client);
    drop(host);

    // Returning puts them down on the brink, and *that* is the survey that
    // reaches past the edge. Twice, because one bad save is survivable: the
    // file that will not parse falls back to the `.old` beside it, quietly
    // losing whatever the session did. It is the second that leaves nothing
    // to fall back to, which is what makes this a world lost rather than a
    // world set back.
    for visit in 0..2 {
        let again = Server::reopen(("127.0.0.1", 0), &path).unwrap_or_else(|why| {
            panic!("a world surveyed at its own edge would not reopen: {why}")
        });
        let addr = again.local_addr().expect("addr");
        let host = again.spawn().expect("spawn");
        let (client, _id, spawn, _facing, dealt) = Client::join_presenting(addr, Some(token));
        assert_eq!(dealt, token, "visit {visit}: kept papers were re-dealt");
        assert_eq!(
            spawn, brink,
            "visit {visit}: the returner was not put down where they left"
        );
        client.hear_the_survey(HashMap::new(), |charted| !charted.is_empty());
        drop(client);
        drop(host);
    }

    Server::reopen(("127.0.0.1", 0), &path)
        .expect("a world surveyed at its own edge would not open again");
}

#[test]
fn a_player_may_hang_up_while_their_survey_is_still_being_told_back() {
    // The backfill runs on a thread of its own and re-grows every island the
    // returner has ever been near, which is seconds of work. Leaving in the
    // middle of it is ordinary — it is what a client that reconnects does —
    // and it must leave a world that goes on serving rather than one with a
    // thread still grinding out a chart for nobody.
    let path = scratch("backfill").join("one.world");
    let first = Server::bind(("127.0.0.1", 0), 7)
        .expect("bind")
        .keeping_at(path.clone())
        .expect("keeping");
    let addr = first.local_addr().expect("addr");
    let host = first.spawn().expect("spawn");

    let (client, _id, spawn, _facing, token) = Client::join_presenting(addr, None);
    let out = spawn + Vec2::new(1_536.0, 0.0);
    client.say(ToServer::Helm {
        hull: Underway::lying(out, 0.0),
        tender: None,
    });
    let sailed = client.hear_the_survey(HashMap::new(), |charted| {
        in_sight_of(out)
            .iter()
            .all(|chunk| charted.contains_key(chunk))
    });
    assert!(
        sailed.len() > 64,
        "a voyage short enough to be told in one go"
    );
    drop(client);
    drop(host);

    let again = Server::reopen(("127.0.0.1", 0), &path).expect("reopen");
    let addr = again.local_addr().expect("addr");
    let host = again.spawn().expect("spawn");

    // Several returns, each hung up on the moment the welcome lands — a
    // reconnect loop, which is the shape that stacks these threads.
    for _ in 0..4 {
        let (client, _id, _spawn, _facing, _dealt) = Client::join_presenting(addr, Some(token));
        drop(client);
    }

    // And the world is still a world: it welcomes somebody, tells them their
    // chart, and answers for ground.
    let (client, _id, spawn, _facing, _dealt) = Client::join_presenting(addr, Some(token));
    client.hear_the_survey(HashMap::new(), |charted| !charted.is_empty());
    // Which either has ground on it or is open water; what matters is that
    // it is answered at all, the read having a deadline on it.
    client.ask_for(chunk_at(spawn));
    drop(client);
    drop(host);
}

// ---------------------------------------------------------------------------
// Claiming an island
// ---------------------------------------------------------------------------
//
// The fiddly part of these is that a claim is settled against the *server's*
// survey, so a test cannot arrange one: it has to sail a client round a real
// coast until the world has followed it all the way round. What that costs is
// the two helpers below — one to find an island small enough to circle, one to
// circle it — and both are deliberately coarse. See each for what it trades.

/// The world a test is allowed to know, sampled for an island worth sailing
/// round: the middle of its land, how far that land reaches, and a spot ashore
/// well inside it.
///
/// The ground is sampled rather than the island's frame read, because a frame
/// is the parcel an island was grown in and says little about how much of it is
/// above water — half the parcels near a spawn hold nothing but a shoal — and
/// what a claim is about is the waterline.
///
/// Bounded at both ends. Too small an island closes without anybody going
/// anywhere — a coast taken in whole from the water off one side of it, which
/// at [`SIGHT_RADIUS`] is anything under a few hundred metres across — so a
/// test of having gone round would pass without the going. Too big is a circuit
/// a test spends minutes on.
fn an_island_to_sail_round(world: &Archipelago, near: Vec2) -> (Vec2, f32, Vec2) {
    let reach = Vec2::splat(2_048.0);
    let mut about: Vec<IslandSpec> = world.islands_within(near - reach, near + reach);
    about.sort_by(|a, b| {
        a.centre()
            .distance(near)
            .total_cmp(&b.centre().distance(near))
    });
    about
        .into_iter()
        .find_map(|spec| {
            let (least, most) = (
                spec.centre() - spec.extent() / 2.0,
                spec.centre() + spec.extent() / 2.0,
            );
            let mut land: Option<(Vec2, Vec2)> = None;
            let mut summit = (f32::MIN, Vec2::ZERO);
            let mut z = least.y;
            while z <= most.y {
                let mut x = least.x;
                while x <= most.x {
                    let at = Vec2::new(x, z);
                    let height = world.height(x, z);
                    if height > 0.0 {
                        land = Some(match land {
                            None => (at, at),
                            Some((lo, hi)) => (lo.min(at), hi.max(at)),
                        });
                        if height > summit.0 {
                            summit = (height, at);
                        }
                    }
                    x += 16.0;
                }
                z += 16.0;
            }
            let (lo, hi) = land?;
            let across = (hi - lo).max_element();
            // Half a kilometre is comfortably past what a survey takes in
            // from one side; three quarters is about as far as a test can
            // afford to sail. The summit is the spot ashore, being the point
            // furthest inside the waterline in every direction at once.
            (512.0..=768.0)
                .contains(&across)
                .then_some(((lo + hi) / 2.0, across, summit.1))
        })
        .expect("an island of the right size within a couple of kilometres of the spawn")
}

/// Legs to a whole circuit of an island — enough that the polygon hugs the
/// shore, few enough that a test is not a thousand round trips.
const LEGS: usize = 24;

/// Sails a client `legs` of the way round an island — [`LEGS`] of them being
/// the whole circuit — and hands back everything it has been told it surveyed.
///
/// Coarse: straight legs rather than a course anybody would steer. The survey
/// follows the *way* between two reports and not only their ends, so a polygon
/// outside the shore inks the same band a circle would, and the test spends
/// seconds rather than minutes.
///
/// The waiting is the part worth understanding. The survey follows a way only as
/// far as it believes somebody could have sailed since the last report — see
/// [`server::PLAUSIBLE_SPEED`] — and sailed with no allowance in hand it
/// follows a straight line towards each report instead of round the shore,
/// which is a stripe of coast nobody inked and a ring that never closes. So
/// every leg is paid for in real seconds at the rate the allowance fills, and
/// each waits to be told its own ground before the next is sailed.
fn sail_around(
    client: &Client,
    from: Vec2,
    centre: Vec2,
    reach: f32,
    legs: usize,
) -> HashMap<IVec2, Soundings> {
    let leg = |turn: usize| {
        let bearing = std::f32::consts::TAU * turn as f32 / LEGS as f32;
        centre + Vec2::from_angle(bearing) * reach
    };

    let mut charted = HashMap::new();
    let mut sailed = from;
    for turn in 0..=legs {
        let at = leg(turn);
        std::thread::sleep(Duration::from_secs_f32(
            sailed.distance(at) / server::PLAUSIBLE_SPEED,
        ));
        client.say(ToServer::Helm {
            hull: Underway::lying(at, 0.0),
            tender: None,
        });
        charted = client.hear_the_survey(charted, |charted| {
            in_sight_of(at)
                .iter()
                .all(|chunk| charted.contains_key(chunk))
        });
        sailed = at;
    }
    charted
}

/// Sails one leg, at the speed the world believes in.
///
/// The sleep is what [`sail_around`]'s is: the survey follows a report only as
/// far as its allowance reaches (see [`server::PLAUSIBLE_SPEED`]), so a leg
/// taken faster than a hull could take it is a leg the world only half
/// follows — and a test whose point is what was passed on the way would be
/// asking about a way that was never gone down.
fn sail_to(client: &Client, from: Vec2, to: Vec2) {
    std::thread::sleep(Duration::from_secs_f32(
        from.distance(to) / server::PLAUSIBLE_SPEED,
    ));
    client.say(ToServer::Helm {
        hull: Underway::lying(to, 0.0),
        tender: None,
    });
}

/// A spot ashore, found by walking in from the water until the test's own copy
/// of the world says there is ground underfoot — where a client would judge
/// its own footing before stepping off a boat.
fn a_shore_to_step_out_onto(world: &Archipelago, offshore: Vec2, inland: Vec2) -> Vec2 {
    (1..=64)
        .map(|step| offshore + (inland - offshore) * (step as f32 / 64.0))
        .find(|at| world.height(at.x, at.y) > 1.0)
        .expect("a shore between the water and the middle of an island")
}

/// The survey a client has been told, as the survey it is — which is how a
/// client will find the island it is standing on, and the same question the
/// server settles a claim by.
fn chart_of(charted: &HashMap<IVec2, Soundings>) -> Survey {
    let mut survey = Survey::default();
    for (chunk, ink) in charted {
        survey.record(*chunk, ink.clone());
    }
    survey
}

/// The seed the claiming tests are sailed in. Chosen rather than arbitrary,
/// and that is the whole of what is special about it: it puts an island of the
/// size these tests want — see [`an_island_to_sail_round`] — a few hundred
/// metres off the spawn, which is a coast a test can afford to go round.
const CLAIMABLE: u32 = 7;

/// How far outside the island's own reach a circuit is sailed, in metres. Off
/// the shore, and well within [`SIGHT_RADIUS`] of it.
const OFFING: f32 = 64.0;

/// A client that has joined and sailed the whole way round that island: the
/// papers it holds, the island's identity, and the spot ashore a claimant
/// would stand on.
fn sail_round_the_island(
    addr: SocketAddr,
    presenting: Option<Token>,
) -> (Client, Token, IVec2, Vec2) {
    let (client, _id, spawn, _facing, token) = Client::join_presenting(addr, presenting);
    let world = behind_the_curtain(CLAIMABLE);
    let (centre, across, ashore) = an_island_to_sail_round(&world, spawn);
    let charted = sail_around(&client, spawn, centre, across / 2.0 + OFFING, LEGS);
    assert!(
        chart_of(&charted).standing(ashore) == Standing::Ashore,
        "a coast sailed right round closes a coastline round the summit"
    );
    // The identity a cairn will be told under is the island's own, and a
    // client never derives it — a test reads it off the layout the way the
    // server will.
    let island = world
        .island_at(ashore.x, ashore.y)
        .expect("the summit stands on an island")
        .origin;
    (client, token, island, ashore)
}

#[test]
fn an_island_sailed_round_and_stood_upon_is_claimed() {
    // The whole rule in one voyage. Going round it is half of it, and the
    // half a client can see for itself; standing on it is the other, a cairn
    // being built by somebody on the ground rather than sailing past.
    let addr = host(CLAIMABLE);
    let (client, _token, island, ashore) = sail_round_the_island(addr, None);

    client.say(ToServer::Claim);
    assert!(
        client.nothing_was_said_about_a_cairn(),
        "a cairn was raised by somebody who never left the helm"
    );

    client.say(ToServer::Disembark { position: ashore });
    client.say(ToServer::Claim);
    let (told, at, covers, name, yours) = client.hear_a_cairn_whole();
    assert_eq!(told, island, "a cairn for some other island");
    assert_eq!(
        at, ashore,
        "the cairn does not stand where the claimant did"
    );
    assert!(yours, "the claimant was not told the cairn was theirs");
    assert_eq!(name, "", "an island nobody has christened came named");
    // The claim's reach holds the ground it was granted on — the whole of
    // what a sheet needs in order to letter this island's coastlines under
    // the one name.
    assert!(
        covers.0.cmple(ashore).all() && ashore.cmple(covers.1).all(),
        "the cairn stands outside what its own claim covers"
    );
}

#[test]
fn part_of_a_coast_earns_uncharted_rather_than_a_cairn() {
    // The claim is settled against the coast the world has watched somebody
    // go round, not against where they are standing: half a survey leaves
    // coastlines unclosed, and the cairn will not take until the whole of
    // the island's coast has been seen.
    let addr = host(CLAIMABLE);
    let (alice, _token, island, ashore) = sail_round_the_island(addr, None);

    // Bob sails a quarter of the same circuit and turns back — the rest is
    // coast nobody has shown him — and then steps ashore on the near side.
    //
    // Ashore *there*, and not on Alice's spot in the middle, which would not
    // be the same test: an island small enough to be seen across from its own
    // summit is one somebody standing on the summit has honestly seen the
    // whole of, and its ring closes for them. The rule is about having seen
    // the whole coast, and never about the manner of the seeing.
    let (bob, _id, spawn, _facing) = Client::join(addr);
    let world = behind_the_curtain(CLAIMABLE);
    let (centre, across, _ashore) = an_island_to_sail_round(&world, spawn);
    let reach = across / 2.0 + OFFING;
    let charted = sail_around(&bob, spawn, centre, reach, LEGS / 4);
    let offshore = centre + Vec2::from_angle(std::f32::consts::TAU / 4.0) * reach;
    let landfall = a_shore_to_step_out_onto(&world, offshore, centre);
    assert!(
        chart_of(&charted).standing(landfall) != Standing::Ashore,
        "a quarter of a coast closed a coastline"
    );

    bob.say(ToServer::Disembark { position: landfall });
    bob.say(ToServer::Claim);
    // The refusal with something to say: Bob is standing on real ground of a
    // claimable island, so what stands between him and the cairn is exactly
    // that there is more coast here than he has surveyed — and he is told
    // that, and no more than that.
    bob.hear_uncharted();

    // And the island was there to be had all along, which is what says the
    // refusal was about Bob's voyage rather than about this island.
    alice.say(ToServer::Disembark { position: ashore });
    alice.say(ToServer::Claim);
    let (told, _at, _name, yours) = alice.hear_a_cairn();
    assert_eq!((told, yours), (island, true));
}

#[test]
fn a_second_claim_is_refused_and_told_the_cairn_that_is_there() {
    // Two players who have both been all the way round it. The first has it,
    // and what the second hears is the cairn already standing — the state
    // being the whole of the answer, as a refused boarding's is.
    let addr = host(CLAIMABLE);
    let (alice, _token, island, ashore) = sail_round_the_island(addr, None);
    alice.say(ToServer::Disembark { position: ashore });
    alice.say(ToServer::Claim);
    let (_told, alices_cairn, _name, _yours) = alice.hear_a_cairn();

    // Bob goes round it too and steps ashore somewhere else on it — his own
    // spot, so that a cairn told him at Alice's is plainly hers and not his
    // own ask coming back.
    let (bob, _papers, _island, _summit) = sail_round_the_island(addr, None);
    let world = behind_the_curtain(CLAIMABLE);
    let (centre, across, _ashore) = an_island_to_sail_round(&world, alices_cairn);
    let offshore = centre + Vec2::new(across / 2.0 + OFFING, 0.0);
    let bobs_spot = a_shore_to_step_out_onto(&world, offshore, centre);
    assert_ne!(bobs_spot, alices_cairn, "both players stood on one spot");
    bob.say(ToServer::Disembark {
        position: bobs_spot,
    });
    bob.say(ToServer::Claim);
    let (told, at, _name, yours) = bob.hear_a_cairn();
    assert_eq!(told, island);
    assert_eq!(at, alices_cairn, "the cairn moved to the second asker");
    assert!(
        !yours,
        "somebody else's cairn was told as this player's own"
    );
}

#[test]
fn only_the_claimant_may_name_the_island() {
    let addr = host(CLAIMABLE);
    let (alice, _token, island, ashore) = sail_round_the_island(addr, None);
    alice.say(ToServer::Disembark { position: ashore });
    alice.say(ToServer::Claim);
    let _ = alice.hear_a_cairn();

    // The claimant's christening is granted, and rides with the cairn.
    alice.say(ToServer::Name {
        island,
        name: "Ilha Verde".to_string(),
    });
    let (told, _at, name, yours) = alice.hear_a_cairn();
    assert_eq!((told, name.as_str(), yours), (island, "Ilha Verde", true));

    // A stranger's is refused, and what comes back is the cairn exactly as it
    // stands. Bob is walked right up to it first, so that what he hears is
    // Alice's word and not an empty name he would have been handed anyway —
    // the refusal has to be shown *not writing*, and a hearer too far off to
    // read the stones could not tell the difference. See
    // [`a_passing_hull_reads_the_stones_and_a_landing_reads_the_word`].
    let (bob, _id, _spawn, _facing) = Client::join(addr);
    bob.say(ToServer::Disembark { position: ashore });
    bob.say(ToServer::Name {
        island,
        name: "Bob's Rock".to_string(),
    });
    let (told, _at, name, yours) = bob.hear_a_cairn_saying(|name| !name.is_empty());
    assert_eq!(told, island);
    assert_eq!(name, "Ilha Verde", "a stranger wrote on somebody's island");
    assert!(!yours);
}

#[test]
fn a_passing_hull_reads_the_stones_and_a_landing_reads_the_word() {
    // Presence-gated knowledge, in one voyage. Stone standing on a headland
    // says *somebody is here* to anybody who passes; the name is lettering,
    // and lettering is read by walking up to it.
    let addr = host(CLAIMABLE);
    let (alice, _token, island, ashore) = sail_round_the_island(addr, None);
    alice.say(ToServer::Disembark { position: ashore });
    alice.say(ToServer::Claim);
    let _ = alice.hear_a_cairn();
    alice.say(ToServer::Name {
        island,
        name: "Ilha Verde".to_string(),
    });
    assert_eq!(alice.hear_a_cairn().2, "Ilha Verde");

    // Bob stands off it — inside the reach a sighting carries, well outside
    // the reach a word does. Seaward of the cairn rather than at some
    // bearing of its own, so that the way he sails to get there runs away from
    // the stones and cannot brush past them.
    let world = behind_the_curtain(CLAIMABLE);
    let (centre, _across, _ashore) = an_island_to_sail_round(&world, ashore);
    let seaward = (ashore - centre).normalize_or(Vec2::X);
    let standing_off = ashore + seaward * ((server::CAIRN_SIGHT + server::CAIRN_VISIT) / 2.0);
    // Put down well out to sea beyond it first: a way sailed from the entry
    // could pass anywhere, the stones included, and a jump is not a way.
    let (bob, _id, _spawn, _facing) = Client::join(addr);
    bob.caught_up();
    let far = ashore + seaward * (2.0 * server::CAIRN_SIGHT);
    bob.say(ToServer::Command {
        line: format!("goto {} {}", far.x, far.y),
    });
    let (seaward_of_it, _) = bob.hear_put_down_alone();
    let _ = bob.hear_reply();
    sail_to(&bob, seaward_of_it, standing_off);

    let (told, at, name, yours) = bob.hear_a_cairn();
    assert_eq!(told, island, "a cairn for some other island");
    assert_eq!(at, ashore, "the stones are not where they were built");
    assert!(
        !yours,
        "somebody else's cairn was told as this player's own"
    );
    assert_eq!(
        name, "",
        "an island's name was read from a hull standing off it"
    );

    // And then he lands on it, and the stones say who was here.
    bob.say(ToServer::Disembark { position: ashore });
    let (told, _at, name, yours) = bob.hear_a_cairn_saying(|name| !name.is_empty());
    assert_eq!(told, island, "a name for some other island");
    assert_eq!(name, "Ilha Verde", "a cairn walked up to said nothing");
    assert!(!yours, "reading a cairn handed the island over");
}

#[test]
fn a_claimant_walking_round_their_own_cairn_is_not_told_about_it_again() {
    // Cairns come into sight as a player sails, and a player told about one
    // every time they report where they are would be a player whose own
    // outbox filled up standing next to their own island. What stops it is
    // that the telling only goes out when something has been *learnt* — and a
    // claimant has nothing left to learn about their own stones, wherever they
    // stand.
    let addr = host(CLAIMABLE);
    let (alice, _token, island, ashore) = sail_round_the_island(addr, None);
    alice.say(ToServer::Disembark { position: ashore });
    alice.say(ToServer::Claim);
    let (told, _at, _name, yours) = alice.hear_a_cairn();
    assert_eq!((told, yours), (island, true));

    // A few paces about it, at every remove that could earn anybody anything:
    // right beside it, a stone's throw off, and out where the stones are a
    // speck on a headland and nothing more.
    for step in [1.0, server::CAIRN_VISIT / 2.0, server::CAIRN_SIGHT / 2.0] {
        alice.say(ToServer::Move {
            position: ashore + Vec2::new(step, 0.0),
        });
    }
    assert!(
        alice.nothing_was_said_about_a_cairn(),
        "a claimant was told about their own cairn for walking past it"
    );
}

#[test]
fn what_a_landing_taught_is_still_known_when_the_world_opens_again() {
    // The knowing is the world's and lasts as long as the world does. A player
    // who has been up to somebody else's cairn does not have to go back and
    // look at it again after a night ashore — that is the whole difference
    // between a chart and a view out of a window.
    let path = scratch("knowing").join("one.world");
    let first = Server::bind(("127.0.0.1", 0), CLAIMABLE)
        .expect("bind")
        .keeping_at(path.clone())
        .expect("keeping");
    let addr = first.local_addr().expect("addr");
    let host = first.spawn().expect("spawn");

    let (alice, _token, island, ashore) = sail_round_the_island(addr, None);
    alice.say(ToServer::Disembark { position: ashore });
    alice.say(ToServer::Claim);
    let _ = alice.hear_a_cairn();
    alice.say(ToServer::Name {
        island,
        name: "Ilha Verde".to_string(),
    });
    assert_eq!(alice.hear_a_cairn().2, "Ilha Verde");

    // Bob lands, reads it, and walks back off down the shore before the world
    // shuts — far enough that where he is put back down could earn him the
    // stones and could never earn him the word on them. So a name at the door
    // is a name he remembered.
    let (bob, _id, _spawn, _facing, papers) = Client::join_presenting(addr, None);
    bob.say(ToServer::Disembark { position: ashore });
    assert_eq!(
        bob.hear_a_cairn_saying(|name| !name.is_empty()).2,
        "Ilha Verde"
    );
    let world = behind_the_curtain(CLAIMABLE);
    let (centre, _across, _ashore) = an_island_to_sail_round(&world, ashore);
    let seaward = (ashore - centre).normalize_or(Vec2::X);
    let along_the_shore = ashore + seaward * ((server::CAIRN_SIGHT + server::CAIRN_VISIT) / 2.0);
    bob.say(ToServer::Move {
        position: along_the_shore,
    });
    // Waited out before the world is shut, because a client that says a thing
    // and hangs up has not necessarily been heard say it, and where he was
    // standing is the whole of what this test rests on. One connection's words
    // are read in the order it says them, so an answer to a line typed after
    // the step is proof the step was taken.
    bob.say(ToServer::Command {
        line: "help".to_string(),
    });
    let _ = bob.hear_reply();
    drop(alice);
    drop(bob);
    drop(host);

    let again = Server::reopen(("127.0.0.1", 0), &path).expect("reopen");
    let addr = again.local_addr().expect("addr");
    let _host = again.spawn().expect("spawn");

    let (bob, _id, spawn, _facing, dealt) = Client::join_presenting(addr, Some(papers));
    assert_eq!(dealt, papers, "kept papers were re-dealt");
    assert!(
        spawn.distance(ashore) > server::CAIRN_VISIT,
        "the returner was put back within reading distance of the cairn, \
         so a name at the door proves nothing about what they remembered"
    );
    let (told, at, name, yours) = bob.hear_a_cairn();
    assert_eq!(told, island, "some other island was remembered");
    assert_eq!(at, ashore, "the cairn moved while the world was shut");
    assert_eq!(name, "Ilha Verde", "a visited cairn came back unread");
    assert!(!yours, "a visit turned into a claim overnight");
}

#[test]
fn a_word_carved_after_a_visit_does_not_chase_the_chart_that_left() {
    // A rename reaches whoever is in reach to watch it happen, and nobody
    // else. Everybody else goes on drawing the word they last read until they
    // are told of that cairn again — at the door, or on coming back into sight
    // of it — which is what a chart is for and is a good deal cheaper than the
    // world keeping every reader their own copy of every word.
    let addr = host(CLAIMABLE);
    let (alice, _token, island, ashore) = sail_round_the_island(addr, None);
    alice.say(ToServer::Disembark { position: ashore });
    alice.say(ToServer::Claim);
    let _ = alice.hear_a_cairn();
    alice.say(ToServer::Name {
        island,
        name: "Ilha Verde".to_string(),
    });
    assert_eq!(alice.hear_a_cairn().2, "Ilha Verde");

    let (bob, _id, _spawn, _facing) = Client::join(addr);
    bob.say(ToServer::Disembark { position: ashore });
    assert_eq!(
        bob.hear_a_cairn_saying(|name| !name.is_empty()).2,
        "Ilha Verde"
    );

    // Bob puts the island hull down behind him, and Alice carves it again. The
    // way there is nobody's business here — only where he ends up is, that
    // being what the telling is decided by — so this goes at the speed a test
    // goes at rather than at a hull's.
    bob.say(ToServer::Move {
        position: ashore + Vec2::splat(server::CAIRN_SIGHT),
    });
    // And waited out before Alice carves, the two of them being two
    // connections with no order between them: a rename that overtook the step
    // would be told to somebody still standing there, which is a different test
    // passing under this one's name.
    bob.say(ToServer::Command {
        line: "help".to_string(),
    });
    let _ = bob.hear_reply();
    alice.say(ToServer::Name {
        island,
        name: "Ilha Roxa".to_string(),
    });
    assert_eq!(alice.hear_a_cairn().2, "Ilha Roxa");
    assert!(
        bob.nothing_was_said_about_a_cairn(),
        "a cairn recarved on the far side of the world followed the chart that left it"
    );
}

#[test]
fn an_ask_inside_the_pace_is_answered_late_rather_than_dropped() {
    // The pacing on claims and names is there to stop one client filling
    // everybody's outbox, and it holds the rate down by making the asker
    // wait. What it must never do is swallow the ask: from the far end of a
    // socket, silence is indistinguishable from a refusal, from a message
    // lost, or from a server that has stopped listening, and a client cannot
    // be asked to tell those apart. So every one of these gets an answer —
    // the later ones a quarter-second late, which nobody sailing will notice.
    let addr = host(CLAIMABLE);
    let (client, _token, island, ashore) = sail_round_the_island(addr, None);
    client.say(ToServer::Disembark { position: ashore });

    // The first is the grant, and pays the pace. The rest arrive well inside
    // it — a claim on an island now held, and two names in a row — and the
    // count is what is being asserted: four asks, four cairns back.
    client.say(ToServer::Claim);
    client.say(ToServer::Claim);
    for name in ["Ilha Verde", "Ilha Vermelha"] {
        client.say(ToServer::Name {
            island,
            name: name.to_string(),
        });
    }

    for ask in 0..4 {
        let (told, _at, _name, yours) = client.hear_a_cairn();
        assert_eq!(told, island, "ask {ask} was answered about another island");
        assert!(yours, "ask {ask} lost the claimant their own claim");
    }
}

#[test]
fn a_name_the_wire_will_not_carry_leaves_the_cairn_as_it_was() {
    // A name is a refusal or a name; it is never half a name, and never an
    // erasure. Whatever is offered, the cairn goes on saying what it said.
    let addr = host(CLAIMABLE);
    let (client, _token, island, ashore) = sail_round_the_island(addr, None);
    client.say(ToServer::Disembark { position: ashore });
    client.say(ToServer::Claim);
    let _ = client.hear_a_cairn();
    client.say(ToServer::Name {
        island,
        name: "Ilha Verde".to_string(),
    });
    assert_eq!(client.hear_a_cairn().2, "Ilha Verde");

    for offered in [
        "   ".to_string(),
        "a".repeat(protocol::NAME_BYTES + 1),
        "Ilha\nVerde".to_string(),
    ] {
        client.say(ToServer::Name {
            island,
            name: offered.clone(),
        });
        let (told, _at, name, yours) = client.hear_a_cairn();
        assert_eq!(told, island);
        assert!(yours, "the claimant stopped holding their own claim");
        assert_eq!(
            name,
            "Ilha Verde",
            "the cairn took `{}` for a name",
            offered.escape_debug()
        );
    }
}

#[test]
fn a_returning_player_is_told_their_own_claims_however_far_off_they_are() {
    // The cairns near where somebody is put down are told because they are
    // things standing in sight; a player's own are told because they are
    // theirs. Sail away from your island, hang up, come back, and the sheet
    // has to letter it and open the pen on it — a client that was not told
    // draws its own island blank and will not write on it, with nothing to
    // say why.
    let addr = host(CLAIMABLE);
    let (client, token, island, ashore) = sail_round_the_island(addr, None);
    client.say(ToServer::Disembark { position: ashore });
    client.say(ToServer::Claim);
    let _ = client.hear_a_cairn();
    client.say(ToServer::Name {
        island,
        name: "Ilha Verde".to_string(),
    });
    assert_eq!(client.hear_a_cairn().2, "Ilha Verde");

    // Away on foot, four kilometres of it — well past `CAIRN_SIGHT` — and
    // then the line drops. The walk is waited out first: a walker hears
    // nothing of their own moving, so without a word to read back the drop
    // could reset the connection over it — see [`Client::caught_up`].
    let away = ashore + Vec2::new(4_096.0, 0.0);
    client.say(ToServer::Move { position: away });
    client.caught_up();
    let watcher = Client::join(addr).0;
    drop(client);
    // Somebody else hearing the departure is what says it has been filed: the
    // world remembers a leaver before anyone is told they left, and a rejoin
    // that raced it would be met as a stranger.
    while !matches!(watcher.hear(), ToClient::Left { .. }) {}

    let (client, _id, spawn, _facing, dealt) = Client::join_presenting(addr, Some(token));
    assert_eq!(dealt, token, "the papers were not the ones handed over");
    assert_eq!(spawn, away, "the world put them back somewhere else");
    let (told, at, name, yours) = client.hear_a_cairn();
    assert_eq!(told, island, "some other island was told");
    assert_eq!(at, ashore, "the cairn moved while they were away");
    assert_eq!(name, "Ilha Verde", "the island came back unlettered");
    assert!(yours, "they came back a stranger to their own claim");
}

#[test]
fn a_claim_and_its_name_survive_the_world_being_closed() {
    // An island claimed is a thing another player is barred from, so of
    // everything a world file keeps it is the part that has to come back
    // exactly: the same island, the same holder, the same cairn, the same
    // word on it.
    let path = scratch("claims").join("one.world");
    let first = Server::bind(("127.0.0.1", 0), CLAIMABLE)
        .expect("bind")
        .keeping_at(path.clone())
        .expect("keeping");
    let addr = first.local_addr().expect("addr");
    let host = first.spawn().expect("spawn");

    let (client, token, island, ashore) = sail_round_the_island(addr, None);
    client.say(ToServer::Disembark { position: ashore });
    client.say(ToServer::Claim);
    let _ = client.hear_a_cairn();
    client.say(ToServer::Name {
        island,
        name: "Ilha Verde".to_string(),
    });
    assert_eq!(client.hear_a_cairn().2, "Ilha Verde");
    drop(client);
    drop(host);

    let again = Server::reopen(("127.0.0.1", 0), &path).expect("reopen");
    let addr = again.local_addr().expect("addr");
    let _host = again.spawn().expect("spawn");

    // Put back down where they left, which is beside their own cairn: a
    // joining player is told the cairns standing near them.
    let (client, _id, spawn, _facing, dealt) = Client::join_presenting(addr, Some(token));
    assert_eq!(dealt, token, "kept papers were re-dealt");
    assert_eq!(
        spawn, ashore,
        "the claimant was not put back on their island"
    );
    let (told, at, name, yours) = client.hear_a_cairn();
    assert_eq!(told, island, "the world reopened on somebody else's island");
    assert_eq!(at, ashore, "the cairn moved while the world was shut");
    assert_eq!(name, "Ilha Verde", "the island forgot its name");
    assert!(
        yours,
        "the claimant came back a stranger to their own claim"
    );
}

/// A point in the shallows a shark calls home, found by walking the line
/// from open water to an island's middle with the test's own copy of the
/// world — the strip where a coast shelves from the deep to the beach.
fn shallows_between(world: &Archipelago, offshore: Vec2, ashore: Vec2) -> Option<Vec2> {
    let span = ashore - offshore;
    let steps = (span.length() / 2.0).ceil() as i32;
    (0..steps).find_map(|step| {
        let at = offshore + span * (step as f32 / steps as f32);
        // Inside the band with a metre to spare either side, so a shark
        // raised on a ring around this point has a strip to live in rather
        // than a line.
        ((-5.0..=-2.5).contains(&world.height(at.x, at.y))).then_some(at)
    })
}

#[test]
fn a_shark_is_raised_out_in_the_deep_water_and_swims_in() {
    let addr = host(7);
    let (client, _id, spawn, facing) = Client::join(addr);

    // Stand in the shallows off the entry island: the strip between the open
    // water players spawn on and the island they face. That is the water a
    // shark lives in, so standing here is what puts one in these waters.
    let world = behind_the_curtain(7);
    let shallows =
        shallows_between(&world, spawn, facing).expect("the entry island should have a coast");
    client.say(ToServer::Helm {
        hull: Underway::lying(shallows, 0.0),
        tender: None,
    });

    let (_id, position, velocity) = client.hear_a_beast(BeastKind::Shark);

    // But it is not raised there: the first anyone hears of a beast, it is
    // out in deep water swimming in — nothing is ever watched appearing, and
    // the swim in is what the shallows get instead. Judged by the test's own
    // world, with slack for the swimming it has already done by the time the
    // word arrives. That it arrives is the beasts' own tests' business, a
    // crossing being minutes of swimming.
    let floor = world.height(position.x, position.y);
    assert!(
        floor <= -7.0,
        "a shark was raised over ground at {floor} m, which is not the deep"
    );

    // And already going somewhere, at a swim rather than a bolt: the wire
    // carries a velocity so a client can draw the glide between tellings.
    let pace = velocity.length();
    assert!(
        (0.2..=4.0).contains(&pace),
        "a shark swimming in at {pace} m/s"
    );
}

#[test]
fn a_shark_is_forgotten_when_everyone_leaves_its_waters() {
    let addr = host(7);
    let (client, _id, spawn, facing) = Client::join(addr);
    let world = behind_the_curtain(7);
    let shallows =
        shallows_between(&world, spawn, facing).expect("the entry island should have a coast");
    client.say(ToServer::Helm {
        hull: Underway::lying(shallows, 0.0),
        tender: None,
    });
    let (shark, _, _) = client.hear_a_beast(BeastKind::Shark);

    // Sail straight out to sea, away from the island, further than any
    // shark is minded.
    let away = (spawn - facing).normalize_or(Vec2::X);
    client.say(ToServer::Helm {
        hull: Underway::lying(shallows + away * 2_000.0, 0.0),
        tender: None,
    });

    // The word comes that the sea is emptier by one — the shark left behind,
    // not killed, and no longer anybody's business to hear about. Not at
    // once, though: leaving an animal's waters is given a few seconds to turn
    // out to have been a tack rather than a departure.
    let deadline = Instant::now() + PATIENCE;
    loop {
        if let ToClient::BeastGone { id } = client.hear_by(deadline, "the shark being let go") {
            if id == shark {
                break;
            }
        }
    }
}

#[test]
fn console_lines_are_answered_and_a_time_command_reaches_everyone() {
    // A world at noon, with one player typing into the console and another
    // just sailing.
    let addr = host_at(7, 0.5);
    let (asker, _, spawn, facing) = Client::join(addr);
    let (bystander, ..) = Client::join(addr);

    // `help` is the vocabulary's own index, answered to the asker alone.
    asker.say(ToServer::Command {
        line: "help".to_string(),
    });
    let help = asker.hear_reply();
    for verb in ["spawn", "time", "weather"] {
        assert!(help.contains(verb), "`help` does not mention {verb}");
    }

    // A line the server does not know gets an answer, not a hang-up: the
    // console is the one place a client speaks words the server never
    // promised to understand.
    asker.say(ToServer::Command {
        line: "dance".to_string(),
    });
    let lost = asker.hear_reply();
    assert!(lost.contains("help"), "no way out of: {lost}");

    // The seed, which is the one thing about this world a client is never
    // sent and so the one thing it could not otherwise say. A guest is every
    // client here: the ground arrives generated, and nothing on the wire
    // carries the number it was generated from.
    asker.say(ToServer::Command {
        line: "world seed".to_string(),
    });
    let seed = asker.hear_reply();
    assert!(seed.contains('7'), "the seed was answered: {seed}");

    // A summons from the shallows, where there is shark water to answer it.
    let world = behind_the_curtain(7);
    let shallows =
        shallows_between(&world, spawn, facing).expect("the entry island should have a coast");
    asker.say(ToServer::Helm {
        hull: Underway::lying(shallows, 0.0),
        tender: None,
    });
    asker.say(ToServer::Command {
        line: "spawn shark".to_string(),
    });
    let summons = asker.hear_reply();
    assert!(
        summons.starts_with("a shark rises") && summons.ends_with("m away"),
        "the summons came to: {summons}"
    );

    // The day run on to evening: the asker hears what came of it, and the
    // bystander's sun moves without them having asked anything — on the beat
    // of the command, not of the sky thread's next telling. The deadline
    // does the proving: at ten minutes to the day, the clock could not reach
    // evening from noon on its own in under two and a half minutes, and a
    // loaded machine has the whole of the minute to tell of the jump.
    asker.say(ToServer::Command {
        line: "world time 18:00".to_string(),
    });
    assert_eq!(asker.hear_reply(), "the day has run on to 18:00");
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut heard = bystander.hear_the_time_by(deadline, "the evening the console ordered");
    while (heard - 0.75).abs() >= 0.01 {
        heard = bystander.hear_the_time_by(
            deadline,
            &format!("the evening the console ordered, the bystander's day standing at {heard}"),
        );
    }
}

#[test]
fn pods_and_whales_share_the_open_water() {
    // The spawn is open water off an island's coast, which is every kind of
    // sea at once: shallows along the shore for sharks, depth on the seaward
    // side for the rest. Standing still there, a player is told about all
    // three kinds — the dolphins and the whale being the animals that were
    // once each client's own dice roll, promoted to beasts so that two
    // players can point at the same one.
    let addr = host(7);
    let (client, _id, spawn, facing) = Client::join(addr);
    let world = behind_the_curtain(7);
    let shallows =
        shallows_between(&world, spawn, facing).expect("the entry island should have a coast");
    client.say(ToServer::Helm {
        hull: Underway::lying(shallows, 0.0),
        tender: None,
    });

    let mut kinds = std::collections::HashSet::new();
    let deadline = Instant::now() + PATIENCE;
    while kinds != [BeastKind::Shark, BeastKind::Dolphins, BeastKind::Whale].into() {
        let awaited = format!("a kind at anchor beyond the {kinds:?} offered so far");
        if let ToClient::Beast { kind, .. } = client.hear_by(deadline, &awaited) {
            kinds.insert(kind);
        }
    }
}

#[test]
fn a_summons_can_raise_a_crowd_worth_timing_the_client_with() {
    // `spawn <kind> <count>` exists to put more animals on one screen than
    // play ever will, which is how the drawing and the beat's own telling get
    // measured. So the thing worth pinning is that the count is honoured all
    // the way onto the wire: a batch that quietly became one beast would make
    // every measurement taken with it a measurement of nothing.
    const CROWD: usize = 25;

    let addr = host(7);
    let (client, ..) = Client::join(addr);
    client.say(ToServer::Command {
        line: format!("spawn dolphins {CROWD}"),
    });
    let reply = client.hear_reply();
    assert!(
        reply.starts_with(&format!("{CROWD} pods surface")),
        "the console answered `{reply}`"
    );

    // And they are all told of, each as its own beast: the ids are what a
    // client keeps its entities under, so distinct ids are the whole of the
    // claim. Counted over a couple of beats, since one beat's worth of
    // tellings is exactly what a client is redrawing from.
    let mut pods = std::collections::HashSet::new();
    let deadline = Instant::now() + PATIENCE;
    while pods.len() < CROWD {
        let awaited = format!(
            "the rest of {CROWD} summoned pods, {} told of so far",
            pods.len()
        );
        if let ToClient::Beast { id, kind, .. } = client.hear_by(deadline, &awaited) {
            if kind == BeastKind::Dolphins {
                pods.insert(id);
            }
        }
    }
}

#[test]
fn grant_deals_a_hull_and_leaves_the_boarding_to_whoever_asked() {
    let addr = host(7);
    let (client, _id, spawn, _token, aboard) = Client::join_aboard(addr, None);
    let ship = aboard.expect("a world is entered at a helm");
    let world = behind_the_curtain(7);
    // The welcome introduces this player to their own ship among the rest of
    // the joining chatter, so everything after here is a word this test
    // asked for.
    client.caught_up();

    // A tender, asked for from a helm: put in the water where the asker is,
    // at anchor with nobody aboard. A grant deals a boat and stops there —
    // taking its helm is the ordinary business of walking up to a free one.
    client.say(ToServer::Command {
        line: "grant rowboat".to_string(),
    });
    let (tender, kind, at, _heading, occupant) = client.hear_a_hull_dealt(&[ship]);
    assert_eq!(kind, BoatKind::Rowboat, "some other hull was dealt");
    assert_ne!(tender, ship, "the ship they were steering answered a grant");
    assert_eq!(occupant, None, "the grant seated its asker");
    assert_eq!(at, spawn, "the hull was dealt somewhere else");
    let reply = client.hear_reply();
    assert!(reply.starts_with("a rowboat is here"), "answered: {reply}");

    // Asked for again, and it is a second hull: a boat is nobody's to bring
    // back, so what a grant adds to the world stays in it.
    client.say(ToServer::Command {
        line: "grant rowboat".to_string(),
    });
    let (again, ..) = client.hear_a_hull_dealt(&[ship, tender]);
    assert_ne!(
        again, tender,
        "a grant brought a hull back rather than dealing one"
    );
    let reply = client.hear_reply();
    assert!(reply.starts_with("a rowboat is here"), "answered: {reply}");

    // A sloop, from the deck of the one they already have: minted, like
    // every other, rather than the hull under them dealt back to them.
    client.say(ToServer::Command {
        line: "grant sloop".to_string(),
    });
    let (other, kind, _at, _heading, occupant) = client.hear_a_hull_dealt(&[ship, tender, again]);
    assert_eq!(kind, BoatKind::Sloop, "some other hull was dealt");
    assert_ne!(other, ship, "the hull under them was dealt back to them");
    assert_eq!(occupant, None, "the grant seated its asker");
    let reply = client.hear_reply();
    assert!(reply.starts_with("a sloop is here"), "answered: {reply}");

    // And from dry land, where a hull cannot be: it lies off the nearest
    // shore instead, and the answer says how far there is to swim for it.
    let inland = world.first_land().expect("a world has islands").centre();
    assert!(
        world.height(inland.x, inland.y) >= 0.0,
        "the middle of an island should be land for this to be about anything"
    );
    client.say(ToServer::Command {
        line: format!("goto {} {}", inland.x, inland.y),
    });
    client.hear_put_down_alone();
    client.hear_reply();
    client.say(ToServer::Disembark { position: inland });
    client.caught_up();

    client.say(ToServer::Command {
        line: "grant rowboat".to_string(),
    });
    let (_ashore, _kind, off, _heading, _occupant) =
        client.hear_a_hull_dealt(&[ship, tender, again, other]);
    assert!(
        world.height(off.x, off.y) < 0.0,
        "a hull was dealt onto dry land at {off}"
    );
    let reply = client.hear_reply();
    assert!(
        reply.starts_with("a rowboat is in the water") && reply.contains("m off"),
        "a grant from a hilltop said nothing about the walk: {reply}"
    );
}

#[test]
fn where_reads_the_helm_the_water_and_the_tow() {
    let world = behind_the_curtain(7);
    let addr = host(7);
    let (client, _id, spawn, _token, aboard) = Client::join_aboard(addr, None);
    let ship = aboard.expect("a newcomer's story starts aboard");

    // Aboard and afloat, which is how every world is entered: the helm names
    // the hull, and the water under it is the open sea the world has there —
    // too deep for the hook, which is the first thing a newcomer learns.
    client.say(ToServer::Command {
        line: "where".to_string(),
    });
    let reply = client.hear_reply();
    let open = -world.height(spawn.x, spawn.y);
    assert!(
        reply.starts_with("at the helm of a sloop at"),
        "a newcomer at a helm was answered: {reply}"
    );
    assert!(
        reply.contains(&format!("afloat in {open:.1} m, too deep to anchor")),
        "the world has {open:.1} m at the entry, and `where` said: {reply}"
    );
    // And the boat astern, which is the third thing the reading is for.
    assert!(
        reply.contains("towing a rowboat"),
        "a newcomer's ship has its boat on the painter, and `where` said: {reply}"
    );

    // Off a shore, taken there by the console, which berths a driven hull
    // in water the anchor holds in — see `world::archipelago::a_berth`.
    let inland = world
        .first_land()
        .expect("a world has islands in it")
        .centre();
    client.say(ToServer::Command {
        line: format!("goto {} {}", inland.x, inland.y),
    });
    let (berth, _) = client.hear_put_down_alone();
    let _ = client.hear_reply();
    client.say(ToServer::Command {
        line: "where".to_string(),
    });
    let reply = client.hear_reply();
    let depth = -world.height(berth.x, berth.y);
    assert!(
        reply.contains(&format!("afloat in {depth:.1} m, and an anchor holds here")),
        "a hull is berthed at an anchorage, and `where` said: {reply}"
    );

    // Ashore on their own feet. The sloop is nobody's now, so nothing is
    // read about it or the boat behind it.
    // The beach the berth lies off, walked to along the line to the island's
    // middle: a few dozen metres, so the hull left behind is still alongside
    // enough to board back from at the end.
    let towards = (inland - berth).normalize();
    let ashore = (1..200)
        .map(|out| berth + towards * out as f32 * 2.0)
        .find(|at| world.height(at.x, at.y) >= 0.0)
        .expect("a berth lies off a shore that can be walked up");
    client.say(ToServer::Disembark { position: ashore });
    client.caught_up();
    client.say(ToServer::Command {
        line: "where".to_string(),
    });
    let reply = client.hear_reply();
    let up = world.height(ashore.x, ashore.y);
    assert!(
        reply.starts_with("afoot at"),
        "somebody who stepped ashore was answered: {reply}"
    );
    assert!(
        reply.contains(&format!("ashore, {up:.1} m above the water")),
        "the land stands {up:.1} m up, and `where` said: {reply}"
    );
    assert!(
        !reply.contains("towing"),
        "a walker was read as towing something: {reply}"
    );

    // Back out to the hull on their own feet, which is a swim: afoot, but
    // with water under them rather than beach, and the fifth wording the
    // reading has. Beside the sloop as well, a boarding being granted only
    // within `BOARD_GRANT` of a helm.
    client.say(ToServer::Move { position: berth });
    client.caught_up();
    client.say(ToServer::Command {
        line: "where".to_string(),
    });
    let reply = client.hear_reply();
    assert!(
        reply.contains(&format!("in {depth:.1} m of water")),
        "a swimmer over {depth:.1} m was answered: {reply}"
    );

    // And back aboard, where the hull is under them again rather than off
    // in the distance: the same hull must not be read both ways at once.
    client.say(ToServer::Board { boat: ship });
    client.caught_up();
    client.say(ToServer::Command {
        line: "where".to_string(),
    });
    let reply = client.hear_reply();
    assert!(
        reply.starts_with("at the helm of a sloop at"),
        "a boarding was not read back: {reply}"
    );
    assert!(
        reply.contains("towing a rowboat"),
        "the tow outlasts a step ashore, and `where` said: {reply}"
    );

    // And run up the beach, which no `goto` will do — a jump to dry land
    // stands a hull off the shore instead — but which a client steering for
    // itself can, the helm being the one thing it is the authority on. That
    // is the state the reading exists to name.
    client.say(ToServer::Helm {
        hull: Underway::lying(ashore, 0.0),
        tender: None,
    });
    client.caught_up();
    client.say(ToServer::Command {
        line: "where".to_string(),
    });
    let reply = client.hear_reply();
    assert!(
        reply.contains(&format!("aground, with {up:.1} m of it out of the water")),
        "a hull run up a beach {up:.1} m above the water was answered: {reply}"
    );
}

#[test]
fn goto_takes_a_player_to_a_place_however_they_are_travelling() {
    // Entry is aboard a ship, so the first jump is a ship's: asked for the
    // middle of an island, which is the one place a hull cannot be.
    let addr = host(7);
    let (client, id, spawn, _token, aboard) = Client::join_aboard(addr, None);
    let ship = aboard.expect("a world is entered at a helm");
    let (tender, _) = client.hear_the_tender_of(ship);
    let world = behind_the_curtain(7);
    let inland = world.first_land().expect("a world has islands").centre();
    assert!(
        world.height(inland.x, inland.y) >= 0.0,
        "the middle of an island should be land for this to be about anything"
    );
    // The welcome introduces this player to their own ship among the rest of
    // the joining chatter, and everything after here reads on the
    // understanding that a word about a boat is a word this jump caused.
    client.caught_up();

    client.say(ToServer::Command {
        line: format!("goto {} {}", inland.x, inland.y),
    });
    // Nothing about the ship on the way to it: this client's own simulation
    // is the authority on the hull it steers, and a jump that told it where
    // its own hull was would be the world overruling it twice — once with
    // the put down, which it is entitled to, and once with a `Boat` telling,
    // which it is not. Everybody else hears that telling.
    let (anchorage, heading) = client.hear_put_down_alone();
    assert!(
        world.height(anchorage.x, anchorage.y) < 0.0,
        "the ship was put down on dry land at {anchorage}"
    );
    let bow = heading.expect("a hull anchored off a shore is turned to face it");
    let towards = f32::atan2(-(inland - anchorage).x, -(inland - anchorage).y);
    assert!(
        (bow - towards).abs() < 1e-3,
        "the ship lies on {bow} rad with the island it was brought to see on {towards}"
    );
    let reply = client.hear_reply();
    assert!(
        reply.starts_with("the shore is") && reply.contains("m off the bow"),
        "the jump was answered: {reply}"
    );

    // Ashore on their own feet, where the ship cannot follow — and a jump
    // out to sea from there is a swim, not a sailing. The world puts them in
    // the water they asked for and deals them nothing: a hull is something
    // to be given, not something to be got at by asking to be somewhere.
    client.say(ToServer::Disembark { position: inland });
    client.caught_up();
    client.say(ToServer::Command {
        line: format!("goto {} {}", anchorage.x, anchorage.y),
    });
    let (afloat, facing) = client.hear_put_down_alone();
    assert_eq!(
        afloat, anchorage,
        "the water asked for is the water arrived at"
    );
    assert_eq!(
        facing, None,
        "the world had an opinion about a swimmer's bearing"
    );
    let reply = client.hear_reply();
    assert!(reply.starts_with("you are at"), "answered: {reply}");
    assert!(
        client.no_hull_was_dealt(&[ship, tender]),
        "a swimmer was dealt a hull"
    );

    // The ship is where they left it, which is the only reason there is a
    // helm to take back: the swim was out to it, and boarding is how anybody
    // gets aboard anything now.
    client.say(ToServer::Board { boat: ship });
    let deadline = Instant::now() + PATIENCE;
    let at = loop {
        let (hull, occupant, _, _) = client.hear_of(ship, deadline, "the boarding granted");
        if occupant == Some(id) {
            break hull.at;
        }
    };
    // Give or take the sea's hand on a hull nobody anchored — see
    // [`ADRIFT`].
    assert!(
        at.distance(anchorage) < ADRIFT,
        "the ship had drifted off its anchorage: {at} for {anchorage}"
    );

    // At a helm again, and asked for open water: the plainest of the cases,
    // the hull simply going where it was sent with its crew aboard. Somewhere
    // well clear of everywhere this session has been, so that what the
    // survey says about it cannot be ground charted earlier — sixteen
    // hundred metres is five times `SIGHT_RADIUS`, and nothing between here
    // and there was sailed past, every leg of this test being a jump.
    let open_sea = (1..64)
        .map(|out| inland + Vec2::new(out as f32 * 250.0, 0.0))
        .find(|at| {
            world.height(at.x, at.y) < 0.0
                && [spawn, anchorage, inland]
                    .iter()
                    .all(|been| at.distance(*been) > 1_600.0)
        })
        .expect("open water somewhere east of the island");
    client.say(ToServer::Command {
        line: format!("goto {} {}", open_sea.x, open_sea.y),
    });
    let (sailed, bearing) = client.hear_put_down_alone();
    assert_eq!(sailed, open_sea, "the hull went somewhere else");
    assert!(
        bearing.is_some(),
        "a hull left to lie where it was put was given no bearing to lie on"
    );
    let reply = client.hear_reply();
    assert!(
        reply.starts_with("you are at"),
        "water at a helm was answered as though it were a shore: {reply}"
    );

    // And the ground of the new place arrives with them, which is the whole
    // reason this command lives on the server rather than in the client's
    // own debug socket: a jump is not a voyage, so the wake reopens where
    // they now are and the survey is theirs on the spot. None of this was
    // sailed to, so a survey that came from anywhere but the reopening would
    // have nothing to say about these chunks at all — a wake left where it
    // was fails this by never sending them, which is the reader's own
    // twenty-second bound rather than an assertion of ours.
    let wanted = in_sight_of(open_sea);
    client.hear_the_survey(HashMap::new(), |charted| {
        wanted.iter().all(|chunk| charted.contains_key(chunk))
    });

    // And a walker asked for land is simply stood on it: the point itself,
    // not an offing, and no hull follows them inland. The ship stays out
    // here, which is what walking away from a boat has always done.
    client.say(ToServer::Disembark { position: open_sea });
    client.caught_up();
    client.say(ToServer::Command {
        line: format!("goto {} {}", inland.x, inland.y),
    });
    let (standing, facing) = client.hear_put_down_alone();
    assert_eq!(
        standing, inland,
        "a walker was moved off the point they named"
    );
    assert_eq!(
        facing, None,
        "the world had an opinion about a walker's bearing"
    );
    assert!(
        client.no_hull_was_dealt(&[ship, tender]),
        "a hull followed a walker up the beach"
    );
}

#[test]
fn an_empty_hull_is_shoved_where_the_client_that_pushed_it_says() {
    // Nobody reports a boat nobody is aboard, so a boat shoved by somebody's
    // bow would go on lying wherever the world last heard of it. Whoever
    // shoved it answers for it instead — see `ToServer::Shove` — and the
    // rest of the world hears where it went, way and all.
    let addr = host(7);
    let (alice, a, spawn, _token, aboard) = Client::join_aboard(addr, None);
    let ship = aboard.expect("a newcomer's story starts aboard");

    // Her boat, stepped down into and then out of onto the shore: off the
    // painter now, and the one hull in this world nobody is answering for.
    let (tender, alongside) = alice.hear_the_tender_of(ship);
    alice.say(ToServer::Board { boat: tender });
    alice.boat_changed_hands(tender, Some(a));
    alice.say(ToServer::Disembark { position: spawn });
    alice.boat_changed_hands(tender, None);

    // Somebody else, to hear about it: a shove reaches everyone but the
    // client that reported it, exactly as a helm report does.
    let (bob, ..) = Client::join_aboard(addr, None);
    bob.caught_up();

    let shoved = alongside + Vec2::new(2.0, -1.0);
    alice.say(ToServer::Shove {
        boat: tender,
        hull: Underway {
            at: shoved,
            heading: 0.25,
            // Still making way, as a boat knocked clear is — and the half of
            // this the wire used to leave out entirely.
            way: Vec2::new(1.0, -0.5),
            swinging: 0.0,
        },
    });

    let deadline = Instant::now() + PATIENCE;
    loop {
        if let ToClient::Boat { id, hull, .. } = bob.hear_by(deadline, "the shoved hull") {
            if id == tender && hull.at == shoved {
                assert_eq!(hull.way, Vec2::new(1.0, -0.5), "it arrived lying still");
                assert_eq!(hull.heading, 0.25);
                break;
            }
        }
    }
}

#[test]
fn a_hull_somebody_is_aboard_is_not_shoved_by_anybody_elses_word() {
    // The rule that keeps a shove from being a way to move somebody else's
    // boat about: a hull with a player at its helm is that player's to
    // report, and a word about it from anyone else changes nothing at all.
    let addr = host(7);
    let (alice, ..) = Client::join_aboard(addr, None);
    let (bob, _, bob_spawn, _token, bobs) = Client::join_aboard(addr, None);
    let bobs_ship = bobs.expect("a newcomer's story starts aboard");
    alice.caught_up();

    // Alice claims to have pushed Bob's ship across the bay.
    let away = bob_spawn + Vec2::new(400.0, 0.0);
    alice.say(ToServer::Shove {
        boat: bobs_ship,
        hull: Underway::lying(away, 0.0),
    });
    alice.caught_up();

    // Nothing was relayed, and the world still has his ship where he left
    // it: he says where it is, and nobody else does. Bob's own word for it
    // is what Alice hears, and it is the one that took.
    bob.say(ToServer::Helm {
        hull: Underway::lying(bob_spawn, 0.0),
        tender: None,
    });
    let deadline = Instant::now() + PATIENCE;
    loop {
        if let ToClient::Boat { id, hull, .. } = alice.hear_by(deadline, "word of Bob's ship") {
            if id == bobs_ship {
                assert_eq!(
                    hull.at, bob_spawn,
                    "somebody else's word moved a hull its own helmsman was holding"
                );
                break;
            }
        }
    }
}

/// Open water past anchoring, out from `spawn`: sounded off the world rather
/// than assumed — the flat ocean floor, anywhere no island answers for.
/// Bounded, so a layout with no such water along this line panics with the
/// message instead of generating islands forever.
fn open_sea_near(world: &Archipelago, spawn: Vec2) -> Vec2 {
    (1..64)
        .map(|ring| spawn + Vec2::new(200.0 * ring as f32, 0.0))
        .find(|at| world.height(at.x, at.y) < -protocol::ground::ANCHOR_DEPTH)
        .expect("an ocean has open water in it")
}

/// Water an anchor holds in with sea room down the wind: the spot nearest
/// `near` over the shelf whose whole swing downwind is afloat. The console
/// berths a hull a sounding off the shore, and a hull whose cable would lay
/// it on the beach fetches up instead of swinging — see `server::sea` — so a
/// test of the swing wants water the swing can happen in.
fn an_anchorage_with_sea_room(world: &Archipelago, near: Vec2, downwind: Vec2) -> Vec2 {
    let afloat = |at: Vec2| world.height(at.x, at.y) < -2.0;
    let holds = |at: Vec2| world.height(at.x, at.y) > -protocol::ground::ANCHOR_DEPTH;
    let swing = protocol::ground::ANCHOR_SWING + 3.0;
    let mut best: Option<(f32, Vec2)> = None;
    for dz in -64..=64 {
        for dx in -64..=64 {
            let at = near + Vec2::new(dx as f32, dz as f32) * 4.0;
            let room = (0..=6).all(|i| afloat(at + downwind * (swing * i as f32 / 6.0)));
            if holds(at) && room && best.is_none_or(|(d, _)| at.distance(near) < d) {
                best = Some((at.distance(near), at));
            }
        }
    }
    best.expect("some water over the shelf with sea room down the wind")
        .1
}

/// The console's `gale`, as the sea tests order it after its `breeze`: a
/// bearing a right angle from the breeze's, so ordering one after the other
/// is a wind that has veered. The vector is the console's own — see
/// `server::console` — and is held to here by the assertions that read it
/// off the water.
const GALE: Vec2 = Vec2::new(11.31, -11.31);

#[test]
fn an_anchor_holds_over_the_shelf_and_not_over_the_open_sea() {
    // The ocean's floor lies past [`protocol::ground::ANCHOR_DEPTH`] by
    // construction, so a ship out there is refused its anchor — and the
    // refusal is the ship's own state, to the asker alone, with no hook
    // down. Back over the shelf the same ask serves, the hook going down
    // where the hull lies; `where` reads the same fact; a deck stepped off
    // at anchor stays within a cable of its hook; and the crew back aboard
    // weighs it.
    let addr = host(7);
    let (client, id, spawn, _token, aboard) = Client::join_aboard(addr, None);
    let ship = aboard.expect("a newcomer's story starts aboard");
    let (tender, _) = client.hear_the_tender_of(ship);
    client.order("world weather breeze");
    let berth = berthed_off_the_first_land(&client, 7);

    let world = behind_the_curtain(7);
    let deep = open_sea_near(&world, spawn);
    client.say(ToServer::Helm {
        hull: Underway::lying(deep, 0.0),
        tender: Some(Underway::lying(deep + Vec2::new(0.0, 8.0), 0.0)),
    });
    client.say(ToServer::Anchor);
    let (hull, _, _, anchor) = client.hear_of(ship, Instant::now() + PATIENCE, "the refusal");
    assert_eq!(hull.at, deep, "the refusal moved the ship");
    assert_eq!(anchor, None, "an anchor held over the open ocean");

    // Back to the berth by the same report, tender and all: a hull the
    // client moves is the client's to report, boat on the painter included.
    client.say(ToServer::Helm {
        hull: Underway::lying(berth, 0.0),
        tender: Some(Underway::lying(berth + Vec2::new(0.0, 8.0), 0.0)),
    });
    client.say(ToServer::Anchor);
    let (hull, _, _, anchor) = client.hear_of(ship, Instant::now() + PATIENCE, "the anchor");
    assert_eq!(hull.at, berth, "dropping the hook moved the ship");
    let hook = anchor.expect("an anchor refused over the shelf");
    assert_eq!(
        hook, berth,
        "the hook went down somewhere other than under the hull"
    );
    let reply = client.order("where");
    assert!(
        reply.contains("riding at anchor"),
        "`where` read a hull at anchor as: {reply}"
    );

    // Stepped down into the boat, the ship is left at anchor: whatever the
    // sea says of it over the next while, it stays within a cable of its
    // hook — falling back downwind on it and turning to ride bow to the
    // wind is the most the sea does to a hull on its hook.
    client.say(ToServer::Board { boat: tender });
    client.boat_changed_hands(tender, Some(id));
    let (_, occupant, _, anchor) = client.hear_of(ship, Instant::now() + PATIENCE, "the ship left");
    assert_eq!(occupant, None, "the ship was not left");
    assert_eq!(anchor, Some(hook), "stepping off weighed the anchor");
    let listened = Instant::now() + Duration::from_secs(3);
    while let Ok(word) = client.heard_by(listened) {
        if let ToClient::Boat {
            id, hull, anchor, ..
        } = word
        {
            if id == ship {
                assert!(
                    hull.at.distance(hook) <= protocol::ground::ANCHOR_SWING + 0.05,
                    "a ship at anchor was carried to {}, {} m from its hook",
                    hull.at,
                    hull.at.distance(hook)
                );
                assert_eq!(anchor, Some(hook), "the sea weighed the anchor");
            }
        }
    }

    // Back aboard from the thwarts, the crew weighs it — told to everyone,
    // the hull's state being the whole of the answer.
    client.say(ToServer::Board { boat: ship });
    client.boat_changed_hands(ship, Some(id));
    client.say(ToServer::Weigh);
    let deadline = Instant::now() + PATIENCE;
    loop {
        let (_, occupant, _, anchor) = client.hear_of(ship, deadline, "the anchor weighed");
        assert_eq!(occupant, Some(id));
        if anchor.is_none() {
            break;
        }
    }
    let reply = client.order("where");
    assert!(
        reply.contains("an anchor holds here"),
        "`where` read a hull with its anchor weighed as: {reply}"
    );
}

#[test]
fn a_ship_stepped_off_unanchored_is_left_adrift_with_its_boat_astern() {
    // The other half of the rule: a deck may be stepped off anywhere now,
    // and one stepped off with no hook down is the sea's. Over the open
    // ocean in a gale, the ship is carried down the wind and told so, way
    // and all, and the boat on its painter goes with it, a painter's
    // length astern.
    let addr = host(7);
    let (client, _id, spawn, _token, aboard) = Client::join_aboard(addr, None);
    let ship = aboard.expect("a newcomer's story starts aboard");
    let (tender, _) = client.hear_the_tender_of(ship);
    client.order("world weather gale");
    client.caught_up();

    let world = behind_the_curtain(7);
    let deep = open_sea_near(&world, spawn);
    client.say(ToServer::Helm {
        hull: Underway::lying(deep, 0.0),
        tender: Some(Underway::lying(deep + Vec2::new(0.0, 8.0), 0.0)),
    });
    // Over the side rather than down into the boat, so the boat stays on
    // the painter to be carried along.
    client.say(ToServer::Disembark {
        position: deep + Vec2::new(3.0, 0.0),
    });
    let (_, occupant, _, anchor) = client.hear_of(ship, Instant::now() + PATIENCE, "the ship left");
    assert_eq!(occupant, None, "the ship was not left");
    assert_eq!(anchor, None, "a ship nobody anchored has a hook down");

    let downwind = GALE.normalize();
    let deadline = Instant::now() + PATIENCE;
    let carried = loop {
        let (hull, _, _, _) = client.hear_of(ship, deadline, "the ship carried off");
        if (hull.at - deep).dot(downwind) > 1.0 {
            assert!(
                hull.way.dot(downwind) > 0.0 && hull.way.length() > 0.1,
                "a hull adrift in a gale was told making {}",
                hull.way
            );
            break hull;
        }
    };
    let (boat, _, towed_by, _) = client.hear_of(tender, deadline, "the boat astern");
    assert_eq!(towed_by, Some(ship), "the boat came off the painter");
    let behind = carried.at
        + Vec2::new(carried.heading.sin(), carried.heading.cos()) * protocol::TENDER_ASTERN;
    assert!(
        boat.at.distance(behind) < 1.0,
        "the boat lies at {} rather than a painter's length astern of a ship at {}",
        boat.at,
        carried.at
    );
}

#[test]
fn an_anchored_hull_swings_to_lie_downwind_of_its_hook() {
    // Anchored in a breeze and left; then the wind veers a right angle. The
    // hull is drawn round to lie downwind of its hook on the new wind, and
    // at no point is it further from the hook than its cable.
    let addr = host(7);
    let (client, _id, _spawn, _token, aboard) = Client::join_aboard(addr, None);
    let ship = aboard.expect("a newcomer's story starts aboard");
    client.order("world weather breeze");
    let berth = berthed_off_the_first_land(&client, 7);
    // And from the berth to water the swing has room in.
    let world = behind_the_curtain(7);
    let anchorage = an_anchorage_with_sea_room(&world, berth, GALE.normalize());
    client.say(ToServer::Helm {
        hull: Underway::lying(anchorage, 0.0),
        tender: None,
    });
    client.say(ToServer::Anchor);
    let (_, _, _, anchor) = client.hear_of(ship, Instant::now() + PATIENCE, "the anchor");
    let hook = anchor.expect("an anchor refused over the shelf");
    let cable = protocol::ground::ANCHOR_SWING;
    client.say(ToServer::Disembark {
        position: anchorage + Vec2::new(3.0, 0.0),
    });
    client.order("world weather gale");

    let lie = hook + GALE.normalize() * cable;
    let deadline = Instant::now() + PATIENCE;
    loop {
        let (hull, _, _, anchor) = client.hear_of(ship, deadline, "the ship swinging");
        assert_eq!(anchor, Some(hook), "the hook moved");
        assert!(
            hull.at.distance(hook) <= cable + 0.05,
            "the hull is {} m from a hook on {cable} m of cable",
            hull.at.distance(hook)
        );
        if hull.way.length() > 0.05 {
            assert!(
                hull.way.dot(lie - hull.at) > 0.0,
                "the hull is making {} away from where a {GALE} wind would lay it",
                hull.way
            );
            break;
        }
    }
}

#[test]
fn a_boat_taken_in_tow_comes_off_its_anchor() {
    // A rowing boat anchored from its thwarts, whose crew then climb up
    // onto the ship alongside: taken in tow, it goes where the ship goes,
    // so the hook comes up with the painter — told in the very telling that
    // ties it.
    let addr = host(7);
    let (client, id, _spawn, _token, aboard) = Client::join_aboard(addr, None);
    let ship = aboard.expect("a newcomer's story starts aboard");
    let (tender, _) = client.hear_the_tender_of(ship);
    // Over the shelf, where a hook can hold; the jump brings the boat on the
    // painter along, so it is alongside to be boarded.
    berthed_off_the_first_land(&client, 7);
    client.say(ToServer::Board { boat: tender });
    client.boat_changed_hands(tender, Some(id));
    client.say(ToServer::Anchor);
    let deadline = Instant::now() + PATIENCE;
    loop {
        let (_, _, _, anchor) = client.hear_of(tender, deadline, "the boat anchored");
        if anchor.is_some() {
            break;
        }
    }
    client.say(ToServer::Board { boat: ship });
    client.boat_changed_hands(ship, Some(id));
    let deadline = Instant::now() + PATIENCE;
    loop {
        let (_, occupant, towed_by, anchor) = client.hear_of(tender, deadline, "the tow tied");
        if occupant.is_none() {
            assert_eq!(towed_by, Some(ship), "the boat was not taken in tow");
            assert_eq!(anchor, None, "a boat under tow with its hook still down");
            break;
        }
    }
}

#[test]
fn a_sleeper_at_a_drifting_helm_is_carried_with_the_hull() {
    // Hang up at the helm over the open sea with no hook down, and the
    // ship drifts. Coming back, the helmsman is aboard it wherever it has
    // got to: the world carries a sleeper with their hull, so a boat the sea
    // moved is still the boat lying where they left it, as far as being
    // seated back into it goes.
    let addr = host(7);
    let (alice, a, spawn, token, aboard) = Client::join_aboard(addr, None);
    let ship = aboard.expect("a newcomer's story starts aboard");
    let (_, astern) = alice.hear_the_tender_of(ship);
    alice.order("world weather gale");
    let world = behind_the_curtain(7);
    let deep = open_sea_near(&world, spawn);
    alice.say(ToServer::Helm {
        hull: Underway::lying(deep, 0.0),
        tender: Some(Underway::lying(astern - spawn + deep, 0.0)),
    });
    alice.caught_up();
    // An onlooker, so the departure can be watched being filed rather than
    // raced — see `hanging_up_in_the_boat_puts_you_back_in_it`.
    let watcher = Client::join(addr).0;
    drop(alice);
    let deadline = Instant::now() + PATIENCE;
    while watcher.hear_by(deadline, "anybody leaving the world") != (ToClient::Left { id: a }) {}

    // Long enough for a gale to have plainly moved the hull.
    std::thread::sleep(Duration::from_secs(3));
    let (back, _id, put_down, _token, seated) = Client::join_aboard(addr, Some(token));
    assert_eq!(
        seated,
        Some(ship),
        "the sleeper was not seated back at their helm"
    );
    assert!(
        (put_down - deep).dot(GALE.normalize()) > 1.0,
        "the ship was put back at {put_down}, not carried off from {deep} by a {GALE} wind"
    );
    let (hull, occupant, _, _) = back.hear_of(ship, Instant::now() + PATIENCE, "the ship");
    assert_eq!(occupant, Some(_id));
    assert_eq!(
        hull.at, put_down,
        "the sleeper woke somewhere other than aboard"
    );
}

#[test]
fn a_kept_world_reopens_with_its_anchors_down() {
    // The hook is a fact about the hull and the file keeps it: a world
    // closed with a ship at anchor reopens with the ship still on it, and
    // so still where it was left rather than gone on the wind.
    let path = scratch("anchor").join("one.world");
    let first = Server::bind(("127.0.0.1", 0), 7)
        .expect("bind")
        .keeping_at(path.clone())
        .expect("keeping");
    let addr = first.local_addr().expect("addr");
    let host = first.spawn().expect("spawn");

    let (client, _id, _spawn, _token, aboard) = Client::join_aboard(addr, None);
    let ship = aboard.expect("aboard");
    berthed_off_the_first_land(&client, 7);
    client.say(ToServer::Anchor);
    let (_, _, _, anchor) = client.hear_of(ship, Instant::now() + PATIENCE, "the anchor");
    let hook = anchor.expect("an anchor refused over the shelf");
    drop(client);
    drop(host);

    let again = Server::reopen(("127.0.0.1", 0), &path).expect("reopen");
    let addr = again.local_addr().expect("addr");
    let _host = again.spawn().expect("spawn");
    let (carol, ..) = Client::join_aboard(addr, None);
    let (_, _, _, anchor) = carol.hear_of(ship, Instant::now() + PATIENCE, "the kept ship");
    assert_eq!(anchor, Some(hook), "the anchor was not kept");
}
