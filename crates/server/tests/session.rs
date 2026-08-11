//! A server and its clients talking over real sockets: the handshake, the
//! introductions, the relay, the ground, and leaving.

use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use glam::{IVec2, Vec2};
use protocol::ground::{dequantize, CHUNK_METRES};
use protocol::{PlayerId, ToClient, ToServer, PROTOCOL_VERSION};
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
        let client = Self::connect(addr);
        client.say(ToServer::Hello {
            version: PROTOCOL_VERSION,
        });
        match client.hear() {
            ToClient::Welcome { id, spawn, facing } => (client, id, spawn, facing),
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
    /// the weather and the time of day: each is sent on joining and again on
    /// a clock nothing in a test controls, so any assertion about message
    /// order would be flaky against them. The tests that *are* about the sky
    /// read for what they want with [`Client::hear_the_time`].
    fn hear(&self) -> ToClient {
        loop {
            match ToClient::read(&mut &self.0).expect("read") {
                ToClient::Weather { .. } | ToClient::Daylight { .. } => continue,
                message => return message,
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
