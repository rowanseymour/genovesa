//! A server and its clients talking over real sockets: the handshake, the
//! introductions, the relay, the ground, and leaving.

use std::collections::HashMap;
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use glam::{IVec2, Vec2};
use protocol::ground::{dequantize, CHUNK_METRES};
use protocol::survey::{in_sight, in_sight_along, Soundings, Survey, SIGHT_RADIUS};
use protocol::{
    BeastId, BeastKind, BoatKind, PlayerId, ToClient, ToServer, Token, PROTOCOL_VERSION,
};
use server::{Host, Server, WorldConfig};
use world::archipelago::{Archipelago, IslandSpec};

/// What a seed's world is, to a test that is allowed to know. A client never
/// gets one of these — that is the whole point of the arrangement — so these
/// are here to say what the server's answers *should* have been.
fn behind_the_curtain(seed: u32) -> Archipelago {
    Archipelago::new(&WorldConfig { seed })
}

/// The chunk a world point stands in.
fn chunk_at(point: Vec2) -> IVec2 {
    (point / CHUNK_METRES).floor().as_ivec2()
}

/// A hosted world on a loopback port of the machine's choosing, running until
/// the test process ends. Most of what is tested here is a conversation, not a
/// lifetime — the tests that are about the lifetime host their own.
fn host(seed: u32) -> SocketAddr {
    forever(Server::bind(("127.0.0.1", 0), WorldConfig { seed }).expect("bind"))
}

/// The same, opened at a chosen hour of its day, for the tests that are
/// about the night — which nothing else can reach, a world's clock running
/// at ten minutes to the day from whenever it was bound.
fn host_at(seed: u32, opening: f32) -> SocketAddr {
    forever(
        Server::bind(("127.0.0.1", 0), WorldConfig { seed })
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
    Server::bind(("127.0.0.1", 0), WorldConfig { seed })
        .expect("bind")
        .spawn()
        .expect("spawn")
}

/// A test client: a socket that speaks the protocol, with a read timeout so a
/// message that never comes fails the test instead of hanging it.
struct Client(TcpStream);

impl Client {
    fn connect(addr: SocketAddr) -> Self {
        let stream = TcpStream::connect(addr).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("set timeout");
        Self(stream)
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
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let ToClient::Boat {
                id,
                position,
                heading,
                occupant,
                ..
            } = ToClient::read(&mut &self.0).expect("read")
            {
                return (id, position, heading, occupant);
            }
            assert!(
                std::time::Instant::now() < deadline,
                "ten seconds and no word of any boat"
            );
        }
    }

    /// The next word about a boat, kind and all — for the tests about what a
    /// hull *is*, where [`Client::hear_a_boat`] only cares where it lies and
    /// whose it is.
    fn hear_a_boat_kinded(&self) -> (protocol::BoatId, BoatKind, Vec2, f32, Option<PlayerId>) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let ToClient::Boat {
                id,
                kind,
                position,
                heading,
                occupant,
            } = ToClient::read(&mut &self.0).expect("read")
            {
                return (id, kind, position, heading, occupant);
            }
            assert!(
                std::time::Instant::now() < deadline,
                "ten seconds and no word of any boat"
            );
        }
    }

    /// The next word that a boat has left the world, ignoring everything
    /// else — bounded like the boats' own reader.
    fn hear_a_boat_gone(&self) -> protocol::BoatId {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let ToClient::BoatGone { id } = ToClient::read(&mut &self.0).expect("read") {
                return id;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "ten seconds and no boat left the world"
            );
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
        loop {
            match ToClient::read(&mut &self.0).expect("read") {
                ToClient::Weather { .. }
                | ToClient::Daylight { .. }
                | ToClient::Beast { .. }
                | ToClient::BeastGone { .. }
                | ToClient::Boat { .. }
                | ToClient::Surveyed { .. }
                | ToClient::Cairn { .. }
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
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        if enough(&charted) {
            return charted;
        }
        loop {
            if let ToClient::Surveyed { found } = ToClient::read(&mut &self.0).expect("read") {
                charted.extend(found);
                if enough(&charted) {
                    return charted;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "twenty seconds and the survey never came to {} chunks",
                charted.len()
            );
        }
    }

    /// The next word about a cairn, ignoring everything else — bounded like
    /// the beasts' reader, the session chattering on regardless.
    fn hear_a_cairn(&self) -> (IVec2, Vec2, String, bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let ToClient::Cairn {
                island,
                at,
                name,
                yours,
            } = ToClient::read(&mut &self.0).expect("read")
            {
                return (island, at, name, yours);
            }
            assert!(
                std::time::Instant::now() < deadline,
                "ten seconds and no word of any cairn"
            );
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
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let told = self.hear_a_cairn();
            if wanted(&told.2) {
                return told;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "ten seconds and no cairn ever said the wanted thing"
            );
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
        loop {
            match ToClient::read(&mut &self.0).expect("read") {
                ToClient::Cairn { .. } => return false,
                ToClient::Reply { .. } => return true,
                _ => {}
            }
        }
    }

    /// The next word about a beast of the wanted kind, ignoring everything
    /// else — bounded, because the rest of the session chatters on
    /// regardless and a world that raises no such beast would otherwise keep
    /// this reading forever.
    fn hear_a_beast(&self, wanted: BeastKind) -> (BeastId, Vec2, Vec2) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let ToClient::Beast {
                id,
                kind,
                position,
                velocity,
                ..
            } = ToClient::read(&mut &self.0).expect("read")
            {
                if kind == wanted {
                    return (id, position, velocity);
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "ten seconds in these waters and no {wanted:?}"
            );
        }
    }

    /// The next answer to a console line, ignoring everything else — the
    /// session goes on introducing players and telling the sky around a
    /// command, and none of that is what a reply is about.
    fn hear_reply(&self) -> String {
        loop {
            if let ToClient::Reply { text } = ToClient::read(&mut &self.0).expect("read") {
                return text;
            }
        }
    }

    /// The next word on what time it is, ignoring everything else.
    fn hear_the_time(&self) -> f32 {
        loop {
            if let ToClient::Daylight { phase } = ToClient::read(&mut &self.0).expect("read") {
                return phase;
            }
        }
    }

    /// Reads until the line gives out, and says how. Whatever the session had
    /// already said is drained first: a client that has not been listening is
    /// still owed its messages, and the question here is only how the
    /// conversation ends.
    fn until_hung_up(&self) -> std::io::Error {
        loop {
            if let Err(error) = ToClient::read(&mut &self.0) {
                return error;
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
    // it is where the player was put down: players enter on the world's own
    // spawn point, the open water off its first island, scattered a few
    // boat-lengths so arrivals don't stack.
    let entry = behind_the_curtain(7)
        .spawn()
        .expect("the seed offers somewhere to enter");
    assert!(
        spawn.distance(entry.point) <= server::SPAWN_SCATTER,
        "{spawn} is not the patch of water players enter on"
    );

    // And the view opens on the island that water stands off, which the
    // client could not have worked out for itself.
    assert_eq!(facing, entry.island.centre());
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
    match ToClient::read(&mut &client.0).expect("read") {
        ToClient::World { .. } => {}
        other => panic!("expected to hear which world, heard {other:?}"),
    }
    client.say(ToServer::Papers { token: None });
    match ToClient::read(&mut &client.0).expect("read") {
        ToClient::Welcome { .. } => {}
        other => panic!("expected a welcome, heard {other:?}"),
    }
    match ToClient::read(&mut &client.0).expect("read") {
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
    match ToClient::read(&mut &client.0).expect("read") {
        ToClient::Daylight { phase } => assert!(
            (phase - server::OPENING).abs() < 0.01,
            "a world seconds old opened at {phase} rather than its morning"
        ),
        other => panic!("expected the time of day, heard {other:?}"),
    }
    // Then the console's words, before the client has asked anything —
    // completion is only worth having from the first line typed.
    match ToClient::read(&mut &client.0).expect("read") {
        ToClient::Vocabulary { verbs } => {
            assert!(
                verbs.iter().any(|verb| verb == "help"),
                "no `help` among {verbs:?}"
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
    assert!(ToClient::read(&mut &client.0).is_err(), "still connected");
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
    let entry = behind_the_curtain(7).spawn().expect("somewhere to enter");
    assert!(spawn.distance(entry.point) <= server::SPAWN_SCATTER);
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
fn a_port_can_be_hosted_again_once_the_host_is_dropped() {
    // Hosting, leaving, and hosting again is an ordinary evening: a new seed
    // from the menu is a new world on the same port. The listener has to be
    // properly gone by the time the drop returns — and so must the connections
    // it was serving, which is the harder half: a guest is still connected
    // when the host leaves, so the host is the end that closes first, and a
    // socket closed from this end is the one that lingers.
    let server = Server::bind(("127.0.0.1", 0), WorldConfig { seed: 1 }).expect("bind");
    let addr = server.local_addr().expect("addr");
    let host = server.spawn().expect("spawn");
    let (guest, _id, _, _) = Client::join(addr);
    drop(host);
    assert_eq!(
        guest.until_hung_up().kind(),
        std::io::ErrorKind::UnexpectedEof
    );

    let again = Server::bind(addr, WorldConfig { seed: 2 })
        .expect("the port is still held")
        .spawn()
        .expect("spawn");
    let (_client, _id, spawn, _) = Client::join(again.addr());
    let entry = behind_the_curtain(2).spawn().expect("somewhere to enter");
    assert!(
        spawn.distance(entry.point) <= server::SPAWN_SCATTER,
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

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut phase = client.hear_the_time();
    while protocol::is_night(phase) && std::time::Instant::now() < deadline {
        client.say(ToServer::WantDawn);
        phase = client.hear_the_time();
    }
    assert!(
        !protocol::is_night(phase),
        "the night never ran off: still {phase}"
    );
    // Landed on daybreak, not somewhere past it: a fast clock that overshot
    // would take the sunrise with it.
    assert!(
        phase < protocol::DAYBREAK + 0.02,
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

    // Asked for long enough that the night is plainly running — a second of
    // one moves a tenth of a day, where the day itself moves a
    // six-hundredth.
    let opened = client.hear_the_time();
    let asking = std::time::Instant::now() + Duration::from_millis(600);
    let mut phase = opened;
    while std::time::Instant::now() < asking {
        client.say(ToServer::WantDawn);
        phase = client.hear_the_time();
    }
    assert!(
        (phase - opened).rem_euclid(1.0) > 0.02,
        "the night never started running: {opened} then {phase}"
    );

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
    let entry = behind_the_curtain(7).spawn().expect("somewhere to enter");
    let (_client, _id, spawn, _facing, dealt) = Client::join_presenting(addr, Some(Token(12_345)));
    assert_ne!(dealt, Token(12_345), "a token nobody dealt was believed");
    assert!(
        spawn.distance(entry.point) <= server::SPAWN_SCATTER,
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
    // goes anywhere. One connection read in order means the sailing is
    // processed before the hang-up that follows it.
    let out = Vec2::new(640.0, -320.0);
    bob.say(ToServer::Helm {
        position: out,
        heading: 0.0,
    });
    drop(bob);
    // Alice hearing the departure is what guarantees it is filed: the world
    // remembers a leaver before anyone is told they left.
    assert_eq!(alice.hear(), ToClient::Left { id: b });

    let (_bob, _id, spawn, facing, dealt) = Client::join_presenting(addr, Some(bobs_token));
    assert_eq!(spawn, out, "Bob was not put back where the world saw him");
    assert_eq!(
        facing,
        out + Vec2::new(0.0, -64.0),
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
fn a_kept_world_reopens_where_it_left_off() {
    // The whole story: a world kept to a file, sailed, left, reopened — and
    // it is the same world, at the same hour, with the player where the
    // world last saw them.
    let path = scratch("kept").join("one.world");
    let first = Server::bind(("127.0.0.1", 0), WorldConfig { seed: 7 })
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
        position: out,
        heading: 0.0,
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

    // ...same player, back where the world last saw them...
    let (client, _id, spawn, _facing, dealt) = Client::join_presenting(addr, Some(token));
    assert_eq!(spawn, out, "the world forgot where it last saw its player");
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
fn a_newcomer_enters_at_the_helm_of_a_minted_boat() {
    let addr = host(7);
    let (client, id, spawn, _token, aboard) = Client::join_aboard(addr, None);
    let boat = aboard.expect("a newcomer's story starts aboard");

    // The hull is introduced to its own keeper like any other: standing
    // where they stand, with their hands on the helm.
    let (told, at, _heading, occupant) = client.hear_a_boat();
    assert_eq!(told, boat, "introduced to somebody else's boat first");
    assert_eq!(at, spawn, "the minted hull is not under its keeper");
    assert_eq!(occupant, Some(id), "the newcomer is not at their own helm");
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
    // nobody's — and then leaves the world entirely.
    let far = alices_spawn + Vec2::new(600.0, 0.0);
    alice.say(ToServer::Helm {
        position: far,
        heading: 1.0,
    });
    alice.say(ToServer::Disembark {
        position: far + Vec2::new(2.0, 0.0),
    });
    drop(alice);

    // Bob hears the helm empty out where she left it.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let at = loop {
        let (told, at, _h, occupant) = bob.hear_a_boat();
        if told == a_boat && occupant.is_none() {
            break at;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the abandoned helm was never told empty"
        );
    };
    assert_eq!(at, far, "the boat did not stay where its keeper left it");

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

    // ...and asking from alongside is granted: boats have keepers, not
    // owners, and Alice's is Bob's now.
    bob.say(ToServer::Move {
        position: far + Vec2::new(1.0, 0.0),
    });
    bob.say(ToServer::Board { boat: a_boat });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let (told, _, _, occupant) = bob.hear_a_boat();
        if told == a_boat && occupant == Some(b) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the helm alongside was never granted"
        );
    }
    let _ = a; // Alice's id has no further part; the boat outlived her visit.
}

#[test]
fn a_tender_is_lowered_alongside_and_the_ship_left_at_anchor() {
    let addr = host(7);
    let (client, id, spawn, _token, aboard) = Client::join_aboard(addr, None);
    let ship = aboard.expect("a newcomer's story starts aboard");
    // The introduction of their own hull, out of the way first.
    let (told, ..) = client.hear_a_boat_kinded();
    assert_eq!(told, ship);

    // Lowered a few metres abeam, pointed however the client liked.
    let alongside = spawn + Vec2::new(3.0, 0.0);
    client.say(ToServer::Lower {
        position: alongside,
        heading: 0.75,
    });

    // The rowing boat first — the telling that seats the asker...
    let (tender, kind, at, heading, occupant) = client.hear_a_boat_kinded();
    assert_ne!(tender, ship, "the ship was dealt again as its own tender");
    assert_eq!(kind, BoatKind::Rowboat);
    assert_eq!(
        at, alongside,
        "the tender was not lowered where it was asked"
    );
    assert_eq!(heading, 0.75);
    assert_eq!(occupant, Some(id), "the asker was not seated in the tender");

    // ...and then the ship, left at anchor for anyone, where it lay.
    let (told, kind, at, _heading, occupant) = client.hear_a_boat_kinded();
    assert_eq!(told, ship);
    assert_eq!(kind, BoatKind::Sloop);
    assert_eq!(at, spawn, "lowering the tender moved the ship");
    assert_eq!(occupant, None, "the ship was not left at anchor");
}

#[test]
fn boarding_the_ship_from_the_tender_hoists_it_back_aboard() {
    let addr = host(7);
    let (client, id, spawn, _token, aboard) = Client::join_aboard(addr, None);
    let ship = aboard.expect("a newcomer's story starts aboard");
    let (told, ..) = client.hear_a_boat_kinded();
    assert_eq!(told, ship);

    let alongside = spawn + Vec2::new(3.0, 0.0);
    client.say(ToServer::Lower {
        position: alongside,
        heading: 0.0,
    });
    let (tender, ..) = client.hear_a_boat_kinded();
    let _ship_at_anchor = client.hear_a_boat_kinded();

    // Rowed off and back — the tender is a boat like any other under way.
    client.say(ToServer::Helm {
        position: spawn + Vec2::new(40.0, 0.0),
        heading: 0.5,
    });
    client.say(ToServer::Helm {
        position: alongside,
        heading: 0.5,
    });

    // Laid alongside again, the ship's helm is granted — and the tender goes
    // back aboard with the boarding, told to everyone as gone, after the
    // telling that seats its crew.
    client.say(ToServer::Board { boat: ship });
    let (told, _kind, at, _heading, occupant) = client.hear_a_boat_kinded();
    assert_eq!(told, ship);
    assert_eq!(
        occupant,
        Some(id),
        "the helm was not granted from alongside"
    );
    assert_eq!(at, spawn, "boarding moved the ship");
    assert_eq!(
        client.hear_a_boat_gone(),
        tender,
        "some other hull was hoisted in"
    );

    // Retired means retired: asking after the hoisted hull is asking after
    // a boat this world never made, which ends the session.
    client.say(ToServer::Board { boat: tender });
    client.until_hung_up();
}

#[test]
fn a_ships_helm_is_taken_from_a_tender_and_never_from_another_deck() {
    let addr = host(1);
    let (alice, a, _alices_spawn, _t, a_boat) = Client::join_aboard(addr, None);
    let a_boat = a_boat.expect("aboard");
    let (bob, b, bobs_spawn, _t2, b_boat) = Client::join_aboard(addr, None);
    let b_boat = b_boat.expect("aboard");

    // Bob steps ashore, leaving his sloop free where he entered.
    bob.say(ToServer::Disembark {
        position: bobs_spawn,
    });

    // Alice's inbox so far, in order: her own hull's introduction, Bob's
    // arriving, and Bob's emptying out.
    let (told, ..) = alice.hear_a_boat_kinded();
    assert_eq!(told, a_boat);
    let (told, _k, _at, _h, occupant) = alice.hear_a_boat_kinded();
    assert_eq!((told, occupant), (b_boat, Some(b)));
    let (told, _k, _at, _h, occupant) = alice.hear_a_boat_kinded();
    assert_eq!((told, occupant), (b_boat, None));

    // Alice lays her ship alongside Bob's and asks for its helm from her own
    // deck: refused — ships do not board ships — and the answer is the
    // boat's state, unchanged.
    let alongside = bobs_spawn + Vec2::new(3.0, 0.0);
    alice.say(ToServer::Helm {
        position: alongside,
        heading: 0.0,
    });
    alice.say(ToServer::Board { boat: b_boat });
    let (told, _k, _at, _h, occupant) = alice.hear_a_boat_kinded();
    assert_eq!(told, b_boat, "a boat other than the asked-for one answered");
    assert_eq!(
        occupant, None,
        "a helm was granted from another ship's deck"
    );

    // From the thwarts of her tender the same ask is granted — boats have
    // keepers, not owners, so the tender goes aboard a hull that never
    // lowered it.
    alice.say(ToServer::Lower {
        position: bobs_spawn + Vec2::new(5.0, 0.0),
        heading: 0.0,
    });
    let (tender, ..) = alice.hear_a_boat_kinded();
    let _her_ship_at_anchor = alice.hear_a_boat_kinded();
    alice.say(ToServer::Board { boat: b_boat });
    let (told, _k, _at, _h, occupant) = alice.hear_a_boat_kinded();
    assert_eq!((told, occupant), (b_boat, Some(a)));
    assert_eq!(alice.hear_a_boat_gone(), tender);
}

#[test]
fn a_returning_keeper_is_seated_back_at_their_helm() {
    // The single-player story: stop the world at a helm somewhere, reopen
    // it, and be exactly there, aboard exactly that boat.
    let path = scratch("helm").join("one.world");
    let first = Server::bind(("127.0.0.1", 0), WorldConfig { seed: 7 })
        .expect("bind")
        .keeping_at(path.clone())
        .expect("keeping");
    let addr = first.local_addr().expect("addr");
    let host = first.spawn().expect("spawn");

    let (client, _id, spawn, token, aboard) = Client::join_aboard(addr, None);
    let boat = aboard.expect("aboard");
    let out = spawn + Vec2::new(900.0, -250.0);
    client.say(ToServer::Helm {
        position: out,
        heading: 2.0,
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
    assert_eq!(spawn, out, "the boat moved while the world was stopped");
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
        position: far,
        heading: 1.0,
    });
    let _ = alice.ask_for(IVec2::new(5_000, 5_000));
    drop(alice);

    let (bob, b, ..) = Client::join_aboard(addr, None);
    bob.say(ToServer::Disembark {
        position: alices_spawn,
    });
    bob.say(ToServer::Move {
        position: far + Vec2::new(1.0, 0.0),
    });
    bob.say(ToServer::Board { boat: a_boat });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let (told, _, _, occupant) = bob.hear_a_boat();
        if told == a_boat && occupant == Some(b) {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "the take never took");
    }

    let (_alice, _id, spawn, _t, aboard) = Client::join_aboard(addr, Some(alices_token));
    assert_eq!(spawn, far, "Alice did not return where she was");
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
        position: far,
        heading: 1.0,
    });
    drop(alice);
    // Bob was introduced to her on arriving; her session read the helm
    // report before it read the end of her line, so hearing the leaving
    // after that is hearing both.
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
        position: moored,
        heading: 2.0,
    });
    bob.say(ToServer::Disembark {
        position: moored + Vec2::new(2.0, 0.0),
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let (told, at, _h, occupant) = bob.hear_a_boat();
        if told == a_boat && occupant.is_none() && at == moored {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the boat was never left free somewhere else"
        );
    }
    let _ = b;

    let (_alice, _id, spawn, _t, aboard) = Client::join_aboard(addr, Some(alices_token));
    assert_eq!(spawn, far, "Alice did not return where she was");
    assert_ne!(
        aboard,
        Some(a_boat),
        "Alice was dragged to where her old boat had got to"
    );
    assert!(aboard.is_some(), "Alice was left standing on open water");
}

#[test]
fn a_hull_nobody_ever_touched_is_handed_to_the_next_arrival() {
    // What bounds the fleet against a client that joins and hangs up in a
    // loop: the boat minted for an arrival who did nothing with it is the
    // boat the next arrival is handed, rather than another being minted.
    let addr = host(1);
    // A watcher, so the leaving can be *heard* to have been dealt with
    // before the next arrival knocks — a hull is only spare once its keeper
    // is off the roster. Their own hull is occupied throughout, so it is
    // never the one handed on.
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
    assert_eq!(
        second,
        Some(first),
        "the world minted a second hull rather than handing on the untouched one"
    );
}

#[test]
fn a_hull_somebody_stepped_off_is_not_handed_to_the_next_arrival() {
    // A boat pulled up on a beach by a player who is still in the world is
    // theirs to come back to, whether or not they ever sailed it. Handing it
    // to a newcomer because no helm report had moved it would take it out
    // from under somebody standing beside it.
    let addr = host(1);
    let (alice, _a, alices_spawn, _t, parked) = Client::join_aboard(addr, None);
    let parked = parked.expect("aboard");
    alice.say(ToServer::Disembark {
        position: alices_spawn,
    });
    // Heard back before the next arrival knocks: the step ashore reaches
    // everyone, the one who took it included.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let (told, _at, _h, occupant) = alice.hear_a_boat();
        if told == parked && occupant.is_none() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the step ashore was never told"
        );
    }

    let (_bob, _b, _spawn, _t, bobs) = Client::join_aboard(addr, None);
    assert_ne!(
        bobs,
        Some(parked),
        "a newcomer was handed the boat somebody had parked and walked away from"
    );
}

#[test]
fn a_kept_world_reopens_with_its_beasts_where_they_were() {
    // The shark scenario, by proxy of a whale: an animal alive when a world
    // closes is in its file, and reopening finds it where it stood — a beast
    // with consequence cannot be escaped by relogging, any more than a gale
    // can.
    let path = scratch("beasts").join("one.world");
    let first = Server::bind(("127.0.0.1", 0), WorldConfig { seed: 7 })
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
    // Being told of it means it is in the flock — and the ledger a save
    // reads is rewritten before each beat's tellings go out, so by now the
    // whale is in it.
    let (_id, seen_at, _velocity) = client.hear_a_beast(BeastKind::Whale);
    drop(client);
    drop(host);

    let again = Server::reopen(("127.0.0.1", 0), &path).expect("reopen");
    let addr = again.local_addr().expect("addr");
    let _host = again.spawn().expect("spawn");
    let (client, ..) = Client::join_presenting(addr, Some(token));

    // The same whale, near where it was — it wanders at a whale's own pace,
    // and no world time passed while the world was closed, so the slack
    // only covers the few real seconds either side of the reopening.
    let (_id, still_at, _velocity) = client.hear_a_beast(BeastKind::Whale);
    assert!(
        still_at.distance(seen_at) < 300.0,
        "the whale was at {seen_at} and reopened at {still_at}"
    );
}

#[test]
fn a_kept_world_cannot_be_hosted_twice_at_once() {
    // Two processes writing one file would be two histories under one name;
    // the world's lock makes the second host an error instead.
    let path = scratch("locked").join("one.world");
    let holding = Server::bind(("127.0.0.1", 0), WorldConfig { seed: 1 })
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
    // and arriving is already having looked. Exactly the ground within sight
    // of the spawn, and no more — the far side of the island they were put
    // down beside is theirs to go round for.
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
    // And there is a coast in it: the world is entered a few dozen metres off
    // one, so a survey of the spawn that found nothing found nothing wrong.
    assert!(
        charted.values().any(|ink| !ink.coast.is_empty()),
        "the water off an island surveyed to no coast at all"
    );
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
    let (client, _id, spawn, _facing) = Client::join(addr);
    let charted = client.hear_the_survey(HashMap::new(), |charted| {
        charted.len() >= in_sight_of(spawn).len()
            && charted.values().any(|ink| !ink.coast.is_empty())
    });

    let (chunk, told) = charted
        .iter()
        .find(|(_, ink)| !ink.coast.is_empty())
        .expect("some coast within sight of the spawn");
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
        position: out,
        heading: 0.0,
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
        position: out,
        heading: 0.0,
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
    let first = Server::bind(("127.0.0.1", 0), WorldConfig { seed: 7 })
        .expect("bind")
        .keeping_at(path.clone())
        .expect("keeping");
    let addr = first.local_addr().expect("addr");
    let host = first.spawn().expect("spawn");

    let (client, _id, spawn, _facing, token) = Client::join_presenting(addr, None);
    let out = spawn + Vec2::new(768.0, 0.0);
    client.say(ToServer::Helm {
        position: out,
        heading: 0.0,
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
        position: claimed,
        heading: 0.0,
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
    let first = Server::bind(("127.0.0.1", 0), WorldConfig { seed: 7 })
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
        position: brink,
        heading: 0.0,
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
    let first = Server::bind(("127.0.0.1", 0), WorldConfig { seed: 7 })
        .expect("bind")
        .keeping_at(path.clone())
        .expect("keeping");
    let addr = first.local_addr().expect("addr");
    let host = first.spawn().expect("spawn");

    let (client, _id, spawn, _facing, token) = Client::join_presenting(addr, None);
    let out = spawn + Vec2::new(1_536.0, 0.0);
    client.say(ToServer::Helm {
        position: out,
        heading: 0.0,
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
            position: at,
            heading: 0.0,
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
        position: to,
        heading: 0.0,
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
    let island = chart_of(&charted)
        .island_under(ashore)
        .expect("a coast sailed right round closes an island");
    (client, token, island.id, ashore)
}

#[test]
fn an_island_sailed_round_and_stood_upon_is_claimed() {
    // The whole rule in one voyage. Going round it is half of it, and the
    // half a client can see for itself; standing on it is the other, a cairn
    // being built by somebody on the ground rather than sailing past.
    let addr = host(CLAIMABLE);
    let (client, _token, island, ashore) = sail_round_the_island(addr, None);

    client.say(ToServer::Claim { island });
    assert!(
        client.nothing_was_said_about_a_cairn(),
        "a cairn was raised by somebody who never left the helm"
    );

    client.say(ToServer::Disembark { position: ashore });
    client.say(ToServer::Claim { island });
    let (told, at, name, yours) = client.hear_a_cairn();
    assert_eq!(told, island, "a cairn for some other island");
    assert_eq!(
        at, ashore,
        "the cairn does not stand where the claimant did"
    );
    assert!(yours, "the claimant was not told the cairn was theirs");
    assert_eq!(name, "", "an island nobody has christened came named");
}

#[test]
fn part_of_a_coast_earns_nothing_even_from_the_beach() {
    // The claim is settled against the coast the world has watched somebody
    // go round, not against where they are standing: half a survey rings
    // nothing, and there is nothing to be standing inside of.
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
        chart_of(&charted).island_under(landfall).is_none(),
        "a quarter of a coast closed an island"
    );

    bob.say(ToServer::Disembark { position: landfall });
    bob.say(ToServer::Claim { island });
    assert!(
        bob.nothing_was_said_about_a_cairn(),
        "a coast nobody had been round was claimed from the beach"
    );

    // And the island was there to be had all along, which is what says the
    // refusal was about Bob's voyage rather than about this island.
    alice.say(ToServer::Disembark { position: ashore });
    alice.say(ToServer::Claim { island });
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
    alice.say(ToServer::Claim { island });
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
    bob.say(ToServer::Claim { island });
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
    alice.say(ToServer::Claim { island });
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
    // Presence-gated knowledge, in one voyage. A cairn is a daymark on a
    // twenty-metre staff and says *somebody is here* from a mile off; the
    // name is lettering, and lettering is read by walking up to it.
    let addr = host(CLAIMABLE);
    let (alice, _token, island, ashore) = sail_round_the_island(addr, None);
    alice.say(ToServer::Disembark { position: ashore });
    alice.say(ToServer::Claim { island });
    let _ = alice.hear_a_cairn();
    alice.say(ToServer::Name {
        island,
        name: "Ilha Verde".to_string(),
    });
    assert_eq!(alice.hear_a_cairn().2, "Ilha Verde");

    // Bob stands off it — inside the reach a staff and banner carry, well
    // outside the reach a word does. Seaward of the cairn rather than at some
    // bearing of its own, so that the way he sails to get there runs away from
    // the stones and cannot brush past them.
    let world = behind_the_curtain(CLAIMABLE);
    let (centre, _across, _ashore) = an_island_to_sail_round(&world, ashore);
    let seaward = (ashore - centre).normalize_or(Vec2::X);
    let standing_off = ashore + seaward * ((server::CAIRN_SIGHT + server::CAIRN_VISIT) / 2.0);
    let (bob, _id, spawn, _facing) = Client::join(addr);
    sail_to(&bob, spawn, standing_off);

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
    alice.say(ToServer::Claim { island });
    let (told, _at, _name, yours) = alice.hear_a_cairn();
    assert_eq!((told, yours), (island, true));

    // A few paces about it, at every remove that could earn anybody anything:
    // right beside it, a stone's throw off, and out where only the banner
    // would show.
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
    let first = Server::bind(("127.0.0.1", 0), WorldConfig { seed: CLAIMABLE })
        .expect("bind")
        .keeping_at(path.clone())
        .expect("keeping");
    let addr = first.local_addr().expect("addr");
    let host = first.spawn().expect("spawn");

    let (alice, _token, island, ashore) = sail_round_the_island(addr, None);
    alice.say(ToServer::Disembark { position: ashore });
    alice.say(ToServer::Claim { island });
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
    alice.say(ToServer::Claim { island });
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
    client.say(ToServer::Claim { island });
    client.say(ToServer::Claim { island });
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
    client.say(ToServer::Claim { island });
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
    client.say(ToServer::Claim { island });
    let _ = client.hear_a_cairn();
    client.say(ToServer::Name {
        island,
        name: "Ilha Verde".to_string(),
    });
    assert_eq!(client.hear_a_cairn().2, "Ilha Verde");

    // Away on foot, four kilometres of it — well past `CAIRN_SIGHT` — and
    // then the line drops. One connection is read in order, so the walk is
    // filed before the hang-up that follows it.
    let away = ashore + Vec2::new(4_096.0, 0.0);
    client.say(ToServer::Move { position: away });
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
    let first = Server::bind(("127.0.0.1", 0), WorldConfig { seed: CLAIMABLE })
        .expect("bind")
        .keeping_at(path.clone())
        .expect("keeping");
    let addr = first.local_addr().expect("addr");
    let host = first.spawn().expect("spawn");

    let (client, token, island, ashore) = sail_round_the_island(addr, None);
    client.say(ToServer::Disembark { position: ashore });
    client.say(ToServer::Claim { island });
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
        position: shallows,
        heading: 0.0,
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
        position: shallows,
        heading: 0.0,
    });
    let (shark, _, _) = client.hear_a_beast(BeastKind::Shark);

    // Sail straight out to sea, away from the island, further than any
    // shark is minded.
    let away = (spawn - facing).normalize_or(Vec2::X);
    client.say(ToServer::Helm {
        position: shallows + away * 2_000.0,
        heading: 0.0,
    });

    // The word comes that the sea is emptier by one — the shark left behind,
    // not killed, and no longer anybody's business to hear about. Not at
    // once, though: leaving an animal's waters is given a few seconds to turn
    // out to have been a tack rather than a departure.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        match ToClient::read(&mut &client.0).expect("read") {
            ToClient::BeastGone { id } if id == shark => break,
            _ => assert!(
                std::time::Instant::now() < deadline,
                "the shark was never let go"
            ),
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

    // A summons from the shallows, where there is shark water to answer it.
    let world = behind_the_curtain(7);
    let shallows =
        shallows_between(&world, spawn, facing).expect("the entry island should have a coast");
    asker.say(ToServer::Helm {
        position: shallows,
        heading: 0.0,
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
    // evening from noon on its own in under a minute.
    asker.say(ToServer::Command {
        line: "time 18:00".to_string(),
    });
    assert_eq!(asker.hear_reply(), "the day has run on to 18:00");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let heard = bystander.hear_the_time();
        if (heard - 0.75).abs() < 0.01 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the bystander's day stands at {heard}, not the evening the console ordered"
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
        position: shallows,
        heading: 0.0,
    });

    let mut kinds = std::collections::HashSet::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while kinds != [BeastKind::Shark, BeastKind::Dolphins, BeastKind::Whale].into() {
        if let ToClient::Beast { kind, .. } = ToClient::read(&mut &client.0).expect("read") {
            kinds.insert(kind);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "fifteen seconds at anchor and the sea only offered {kinds:?}"
        );
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
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while pods.len() < CROWD {
        if let ToClient::Beast { id, kind, .. } = ToClient::read(&mut &client.0).expect("read") {
            if kind == BeastKind::Dolphins {
                pods.insert(id);
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "fifteen seconds after summoning {CROWD} pods, {} had been told of",
            pods.len()
        );
    }
}
