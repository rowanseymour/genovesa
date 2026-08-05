//! A server and its clients talking over real sockets: the handshake, the
//! introductions, the relay, and leaving.

use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use glam::Vec2;
use protocol::{PlayerId, ToClient, ToServer, PROTOCOL_VERSION};
use server::{Host, Server};
use world::archipelago::{Archipelago, WorldConfig};

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

    fn join(addr: SocketAddr) -> (Self, PlayerId, u32, Vec2) {
        let client = Self::connect(addr);
        client.say(ToServer::Hello {
            version: PROTOCOL_VERSION,
        });
        match client.hear() {
            ToClient::Welcome { id, seed, spawn } => (client, id, seed, spawn),
            other => panic!("expected a welcome, heard {other:?}"),
        }
    }

    fn say(&self, message: ToServer) {
        message.write(&mut &self.0).expect("write");
    }

    fn hear(&self) -> ToClient {
        ToClient::read(&mut &self.0).expect("read")
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
fn a_client_is_welcomed_with_the_world() {
    let addr = host(7);
    let (_client, _id, seed, spawn) = Client::join(addr);

    assert_eq!(seed, 7, "the welcome names a different world");
    // Players enter on the world's own spawn point — the open water off the
    // first island that a lone run of the seed opens on — scattered a few
    // boat-lengths so arrivals don't stack. What is pinned is that the
    // served spawn stays inside that scatter, and so on the same patch of
    // water every other machine computes for this seed.
    let entry = Archipelago::new(&WorldConfig { seed: 7 })
        .spawn()
        .expect("the seed offers somewhere to enter")
        .point;
    assert!(
        spawn.distance(entry) <= server::SPAWN_SCATTER,
        "{spawn} is not the patch of water players enter on"
    );
}

#[test]
fn two_players_are_never_put_down_in_the_same_spot() {
    // Otherwise the first thing a joined session shows is one marker where
    // there are two players.
    let addr = host(1);
    let (_alice, _a, _, first) = Client::join(addr);
    let (_bob, _b, _, second) = Client::join(addr);
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
    let (alice, a, _, alices_spawn) = Client::join(addr);
    let (bob, b, _, bobs_spawn) = Client::join(addr);
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
    let (carol, c, _, carols_spawn) = Client::join(addr);
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
    let (_client, _id, seed, _spawn) = Client::join(host.addr());
    assert_eq!(seed, 7);
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
    let (_client, _id, seed, _) = Client::join(again.addr());
    assert_eq!(seed, 2, "the second world is not the one being served");
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
