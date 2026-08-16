//! A server and its clients talking over real sockets: the handshake, the
//! introductions, the relay, the ground, and leaving.

use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use glam::{IVec2, Vec2};
use protocol::ground::{dequantize, CHUNK_METRES};
use protocol::{BeastId, BeastKind, PlayerId, ToClient, ToServer, Token, PROTOCOL_VERSION};
use server::{Host, Server, WorldConfig};
use world::archipelago::Archipelago;

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
    let server = Server::bind(("127.0.0.1", 0), WorldConfig { seed }).expect("bind");
    let addr = server
        .local_addr()
        .expect("a bound listener has an address");
    std::thread::spawn(move || server.run());
    addr
}

/// The same, opened at a chosen hour of its day, for the tests that are
/// about the night — which nothing else can reach, a world's clock running
/// at ten minutes to the day from whenever it was bound.
fn host_at(seed: u32, opening: f32) -> SocketAddr {
    let server = Server::bind(("127.0.0.1", 0), WorldConfig { seed })
        .expect("bind")
        .opening_at(opening);
    let addr = server
        .local_addr()
        .expect("a bound listener has an address");
    std::thread::spawn(move || server.run());
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
            } => (client, id, spawn, facing, token),
            other => panic!("expected a welcome, heard {other:?}"),
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
    /// chatter; the newcomer test reads it raw.
    fn hear(&self) -> ToClient {
        loop {
            match ToClient::read(&mut &self.0).expect("read") {
                ToClient::Weather { .. }
                | ToClient::Daylight { .. }
                | ToClient::Beast { .. }
                | ToClient::BeastGone { .. }
                | ToClient::Vocabulary { .. } => continue,
                message => return message,
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
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir()
        .join("genovesa-session-tests")
        .join(format!("{name}-{}", std::process::id()));
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

    let out = Vec2::new(640.0, -320.0);
    bob.say(ToServer::Move { position: out });
    let _ = alice.hear(); // the move
    drop(bob);
    // Alice hearing the departure is what guarantees it is filed: the world
    // remembers a leaver before anyone is told they left.
    assert_eq!(alice.hear(), ToClient::Left { id: b });

    let (_bob, _id, spawn, facing, dealt) = Client::join_presenting(addr, Some(bobs_token));
    assert_eq!(spawn, out, "Bob was not put back where the world saw him");
    assert_eq!(
        facing, out,
        "a resumed player's facing should name no direction"
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
    client.say(ToServer::Move { position: out });
    // An answered chunk is proof the move was processed: one connection,
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
    client.say(ToServer::Move { position: shallows });

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
    client.say(ToServer::Move { position: shallows });
    let (shark, _, _) = client.hear_a_beast(BeastKind::Shark);

    // Sail straight out to sea, away from the island, further than any
    // shark is minded.
    let away = (spawn - facing).normalize_or(Vec2::X);
    client.say(ToServer::Move {
        position: shallows + away * 2_000.0,
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
    asker.say(ToServer::Move { position: shallows });
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
    client.say(ToServer::Move { position: shallows });

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
