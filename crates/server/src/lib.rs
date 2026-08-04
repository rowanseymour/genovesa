//! The server: what makes a world *shared*.
//!
//! There is deliberately little of it. A seed is a world — the `world` crate
//! generates the same ocean, bit for bit, on every machine — so terrain never
//! crosses the wire, and the server's whole authority is the session: which
//! world this is, who is in it, and where they are. Clients generate the same
//! ocean for themselves and meet in it here.
//!
//! Concurrency is plain std threading: a thread per connection blocking on
//! its reads, a writer thread per connection draining a channel, and one lock
//! around the roster. A session's traffic is a trickle of tiny messages, so
//! nothing here needs to be clever — it needs to be obviously correct.

pub mod cli;

use std::collections::HashMap;
use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use glam::Vec2;
use protocol::{PlayerId, ToClient, ToServer, PROTOCOL_VERSION};
use world::archipelago::{Archipelago, WorldConfig};

/// A hosted world, listening for players.
pub struct Server {
    listener: TcpListener,
    shared: Arc<Shared>,
}

/// What every connection's thread shares.
struct Shared {
    config: WorldConfig,
    /// Where players enter the world: the centre of the island nearest the
    /// origin — the same rule a lone run uses to point its opening view, so
    /// joining a server lands on the very island a solo run of the seed
    /// would open on.
    spawn: Vec2,
    /// Dealt in joining order, and never reused within a session.
    next_id: AtomicU32,
    players: Mutex<HashMap<PlayerId, Player>>,
}

/// One connected player, as the roster sees them.
struct Player {
    position: Vec2,
    /// The way to this player's ear: their writer thread drains this onto
    /// their socket.
    outbox: mpsc::Sender<ToClient>,
}

impl Server {
    /// Binds the listener and settles everything a session hands out — the
    /// spawn point costs a few hash mixes of layout, no terrain.
    pub fn bind(addr: impl ToSocketAddrs, config: WorldConfig) -> io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        let spawn = Archipelago::new(&config)
            .nearest_island(Vec2::ZERO)
            .map(|spec| spec.centre())
            .unwrap_or(Vec2::ZERO);
        Ok(Self {
            listener,
            shared: Arc::new(Shared {
                config,
                spawn,
                next_id: AtomicU32::new(1),
                players: Mutex::new(HashMap::new()),
            }),
        })
    }

    /// The address actually bound — port 0 in, a real port out, which is how
    /// tests host on whatever the machine has free.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Serves forever: every connection gets a thread of its own, from
    /// handshake to hang-up.
    pub fn run(self) {
        for stream in self.listener.incoming() {
            // A connection that failed to arrive is its problem, not the
            // session's.
            let Ok(stream) = stream else { continue };
            let shared = self.shared.clone();
            thread::spawn(move || serve(stream, shared));
        }
    }
}

/// One connection, cradle to grave: handshake, introductions, relay,
/// departure.
fn serve(stream: TcpStream, shared: Arc<Shared>) {
    // Position reports are a dozen bytes that matter now or not at all;
    // batching them behind Nagle's algorithm would only add lag.
    let _ = stream.set_nodelay(true);
    let Ok(mut reader) = stream.try_clone() else {
        return;
    };

    // The first word must be a hello in the version this build speaks. A
    // refusal answers with the version it wanted, so the client can say
    // something more useful than "hung up".
    match ToServer::read(&mut reader) {
        Ok(ToServer::Hello { version }) if version == PROTOCOL_VERSION => {}
        Ok(ToServer::Hello { .. }) => {
            let _ = (ToClient::Refused {
                version: PROTOCOL_VERSION,
            })
            .write(&mut &stream);
            return;
        }
        _ => return,
    }
    let id = PlayerId(shared.next_id.fetch_add(1, Ordering::Relaxed));

    // The player's writer: everything the session wants them to hear goes
    // down the channel, and this thread puts it on the socket. It ends when
    // the channel closes — the roster dropping the player closes it — or
    // when a write fails because the player is gone.
    let (outbox, letters) = mpsc::channel::<ToClient>();
    let mut writer = stream;
    thread::spawn(move || {
        for message in letters {
            if message.write(&mut writer).is_err() {
                break;
            }
        }
    });

    let _ = outbox.send(ToClient::Welcome {
        id,
        seed: shared.config.seed,
        spawn: shared.spawn,
    });

    // Onto the roster under one hold of the lock: the newcomer hears exactly
    // who is already here, everyone else hears the newcomer, and no move can
    // slip between the two — a `Moved` about a player a client has not been
    // introduced to would be about nobody.
    {
        let mut players = shared.players.lock().expect("no poisoned lock");
        for (other, player) in players.iter() {
            let _ = outbox.send(ToClient::Joined {
                id: *other,
                position: player.position,
            });
        }
        players.insert(
            id,
            Player {
                position: shared.spawn,
                outbox,
            },
        );
        broadcast(
            &players,
            id,
            ToClient::Joined {
                id,
                position: shared.spawn,
            },
        );
    }
    println!("{id} joined");

    // Relay until the line drops. Anything else ends the session too: after
    // a framing error nothing later on the stream can be trusted, a second
    // hello is a client that has lost its place, and a non-finite position
    // would poison every machine that eased a marker towards it.
    loop {
        match ToServer::read(&mut reader) {
            Ok(ToServer::Move { position }) if position.is_finite() => {
                let mut players = shared.players.lock().expect("no poisoned lock");
                if let Some(player) = players.get_mut(&id) {
                    player.position = position;
                }
                broadcast(&players, id, ToClient::Moved { id, position });
            }
            _ => break,
        }
    }

    {
        let mut players = shared.players.lock().expect("no poisoned lock");
        players.remove(&id);
        broadcast(&players, id, ToClient::Left { id });
    }
    println!("{id} left");
}

/// Sends to everyone on the roster but `from`. A failed send means that
/// player's writer has already died, and their own thread is on its way to
/// taking them off the roster — nothing for the caller to do about it.
fn broadcast(players: &HashMap<PlayerId, Player>, from: PlayerId, message: ToClient) {
    for (id, player) in players {
        if *id != from {
            let _ = player.outbox.send(message);
        }
    }
}
