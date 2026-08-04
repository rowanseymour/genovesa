//! A server and its clients talking over real sockets: the handshake, the
//! introductions, the relay, and leaving.

use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use glam::Vec2;
use protocol::{PlayerId, ToClient, ToServer, PROTOCOL_VERSION};
use server::Server;
use world::archipelago::{Archipelago, WorldConfig};

/// A hosted world on a loopback port of the machine's choosing. The serving
/// thread runs until the test process ends — a listener has no way to be told
/// the session is over, and doesn't need one here.
fn host(seed: u32) -> SocketAddr {
    let server = Server::bind(("127.0.0.1", 0), WorldConfig { seed }).expect("bind");
    let addr = server
        .local_addr()
        .expect("a bound listener has an address");
    std::thread::spawn(move || server.run());
    addr
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
}

#[test]
fn a_client_is_welcomed_with_the_world() {
    let addr = host(7);
    let (_client, _id, seed, spawn) = Client::join(addr);

    assert_eq!(seed, 7, "the welcome names a different world");
    // The spawn is the island a lone run of the seed would open on — layout
    // only, so the expectation is cheap to recompute here.
    let expected = Archipelago::new(&WorldConfig { seed: 7 })
        .nearest_island(Vec2::ZERO)
        .expect("seed 7 has land near the origin")
        .centre();
    assert_eq!(spawn, expected, "players would enter in the wrong place");
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
    let (alice, a, _, spawn) = Client::join(addr);
    let (bob, b, _, _) = Client::join(addr);
    assert_ne!(a, b, "two players were dealt one id");

    // Introductions both ways: the newcomer hears who was already here, and
    // whoever is here hears the newcomer.
    assert_eq!(
        bob.hear(),
        ToClient::Joined {
            id: a,
            position: spawn
        }
    );
    assert_eq!(
        alice.hear(),
        ToClient::Joined {
            id: b,
            position: spawn
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
    let (carol, c, _, _) = Client::join(addr);
    assert_eq!(
        alice.hear(),
        ToClient::Joined {
            id: c,
            position: spawn
        }
    );
    assert_eq!(
        bob.hear(),
        ToClient::Joined {
            id: c,
            position: spawn
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
                position: spawn
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
