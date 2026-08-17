//! The server: what makes a world *exist*.
//!
//! It has two jobs. One is the session — who is in the world and where they
//! are — and that part is a roster and a relay. The other is the world
//! itself: this is the only process in a session that generates anything. A
//! client asks for a chunk by coordinate and is sent either open water or the
//! ground, and it is never told the seed, the layout, or which chunks are
//! worth asking for. See the `protocol` crate for why the ground travels
//! rather than the seed.
//!
//! Concurrency is plain std threading. Per connection: a thread blocking on
//! its reads, and a writer thread draining a channel onto the socket. Shared
//! between them: one lock around the roster, and a small pool of workers that
//! turn chunk requests into payloads. The pool is what keeps generation off
//! the connection threads — an island costs tens to hundreds of milliseconds
//! the first time anyone approaches it, which is far too long to spend inside
//! a read loop that also has to relay positions.

pub mod beasts;
pub mod cli;
mod console;
mod keeper;

use std::collections::HashMap;
use std::io;
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use glam::{IVec2, Vec2};
use protocol::{BeastKind, PlayerId, ToClient, ToServer, Token, WorldId, PROTOCOL_VERSION};
use world::archipelago::Archipelago;

pub use keeper::{data_dir, kept_worlds, KeptWorld};
pub use world::archipelago::{random_seed, WorldConfig, MAX_SEED};

/// How long a fresh connection has to say hello. Generous for a slow link,
/// and the point is only that a connection which arrives and then says
/// nothing — a port scan, a client that died mid-dial — cannot hold a thread
/// for the life of the process. The session that follows has no deadline:
/// going quiet is what a player standing still sounds like.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// How many messages may be waiting for one player before the session gives
/// up on them.
///
/// Most of the traffic is a trickle — a few tiny positions a second per
/// player — but chunks are not: a client entering the world asks for a couple
/// of hundred at once, and a chunk of ground is sixteen kilobytes. So this
/// has to be deep enough to hold an arrival's whole burst while the socket
/// drains it, and shallow enough that a client which has stopped reading
/// altogether is noticed rather than buffered forever. A full queue at this
/// depth is a few megabytes and several seconds of a client saying nothing,
/// which is not "briefly behind" — see [`post`], which hangs up on it.
const OUTBOX_DEPTH: usize = 256;

/// How many chunk requests may be waiting to be generated, across the whole
/// server.
///
/// Deep enough for several clients' arrival bursts at once, and bounded on
/// purpose: when it fills, the connection threads posting into it block, so a
/// client asking for ground faster than the world can make it simply waits.
/// The alternative is a queue a client can grow without limit by asking for
/// chunks it never intends to look at.
const CHUNK_QUEUE_DEPTH: usize = 1024;

/// How far from any player an island is kept in the world's cache, in metres.
///
/// A client streams a kilometre or so around its camera, and an island is
/// worth keeping for as long as any of its chunks might be asked for again —
/// so this is that reach with a big island's width of slack on top, which
/// means panning back and forth across a coast never pays to regenerate.
/// Beyond it an island is dropped and regenerates, identical to the bit, if
/// anybody sails back.
const ISLAND_CACHE_RADIUS: f32 = 6_144.0;

/// How often the world is asked to forget the islands nobody is near.
///
/// Rare, because it takes the cache's write lock and the thing it is bounding
/// — memory — moves at the speed of players sailing. Between sweeps a world
/// holds the islands of wherever everyone has recently been, which for a
/// session of any normal size is a handful.
const CACHE_SWEEP: Duration = Duration::from_secs(5);

/// How far from the world's spawn point an arriving player may be put down,
/// in metres. A few boat-lengths: enough that two markers are plainly two
/// markers, small enough that everyone still arrives in the same patch of
/// open water — the spawn point stands `world::archipelago::SPAWN_OFFSHORE`
/// metres off the nearest frame, so the whole scatter is afloat with a
/// hundred metres and more to spare.
///
/// Public so a test of the welcome can pin "players enter on the world's
/// spawn" without repeating the number.
pub const SPAWN_SCATTER: f32 = 12.0;

/// How often a listening server looks up from its accept to see whether it
/// has been asked to stop.
///
/// It is paid twice, and both times it is worth the wake-ups: an arriving
/// connection waits up to this long to be picked up, and a game leaving a
/// world it hosted spends up to this long inside the drop that ends it, which
/// is a frame of a screen it is leaving anyway.
///
/// The alternative is a blocking accept woken by connecting to the listener
/// from the stopping thread — free while idle, immediate when it works, and a
/// deadlock in the drop on any machine where that connection is the one thing
/// that doesn't work.
const STOP_POLL: Duration = Duration::from_millis(10);

/// How far from the origin a reported position may be, in metres. The world
/// crate measures where `f32` ground stops being exactly the ground at about
/// 1,280 km out (see its "How endless is endless"), and no honest client gets
/// near that — a player panning at the camera's own speed spends something
/// over a year of continuous play reaching it. A position past it is a broken
/// or hostile client, and believing one would walk every other machine's
/// marker of that player out to where the world no longer resolves.
const MAX_RANGE: f32 = 1_280_000.0;

/// What time of day a world opens at, as a phase of the day — see
/// [`ToClient::Daylight`]. Mid-morning: everyone arrives in daylight with a
/// good part of the day ahead of them, and the sun stands about where it
/// stood when there was only ever one hour.
pub const OPENING: f32 = 0.35;

/// How much faster than real time the world's clock runs while a night is
/// being waited out. A night is a little under half of a
/// [`protocol::DAY_SECONDS`] day, so at this pace waiting one out takes a
/// few seconds — long enough to watch the moon cross and the light come
/// back, short enough that nobody puts the kettle on.
const NIGHT_PACE: f32 = 60.0;

/// How long a [`ToServer::WantDawn`] stands before it lapses.
///
/// A wish rather than a switch: a client repeats it while the player holds
/// the key down, so this only has to outlast the gap between two of those.
/// Short, because what it costs to be long is a night that keeps racing
/// after somebody has taken the helm again.
const WAIT_LAPSE: Duration = Duration::from_millis(1_500);

/// How often the sky thread wakes: to notice the wind has moved, to tell the
/// time, and to run the clock on through a night everybody is waiting out.
const SKY_TICK: Duration = Duration::from_millis(200);

/// How often the time of day is told when the day is simply passing. Clients
/// run the same clock themselves between tellings — see
/// [`ToClient::Daylight`] — so this is a correction, not the sun's only
/// means of moving, and a second of drift is a fifth of a degree of arc.
const SKY_TELL: Duration = Duration::from_secs(1);

/// How often a kept world is written back to its file while the session
/// runs. The closing save is the one that matters — it is what makes
/// leaving and returning seamless — and these are insurance against the
/// session never reaching it: a crash or a kill costs at most this much
/// history, of a world whose whole file is smaller than one chunk of
/// ground.
const KEEP_INTERVAL: Duration = Duration::from_secs(30);

/// How much the wind must have moved, in metres per second of vector change,
/// before it is worth telling everyone about.
const WIND_STEP: f32 = 0.25;

/// Where session news goes. Boxed rather than a type parameter so that a
/// `Server` is one type however it reports, and defaulted to silence: what a
/// library does to somebody's stdout is not the library's decision.
type Report = Box<dyn Fn(&str) + Send + Sync>;

/// A hosted world, listening for players.
pub struct Server {
    listener: TcpListener,
    shared: Arc<Shared>,
    /// Whether this world was reopened from a file rather than freshly made
    /// — what [`Server::opening_at`] reads to decide between setting the
    /// clock and winding it, a kept world's day only ever growing older.
    loaded: bool,
    /// Both ends of the chunk queue, held until there is something to serve.
    ///
    /// Deliberately not in [`Shared`], and that is the whole of how the
    /// workers ever die. They hold the shared state, so a sender kept in there
    /// would be a sender they were keeping alive themselves: the channel could
    /// never close, `recv` could never fail, and every server would leave its
    /// pool running — and its whole world, islands and all, reachable through
    /// their copies of the state — for the life of the process. Out here the
    /// senders belong to the things that serve, and when the last of those has
    /// gone the workers are told so.
    ///
    /// It is also why the pool starts at [`Server::run`] rather than at
    /// [`Server::bind`]: starting it clones the shared state, and
    /// [`Server::reporting_to`] has to be the only owner of it.
    queue: (mpsc::SyncSender<ChunkRequest>, mpsc::Receiver<ChunkRequest>),
}

/// What every connection's thread shares.
pub(crate) struct Shared {
    /// The world, generated on demand and cached. Every chunk anyone is ever
    /// sent comes out of this one, which is what makes a session one place:
    /// two clients asking for the same chunk are answered from the same
    /// island, not from two generations of it that merely ought to agree.
    pub(crate) world: Arc<Archipelago>,
    /// Where this world is entered — [`Archipelago::spawn`]'s answer, asked
    /// once when the server binds. A client has no layout to work it out
    /// from, so this and [`Shared::facing`] are the whole of what it is told
    /// about where it has arrived.
    spawn: Vec2,
    /// The middle of the island the spawn stands off, so that a client opens
    /// its view looking at land rather than out to sea. Equal to the spawn
    /// itself when the layout offered nothing, which names no direction.
    facing: Vec2,
    /// Which world this is — see [`protocol::WorldId`]. Minted when the
    /// world was first made and constant for its life, however many times it
    /// is reopened or rehosted.
    world_id: WorldId,
    /// What the world is called on screens that list worlds. Empty for every
    /// world today — naming is a design still owed — and carried through
    /// from the file so a hand-named world keeps its name. Nothing on the
    /// wire carries it, and nothing here sets it: the file's own line-based
    /// parse is what guarantees it can never hold a newline, which the
    /// format could not survive.
    name: String,
    /// Dealt in joining order, and never reused within a session.
    next_id: AtomicU32,
    pub(crate) players: Mutex<HashMap<PlayerId, Player>>,
    /// Where the world last saw each player not currently in it, by the
    /// token it dealt them. What [`ToServer::Papers`] is answered from, and
    /// — merged with the roster, which holds the players who are here — what
    /// a save writes down. Loaded from the world's file when there is one,
    /// and kept regardless, so leaving and rejoining works even in a world
    /// nobody is keeping.
    remembered: Mutex<HashMap<Token, Vec2>>,
    /// The file this world survives in, if it is being kept: `None` is an
    /// ephemeral world — a test's, or a dedicated server nobody asked to
    /// remember — which lives exactly as long as its process.
    keeper: Option<keeper::Keeper>,
    /// Set once, by the end of the session, and read by every thread that
    /// might still be joining one: see [`Host::drop`], which is what makes it
    /// true, and [`serve`], which is what makes it mean something.
    pub(crate) stopping: AtomicBool,
    /// When this world was opened — the zero of the weather's clock. The
    /// weather is a pure function of seed and elapsed time (see
    /// [`world::weather`]), so this is the whole of the state it needs.
    started: Instant,
    /// What time of day the world opened at, as a phase — [`OPENING`] unless
    /// [`Server::opening_at`] said otherwise.
    opening: f32,
    /// Seconds of the world's own time run off over and above the session's:
    /// the nights its players have waited out, and the hours the console's
    /// `time` command has asked to have over with. Only ever grows — the
    /// world's day gets older or it gets older faster, never younger, which
    /// is the promise [`ToClient::Daylight`] makes. The only state the day
    /// has beyond the clock, which is what keeps two askers from disagreeing
    /// about what time it is — see [`Shared::phase`].
    skipped: Mutex<f32>,
    /// A wind ordered from the console, outranking the world's own weather
    /// for as long as it is set — see [`console`], where the ordering
    /// happens, and [`Shared::wind`], which is where it takes effect.
    commanded_wind: Mutex<Option<Vec2>>,
    /// Beasts the console has summoned and the warden has not yet raised:
    /// each with the spot the command already found water at, absorbed into
    /// the flock on the next beat — see [`beasts::mind_the_beasts`].
    pub(crate) summoned: Mutex<Vec<(BeastKind, Vec2)>>,
    /// The beasts as the world last knew them: loaded from its file at bind,
    /// taken back up by the warden's first beat, and rewritten with the
    /// living flock on every beat after — which is what a save writes down,
    /// whichever side of the first beat it lands. See
    /// [`beasts::mind_the_beasts`] for both halves.
    pub(crate) beasts: Mutex<Vec<keeper::BeastRecord>>,
    report: Report,
}

/// A chunk somebody wants, waiting for a worker to make it.
///
/// Carries the player rather than their outbox so that a worker finishing
/// long after they left has nothing to deliver to: the roster no longer holds
/// them, and the answer is dropped where it stands.
struct ChunkRequest {
    for_player: PlayerId,
    chunk: IVec2,
}

/// One connected player, as the roster sees them.
pub(crate) struct Player {
    pub(crate) position: Vec2,
    /// The token this player holds the world by — presented at the door or
    /// dealt there, and where the world will file their position when they
    /// leave. See [`protocol::Token`].
    token: Token,
    /// When this player last asked for the night to be over, if they have —
    /// see [`ToServer::WantDawn`], which stands only for [`WAIT_LAPSE`].
    waiting_since: Option<Instant>,
    /// The way to this player's ear: their writer thread drains this onto
    /// their socket.
    outbox: mpsc::SyncSender<ToClient>,
    /// This player's socket, kept only so that the session can hang up on
    /// somebody who has stopped reading it — see [`post`].
    line: TcpStream,
}

impl Player {
    /// Whether this player is, as of `now`, asking for the night to be over.
    /// A wish that lapses, so a client that has gone quiet — taken the helm
    /// again, or stopped talking altogether — is no longer asking.
    fn is_waiting_for_dawn(&self, now: Instant) -> bool {
        self.waiting_since
            .is_some_and(|asked| now.duration_since(asked) < WAIT_LAPSE)
    }
}

impl Server {
    /// Binds the listener, makes a fresh world, and asks it where it is
    /// entered.
    ///
    /// That generates the entry island — tens to hundreds of milliseconds,
    /// once, before anyone can join — and the origin is the fallback the
    /// world's clearing keeps open should the layout offer nothing. The world
    /// then stays: everything served afterwards comes out of it.
    ///
    /// Nothing is generated beyond that entry island, and no worker is
    /// started: a bound server is a world with a door, and [`Server::run`] or
    /// [`Server::spawn`] is what opens it. The world is ephemeral until
    /// [`Server::keeping_in`] or [`Server::keeping_at`] says otherwise.
    pub fn bind(addr: impl ToSocketAddrs, config: WorldConfig) -> io::Result<Self> {
        Self::from_record(addr, keeper::WorldRecord::fresh(config.seed), None, false)
    }

    /// Reopens the kept world at `path` and binds a listener for it: the
    /// same islands, the sun where it stood, the players where the world
    /// last saw them. The world's lock is taken before its file is read, so
    /// a world cannot be reopened out from under a session that is still
    /// writing it.
    pub fn reopen(addr: impl ToSocketAddrs, path: &Path) -> io::Result<Self> {
        let keeper = keeper::Keeper::hold(path)?;
        let record = keeper::load(path)?;
        Self::from_record(addr, record, Some(keeper), true)
    }

    fn from_record(
        addr: impl ToSocketAddrs,
        record: keeper::WorldRecord,
        keeper: Option<keeper::Keeper>,
        loaded: bool,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        let world = Arc::new(Archipelago::new(&WorldConfig { seed: record.seed }));
        let entry = world.spawn();
        let spawn = entry.map_or(Vec2::ZERO, |entry| entry.point);

        Ok(Self {
            listener,
            loaded,
            queue: mpsc::sync_channel(CHUNK_QUEUE_DEPTH),
            shared: Arc::new(Shared {
                world,
                spawn,
                // A world with no island to look at leaves the bearing to the
                // client, which is what a facing equal to the spawn means.
                facing: entry.map_or(spawn, |entry| entry.island.centre()),
                world_id: record.id,
                name: record.name,
                next_id: AtomicU32::new(1),
                players: Mutex::new(HashMap::new()),
                remembered: Mutex::new(record.players),
                keeper,
                stopping: AtomicBool::new(false),
                started: Instant::now(),
                opening: record.opening,
                // The whole of how a world resumes mid-story: its lived
                // seconds are on the clock before the session's first tick,
                // so the hour and the weather carry on from where the last
                // session left them — see [`Shared::age`].
                skipped: Mutex::new(record.age),
                commanded_wind: Mutex::new(None),
                summoned: Mutex::new(Vec::new()),
                beasts: Mutex::new(record.beasts),
                report: Box::new(|_| {}),
            }),
        })
    }

    /// Keeps this world at exactly `path`, writing it there now so it exists
    /// on disk from birth. For the caller who names the file — a dedicated
    /// server's `--world`; a game keeps its worlds with
    /// [`Server::keeping_in`] instead.
    pub fn keeping_at(mut self, path: PathBuf) -> io::Result<Self> {
        let keeper = keeper::Keeper::hold(&path)?;
        keeper.save(&self.shared.record())?;
        // The same moment [`Server::reporting_to`] uses, and for the same
        // reason: nothing is serving yet, so the shared state has one owner.
        Arc::get_mut(&mut self.shared)
            .expect("a server that has not been run yet owns its shared state")
            .keeper = Some(keeper);
        Ok(self)
    }

    /// Keeps this world in the directory `dir`, filed under its own id —
    /// how a game keeps every world it opens without inventing names for
    /// files.
    pub fn keeping_in(self, dir: &Path) -> io::Result<Self> {
        let id = self.shared.world_id;
        self.keeping_at(dir.join(format!("{id}.world")))
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

    /// What time of day the world opens at, as a phase — see
    /// [`ToClient::Daylight`]. Worlds open in the morning unless somebody
    /// asks for otherwise, which is what `--time` and the tests of the night
    /// do.
    ///
    /// On a *reopened* world this winds the clock forward to the next
    /// occurrence of that hour rather than setting it, exactly as the
    /// console's `time` command would: a kept world's day only ever grows
    /// older — the promise [`ToClient::Daylight`] makes — and its weather,
    /// running on the same clock, moves on with it.
    pub fn opening_at(mut self, phase: f32) -> Self {
        // Dropped rather than clamped when it is not a number, because a NaN
        // is not an hour that overshot — there is no hour it was nearly
        // asking for, so the world opens at the one worlds open at. What it
        // costs to let one through is the whole session: [`Shared::phase`]
        // would answer NaN for ever, [`protocol::is_night`] calls that night,
        // and every client would refuse every telling of the time.
        let phase = if phase.is_finite() { phase } else { OPENING };
        let phase = phase.rem_euclid(1.0);

        // The same moment [`Server::reporting_to`] uses, and for the same
        // reason: nothing is serving yet, so this is where the shared state
        // still has one owner.
        if self.loaded {
            self.shared.wind_forward_to(phase);
        } else {
            Arc::get_mut(&mut self.shared)
                .expect("a server that has not been run yet owns its shared state")
                .opening = phase;
        }
        self
    }

    /// The address actually bound — port 0 in, a real port out, which is how
    /// tests host on whatever the machine has free.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// The seed of the world being served — for the process hosting, which
    /// is entitled to say which world it made; nothing on the wire carries
    /// it. What a reopened world's host prints, having never been told.
    pub fn seed(&self) -> u32 {
        self.shared.world.seed()
    }

    /// Serves forever on this thread: every connection gets a thread of its
    /// own, from handshake to hang-up. What a dedicated server does, there
    /// being nothing else for its process to be doing.
    pub fn run(self) {
        let (wanted, requests) = self.queue;
        make_ground(&self.shared, requests);
        watch_the_sky(&self.shared);
        beasts::mind_the_beasts(&self.shared);
        accept(&self.listener, &self.shared, &wanted);
    }

    /// Serves on a thread of its own, and hands back the handle that ends it.
    ///
    /// What a game hosting a world for its own player does: the session has to
    /// run alongside a frame loop rather than instead of it, and it has to
    /// *stop* when the player leaves the world — a listener still holding the
    /// port, and a roster still relaying the last positions of a world nobody
    /// is in, would outlive the match that made them.
    pub fn spawn(self) -> io::Result<Host> {
        let addr = self.listener.local_addr()?;
        let (wanted, requests) = self.queue;
        make_ground(&self.shared, requests);
        watch_the_sky(&self.shared);
        beasts::mind_the_beasts(&self.shared);
        let shared = self.shared.clone();
        let thread = {
            let (listener, shared) = (self.listener, self.shared);
            thread::spawn(move || accept(&listener, &shared, &wanted))
        };
        Ok(Host {
            addr,
            shared,
            thread: Some(thread),
        })
    }
}

/// A world being hosted, for as long as the handle lives.
///
/// Dropping it is how a session ends: see the [`Drop`] implementation, which
/// is the whole of the type's behaviour.
pub struct Host {
    addr: SocketAddr,
    shared: Arc<Shared>,
    /// Taken by [`Drop`], which is the only place it is looked at. An `Option`
    /// because joining a thread consumes its handle, and a value being dropped
    /// can only be borrowed.
    thread: Option<thread::JoinHandle<()>>,
}

impl Host {
    /// The address being served on. Port 0 in, a real port out, as with
    /// [`Server::local_addr`].
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The seed of the world being served. Nothing on the wire carries it —
    /// a client is sent ground and never the recipe — so this is for the
    /// process hosting, which is entitled to say which world it made.
    pub fn seed(&self) -> u32 {
        self.shared.world.seed()
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        // Before the roster is read, not after: a connection still in its
        // handshake is on nobody's roster, so the flag is the only thing that
        // can reach it. Taking the lock below is what publishes it — a thread
        // that joins the roster does so under the same lock, so it either got
        // there first and is shut down here, or it arrives to find the flag
        // set. There is no third case, and so nobody is left connected to a
        // world that has ended.
        self.shared.stopping.store(true, Ordering::Relaxed);

        // Everyone still connected is blocked in a read that only their own
        // client could end, and their threads each hold a share of the state
        // this is trying to be the end of. Shutting their sockets down from
        // here is, at their end of it, exactly what a server hanging up looks
        // like — which it is — and at this end it is what lets their threads
        // reach the departure they would otherwise never get to.
        {
            let players = self.shared.players.lock().expect("no poisoned lock");
            for player in players.values() {
                let _ = player.line.shutdown(Shutdown::Both);
            }
        }

        // Waited for rather than left to wind down, so that a host dropped and
        // another bound in its place — a player leaving a world they hosted
        // and starting a second one — cannot find the old listener still
        // holding the port.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Takes connections until asked to stop, giving each a thread of its own,
/// lets the world forget the islands nobody is near, and keeps the world's
/// file current.
///
/// The sweep and the saves ride along here rather than on threads of their
/// own because this loop already wakes on a timer and has nothing else to do
/// between connections. Each has to happen *somewhere*: a world that only
/// ever grew would hold every island anybody had sailed past for the life of
/// the process, and a world only ever saved at the end would lose its whole
/// session to a crash.
fn accept(listener: &TcpListener, shared: &Arc<Shared>, wanted: &mpsc::SyncSender<ChunkRequest>) {
    // Non-blocking, so that the stop above is noticed within [`STOP_POLL`]
    // rather than whenever the next connection happens to arrive.
    let _ = listener.set_nonblocking(true);
    let mut swept = Instant::now();
    let mut kept = Instant::now();

    while !shared.stopping.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                // An accepted socket inherits the listener's mode on some
                // platforms, and every read a session does is meant to block.
                if stream.set_nonblocking(false).is_err() {
                    continue;
                }
                let (shared, wanted) = (shared.clone(), wanted.clone());
                thread::spawn(move || serve(stream, shared, wanted));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(STOP_POLL),
            // A connection that failed to arrive is its problem, not the
            // session's.
            Err(_) => {}
        }

        if swept.elapsed() >= CACHE_SWEEP {
            swept = Instant::now();
            let where_everyone_is: Vec<Vec2> = {
                let players = shared.players.lock().expect("no poisoned lock");
                players.values().map(|player| player.position).collect()
            };
            shared
                .world
                .retain_near(&where_everyone_is, ISLAND_CACHE_RADIUS);
        }

        if kept.elapsed() >= KEEP_INTERVAL {
            kept = Instant::now();
            shared.keep();
        }
    }

    // The closing save: this loop ends inside the drop of a [`Host`] — a
    // player leaving the world they hosted — so by the time the drop
    // returns, everything the session was is in the file. The lock is let
    // go here too, rather than when the shared state finally drops: the
    // worker threads hold that state for a beat past the end, and a world
    // left and immediately reopened must not be refused by its own
    // session's shadow.
    shared.keep();
    if let Some(keeper) = &shared.keeper {
        keeper.release();
    }
}

/// Starts the workers that turn chunk requests into ground.
///
/// One per core, near enough, because generating an island is the most
/// expensive thing this process does and a client arriving somewhere new
/// blocks on it: with one worker a player entering the world would watch the
/// ground arrive an island at a time, however many cores the machine had
/// spare. They share one queue rather than having one each, so a client whose
/// island is still being made does not hold up everyone else's open water.
///
/// Nothing joins them. They end when the last [`Shared`] is dropped and the
/// queue closes, which is after the session that could still want them has
/// gone; a worker outliving its server by one island's generation holds
/// nothing anybody needs back.
fn make_ground(shared: &Arc<Shared>, requests: mpsc::Receiver<ChunkRequest>) {
    let workers = thread::available_parallelism()
        .map(|cores| cores.get())
        .unwrap_or(4)
        .clamp(2, 8);
    let requests = Arc::new(Mutex::new(requests));

    for _ in 0..workers {
        let shared = shared.clone();
        let requests = requests.clone();
        thread::spawn(move || loop {
            // Waiting for work holds the lock, since a blocking `recv` needs
            // the receiver borrowed for the length of it — so only one worker
            // is ever the one listening, and the rest are queued behind it.
            // That serialises the *handing out*, which is a mutex handoff
            // against a hundred milliseconds of generating an island, and the
            // guard is dropped before any of that happens. What matters is
            // that the work itself runs on all of them at once, and it does.
            let request = {
                let queue = requests.lock().expect("no poisoned lock");
                queue.recv()
            };
            // The channel has closed, which means every sender has gone: the
            // listener has stopped and the last connection has ended, so
            // there is nobody left to want ground. This is the pool's death,
            // and with it the last hold on the world it was generating.
            let Ok(request) = request else {
                return;
            };

            let ground = shared.world.chunk_payload(request.chunk);
            let answer = ToClient::Chunk {
                chunk: request.chunk,
                ground,
            };

            // Posted under the roster's lock, like everything else a player
            // hears, so an answer cannot overtake the departure of the player
            // it was for. Somebody who left while their ground was being made
            // is simply no longer here, and the answer goes nowhere.
            let players = shared.players.lock().expect("no poisoned lock");
            if let Some(player) = players.get(&request.for_player) {
                post(player, answer);
            }
        });
    }
}

/// Watches the sky on a thread of its own: the wind, the time of day, and
/// the nights the players ask to have over with.
///
/// Neither the weather nor the clock needs ticking — both are functions of
/// how long the world has been open — so most of what this does is notice
/// that an answer has moved and pass it on. The wind is told when it has
/// meaningfully changed: [`WIND_STEP`] of vector change covers a shift in
/// strength and a shift in bearing with one test, keeps a steady sky silent,
/// and during a real change works out to a message every few seconds. The
/// time is told on a beat instead, because it is always changing and a
/// client is running the same clock itself between tellings.
///
/// The one thing here that does more than watch is the night: while everyone
/// in the world is waiting one out, this is what runs the clock fast — see
/// [`Shared::run_off_the_night`].
///
/// The thread ends with the session, within a beat of [`Shared::stopping`]
/// being set, and is deliberately not joined: unlike a connection it holds
/// nothing but a share of the state, and making every [`Host`] drop wait out
/// the last beat would slow every session's end for nothing.
fn watch_the_sky(shared: &Arc<Shared>) {
    let shared = shared.clone();
    thread::spawn(move || {
        let mut told_wind = shared.wind();
        let mut told_time = Instant::now();
        loop {
            if shared.stopping.load(Ordering::Relaxed) {
                return;
            }
            thread::sleep(SKY_TICK);

            let wound = shared.run_off_the_night(SKY_TICK);
            let wind = shared.wind();

            // Gathered before the roster is locked, which is what every path
            // into the clock does — the welcome in [`serve`] asks for the sky
            // outside its own hold for the same reason. So the day's lock is
            // never taken by a thread already holding the roster's, and the
            // two have no order to disagree about.
            let mut news = Vec::new();
            if (wind - told_wind).length() > WIND_STEP {
                told_wind = wind;
                news.push(ToClient::Weather { wind });
            }
            // A wound clock is told at once, whatever the beat: it is the
            // one thing that moves the day other than the day passing, and
            // it is what the client waiting for dawn is watching for.
            if wound || told_time.elapsed() >= SKY_TELL {
                told_time = Instant::now();
                news.push(ToClient::Daylight {
                    phase: shared.phase(),
                });
            }

            if !news.is_empty() {
                let players = shared.players.lock().expect("no poisoned lock");
                for word in news {
                    broadcast_all(&players, word);
                }
            }
        }
    });
}

impl Shared {
    /// World-seconds lived: the session's own clock plus [`Shared::skipped`]
    /// — which a reopened world starts with its whole past in, so age spans
    /// every session the world has ever run. The hour and the weather are
    /// both functions of it, and only of it: while the world is closed no
    /// time passes at all, which is what makes quitting mid-gale and
    /// reloading land back in the same gale rather than past it.
    fn age(&self) -> f32 {
        let skipped = *self.skipped.lock().expect("no poisoned lock");
        self.started.elapsed().as_secs_f32() + skipped
    }

    /// The wind over this world right now. Asked rather than kept: the
    /// weather is a pure function of the seed and the world's age, so there
    /// is no cached state for two askers to disagree over — unless the
    /// console has taken the weather in hand, which *is* state, and then its
    /// order is the answer for everyone until it lets go.
    ///
    /// The age rather than the session's own clock, deliberately, on both
    /// counts: a kept world resumes the sky it closed under, and a night
    /// waited out is weather passing too — a crew at anchor through till
    /// dawn has sat out some of the blow.
    fn wind(&self) -> Vec2 {
        let commanded = *self.commanded_wind.lock().expect("no poisoned lock");
        commanded.unwrap_or_else(|| world::weather::wind(self.world.seed(), self.age()))
    }

    /// Orders the wind, or — with `None` — gives the weather back to the
    /// world. The sky thread notices the answer to [`Shared::wind`] moving
    /// and tells everyone, exactly as it does when the real weather turns.
    fn command_wind(&self, wind: Option<Vec2>) {
        *self.commanded_wind.lock().expect("no poisoned lock") = wind;
    }

    /// Runs the world's clock forward to the next time it reads `target`,
    /// and says what the phase now is.
    ///
    /// Forward to the *next* occurrence, never backwards to the last one —
    /// asking for an hour already struck means asking for tomorrow's, so the
    /// world's overall time only ever grows and the promise
    /// [`ToClient::Daylight`] makes holds. Asking for the very hour it is
    /// moves nothing.
    fn wind_forward_to(&self, target: f32) -> f32 {
        let mut skipped = self.skipped.lock().expect("no poisoned lock");
        // The phase worked out inline rather than asked of [`Shared::phase`],
        // which takes this same lock — and it has to be under the lock, or a
        // night being run off between the read and the write would be run
        // off twice.
        let seconds = self.started.elapsed().as_secs_f32() + *skipped;
        let phase = (self.opening + seconds / protocol::DAY_SECONDS).rem_euclid(1.0);
        let ahead = (target - phase).rem_euclid(1.0);
        *skipped += ahead * protocol::DAY_SECONDS;
        (phase + ahead).rem_euclid(1.0)
    }

    /// What time of day it is here, as a phase — see [`ToClient::Daylight`].
    ///
    /// The same construction as the wind, and for the same reason: the hour
    /// follows from the world's age, so nothing has to be kept in step. The
    /// one piece of state is [`Shared::skipped`], which is what a night
    /// waited out — and, for a kept world, every earlier session — leaves
    /// behind.
    fn phase(&self) -> f32 {
        (self.opening + self.age() / protocol::DAY_SECONDS).rem_euclid(1.0)
    }

    /// This world, summarised for its file: the identity, the age as of
    /// now, and everyone the world has ever seen — the roster's positions
    /// laid over the remembered ones, so a player who is here is written
    /// where they are, not where they last left.
    ///
    /// The locks are taken one at a time, never nested, like every other
    /// path through them.
    fn record(&self) -> keeper::WorldRecord {
        let aboard: Vec<(Token, Vec2)> = {
            let players = self.players.lock().expect("no poisoned lock");
            players
                .values()
                .map(|player| (player.token, player.position))
                .collect()
        };
        let mut players = self.remembered.lock().expect("no poisoned lock").clone();
        players.extend(aboard);
        let beasts = self.beasts.lock().expect("no poisoned lock").clone();
        keeper::WorldRecord {
            id: self.world_id,
            seed: self.world.seed(),
            name: self.name.clone(),
            opening: self.opening,
            age: self.age(),
            players,
            beasts,
        }
    }

    /// Writes the world to its file, if it is being kept — the world's own
    /// failure story: a save that fails is reported and the session sails
    /// on, the next interval being another chance.
    fn keep(&self) {
        if let Some(keeper) = &self.keeper {
            if let Err(error) = keeper.save(&self.record()) {
                (self.report)(&format!("the world could not be kept: {error}"));
            }
        }
    }

    /// Runs the clock on through a night everybody is waiting out, and says
    /// whether it did.
    ///
    /// Everybody, not anybody: a world where one player is still sailing
    /// keeps its night, because the sky is one sky and having it whipped
    /// away is worse than being kept up. Alone — which is every solo game,
    /// the world it hosts having exactly one player in it — that reads as
    /// simply asking.
    ///
    /// It runs the clock rather than jumping it so that the night visibly
    /// passes: everyone watches the moon cross and the light come back, and
    /// a client has only ever a small step to make up. The run stops at
    /// [`protocol::DAYBREAK`] rather than at sunrise, so a night waited out
    /// ends looking at one.
    fn run_off_the_night(&self, tick: Duration) -> bool {
        let phase = self.phase();
        if !protocol::is_night(phase) {
            return false;
        }
        // One now for the whole roster, read before the lock: everybody's
        // wish is judged against the same instant, and none of them ages
        // while the players ahead of them are being looked at.
        let now = Instant::now();
        {
            let players = self.players.lock().expect("no poisoned lock");
            let all_waiting = !players.is_empty()
                && players
                    .values()
                    .all(|player| player.is_waiting_for_dawn(now));
            if !all_waiting {
                return false;
            }
        }

        // The extra seconds only: the tick's own second passes anyway, and
        // counting it twice would put the pace out by one. Capped at what is
        // left of the night, so the fast clock lands on daybreak rather than
        // carrying the morning away with it.
        let to_dawn = (protocol::DAYBREAK - phase).rem_euclid(1.0) * protocol::DAY_SECONDS;
        let run = (tick.as_secs_f32() * (NIGHT_PACE - 1.0)).min(to_dawn);
        *self.skipped.lock().expect("no poisoned lock") += run;
        true
    }

    /// Where a given player is put down. Everyone enters on the world's
    /// spawn point — open water just off the first island's coast, see
    /// [`Archipelago::spawn`] — but not on the same square metre: markers
    /// standing exactly on top of each other read as one player, and what a
    /// joined session has to show first is that there is somebody else here.
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
        self.spawn + Vec2::from_angle(bearing) * out
    }
}

/// One connection, cradle to grave: handshake, introductions, relay,
/// departure.
fn serve(stream: TcpStream, shared: Arc<Shared>, wanted: mpsc::SyncSender<ChunkRequest>) {
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

    // Which world this is, so the client can look out what it holds of it —
    // and then the papers themselves, under the same deadline the hello was:
    // a connection that says hello and then nothing is the same stalled
    // stranger either way.
    if (ToClient::World {
        id: shared.world_id,
    })
    .write(&mut &stream)
    .is_err()
    {
        return;
    }
    let presented = match ToServer::read(&mut reader) {
        Ok(ToServer::Papers { token }) => token,
        _ => return,
    };
    let _ = reader.set_read_timeout(None);
    let id = PlayerId(shared.next_id.fetch_add(1, Ordering::Relaxed));

    // The papers, resolved against what the world remembers: a recognised
    // token re-enters where the world last saw its holder, and anything else
    // — no token, or one this world never dealt — is a stranger, dealt a
    // fresh token on the spot. An unrecognised token is not an offence,
    // because the world's memory and a client's can part ways honestly: a
    // world file lost, or a world rebuilt under the same address.
    let (token, returning_to) = {
        let remembered = shared.remembered.lock().expect("no poisoned lock");
        match presented {
            Some(token) => match remembered.get(&token) {
                Some(&position) => (token, Some(position)),
                None => (Token(keeper::mint()), None),
            },
            None => (Token(keeper::mint()), None),
        }
    };

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

    let mut player = Player {
        position: returning_to.unwrap_or_else(|| shared.spawn_for(id)),
        token,
        waiting_since: None,
        outbox,
        line: stream,
    };
    let mut returning = returning_to.is_some();

    // Onto the roster and then welcomed, under one hold of the lock.
    //
    // In that order, because a client's join is over the moment it reads its
    // welcome: a player welcomed before they were listed is, for that instant,
    // one who believes they are in the world while the session cannot see them
    // to hang up on them — and the end of a session is exactly the moment that
    // instant is unaffordable. Listing them first costs the welcome nothing,
    // since everything else that writes to a player takes this lock too, so
    // nothing can get in front of it.
    //
    // The rest is why the hold is one hold: the newcomer hears exactly who is
    // already here, everyone else hears the newcomer, and no move can slip
    // between the two — a `Moved` about a player a client has not been
    // introduced to would be about nobody.
    //
    // The sky is asked for out here rather than inside that hold, because the
    // hour has a lock of its own — see [`Shared::skipped`] — and asking for it
    // with the roster held would be the one path in the process that nested
    // the two. Nothing needs it to be inside: what the newcomer is owed is a
    // sky from about the moment they arrived, and a few microseconds older is
    // the same sky.
    let wind = shared.wind();
    let phase = shared.phase();
    {
        let mut players = shared.players.lock().expect("no poisoned lock");

        // A world that has already ended has nobody to introduce and no way to
        // hear of anyone arriving now. Its drop set this before reaching for
        // the lock, so a connection that gets here afterwards finds it — and
        // returning drops the socket, which is the same hang-up the drop would
        // have delivered had this player made it onto the roster in time.
        if shared.stopping.load(Ordering::Relaxed) {
            return;
        }

        // A token already on the roster is one client's files opened twice —
        // somebody joining as themselves while themselves. The second
        // arrival enters as a stranger rather than being refused: a refusal
        // would lock a player out of a world over a copied file, where a
        // fresh start merely puzzles them, and only the world's memory of
        // one position was ever at stake.
        if players.values().any(|other| other.token == player.token) {
            player.token = Token(keeper::mint());
            player.position = shared.spawn_for(id);
            returning = false;
        }

        let welcome = ToClient::Welcome {
            id,
            spawn: player.position,
            // A returning player is not put down beside the entry island, so
            // its centre is nothing to turn their view towards: their own
            // position names no direction, which leaves the bearing to the
            // client, exactly as a world with no island to look at does.
            facing: if returning {
                player.position
            } else {
                shared.facing
            },
            token: player.token,
        };
        let arrival = ToClient::Joined {
            id,
            position: player.position,
        };
        players.insert(id, player);

        let newcomer = &players[&id];
        post(newcomer, welcome);
        // The sky, straight after the session itself: a client draws the sea
        // from the moment it has ground, and until this arrives it can only
        // assume a day nobody promised. Inside the same hold of the lock as
        // the welcome, so the watcher's broadcasts cannot slip in front of it
        // and arrive before the client knows who it is.
        post(newcomer, ToClient::Weather { wind });
        // And what hour it is, for the same reason: a client with no word on
        // the time can only draw an assumed one, and an arrival that snapped
        // from midday to a night already half gone would be a worse opening
        // than a moment's wait.
        post(newcomer, ToClient::Daylight { phase });
        // And the words this console answers to, so a client's console can
        // offer them as the player types. Hints only: lines cross verbatim
        // and are answered whether or not they start with any of these.
        post(
            newcomer,
            ToClient::Vocabulary {
                verbs: console::VERBS.iter().map(|verb| verb.to_string()).collect(),
            },
        );
        for (other, existing) in players.iter() {
            if *other != id {
                post(
                    newcomer,
                    ToClient::Joined {
                        id: *other,
                        position: existing.position,
                    },
                );
            }
        }
        broadcast(&players, id, arrival);
    }
    (shared.report)(&format!("{id} joined"));

    // Relay and take orders for ground until the line drops. Anything else
    // ends the session too: after a framing error nothing later on the stream
    // can be trusted, a second hello is a client that has lost its place, and
    // a position or a chunk no player could be at is one to hang up over
    // rather than to answer.
    loop {
        match ToServer::read(&mut reader) {
            Ok(ToServer::Move { position }) if reachable(position) => {
                let mut players = shared.players.lock().expect("no poisoned lock");
                if let Some(player) = players.get_mut(&id) {
                    player.position = position;
                }
                broadcast(&players, id, ToClient::Moved { id, position });
            }
            Ok(ToServer::WantDawn) => {
                // Noted rather than acted on: whether the night actually
                // runs depends on what everyone else wants — see
                // [`Shared::run_off_the_night`], which is where the sky
                // thread reads this.
                let mut players = shared.players.lock().expect("no poisoned lock");
                if let Some(player) = players.get_mut(&id) {
                    player.waiting_since = Some(Instant::now());
                }
            }
            Ok(ToServer::Command { line }) => {
                // Interpreted holding nothing: a command takes the day's or
                // the weather's own locks, and broadcasts under the roster's,
                // so the roster must not already be held here. Anyone in the
                // world may command — a session is a game among people who
                // chose each other — and the host's log says who asked what.
                let reply = console::interpret(&shared, id, &line);
                (shared.report)(&format!("{id}: {line}"));
                let players = shared.players.lock().expect("no poisoned lock");
                if let Some(player) = players.get(&id) {
                    post(player, ToClient::Reply { text: reply });
                }
            }
            Ok(ToServer::WantChunk { chunk }) if in_the_world(chunk) => {
                // Queued rather than answered, because answering means
                // possibly generating an island and this thread also has to
                // stay listening. A full queue blocks here, which is the
                // backpressure that keeps one client from ordering ground
                // faster than the world can make it — and a send that fails
                // outright means the workers have gone, so there is no more
                // ground to be had and the session is over.
                if wanted
                    .send(ChunkRequest {
                        for_player: id,
                        chunk,
                    })
                    .is_err()
                {
                    break;
                }
            }
            _ => break,
        }
    }

    // Where the world last saw them, filed under their token *before* they
    // leave the roster — a save can land between the two steps, and a player
    // momentarily in both places is written once, where they are, while one
    // momentarily in neither would be a position lost. The position cannot
    // move between the snapshot and the removal: only this thread's read
    // loop ever moved it, and the read loop is over. And the two locks are
    // taken one after the other, never together, like every other path
    // through them.
    let leaving = {
        let players = shared.players.lock().expect("no poisoned lock");
        players
            .get(&id)
            .map(|player| (player.token, player.position))
    };
    if let Some((token, position)) = leaving {
        shared
            .remembered
            .lock()
            .expect("no poisoned lock")
            .insert(token, position);
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
/// that eased a marker towards it. The keeper holds a loaded world file to
/// the same test, an edited file being the one other door positions arrive
/// through.
pub(crate) fn reachable(position: Vec2) -> bool {
    position.is_finite() && position.abs().max_element() <= MAX_RANGE
}

/// Whether a chunk coordinate names ground anybody could stand on — the same
/// [`MAX_RANGE`] test, in chunks. A client asking for ground a thousand
/// kilometres past where the world resolves is broken or hostile, and
/// answering would put a worker to work generating an island out of
/// coordinates that no longer have a metre between them.
fn in_the_world(chunk: IVec2) -> bool {
    reachable(chunk.as_vec2() * protocol::ground::CHUNK_METRES)
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

/// Sends to the whole roster — what the weather takes, and the beasts,
/// nobody having caused it the way a move or a leaving has an author to skip.
pub(crate) fn broadcast_all(players: &HashMap<PlayerId, Player>, message: ToClient) {
    for player in players.values() {
        post(player, message.clone());
    }
}

/// Sends to everyone on the roster but `from`.
fn broadcast(players: &HashMap<PlayerId, Player>, from: PlayerId, message: ToClient) {
    for (id, player) in players {
        if *id != from {
            // Cloned per recipient: a message is no longer a handful of bytes
            // that copy for free. Nothing broadcast is ever a chunk, though —
            // ground is answered to the one player who asked for it — so what
            // is being cloned here is still a position and an id.
            post(player, message.clone());
        }
    }
}
