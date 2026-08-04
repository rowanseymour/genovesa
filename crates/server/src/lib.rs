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
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use glam::Vec2;
use protocol::{PlayerId, ToClient, ToServer, PROTOCOL_VERSION};
use world::archipelago::{Archipelago, WorldConfig};

/// How long a fresh connection has to say hello. Generous for a slow link,
/// and the point is only that a connection which arrives and then says
/// nothing — a port scan, a client that died mid-dial — cannot hold a thread
/// for the life of the process. The session that follows has no deadline:
/// going quiet is what a player standing still sounds like.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// How many messages may be waiting for one player before the session gives
/// up on them. A session's traffic is a trickle — a few tiny positions a
/// second per player — so a queue this deep does not mean briefly behind, it
/// means a client that has stopped reading its socket altogether. Bounded
/// because the alternative is a queue that grows for as long as such a client
/// stays connected.
const OUTBOX_DEPTH: usize = 256;

/// How far from the island's centre an arriving player may be put down, in
/// metres. A few strides: enough that two markers are plainly two markers,
/// small enough that everyone still arrives on the same beach.
const SPAWN_SCATTER: f32 = 12.0;

/// How far from the origin a reported position may be, in metres. The world
/// crate measures where `f32` ground stops being exactly the ground at about
/// 1,280 km out (see its "How endless is endless"), and no honest client gets
/// near that — a player panning at the camera's own speed spends something
/// over a year of continuous play reaching it. A position past it is a broken
/// or hostile client, and believing one would walk every other machine's
/// marker of that player out to where the world no longer resolves.
const MAX_RANGE: f32 = 1_280_000.0;

/// Where session news goes. Boxed rather than a type parameter so that a
/// `Server` is one type however it reports, and defaulted to silence: what a
/// library does to somebody's stdout is not the library's decision.
type Report = Box<dyn Fn(&str) + Send + Sync>;

/// A hosted world, listening for players.
pub struct Server {
    listener: TcpListener,
    shared: Arc<Shared>,
}

/// What every connection's thread shares.
struct Shared {
    config: WorldConfig,
    /// The island players enter on: the one nearest the origin — the same rule
    /// a lone run uses to point its opening view, so joining a server lands on
    /// the very island a solo run of the seed would open on. Where on it each
    /// player lands is [`Shared::spawn_for`].
    centre: Vec2,
    /// Dealt in joining order, and never reused within a session.
    next_id: AtomicU32,
    players: Mutex<HashMap<PlayerId, Player>>,
    report: Report,
}

/// One connected player, as the roster sees them.
struct Player {
    position: Vec2,
    /// The way to this player's ear: their writer thread drains this onto
    /// their socket.
    outbox: mpsc::SyncSender<ToClient>,
    /// This player's socket, kept only so that the session can hang up on
    /// somebody who has stopped reading it — see [`post`].
    line: TcpStream,
}

impl Server {
    /// Binds the listener and settles everything a session hands out — the
    /// spawn point costs a few hash mixes of layout, no terrain.
    pub fn bind(addr: impl ToSocketAddrs, config: WorldConfig) -> io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        let centre = Archipelago::new(&config)
            .nearest_island(Vec2::ZERO)
            .map(|spec| spec.centre())
            .unwrap_or(Vec2::ZERO);
        Ok(Self {
            listener,
            shared: Arc::new(Shared {
                config,
                centre,
                next_id: AtomicU32::new(1),
                players: Mutex::new(HashMap::new()),
                report: Box::new(|_| {}),
            }),
        })
    }

    /// Where to send news of players coming and going: called with a line of
    /// prose, from the thread it happened on. A server says nothing until
    /// asked to, which is what keeps `println!` out of a library and out of
    /// the tests' output.
    pub fn reporting_to(mut self, report: impl Fn(&str) + Send + Sync + 'static) -> Self {
        // Nothing is serving yet — `run` is what hands the shared state to
        // connection threads — so this is the one moment it has a single
        // owner, and the only place this can be set.
        Arc::get_mut(&mut self.shared)
            .expect("a server that has not been run yet owns its shared state")
            .report = Box::new(report);
        self
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

impl Shared {
    /// Where a given player is put down. Everyone enters on the same island,
    /// but not on the same square metre: markers standing exactly on top of
    /// each other read as one player, and what a joined session has to show
    /// first is that there is somebody else here.
    ///
    /// The offset is the player's id run through two irrational strides — the
    /// golden angle for the bearing, and a smaller one for how far out — so
    /// that arrivals land well apart from each other without any of them
    /// leaving one small circle, however many ids a long-lived server has
    /// dealt. Deterministic, so a client could work out the same point, and
    /// cheap, because it is arithmetic on an integer and touches no terrain.
    fn spawn_for(&self, id: PlayerId) -> Vec2 {
        let n = id.0 as f32;
        let bearing = n * 137.508_f32.to_radians();
        let out = SPAWN_SCATTER * (0.4 + 0.6 * (n * 0.618_034).fract());
        self.centre + Vec2::from_angle(bearing) * out
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

    // The first word must be a hello in the version this build speaks, and it
    // must come soon — see HANDSHAKE_TIMEOUT, which the welcome below lifts
    // again. A refusal answers with the version this build wanted, so
    // the client can say something more useful than "hung up".
    let _ = reader.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
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
    let _ = reader.set_read_timeout(None);
    let id = PlayerId(shared.next_id.fetch_add(1, Ordering::Relaxed));

    // The player's writer: everything the session wants them to hear goes
    // down the channel, and this thread puts it on the socket. It ends when
    // the channel closes — the roster dropping the player closes it — or
    // when a write fails because the player is gone.
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let (outbox, letters) = mpsc::sync_channel::<ToClient>(OUTBOX_DEPTH);
    thread::spawn(move || {
        for message in letters {
            if message.write(&mut writer).is_err() {
                break;
            }
        }
    });

    let player = Player {
        position: shared.spawn_for(id),
        outbox,
        line: stream,
    };
    post(
        &player,
        ToClient::Welcome {
            id,
            seed: shared.config.seed,
            spawn: player.position,
        },
    );

    // Onto the roster under one hold of the lock: the newcomer hears exactly
    // who is already here, everyone else hears the newcomer, and no move can
    // slip between the two — a `Moved` about a player a client has not been
    // introduced to would be about nobody.
    {
        let mut players = shared.players.lock().expect("no poisoned lock");
        for (other, existing) in players.iter() {
            post(
                &player,
                ToClient::Joined {
                    id: *other,
                    position: existing.position,
                },
            );
        }
        let arrival = ToClient::Joined {
            id,
            position: player.position,
        };
        players.insert(id, player);
        broadcast(&players, id, arrival);
    }
    (shared.report)(&format!("{id} joined"));

    // Relay until the line drops. Anything else ends the session too: after a
    // framing error nothing later on the stream can be trusted, a second hello
    // is a client that has lost its place, and a position no player could be
    // at is one nobody else should be shown walking towards.
    loop {
        match ToServer::read(&mut reader) {
            Ok(ToServer::Move { position }) if reachable(position) => {
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
    (shared.report)(&format!("{id} left"));
}

/// Whether a reported position is one a player could actually be standing at:
/// finite, and inside the coordinate space the world resolves — see
/// [`MAX_RANGE`]. A non-finite or absurd position would poison every machine
/// that eased a marker towards it.
fn reachable(position: Vec2) -> bool {
    position.is_finite() && position.abs().max_element() <= MAX_RANGE
}

/// Puts a message in a player's outbox, or hangs up on them.
///
/// Never waits. Most of the sends here happen with the roster's lock held,
/// and a thread stalled on one client's full queue would be a thread holding
/// the session still for everybody. A queue that deep is a client that has
/// stopped reading, so their connection is shut down instead — which is what a
/// departure looks like from this end anyway, so their own thread takes them
/// off the roster and tells everyone, exactly as if they had hung up.
fn post(player: &Player, message: ToClient) {
    if player.outbox.try_send(message).is_err() {
        let _ = player.line.shutdown(Shutdown::Both);
    }
}

/// Sends to everyone on the roster but `from`.
fn broadcast(players: &HashMap<PlayerId, Player>, from: PlayerId, message: ToClient) {
    for (id, player) in players {
        if *id != from {
            post(player, message);
        }
    }
}
