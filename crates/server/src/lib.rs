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
pub mod signals;

use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use glam::{IVec2, Vec2};
use protocol::ground::{chunk_at, dequantize};
use protocol::survey::{in_sight_along, Soundings, Survey, SIGHT_RADIUS};
use protocol::{
    BeastKind, BoatId, BoatKind, PlayerId, ToClient, ToServer, Token, WorldId, PROTOCOL_VERSION,
    SURVEY_BATCH_BYTES,
};
use world::archipelago::Archipelago;

pub use keeper::{data_dir, discard, keep_data_in, kept_worlds, KeptWorld};
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
/// Deep enough to hold an arrival's whole burst — a couple of hundred chunks
/// at sixty-six kilobytes each on the wire (and nearer twice that as the
/// decoded values this queue actually holds), plus a word per boat in the
/// world — while the socket drains it, and shallow enough that a client
/// which has stopped reading altogether is noticed rather than buffered
/// forever. Size it against the worst message and not the ordinary one: a
/// queue this deep in longest-case survey batches is some sixty megabytes
/// for one player, which is why [`post`] hangs up on a full outbox rather
/// than holding it. That bound quadrupled when the facet went to a metre;
/// the depth stays where the arrival burst needs it, and the memory is the
/// price of a slow reader, paid knowingly.
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
/// open water, which the offing `world::archipelago::SPAWN_OFFSHORE` leaves
/// between the spawn and the nearest frame swallows several times over.
pub const SPAWN_SCATTER: f32 = 12.0;

/// How near a boat a player must stand for a boarding to be granted, in
/// metres. The client only offers the key within arm's reach of a hull —
/// its own boarding reach is a stride or two — so this is that reach with
/// room for the tenth of a second by which the two machines' pictures of a
/// moving player differ.
const BOARD_GRANT: f32 = 12.0;

/// How far a remembered boat may lie from where its returning keeper left
/// it and still be theirs to resume at, in metres. After a clean stop the
/// two agree exactly; a boat found beyond this has been sailed somewhere by
/// somebody else in the meantime, and its old keeper enters in a fresh hull
/// rather than being teleported to wherever their old one was abandoned.
const KEPT_BERTH: f32 = 16.0;

/// How far from where an arrival is being put down a free hull nobody is
/// keeping may lie and still be handed to them rather than a new one minted,
/// in metres — see [`BoatState::keeper`].
///
/// Wide enough to swallow the whole spawn scatter several times over, so the
/// join-and-hang-up loop this exists to bound is handed the same boat back
/// every time however the arrivals are strewn; narrow enough that a returner
/// entering on some far coast gets a hull where *they* are rather than being
/// pointed at one waiting off the entry island.
const SPARE_BERTH: f32 = 64.0;

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

/// The most way the survey will follow at once, in metres.
///
/// The survey follows the *way* between two reports and not just their ends —
/// see [`protocol::survey::in_sight_along`] — which only holds while the two
/// are a report apart, and no hull here makes two kilometres in that. A
/// longer jump is a client that stopped talking or one that is lying, and
/// following the whole of it would ink water nobody crossed, at a cost in
/// ground worked out that a client could order by the megametre.
///
/// This is the ceiling and [`PLAUSIBLE_SPEED`] the rate: an allowance never
/// banks past a sweep, so an hour of silence is not an hour of work.
pub const SURVEY_SWEEP: f32 = 2_048.0;

/// How fast a player may be believed to have travelled, in metres per second,
/// as far as the survey is concerned.
///
/// Unbounded, a client that never asks for a chunk can order a hundred chunks
/// of ground worked out per twelve-byte `Move` — on the connection thread,
/// outside the worker pool and past every piece of backpressure the chunk
/// path has — as fast as it can write. Bounding the way *per second* rather
/// than per message makes that arithmetic about the world instead of about
/// how fast a socket can be fed.
///
/// Twenty-odd times what the fastest hull here makes, and it wants to stay
/// absurd: every metre of headroom is a lag spike, a stall on some far
/// machine, or a legitimate catch-up that this must never clip.
pub const PLAUSIBLE_SPEED: f32 = 256.0;

/// How many chunks of a returning player's survey are worked out between one
/// look at whether there is still anybody to tell — see
/// [`tell_the_survey_so_far`].
///
/// A unit of *work*, not of wire — that budget is [`SURVEY_BATCH_BYTES`].
/// Each of these chunks may mean growing an island, so this is how long the
/// backfill carries on for somebody who has already left. The chunks come
/// sorted, so a slab is usually one stretch of coast and the islands under it
/// are grown once between them: milliseconds ordinarily, under a second at
/// worst.
const SURVEY_SLAB: usize = 64;

/// How near a cairn a player has to be to be told about it, in metres.
///
/// A cairn is a thing standing in the world rather than an announcement, so it
/// is told to whoever could be looking at it. Narrow enough that who holds what
/// is something a player finds out by going there, and wide enough to be a
/// *sighting* — the range at which a pale pillar on a headland is a thing you
/// could pick out, rather than the range the server happens to be willing to
/// say so at. It earns [`Knowing::Sighted`] and no more — that there is a
/// cairn, and where; what the island is *called* costs a landing, see
/// [`CAIRN_VISIT`].
///
/// It does not gate a player's own claims, which they are told wherever they
/// stand — see [`tell_the_cairns_about`], where the difference is argued.
///
/// It was half as wide again when a cairn was a banner on a twenty-metre staff,
/// which carried a mile; it was cut to match the mark becoming a stone pillar
/// at head height (see `game::cairn` for why it did). That puts it just inside
/// the radius a client streams terrain over, where it used to sit well outside.
/// Not load-bearing — a cairn waits unfooted and unseen until there is ground
/// under it either way — and the easier way round: the wait is now a few frames
/// rather than half a kilometre of sailing.
pub const CAIRN_SIGHT: f32 = 1_000.0;

/// How near a cairn a player has to come to read what is written on it, in
/// metres.
///
/// Nearly eight times narrower than [`CAIRN_SIGHT`], and the whole difference
/// between the two tiers of knowing. Stone standing on a headland says *somebody
/// is here*, and says it to anyone who passes. A name is lettering, and
/// lettering is read by walking up to it.
///
/// Short enough that no honest voyage collects a name in passing, long enough
/// that a player who has beached is not left hunting for the spot — a stone's
/// throw, not a doorstep. A cairn on a headland can be read from a boat lying
/// right off it, which is the claimant's own doing: build inland and the name
/// stays inland with it.
pub const CAIRN_VISIT: f32 = 128.0;

/// How often one player may have a cairn raised or rewritten, at most.
///
/// Two different costs, one pace, a cairn being hand-carved either way: a
/// quarter of a second between asks is nothing to a player and everything to
/// a pestering one. Settling a claim walks every coastline that player has
/// surveyed, which grows with the voyage. Christening one is told to every
/// player within [`CAIRN_SIGHT`], so unpaced, a client alternating two
/// perfectly good names could fill — and so hang up, see [`post`] — every
/// outbox near its island.
const CAIRN_PACE: Duration = Duration::from_millis(250);

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

/// Taking one of this session's locks.
///
/// Poisoning means a thread panicked while holding it, and a session with a
/// panicked thread in it is over — there is no state left worth carrying on
/// with, so the honest answer is the panic the guard already carries. Said
/// once here it is a rule, and the code below is left saying which lock it
/// wants and nothing else.
pub(crate) trait Held<T> {
    /// This lock, held.
    fn held(&self) -> MutexGuard<'_, T>;
}

impl<T> Held<T> for Mutex<T> {
    fn held(&self) -> MutexGuard<'_, T> {
        self.lock().expect("no poisoned lock")
    }
}

/// One player's survey, held behind a lock of its own rather than inside the
/// roster's — see [`Player::surveyed`].
///
/// An hour's sailing is thousands of chunks, and the two things that want the
/// whole of it — the periodic save and the departure — would otherwise copy
/// it with the roster held, stopping every other player's `Move` and `Helm`
/// for the length of the copy. What the roster hands out is a pointer, and
/// the copying happens where nobody is waiting on it.
///
/// Lock order: a leaf. Everybody takes the roster, clones the handle, lets the
/// roster go, and only then locks this, so no thread ever holds both.
///
/// The ink is kept and not only the coordinates, at a few megabytes for a
/// player who has called at a thousand islands, because it buys the one thing
/// a bare list of chunks cannot answer: whether a coastline closes. A claim is
/// settled by asking exactly that — see [`settle_a_claim`] — and asking it of
/// a list would mean working a whole voyage's ground out again, per ask, on
/// the connection's own thread.
type Surveyed = Arc<Mutex<Survey>>;

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
    /// would be one they were keeping alive themselves: the channel could
    /// never close and every server would leave its pool — and its whole
    /// world, reachable through their copies of the state — running for the
    /// life of the process. Out here the senders belong to the things that
    /// serve, and when the last of those has gone the workers are told so.
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
    /// world today — naming is a design still owed — and carried through from
    /// the file so a hand-named world keeps its name. The file's line-based
    /// parse is what guarantees it can never hold a newline, which the format
    /// could not survive.
    name: String,
    /// Dealt in joining order, and never reused within a session.
    next_id: AtomicU32,
    pub(crate) players: Mutex<HashMap<PlayerId, Player>>,
    /// Where the world last saw each player not currently in it, by the
    /// token it dealt them — and whether they were at a helm. What
    /// [`ToServer::Papers`] is answered from, and — merged with the roster,
    /// which holds the players who are here — what a save writes down.
    /// Loaded from the world's file when there is one, and kept regardless,
    /// so leaving and rejoining works even in a world nobody is keeping.
    remembered: Mutex<HashMap<Token, keeper::PlayerRecord>>,
    /// Every boat in the world, by its lasting name — entities of the world
    /// and never anybody's appendage: they outlive visits, lie at anchor
    /// while unoccupied, and change hands by [`ToServer::Board`].
    ///
    /// Lock order: a thread holding [`Shared::players`] may take this, and
    /// nothing holding this ever reaches for the roster. That one-way rule is
    /// why the pair cannot deadlock.
    pub(crate) boats: Mutex<HashMap<BoatId, BoatState>>,
    /// Every island anybody has claimed, by the identity of the ring that is
    /// it — see [`protocol::survey::Island::id`]. What a cairn stands for, and
    /// the whole of who may name what.
    ///
    /// Lock order: a leaf, like a player's survey, and held alone. Everything
    /// a grant needs from the roster is read and let go before this is taken,
    /// and everything anybody is told is posted after it has been let go
    /// again.
    claims: Mutex<HashMap<IVec2, Claim>>,
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
    /// nights waited out, and hours the console has asked to have over with.
    /// Only ever grows — the world's day gets older or it gets older faster,
    /// never younger, which is the promise [`ToClient::Daylight`] makes. The
    /// only state the day has beyond the clock, which is what keeps two
    /// askers from disagreeing about the time — see [`Shared::phase`].
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
    /// taken back up by the warden's first beat, and rewritten with the living
    /// flock on every beat after, so a save has something to write down
    /// whichever side of that beat it lands — see [`beasts::mind_the_beasts`].
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
    /// The boat this player occupies, if any — the roster's half of what
    /// [`BoatState::occupant`] says from the boat's side. The two are only
    /// ever written together, under the lock order [`Shared::boats`] sets.
    aboard: Option<BoatId>,
    /// When this player last asked for the night to be over, if they have —
    /// see [`ToServer::WantDawn`], which stands only for [`WAIT_LAPSE`].
    waiting_since: Option<Instant>,
    /// The way to this player's ear: their writer thread drains this onto
    /// their socket.
    outbox: mpsc::SyncSender<ToClient>,
    /// Every chunk this player has been near enough to look at — see
    /// [`survey_the_way`], the only thing that adds to it.
    ///
    /// The world's, not the client's: a claim is judged against a coastline
    /// somebody has actually closed, so which ground that is has to be a fact
    /// the server holds rather than one a client reports. Loaded from the
    /// world's memory of this token at the door and filed back there on the
    /// way out, so a voyage outlives the visit that made it. Behind a lock of
    /// its own — see [`Surveyed`].
    surveyed: Surveyed,
    /// What this player has found out about other people's claims, by the
    /// island each stands for — see [`Knowing`]. Deepened by
    /// [`sight_the_cairns`] and [`tell_the_cairn`] and by nothing else.
    ///
    /// Small where the survey is huge — one entry per cairn come near, against
    /// thousands of chunks — so it lives under the roster's own lock rather
    /// than behind one of its own.
    ///
    /// A player's own claims are deliberately *not* kept here. They are held by
    /// token in [`Shared::claims`] and are true whether or not this map says
    /// so, which is one fewer thing that can be lost: an entry missing from a
    /// hand-edited file costs a stranger's name, never somebody's island.
    known: HashMap<IVec2, Knowing>,
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

/// One island claimed, as the session holds it: whose it is, where their cairn
/// stands, and what they have christened it.
///
/// An island is claimed by a player who has sailed the whole way round it and
/// then stood on it — see [`settle_a_claim`], which is where that is judged
/// against the world's own survey of them rather than against anything a
/// client says.
#[derive(Clone)]
struct Claim {
    /// The token it belongs to, and never the player id: a claim outlives the
    /// visit that made it, where an id is good for one session. It is also a
    /// credential, so it never leaves this process — see
    /// [`ToClient::Cairn`]'s `yours`, which is all a client is told of it.
    by: Token,
    /// Where the claimant stood when they claimed it, which is where the cairn
    /// stands for good.
    at: Vec2,
    /// Empty for an island nobody has christened yet.
    name: String,
}

/// How well one player knows one cairn — the whole of presence-gated
/// knowledge, in two words.
///
/// Knowledge here is a fact about a *player*, not about a cairn, and it is the
/// third of the three things this world keeps per person, beside
/// [`Player::surveyed`] and [`Shared::claims`]. They are deliberately three and
/// not one: only a survey earns a claim, so what a player has *seen of a coast*
/// and what they have *heard about it* must never be able to stand in for each
/// other. A chart that let hearsay close a ring would let a player claim an
/// island by being told about it.
///
/// It only ever grows — sighting a cairn and sailing away does not unsee it —
/// so the ordering here is the whole of the merge: what somebody knows is the
/// deeper of what they knew and what standing where they are earns them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Knowing {
    /// The cairn has been seen standing — from [`CAIRN_SIGHT`] or nearer. The
    /// island is spoken for and the stones are on the chart; whose word is on
    /// them is not.
    Sighted,
    /// The cairn has been walked up to — within [`CAIRN_VISIT`] — and what is
    /// written on it read. This is the only way a stranger's name for an island
    /// reaches anybody.
    Visited,
}

/// One boat, as the session holds it: its file record's fields plus the
/// two things the file never keeps — whose hands are on the helm right now,
/// and whose hull it is.
pub(crate) struct BoatState {
    pub(crate) kind: BoatKind,
    pub(crate) position: Vec2,
    pub(crate) heading: f32,
    pub(crate) occupant: Option<PlayerId>,
    /// Whose hull this is: the last hands to hold its helm, or `None` for one
    /// nobody has been aboard this session. Set wherever a helm is taken up —
    /// minting, being handed a spare, resuming a kept boat, boarding — and
    /// never cleared, a hull somebody stepped out of still being theirs.
    ///
    /// A free hull whose keeper is not in the world is what an arrival is
    /// offered instead of a newly minted one — see `fresh_hull` in the join
    /// path for what that buys.
    ///
    /// On a rowing boat it answers a second question, the two being one
    /// fact: whose ship's boat is in the water. Both hoists ask this rather
    /// than a name remembered against the player, which is the whole point —
    /// a claim that did not travel with the hull went on meaning a boat
    /// somebody else had since rowed off in, taking a dinghy out from under
    /// its new keeper.
    ///
    /// It is not ownership, which boats do not have: anyone may take up a
    /// free hull and doing so makes it theirs. Nor is it one apiece — board
    /// two beached dinghies and both say they are yours, and a hoist takes
    /// whichever it finds first. Arbitrary and left so, only a free hull ever
    /// being taken.
    ///
    /// Session-local, and deliberately: the whole of what it guards is a
    /// keeper who is *here*, so that a boat a live player parked on a beach
    /// and walked away from cannot go out from under them. Nobody away is
    /// keeping anything — which is why a reopened world's hulls all start
    /// nobody's, and why a world kept over many sessions does not silently
    /// gather the hulls of everyone who ever visited it.
    pub(crate) keeper: Option<Token>,
}

impl BoatState {
    /// This hull as a client hears of it.
    ///
    /// Every telling of a boat goes through here, because what a client is
    /// told about a boat is one fact and not six: a telling that left a field
    /// behind would be a hull that quietly disagreed with itself at one end of
    /// the wire. The id is handed in rather than kept — a boat's lasting name
    /// is what the roster files it under, and a state does not carry its own
    /// key.
    fn told(&self, id: BoatId) -> ToClient {
        ToClient::Boat {
            id,
            kind: self.kind,
            position: self.position,
            heading: self.heading,
            occupant: self.occupant,
        }
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
    /// started: a bound server is a world with a door, and [`Server::spawn`]
    /// is what opens it. The world is ephemeral until [`Server::keeping_in`]
    /// or [`Server::keeping_at`] says otherwise.
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
                boats: Mutex::new(
                    record
                        .boats
                        .into_iter()
                        .map(|boat| {
                            (
                                boat.id,
                                BoatState {
                                    kind: boat.kind,
                                    position: boat.position,
                                    heading: boat.heading,
                                    // Occupancy is session state: everyone
                                    // stepped out of the record when the
                                    // world stopped, and steps back in at
                                    // the door — see the welcome.
                                    occupant: None,
                                    // Nobody's until somebody here takes it
                                    // up — see [`BoatState::keeper`]. Which
                                    // goes for the dinghies a file carries
                                    // too: nobody is keeping one across a
                                    // reopening, so none is anybody's tender
                                    // until it is taken up again.
                                    keeper: None,
                                },
                            )
                        })
                        .collect(),
                ),
                claims: Mutex::new(
                    record
                        .claims
                        .into_iter()
                        .map(|claim| {
                            (
                                claim.island,
                                Claim {
                                    by: claim.by,
                                    at: claim.at,
                                    name: claim.name,
                                },
                            )
                        })
                        .collect(),
                ),
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
    /// asks for otherwise, which is what the tests of the night do — and the
    /// menu, opening a world the game is hosting for itself.
    ///
    /// On a *reopened* world this winds the clock forward to the next
    /// occurrence of that hour rather than setting it, exactly as the
    /// console's `time` command would: a kept world's day only ever grows
    /// older — the promise [`ToClient::Daylight`] makes — and its weather,
    /// running on the same clock, moves on with it.
    pub fn opening_at(mut self, phase: f32) -> Self {
        // Dropped rather than clamped when it is not a number: a NaN is not an
        // hour that overshot, so there is no hour it was nearly asking for.
        // Letting one through costs the whole session — [`Shared::phase`]
        // would answer NaN for ever, and that is a night without a dawn.
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

    /// Serves on a thread of its own, and hands back the handle that ends it.
    ///
    /// The only way to serve, and so the only way to stop: every connection
    /// gets a thread of its own from handshake to hang-up, and dropping what
    /// this hands back is what ends the world — see [`Host`]. Both callers
    /// need that. A game hosts alongside its own frame loop and must stop when
    /// the player leaves; a dedicated server is asked to stop by a signal, and
    /// a world that could only be served forever would have nowhere to be
    /// written down when that came. See [`signals`].
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
        // can reach it. Taking the lock below publishes it, and a thread that
        // joins the roster does so under the same lock — so it either got
        // there first and is shut down here, or it arrives to find the flag
        // set. There is no third case.
        self.shared.stopping.store(true, Ordering::Relaxed);

        // Everyone still connected is blocked in a read that only their own
        // client could end, and their threads each hold a share of the state
        // this is trying to be the end of. Shutting their sockets down is a
        // server hanging up, which it is, and it is what lets those threads
        // reach the departure they would otherwise never get to.
        {
            let players = self.shared.players.held();
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
                let players = shared.players.held();
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

    // The closing save: this loop ends inside [`Host`]'s drop, so by the time
    // that returns, everything the session was is in the file. The lock is let
    // go here too rather than when the shared state finally drops — the worker
    // threads hold that state for a beat past the end, and a world left and
    // immediately reopened must not be refused by its own session's shadow.
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
            // A blocking `recv` borrows the receiver for the length of the
            // wait, so only one worker listens and the rest queue behind it.
            // That serialises the *handing out* — a mutex handoff against a
            // hundred milliseconds of generating an island — and the guard is
            // dropped before any of the work itself, which does run on all of
            // them at once.
            let request = {
                let queue = requests.held();
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
            let players = shared.players.held();
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
/// meaningfully changed, [`WIND_STEP`] of vector change covering a shift in
/// strength and one in bearing with a single test; the time is told on a beat
/// instead, being always changing. The one thing here that does more than
/// watch is the night — see [`Shared::run_off_the_night`].
///
/// The thread ends with the session and is deliberately not joined: unlike a
/// connection it holds nothing but a share of the state, and making every
/// [`Host`] drop wait out the last beat would slow every session's end for
/// nothing.
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

            // Gathered before the roster is locked, as every path into the
            // clock does: the day's lock is never taken by a thread already
            // holding the roster's, so the two have no order to disagree
            // about.
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
                let players = shared.players.held();
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
        let skipped = *self.skipped.held();
        self.started.elapsed().as_secs_f32() + skipped
    }

    /// The wind over this world right now. Asked rather than kept: the
    /// weather is a pure function of the seed and the world's age, so there
    /// is no cached state for two askers to disagree over — unless the
    /// console has taken the weather in hand, which *is* state, and then its
    /// order is the answer for everyone until it lets go.
    ///
    /// The age rather than the session's own clock on both counts: a kept
    /// world resumes the sky it closed under, and a crew at anchor till dawn
    /// has sat out some of the blow.
    fn wind(&self) -> Vec2 {
        let commanded = *self.commanded_wind.held();
        commanded.unwrap_or_else(|| world::weather::wind(self.world.seed(), self.age()))
    }

    /// Orders the wind, or — with `None` — gives the weather back to the
    /// world. The sky thread notices the answer to [`Shared::wind`] moving
    /// and tells everyone, exactly as it does when the real weather turns.
    fn command_wind(&self, wind: Option<Vec2>) {
        *self.commanded_wind.held() = wind;
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
        let mut skipped = self.skipped.held();
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
    /// The locks are taken one at a time, never nested, and the surveys are
    /// copied out after the roster has been let go: a save that copied
    /// thousands of chunks per player under it would stop everyone else's
    /// `Move` for the length of the file.
    fn record(&self) -> keeper::WorldRecord {
        // Everything about a present player the roster itself can say, and
        // beside it the handle to the one thing it cannot — the survey, whose
        // own lock is taken below with this one let go.
        let present: Vec<(Token, keeper::PlayerRecord, Surveyed)> = {
            let players = self.players.held();
            players
                .values()
                .map(|player| {
                    (
                        player.token,
                        keeper::PlayerRecord {
                            position: player.position,
                            aboard: player.aboard,
                            surveyed: Vec::new(),
                            known: filed(&player.known),
                        },
                        player.surveyed.clone(),
                    )
                })
                .collect()
        };
        let here: Vec<(Token, keeper::PlayerRecord)> = present
            .into_iter()
            .map(|(token, mut record, surveyed)| {
                record.surveyed = surveyed.held().charted().collect();
                (token, record)
            })
            .collect();
        let mut players = self.remembered.held().clone();
        players.extend(here);
        let boats = {
            let boats = self.boats.held();
            boats
                .iter()
                .map(|(id, boat)| keeper::BoatRecord {
                    id: *id,
                    kind: boat.kind,
                    position: boat.position,
                    heading: boat.heading,
                })
                .collect()
        };
        let claims = {
            let claims = self.claims.held();
            claims
                .iter()
                // Nothing the file could not be read back saying, on the terms
                // the beasts are held to — see `beasts::Flock::records`. A
                // guarantee rather than a fix: nothing granted can fail this,
                // but a claim the reader refuses would take the whole world
                // with it. See [`island_in_the_world`].
                .filter(|(island, claim)| island_in_the_world(**island) && reachable(claim.at))
                .map(|(island, claim)| keeper::ClaimRecord {
                    island: *island,
                    by: claim.by,
                    at: claim.at,
                    name: claim.name.clone(),
                })
                .collect()
        };
        let beasts = self.beasts.held().clone();
        keeper::WorldRecord {
            id: self.world_id,
            seed: self.world.seed(),
            name: self.name.clone(),
            opening: self.opening,
            age: self.age(),
            players,
            boats,
            claims,
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
            let players = self.players.held();
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
        *self.skipped.held() += run;
        true
    }

    /// Where a given player is put down. Everyone enters on the world's
    /// spawn point — open water just off the first island's coast, see
    /// [`Archipelago::spawn`] — but not on the same square metre: markers
    /// standing exactly on top of each other read as one player, and what a
    /// joined session has to show first is that there is somebody else here.
    ///
    /// The offset is the player's id run through two irrational strides — the
    /// golden angle for the bearing, a smaller one for how far out — so that
    /// arrivals land well apart without any of them leaving one small circle,
    /// however many ids a long-lived server has dealt.
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
    // is a stranger, dealt a fresh token on the spot. An unrecognised token is
    // not an offence — a world file lost, or a world rebuilt under the same
    // address, parts the two memories honestly.
    let (token, returning_to) = {
        let remembered = shared.remembered.held();
        match presented {
            Some(token) => match remembered.get(&token) {
                Some(record) => (token, Some(record.clone())),
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
        position: returning_to
            .as_ref()
            .map_or_else(|| shared.spawn_for(id), |record| record.position),
        token,
        aboard: None,
        waiting_since: None,
        // Where this player has already been, taken back up: the survey is
        // the world's memory of them and picks up where it left off.
        //
        // The chunks come back blank, the file keeping only where somebody
        // went — see [`tell_the_survey_so_far`], which works the ink out again
        // and fills it in here as it goes. So a returner's coastlines arrive
        // over the first seconds of their visit, which is the same window in
        // which their client has no chart drawn either.
        surveyed: Arc::new(Mutex::new({
            let mut survey = Survey::default();
            for chunk in returning_to.iter().flat_map(|record| &record.surveyed) {
                survey.record(*chunk, Soundings::default());
            }
            survey
        })),
        // And what they had found out about other people's islands, which
        // comes back whole: unlike the survey there is nothing to work out
        // again, the file keeping the knowing itself and not a place it can be
        // re-earned from.
        known: returning_to
            .as_ref()
            .map(|record| record.known.clone())
            .unwrap_or_default(),
        outbox,
        line: stream,
    };
    let mut returning = returning_to;

    // Onto the roster and then welcomed, under one hold of the lock.
    //
    // In that order, because a client's join is over the moment it reads its
    // welcome: a player welcomed before they were listed would, for that
    // instant, believe they are in a world the session cannot see them in to
    // hang up on them — and the end of a session is exactly when that instant
    // is unaffordable. Listing first costs the welcome nothing, everything
    // else that writes to a player taking this lock too.
    //
    // One hold, so that the newcomer hears exactly who is already here,
    // everyone else hears the newcomer, and no move slips between the two: a
    // `Moved` about a player a client has not met would be about nobody.
    //
    // The sky is asked for out here because the hour has a lock of its own —
    // see [`Shared::skipped`] — and asking with the roster held would be the
    // one path in the process that nested the two. A sky a few microseconds
    // old is the same sky.
    let wind = shared.wind();
    let phase = shared.phase();
    {
        let mut players = shared.players.held();

        // A world that has already ended has nobody to introduce and no way to
        // hear of anyone arriving now. Its drop set this before reaching for
        // the lock, so a connection that gets here afterwards finds it — and
        // returning drops the socket, which is the same hang-up the drop would
        // have delivered had this player made it onto the roster in time.
        if shared.stopping.load(Ordering::Relaxed) {
            return;
        }

        // A token already on the roster is one client's files opened twice.
        // The second arrival enters as a stranger rather than being refused:
        // a refusal would lock a player out of a world over a copied file,
        // where a fresh start merely puzzles them.
        if players.values().any(|other| other.token == player.token) {
            player.token = Token(keeper::mint());
            player.position = shared.spawn_for(id);
            // Including where the papers had been: the survey belongs to
            // whoever is still holding them rather than to both. Replaced
            // rather than emptied, so that nothing locks a survey with the
            // roster in hand — see [`Surveyed`].
            player.surveyed = Arc::new(Mutex::new(Survey::default()));
            player.known.clear();
            returning = None;
        }

        // Where this player enters, and at whose helm — the boats being world
        // entities with keepers rather than owners. A newcomer's story starts
        // aboard, on a sloop the world mints them. A returner who left at a
        // helm is seated back into that boat only if it still lies where they
        // left it and is nobody else's; otherwise somebody here has taken it
        // up in the meantime, and stepping ashore again is not letting go of
        // it. A fresh hull where they stood is the interim answer until there
        // is any other way to be on open water. A returner who left ashore
        // enters on their own feet — and is dealt a hull all the same if
        // nothing free lies within reach of where they stood.
        //
        // Inside the roster's hold, with the boats' lock nested under it —
        // the one nesting [`Shared::boats`]'s order allows — so the seat is
        // taken before anyone can be told about the boat it claims.
        // The way the hull a returner is seated at lies — what their view
        // is opened along, there being no entry island to face them at.
        let mut bow = None;
        {
            // The papers this arrival actually enters on, the duplicate above
            // having possibly dealt them fresh ones.
            let token = player.token;
            // Who is here — the one claim on a hull nobody is aboard.
            // Gathered once, rather than asked again of every hull the
            // search below walks past.
            let here: HashSet<Token> = players.values().map(|other| other.token).collect();
            let mut boats = shared.boats.held();
            let fresh_hull = |boats: &mut HashMap<BoatId, BoatState>, at: Vec2| {
                // A free hull nobody is keeping, lying where this arrival is
                // being put down, is theirs before any new one is minted —
                // see [`BoatState::keeper`] and [`SPARE_BERTH`]. It reads as
                // the world having a boat ready rather than conjuring one,
                // and it is the whole of what bounds the fleet: a client that
                // joins, steps ashore and hangs up in a loop is handed the
                // same hull every time round rather than leaving one behind
                // every time round, for the world to file and to post to
                // every future joiner.
                //
                // Nobody's counts the hull somebody left the world aboard,
                // which sounds like a claim and is not one: a returner is put
                // down where the world last saw them and seated at a helm
                // either way — their own if it is still lying there, see
                // [`KEPT_BERTH`], and one where they stand if it is not.
                // Holding hulls for the away would leave the plainer loop —
                // join, hang up, touch nothing — unbounded again.
                //
                // A sloop, and now that there are dinghies in the water that
                // has to be said: an arrival's story starts at a ship's helm,
                // never in a boat somebody rowed ashore and left on a beach.
                let handed_down = boats.iter_mut().find(|(_, boat)| {
                    boat.kind == BoatKind::Sloop
                        && boat.occupant.is_none()
                        && boat.position.distance(at) <= SPARE_BERTH
                        && boat.keeper.is_none_or(|keeper| !here.contains(&keeper))
                });
                if let Some((&boat, state)) = handed_down {
                    state.occupant = Some(id);
                    state.keeper = Some(token);
                    return (boat, state.position);
                }
                let boat = BoatId(keeper::mint());
                boats.insert(
                    boat,
                    BoatState {
                        kind: BoatKind::Sloop,
                        position: at,
                        heading: aimed(at, shared.facing),
                        occupant: Some(id),
                        keeper: Some(token),
                    },
                );
                (boat, at)
            };
            player.aboard = match &returning {
                None => {
                    let (boat, at) = fresh_hull(&mut boats, player.position);
                    player.position = at;
                    Some(boat)
                }
                Some(record) => match record.aboard {
                    // Left afoot, with something free lying where they stood:
                    // they walk to it, and the world adds nothing.
                    None if boats.values().any(|boat| {
                        boat.occupant.is_none()
                            && boat.position.distance(record.position) <= KEPT_BERTH
                    }) =>
                    {
                        None
                    }
                    // Left afoot with nothing there, which is a player on an
                    // island who cannot leave it. Every other state in this
                    // world can be undone by playing on; being marooned is the
                    // one that cannot, and it is reachable without anybody
                    // cheating — boats have keepers and not owners, so the
                    // dinghy somebody beached and logged off beside is one
                    // another player may honestly row away while they are
                    // gone. So they are dealt a hull, the same way an arrival
                    // is. What keeps that from repeating on one beach is the
                    // arm above rather than anything here: a player dealt one
                    // leaves at a helm, and a helm is resumed rather than
                    // re-dealt.
                    //
                    // Where they stood, which for somebody who rowed ashore is
                    // the waterline, and inland for somebody who walked. A
                    // hull the ground turns up underneath is a case the client
                    // already sails out of — every way down to the sea is
                    // downhill, see `boat::grounding` — so an unlucky mint is
                    // an ungainly launch and not a second strand.
                    //
                    // What this does not do is bound the minting a handshake
                    // can already ask for, and it does not widen it either. A
                    // stranger presenting no papers is dealt a hull too, and
                    // one who sails it a boat's length before hanging up
                    // leaves a permanent one behind every time. This is that
                    // rate, not a new one.
                    None => {
                        let (boat, at) = fresh_hull(&mut boats, record.position);
                        player.position = at;
                        bow = boats.get(&boat).map(|state| state.heading);
                        Some(boat)
                    }
                    Some(kept) => match boats.get_mut(&kept) {
                        Some(boat)
                            if boat.occupant.is_none()
                                && boat.position.distance(record.position) <= KEPT_BERTH
                                && boat.keeper.is_none_or(|keeper| !here.contains(&keeper)) =>
                        {
                            boat.occupant = Some(id);
                            // Which is also what re-claims a dinghy somebody
                            // resumes in: taking a hull up is taking it up,
                            // whichever door it happens through. See
                            // [`BoatState::keeper`].
                            boat.keeper = Some(token);
                            player.position = boat.position;
                            bow = Some(boat.heading);
                            Some(kept)
                        }
                        _ => {
                            let (boat, at) = fresh_hull(&mut boats, record.position);
                            player.position = at;
                            bow = boats.get(&boat).map(|state| state.heading);
                            Some(boat)
                        }
                    },
                },
            };
        }

        let welcome = ToClient::Welcome {
            id,
            spawn: player.position,
            // A returning player is not put down beside the entry island,
            // so its centre is nothing to turn their view towards. Seated
            // at a helm, they open looking the way the hull lies — the view
            // behind their own bow. Afoot, their own position names no
            // direction, which leaves the bearing to the client, exactly as
            // a world with no island to look at does.
            facing: match (returning.is_some(), bow) {
                (false, _) => shared.facing,
                (true, Some(heading)) => {
                    player.position + Vec2::new(-heading.sin(), -heading.cos()) * 64.0
                }
                (true, None) => player.position,
            },
            token: player.token,
            aboard: player.aboard,
        };
        let arrival = ToClient::Joined {
            id,
            position: player.position,
        };
        let seated = player.aboard;
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
        // Every boat there is, the hulls being as much of the scene as the
        // players — and everyone else hears about the one this arrival
        // minted or took up, under the same hold as the arrival itself.
        {
            let boats = shared.boats.held();
            for (boat, state) in boats.iter() {
                post(newcomer, state.told(*boat));
            }
            if let Some(boat) = seated {
                broadcast(&players, id, boats[&boat].told(boat));
            }
        }
        broadcast(&players, id, arrival);
    }
    (shared.report)(&format!("{id} joined"));

    // The chart, once the world itself has been handed over: first everything
    // this player had surveyed before — nothing at all, for a newcomer — and
    // then whatever is within sight of where they have been put down, which
    // for a returner is usually nothing new either. Outside the roster's hold,
    // both of them, because both mean asking the world for ground.
    let mut wake = {
        let players = shared.players.held();
        Wake::opening(
            players
                .get(&id)
                .map_or(Vec2::ZERO, |player| player.position),
        )
    };
    tell_the_survey_so_far(&shared, id);
    // And the cairns they had already found out about, with their own among
    // them wherever those stand — a claim is a thing its holder is entitled to
    // know they still hold. Outside the roster's hold, the claims being a leaf
    // lock nothing may reach for with the roster in hand.
    tell_the_cairns_about(&shared, id);
    // Then a way that goes nowhere, which is what standing where you were put
    // down is: what it earns is the ground within sight of that spot, and any
    // cairn standing in it that this player did not already know of. After the
    // memory rather than before it, so that a cairn arriving twice arrives the
    // second time saying the more, not the less.
    let put_down = wake.at;
    follow_the_way(&shared, id, &mut wake, put_down);

    // When this player last had a cairn raised, and when they last had one
    // rewritten — see [`CAIRN_PACE`]. A connection's own locals, like the
    // [`Wake`]: they are about the rate this thread is being asked to work at,
    // and nothing shared has business with them. Nothing yet, so the first ask
    // of a session is answered at once.
    let mut asked_to_claim: Option<Instant> = None;
    let mut asked_to_name: Option<Instant> = None;

    // Relay and take orders for ground until the line drops. Anything else
    // ends the session too: after a framing error nothing later on the stream
    // can be trusted, a second hello is a client that has lost its place, and
    // a position or a chunk no player could be at is one to hang up over
    // rather than to answer.
    loop {
        match ToServer::read(&mut reader) {
            Ok(ToServer::Move { position }) if reachable(position) => {
                let walked = {
                    let mut players = shared.players.held();
                    // Quietly ignored from a player at a helm, on Helm's own
                    // terms: a `Move` can honestly cross a boarding grant on
                    // the wire, and believing it would walk the player away
                    // from a boat everyone else sees them steering.
                    let afoot = players
                        .get_mut(&id)
                        .filter(|player| player.aboard.is_none());
                    let walked = afoot.is_some();
                    if let Some(player) = afoot {
                        player.position = position;
                        broadcast(&players, id, ToClient::Moved { id, position });
                    }
                    walked
                };
                // Outside the hold, the survey meaning ground to work out —
                // and only for a move that was believed, or a report the
                // session ignored would still put ink on somebody's chart.
                if walked {
                    follow_the_way(&shared, id, &mut wake, position);
                }
            }
            Ok(ToServer::Helm { position, heading })
                if reachable(position) && heading.is_finite() =>
            {
                let sailed = {
                    let mut players = shared.players.held();
                    // The rider goes with the vehicle: one report moves both.
                    // Quietly ignored from a player occupying nothing — see the
                    // wire's own doc for how that happens honestly.
                    let steering = players
                        .get_mut(&id)
                        .and_then(|player| player.aboard.inspect(|_| player.position = position));
                    if let Some(boat) = steering {
                        let told = {
                            let mut boats = shared.boats.held();
                            let state = boats.get_mut(&boat).expect("a boat once boarded exists");
                            state.position = position;
                            state.heading = heading;
                            state.told(boat)
                        };
                        broadcast(&players, id, told);
                    }
                    steering.is_some()
                };
                // The coast a voyage runs past, on the terms a walk's is —
                // and this is the report the way between two of them exists
                // for, a hull under sail covering ground a walker cannot.
                if sailed {
                    follow_the_way(&shared, id, &mut wake, position);
                }
            }
            Ok(ToServer::Board { boat }) => {
                let mut players = shared.players.held();
                let Some(player) = players.get_mut(&id) else {
                    break;
                };
                let (answer, granted, hoisted) = {
                    let mut boats = shared.boats.held();
                    // A name no boat answers to is answered with silence, and
                    // not — as it once was — with a hang-up. Ids are retired
                    // now: a tender hoisted in is gone the instant its crew
                    // steps aboard the ship, and any client that heard of it
                    // may honestly ask after it in the same breath, its
                    // [`ToClient::BoatGone`] still on the wire. There is no
                    // state to answer with either way, and a refusal that
                    // says nothing is what every other ungranted boarding
                    // sounds like.
                    let Some(state) = boats.get(&boat) else {
                        continue;
                    };
                    // What the asker would be stepping aboard from: their own
                    // feet, or — for a ship's helm only — the thwarts of a
                    // rowing boat laid alongside, which the grant then hoists
                    // back in. Any other helm refuses the ask: ships do not
                    // board ships.
                    let stepping_from = match player.aboard {
                        None => Some(None),
                        Some(tender) => (tender != boat
                            && state.kind == BoatKind::Sloop
                            && boats
                                .get(&tender)
                                .is_some_and(|held| held.kind == BoatKind::Rowboat))
                        .then_some(Some(tender)),
                    };
                    // Granted only beside an empty helm; anything else leaves
                    // the boat as it was, and the state is the whole of the
                    // answer either way.
                    let granted = stepping_from.is_some()
                        && state.occupant.is_none()
                        && state.position.distance(player.position) <= BOARD_GRANT;
                    let mut hoisted = None;
                    if granted {
                        let state = boats.get_mut(&boat).expect("looked up a breath ago");
                        state.occupant = Some(id);
                        // Theirs from here on, and still theirs when they step
                        // off — see [`BoatState::keeper`]. A dinghy taken up
                        // is the taker's tender from that moment, whoever put
                        // it in the water: the claim travels with the hull,
                        // so the last hands on it are the ones the hoists
                        // answer to.
                        state.keeper = Some(player.token);
                        player.aboard = Some(boat);
                        player.position = state.position;
                        // The rowing boat goes back aboard as the ship's own:
                        // out of the world, its name retired with it — and
                        // with it whatever claim it carried.
                        if let Some(Some(tender)) = stepping_from {
                            boats.remove(&tender);
                            hoisted = Some(tender);
                        }
                    }
                    (
                        boats.get(&boat).expect("never removed").told(boat),
                        granted,
                        hoisted,
                    )
                };
                // Where a granted boarding has put them, kept for the survey
                // below while the roster is still in hand.
                let stepped = granted.then_some(player.position);
                // A grant is news for everyone, the asker included — the
                // telling is what seats them. A refusal changed nothing and
                // is news only to the one who asked: answered to them alone,
                // or a client could make the whole roster's outboxes carry
                // its pestering of a distant helm.
                if granted {
                    broadcast_all(&players, answer);
                } else if let Some(player) = players.get(&id) {
                    post(player, answer);
                }
                // The tender's going is told after the boarding that took its
                // crew, so nobody — the asker least of all — hears of a hull
                // vanishing under a player still seated in it.
                if let Some(tender) = hoisted {
                    broadcast_all(&players, ToClient::BoatGone { id: tender });
                }
                drop(players);
                // A grant moves the player onto the hull — a stride, never
                // more than [`BOARD_GRANT`] — and a stride can still bring
                // ground into sight.
                if let Some(aboard) = stepped {
                    follow_the_way(&shared, id, &mut wake, aboard);
                }
            }
            Ok(ToServer::Disembark { position }) if reachable(position) => {
                let mut stepped = false;
                let mut players = shared.players.held();
                if let Some(player) = players.get_mut(&id) {
                    // Ignored when not aboard, on Helm's terms.
                    if let Some(boat) = player.aboard.take() {
                        stepped = true;
                        player.position = position;
                        let told = {
                            let mut boats = shared.boats.held();
                            let state = boats.get_mut(&boat).expect("a boat once boarded exists");
                            // The occupant goes and the keeper stays: a hull
                            // parked ashore is still theirs while they are
                            // here, and is only handed on once they have left
                            // the world — see [`BoatState::keeper`].
                            state.occupant = None;
                            state.told(boat)
                        };
                        broadcast_all(&players, told);
                        broadcast(&players, id, ToClient::Moved { id, position });
                    }
                }
                drop(players);
                // A step ashore is a step, and the shore is exactly the place
                // a step of it can be worth surveying.
                if stepped {
                    follow_the_way(&shared, id, &mut wake, position);
                }
            }
            Ok(ToServer::Lower { position, heading })
                if reachable(position) && heading.is_finite() =>
            {
                let mut players = shared.players.held();
                let Some(player) = players.get_mut(&id) else {
                    break;
                };
                let granted = {
                    let mut boats = shared.boats.held();
                    // Granted to a player at the helm of a boat that carries
                    // one, and only alongside it — a tender is lowered over
                    // the side, not sent across the bay. Anything else is
                    // answered with the usual silence.
                    let ship = player.aboard.filter(|aboard| {
                        boats.get(aboard).is_some_and(|state| {
                            state.kind == BoatKind::Sloop
                                && state.position.distance(position) <= BOARD_GRANT
                        })
                    });
                    match ship {
                        Some(ship) => {
                            // The ship is left at anchor for anyone, exactly
                            // as a disembark leaves it.
                            let ship_told = {
                                let state = boats.get_mut(&ship).expect("looked up a breath ago");
                                state.occupant = None;
                                // Still theirs, lying at anchor with nobody
                                // aboard — see [`BoatState::keeper`], which
                                // the helm they are stepping off does not
                                // give up.
                                state.keeper = Some(token);
                                state.told(ship)
                            };
                            // A rowing boat already lying free where this one
                            // is going *is* the boat being lowered: stepped
                            // down into rather than minted alongside. Which
                            // is one of the two things bounding the fleet on
                            // this path — a client looping lower, step out,
                            // board, lower is handed the same hull every
                            // time, exactly as the join loop is handed the
                            // same free sloop, see [`BoatState::keeper`].
                            // A dinghy somebody else left floating there is
                            // fair game for the same reason a free helm is:
                            // boats have keepers, not owners.
                            let alongside = boats
                                .iter()
                                .find(|(_, boat)| {
                                    boat.kind == BoatKind::Rowboat
                                        && boat.occupant.is_none()
                                        && boat.position.distance(position) <= BOARD_GRANT
                                })
                                .map(|(&found, _)| found);
                            let tender = alongside.unwrap_or_else(|| BoatId(keeper::mint()));
                            // Written whole rather than settled field by
                            // field, so the minted hull and the re-used one
                            // leave here in the same shape by construction:
                            // there is no reading of this that has to argue
                            // which fields a re-use carries over. The keeper
                            // above all — a tender is somebody's doing from
                            // the moment it touches the water, and this is
                            // the word both hoists go looking for.
                            let state = BoatState {
                                kind: BoatKind::Rowboat,
                                position,
                                heading,
                                occupant: Some(id),
                                keeper: Some(token),
                            };
                            let tender_told = state.told(tender);
                            boats.insert(tender, state);
                            // And the other thing bounding it: one boat in
                            // the water each. Whatever else in the world is
                            // this player's tender is hoisted out of it as
                            // this one goes in — otherwise a client that
                            // lowers, steps out, boards and sails on leaves a
                            // hull behind every time, and every one of them
                            // is written to the world file and posted to
                            // every future joiner. Asked of the hulls rather
                            // than of a name the player remembers, so that a
                            // dinghy somebody else has taken up since is no
                            // longer theirs to hoist — see
                            // [`BoatState::keeper`]. Only while it still lies
                            // free, too: a hull somebody is sitting in is
                            // theirs until they step out of it.
                            let hoisted = boats
                                .iter()
                                .find(|(&held, state)| {
                                    held != tender
                                        && state.kind == BoatKind::Rowboat
                                        && state.keeper == Some(token)
                                        && state.occupant.is_none()
                                })
                                .map(|(&held, _)| held);
                            if let Some(last) = hoisted {
                                boats.remove(&last);
                            }
                            player.aboard = Some(tender);
                            player.position = position;
                            Some((tender_told, ship_told, hoisted))
                        }
                        None => None,
                    }
                };
                if let Some((tender_told, ship_told, hoisted)) = &granted {
                    // The rowing boat first — that telling is what seats the
                    // asker — and only then the ship it stepped down from,
                    // so no client ever holds a helm the world has already
                    // given away. The hull the world took back last of all,
                    // on the same rule the hoist of a tender goes by: nobody
                    // hears of a boat vanishing before they have heard where
                    // its crew went.
                    broadcast_all(&players, tender_told.clone());
                    broadcast_all(&players, ship_told.clone());
                    if let Some(last) = hoisted {
                        broadcast_all(&players, ToClient::BoatGone { id: *last });
                    }
                }
                drop(players);
                // Stepping down moves the player at most a boat-length, and
                // a boat-length can still bring ground into sight.
                if granted.is_some() {
                    follow_the_way(&shared, id, &mut wake, position);
                }
            }
            Ok(ToServer::Claim { island }) => {
                // Paced by waiting out what is left of [`CAIRN_PACE`] rather
                // than by dropping the ask: a client that asks too soon is
                // answered late, never with silence, which would leave an
                // honest one unable to tell a refusal from a message that went
                // nowhere. The waiting is on this connection's own thread, so
                // the only session it slows is the one asking.
                //
                // The clock is set from what came back rather than from having
                // asked, because what is being rationed is the walk. An ask
                // answered off the roster or the claims alone cost nothing,
                // and charging it would mean a player who asked from the deck
                // and then stepped ashore waited for no reason.
                wait_out(asked_to_claim, CAIRN_PACE);
                if settle_a_claim(&shared, id, island) {
                    asked_to_claim = Some(Instant::now());
                }
            }
            Ok(ToServer::Name { island, name }) => {
                // Paced on the claim's terms — see [`CAIRN_PACE`]. What pays is
                // a christening that was granted, that being the one that goes
                // to everybody near the cairn; a refusal reaches its asker and
                // nobody else.
                wait_out(asked_to_name, CAIRN_PACE);
                if christen(&shared, id, island, &name) {
                    asked_to_name = Some(Instant::now());
                }
            }
            Ok(ToServer::WantDawn) => {
                // Noted rather than acted on: whether the night actually
                // runs depends on what everyone else wants — see
                // [`Shared::run_off_the_night`], which is where the sky
                // thread reads this.
                let mut players = shared.players.held();
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
                let served = console::interpret(&shared, id, &line);
                (shared.report)(&format!("{id}: {line}"));
                {
                    let players = shared.players.held();
                    if let Some(player) = players.get(&id) {
                        post(player, ToClient::Reply { text: served.reply });
                    }
                }
                // A line that moved them moved them somewhere the survey has
                // never been, and the way there is not a voyage: the wake
                // reopens where they now stand, exactly as it does for
                // somebody coming through the door, and the ground within
                // sight of the new place is theirs on the spot. Followed to
                // itself for the same reason entry follows a way that goes
                // nowhere.
                //
                // A `Move` or a `Helm` this client had already put on the
                // wire from the old place arrives after all this and is
                // believed — nothing here can tell a stale report from a
                // fresh one, and inventing a sequence number to tell them
                // apart would be a wire change to fix a debugging command.
                // So the roster rewinds, the hull with it, and the wake is
                // charged one run back the way it came. Known, and it
                // settles itself: the client is drawing from the put down by
                // then, so its next report is from the new place and puts
                // everything back. What it costs meanwhile is bounded by
                // [`SURVEY_SWEEP`] — one sweep of ground surveyed towards
                // somewhere nobody is, and the first honest report finding
                // it already charted.
                if let Some(put) = served.put {
                    wake = Wake::opening(put);
                    follow_the_way(&shared, id, &mut wake, put);
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
    // momentarily in neither would be a position lost. Nothing can move it
    // meanwhile: only this thread's read loop ever did, and it is over.
    let last_seen = {
        let players = shared.players.held();
        players.get(&id).map(|player| {
            (
                player.token,
                player.position,
                player.aboard,
                player.surveyed.clone(),
                filed(&player.known),
            )
        })
    };
    // The survey copied out with the roster let go, as the saves do it: a
    // departure that gathered thousands of chunks under that lock would be
    // one player leaving and everybody else's report waiting on it. See
    // [`Surveyed`].
    let leaving = last_seen.map(|(token, position, aboard, surveyed, known)| {
        (
            token,
            keeper::PlayerRecord {
                position,
                aboard,
                surveyed: surveyed.held().charted().collect(),
                known,
            },
        )
    });
    if let Some((token, record)) = &leaving {
        shared.remembered.held().insert(*token, record.clone());
    }
    {
        let mut players = shared.players.held();
        players.remove(&id);
        broadcast(&players, id, ToClient::Left { id });
        // The helm they held is anyone's now: an offline player's boat lies
        // at anchor, visible and takeable, and being seated back into it on
        // return is a memory rather than a hold. Told under the same hold
        // as the departure, so nobody hears of a free boat before its
        // keeper has left.
        // The papers they held and the helm they left at: the first is what
        // says which hulls out there are theirs, now that a claim lives on
        // the boat — see [`BoatState::keeper`].
        let (papers, helm) =
            leaving.map_or((None, None), |(token, record)| (Some(token), record.aboard));
        let (told, hoisted) = {
            let mut boats = shared.boats.held();
            if let Some(state) = helm.and_then(|boat| boats.get_mut(&boat)) {
                state.occupant = None;
            }
            // A ship's boat goes back aboard when its keeper leaves the world
            // at some *other* helm — the half of the one-boat-in-the-water
            // bound that survives a reconnect. Without it a client loops
            // lower, step out, board, hang up, leaving a permanent hull behind
            // every handshake.
            //
            // Both conditions are exceptions that would otherwise strand
            // somebody. Not afoot: a player who left on their own feet has
            // their ship anchored past wading depth and the dinghy on the
            // beach is how they get out to it. And not the tender itself:
            // hanging up while rowing is the ordinary way to stop mid-passage,
            // and hoisting the boat they are sitting in leaves the entry block
            // dealing them a fresh sloop per handshake. Whoever comes to
            // simplify this to hoisting unconditionally is re-creating both,
            // so: don't.
            //
            // Asked of the hulls rather than of a name the leaver remembered —
            // see [`BoatState::keeper`], which is what makes "theirs" survive
            // somebody else rowing off in it — and only while it lies free.
            let hoisted = helm.zip(papers).and_then(|(held, papers)| {
                boats
                    .iter()
                    .find(|(&boat, state)| {
                        boat != held
                            && state.kind == BoatKind::Rowboat
                            && state.keeper == Some(papers)
                            && state.occupant.is_none()
                    })
                    .map(|(&boat, _)| boat)
            });
            if let Some(boat) = hoisted {
                boats.remove(&boat);
            }
            // The freed helm is never the hull just hoisted — the find above
            // passes over it — so this says a hull lies free without any risk
            // of contradicting the going that follows.
            let told = helm.and_then(|boat| boats.get(&boat).map(|state| state.told(boat)));
            (told, hoisted)
        };
        if let Some(told) = told {
            broadcast_all(&players, told);
        }
        // Last of all, on the rule the lowering's own hoist goes by: nobody
        // hears of a boat vanishing before they have heard where its crew
        // went.
        if let Some(boat) = hoisted {
            broadcast_all(&players, ToClient::BoatGone { id: boat });
        }
    }
    (shared.report)(&format!("{id} left"));
}

/// Settles a claim: grants it if the world's own record says this player has
/// earned the island, and tells whoever can see the answer either way.
///
/// The three things a grant wants are all facts the server holds. The claimant
/// must be afoot, a cairn being built by somebody standing on the ground. The
/// server's survey *for that player* must answer [`Survey::island_under`] with
/// the island they named, which is both halves of the rule in one question —
/// an unclosed coast rings nothing, and a point outside a ring is somewhere
/// else. And nobody may hold it already, first asker taking it.
///
/// A refusal is posted to the asker alone and never broadcast: one every other
/// player's outbox carried would be a client able to pester the whole roster.
/// Where the refusal is that somebody got there first, what goes back is that
/// cairn, if the asker is near enough to be looking at it — see
/// [`tell_the_asker`]. Otherwise the answer is silence.
///
/// The claims are asked *before* the coastlines are walked, and that order is
/// what stops a pestering client: an island somebody already holds is settled
/// by the claims alone, and the walk is what the pace exists to ration. Asked
/// twice, which is not a race — the second hold is the one that decides, so
/// two players walking the same shore still leave one cairn.
///
/// Says whether the survey was walked, which is the expensive half and the
/// only half worth pacing.
fn settle_a_claim(shared: &Shared, id: PlayerId, island: IVec2) -> bool {
    let Some((token, at, afoot, surveyed)) = ({
        let players = shared.players.held();
        players.get(&id).map(|player| {
            (
                player.token,
                player.position,
                player.aboard.is_none(),
                player.surveyed.clone(),
            )
        })
    }) else {
        return false;
    };

    // Somebody's already — theirs or another's, and either way the cairn that
    // stands there is the answer and no coastline needs walking to find it.
    let held = shared.claims.held().get(&island).cloned();
    if let Some(claim) = held {
        let players = shared.players.held();
        tell_the_asker(&players, id, island, &claim);
        return false;
    }
    // Nor for somebody who could not be building a cairn wherever they are:
    // one is built by a player standing on the ground, not sailing past.
    if !afoot {
        return false;
    }

    let stands_on = surveyed
        .held()
        .island_under(at)
        .is_some_and(|found| found.id == island);

    let (told, granted) = {
        let mut claims = shared.claims.held();
        match claims.get(&island) {
            // Claimed while this ask was walking the shore, which is the only
            // way it gets here: the answer is that cairn, as it would have
            // been a moment earlier.
            Some(held) => (Some(held.clone()), false),
            // Earned, and the identity is one the world's own file can carry
            // back — see [`island_in_the_world`]. A ring closed out at the
            // very brink of the world is refused rather than granted and lost,
            // a claim the reader cannot read being a world that will not open.
            None if stands_on && island_in_the_world(island) => {
                let raised = Claim {
                    by: token,
                    at,
                    name: String::new(),
                };
                claims.insert(island, raised.clone());
                (Some(raised), true)
            }
            // Nothing there, and nothing earned: an island this player has
            // not been round, or is not standing on.
            None => (None, false),
        }
    };

    if let Some(claim) = told {
        let mut players = shared.players.held();
        if granted {
            (shared.report)(&format!("{id} claimed an island"));
            tell_the_cairn(&mut players, island, &claim, id);
        } else {
            tell_the_asker(&players, id, island, &claim);
        }
    }
    // The shore was walked: everything that gets this far is an afoot asker
    // after an island nobody held.
    true
}

/// Christens a claimed island, if the asker is the one holding the claim.
///
/// A name is the world's rather than one client's notebook: it rides with the
/// cairn, so everybody who passes reads the same word. Which is why it is
/// earned the way the island was — only the claimant may write it, and a name
/// the wire will not carry (see [`protocol::island_name`]) is a refusal rather
/// than an erasure, leaving the cairn saying whatever it said before. An island
/// nobody has claimed has no state to answer with, so that ask is met with
/// silence.
///
/// Says whether the christening was granted, which is the half that reaches
/// anybody but the asker and so the half worth pacing.
fn christen(shared: &Shared, id: PlayerId, island: IVec2, name: &str) -> bool {
    let Some(token) = ({
        let players = shared.players.held();
        players.get(&id).map(|player| player.token)
    }) else {
        return false;
    };

    let (told, granted) = {
        let mut claims = shared.claims.held();
        let Some(claim) = claims.get_mut(&island) else {
            return false;
        };
        match protocol::island_name(name) {
            // The name it already had is not news: a client repeating itself
            // should not read as the cairn changing. This suppresses a
            // repetition and nothing more — a client alternating two good
            // names passes it every time — so what holds the rate down is
            // [`CAIRN_PACE`].
            Some(written) if claim.by == token && written != claim.name => {
                claim.name = written;
                (claim.clone(), true)
            }
            // Somebody else's island, or nothing anybody could call a name:
            // the cairn goes back to the asker exactly as it stands.
            _ => (claim.clone(), false),
        }
    };

    let mut players = shared.players.held();
    if granted {
        tell_the_cairn(&mut players, island, &told, id);
    } else {
        tell_the_asker(&players, id, island, &told);
    }
    granted
}

/// Tells a joining player back every cairn they already knew of, each at the
/// depth they knew it — and every one of their own, however far away it stands.
///
/// The two rules are not the same rule. What somebody knows of other people's
/// cairns is [`Knowing`]'s business, earned by going there in some earlier hour
/// of the world, and comes back exactly as it was left. A player's own claim is
/// theirs to know outright: they are the only one who may write on it, and
/// there is no other way for them to hear of it again — a client not told draws
/// its own island blank and will not open the pen on it.
///
/// What this deliberately does *not* do is look at where the player has been
/// put down: standing somewhere is worth what standing there is worth to
/// anybody, which is [`sight_the_cairns`]'s business and run on the spot.
///
/// The locks are taken one at a time in the order [`Shared::claims`] sets.
fn tell_the_cairns_about(shared: &Shared, id: PlayerId) {
    let Some((token, known)) = ({
        let players = shared.players.held();
        players
            .get(&id)
            .map(|player| (player.token, player.known.clone()))
    }) else {
        return;
    };
    let worth_telling: Vec<(IVec2, Claim)> = {
        let claims = shared.claims.held();
        claims
            .iter()
            .filter(|(island, claim)| knows(&known, token, **island, claim).is_some())
            .map(|(island, claim)| (*island, claim.clone()))
            .collect()
    };
    if worth_telling.is_empty() {
        return;
    }
    let players = shared.players.held();
    let Some(player) = players.get(&id) else {
        return;
    };
    for (island, claim) in worth_telling {
        post(player, cairn_told_to(player, island, &claim));
    }
}

/// What one player knows of one cairn as things stand: what they have been
/// told of it, or the whole of it if the claim is their own.
///
/// The one place the claimant's rule is written. A player's own claims are
/// deliberately not kept in [`Player::known`], so every reader of that map has
/// to lay the same rule over it, and every reader does it here.
///
/// Takes the map rather than the player because one caller has only a copy of
/// it: the join reads the knowing under the roster's lock, lets the roster go,
/// and asks this while holding the claims.
fn knows(
    known: &HashMap<IVec2, Knowing>,
    token: Token,
    island: IVec2,
    claim: &Claim,
) -> Option<Knowing> {
    if claim.by == token {
        return Some(Knowing::Visited);
    }
    known.get(&island).copied()
}

/// Brings one player's knowledge of one cairn up to what being `near` metres
/// from it earns, and says whether that is any deeper than what they knew a
/// moment ago.
///
/// The merge is [`Knowing`]'s ordering and nothing else: the deeper of what
/// they knew and what they have just earned, never the newer. Sailing away from
/// a cairn does not unlearn it, so this only ever climbs.
///
/// Whether it climbed is the whole of the answer, and it is what spares the
/// caller looking the entry up itself — this is asked of every claim in the
/// world on every position report.
///
/// A claimant knows their own outright without it being written down — see
/// [`knows`] — so nothing is written and nothing deepened, which is why
/// [`sight_the_cairns`] passes over a player's own cairns rather than finding
/// one perpetually newsworthy.
fn learns(player: &mut Player, island: IVec2, claim: &Claim, near: f32) -> bool {
    if claim.by == player.token {
        return false;
    }
    let earned = if near <= CAIRN_VISIT {
        Some(Knowing::Visited)
    } else if near <= CAIRN_SIGHT {
        Some(Knowing::Sighted)
    } else {
        None
    };
    let held = player.known.get(&island).copied();
    let now = earned.max(held);
    // Written back only when it is deeper, so that the common case — a player
    // standing about beside a cairn they already know — is a lookup and no
    // write at all.
    if now <= held {
        return false;
    }
    player
        .known
        .insert(island, now.expect("deeper than nothing is something"));
    true
}

/// Takes down whatever cairns the way from `from` to `to` brought within reach,
/// and tells this player about the ones that are news to them.
///
/// The other half of what a position report earns, alongside the survey, and
/// run on the same terms: over the way rather than its end, because a hull
/// between two reports must no more slip past a cairn than past a coast. A run
/// can be most of [`SURVEY_SWEEP`] long, which is longer than [`CAIRN_SIGHT`],
/// so an endpoint test would let a cairn passed at speed go unseen — silently,
/// and only sometimes.
///
/// Told only when it is news, which is the whole reason [`Player::known`]
/// exists: a player parked beside a cairn reports ten times a second, and each
/// of those would otherwise be a message about a pillar of stone that has not
/// moved.
///
/// A walk of every claim in the world per report — bounded by how many islands
/// anybody has claimed rather than by how far anybody has sailed — and a copy
/// of every claim in reach, carved name and all. The copy is the price of the
/// lock order: what counts as news lives under the roster's lock, and reaching
/// for the roster with the claims held is what [`Shared::claims`] forbids. So
/// the claims are read, let go, and only then judged.
fn sight_the_cairns(shared: &Shared, id: PlayerId, from: Vec2, to: Vec2) {
    let within_reach: Vec<(IVec2, Claim, f32)> = {
        let claims = shared.claims.held();
        claims
            .iter()
            .map(|(island, claim)| (*island, claim, off_the_way(claim.at, from, to)))
            .filter(|(_, _, near)| *near <= CAIRN_SIGHT)
            .map(|(island, claim, near)| (island, claim.clone(), near))
            .collect()
    };
    if within_reach.is_empty() {
        return;
    }

    let mut players = shared.players.held();
    let Some(player) = players.get_mut(&id) else {
        return;
    };
    let theirs = player.token;
    for (island, claim, near) in within_reach {
        // Their own passed over: [`learns`] deliberately writes nothing down
        // for it, so without this it would be news on every report they make.
        if claim.by == theirs {
            continue;
        }
        if learns(player, island, &claim, near) {
            post(player, cairn_told_to(player, island, &claim));
        }
    }
}

/// What one player's knowledge of the cairns goes into the world's file as.
///
/// Nothing the file could not be read back saying, on the terms the claims
/// themselves are held to — see [`Shared::record`], and [`island_in_the_world`]
/// for what it costs to get this wrong. Nothing that gets in here can fail it,
/// every entry being about a claim that passed the same test to exist at all;
/// it is the guarantee rather than the fix, and one line the reader refuses is
/// not a fact lost but a *world* lost.
fn filed(known: &HashMap<IVec2, Knowing>) -> HashMap<IVec2, Knowing> {
    known
        .iter()
        .filter(|(island, _)| island_in_the_world(**island))
        .map(|(island, knowing)| (*island, *knowing))
        .collect()
}

/// How far a point lies from the way between two points — the segment, not the
/// line it lies on, so a cairn well off either end of a run is far from it.
fn off_the_way(point: Vec2, from: Vec2, to: Vec2) -> f32 {
    let along = to - from;
    let reach = along.length_squared();
    // A way that goes nowhere is its own nearest point, which is what the zero
    // guard is for — and it is what makes this a plain distance for a player
    // standing still.
    let t = if reach > 0.0 {
        ((point - from).dot(along) / reach).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (from + along * t).distance(point)
}

/// Answers one asker with a cairn, if they are near enough to be looking at
/// it — what a refusal that has state to show comes down to.
///
/// The gate is [`CAIRN_SIGHT`]'s own rule kept honestly. A refusal carrying the
/// cairn is how a picture corrects itself for somebody who finds an island
/// already taken; ungated it would be a client reading off that the place is
/// spoken for, and where the stones stand, from anywhere in the world.
///
/// Answered every time and not only when it is news, unlike
/// [`sight_the_cairns`]: an ask is owed an answer, or an honest client could
/// not tell a refusal from a message that went nowhere.
///
/// But answered at the depth already held, and no deeper: this reads
/// [`Player::known`] and never writes to it. Deepening here would hand a client
/// the sighting rule for the asking — this ask is deliberately unpaced, so a
/// jump onto the stones and a claim in the same breath would buy back the very
/// name the sighting walk had just declined to give. An honest client loses
/// nothing, their report having run [`sight_the_cairns`] before they asked.
fn tell_the_asker(
    players: &HashMap<PlayerId, Player>,
    asker: PlayerId,
    island: IVec2,
    claim: &Claim,
) {
    if let Some(player) = players.get(&asker) {
        if player.position.distance(claim.at) <= CAIRN_SIGHT {
            post(player, cairn_told_to(player, island, claim));
        }
    }
}

/// Tells everyone near enough to see a cairn about it, and the `asker`
/// whether they are near it or not — a player who names an island from the
/// other side of the world still hears what became of their asking.
///
/// Told to everyone in reach whether or not it is news to them, which is the
/// difference from [`sight_the_cairns`]: the cairn itself has just changed, so
/// a player standing beside one they already knew is exactly who needs to hear
/// it. The world keeps how well a player knows a cairn and not a copy of the
/// word they read off it, so everybody out of reach reads the new word the next
/// time they are told of that cairn at all.
///
/// The gate here is the position last *reported* rather than the way the
/// [`Wake`] followed to it: the stones may have come into existence a moment
/// ago, and a run believed before they stood there says nothing about them.
/// What holds it down is that this runs only on a granted ask, which is what
/// [`CAIRN_PACE`] rations.
fn tell_the_cairn(
    players: &mut HashMap<PlayerId, Player>,
    island: IVec2,
    claim: &Claim,
    asker: PlayerId,
) {
    for (id, player) in players.iter_mut() {
        let near = player.position.distance(claim.at);
        if near > CAIRN_SIGHT && *id != asker {
            continue;
        }
        // Whether it was news is nothing to this: standing in reach of a cairn
        // that has just changed is the telling, news or no news. What [`learns`]
        // is here for is the writing down.
        learns(player, island, claim, near);
        post(player, cairn_told_to(player, island, claim));
    }
}

/// One cairn as one player hears it, at the depth they know it.
///
/// Built per hearer because both of the things it says beyond the stones
/// themselves are facts about the hearer. `yours` is what a client needs in
/// order to know which cairns are its own player's doing — and never the
/// [`Token`] that actually settles it, tokens being credentials that go nowhere
/// but to the client holding them.
///
/// And the name is withheld from anybody who has not been up to read it — see
/// [`Knowing`]. On the wire that is indistinguishable from an island nobody has
/// christened, deliberately: a hearer who has only sighted the stones has no
/// business telling *there is a name here you may not read* from *there is no
/// name here*.
///
/// The depth is read off the hearer rather than handed in: the callers that
/// deepen it have already written it down through [`learns`] by the time they
/// get here.
fn cairn_told_to(player: &Player, island: IVec2, claim: &Claim) -> ToClient {
    ToClient::Cairn {
        island,
        at: claim.at,
        name: match knows(&player.known, player.token, island, claim) {
            Some(Knowing::Visited) => claim.name.clone(),
            _ => String::new(),
        },
        yours: player.token == claim.by,
    }
}

/// One chunk of the world, surveyed exactly as a client would survey it.
///
/// Through [`Archipelago::chunk_payload`] and back out of it — quantised
/// heights and all — rather than off the generator's own `f32`s, which is the
/// whole point of this function existing. A client surveys what it was *sent*,
/// and a corner a centimetre either side of the sea puts the waterline
/// somewhere else: two ends that disagree about a coastline disagree about
/// whether it closes.
///
/// Open water carries no ground, and comes back surveyed and blank — which is
/// exactly what a client makes of an ocean answer, and is worth recording:
/// water somebody has crossed is not water nobody has.
fn survey_chunk(world: &Archipelago, chunk: IVec2) -> Soundings {
    match world.chunk_heights(chunk) {
        None => Soundings::default(),
        Some(heights) => {
            let heights: Vec<f32> = heights.iter().copied().map(dequantize).collect();
            protocol::survey::survey(&heights)
        }
    }
}

/// Cuts surveyed chunks into the messages they travel in: filled to
/// [`SURVEY_BATCH_BYTES`], and never splitting one chunk across two — which
/// is what lets a frame's ceiling be worked out at all. See
/// [`ToClient::Surveyed`].
///
/// The one place the wire's budget is spent, on purpose. Everything with
/// survey to hand over comes through here, so what a message costs is one
/// arithmetic rather than a rule and somebody's recollection of it.
fn survey_batches(found: Vec<(IVec2, Soundings)>) -> Vec<Vec<(IVec2, Soundings)>> {
    let mut batches: Vec<Vec<(IVec2, Soundings)>> = Vec::new();
    let mut batch: Vec<(IVec2, Soundings)> = Vec::new();
    let mut spent = 0;
    for (chunk, ink) in found {
        let costs = protocol::surveyed_bytes(&ink);
        // A chunk whose own ink overruns the whole budget still travels
        // whole, alone in a message of its own — which is the case the
        // frame's ceiling is worked out from, rather than this one.
        if spent + costs > SURVEY_BATCH_BYTES && !batch.is_empty() {
            batches.push(std::mem::take(&mut batch));
            spent = 0;
        }
        spent += costs;
        batch.push((chunk, ink));
    }
    if !batch.is_empty() {
        batches.push(batch);
    }
    batches
}

/// Tells one player what has been surveyed for them, a message at a time.
fn post_the_survey(
    players: &HashMap<PlayerId, Player>,
    id: PlayerId,
    found: Vec<(IVec2, Soundings)>,
) {
    let Some(player) = players.get(&id) else {
        return;
    };
    for batch in survey_batches(found) {
        post(player, ToClient::Surveyed { found: batch });
    }
}

/// What the survey knows of one player between their reports: where it last
/// got to, and how much way it will follow for them now.
///
/// A connection thread's own local, not a field on the roster: a player's
/// reports are read by exactly one thread, and this is about the rate that
/// thread is being asked to work at.
struct Wake {
    /// Where the survey last got to. Not quite where the player is, in the
    /// one case that matters: a report too far ahead to be believed is
    /// followed as far as the allowance carries it and no further, and this
    /// is where that left off. For anyone actually sailing the two are the
    /// same point, every time.
    at: Vec2,
    /// When the allowance was last worked out.
    since: Instant,
    /// Metres of way in hand: filling at [`PLAUSIBLE_SPEED`], never past
    /// [`SURVEY_SWEEP`].
    allowance: f32,
}

impl Wake {
    /// A wake opened where a player has been put down, with a full
    /// allowance. Arriving is not a voyage anybody has to earn: the ground
    /// around where the world sets somebody down is theirs on the spot, and
    /// there is no earlier report for the pace to be measured from anyway.
    fn opening(at: Vec2) -> Self {
        Self {
            at,
            since: Instant::now(),
            allowance: SURVEY_SWEEP,
        }
    }

    /// Takes a report and says what way the survey follows for it: from where
    /// it last got to, out to as far along as the allowance reaches.
    ///
    /// A report inside the allowance is believed whole, which is every report
    /// any real hull ever makes. One beyond it is neither called a lie nor
    /// thrown away: the survey follows at the speed it believes in, and the
    /// rest of the way is there to be had once the allowance has filled again.
    /// So a client hopping about the world orders ground worked out at the
    /// pace of somebody sailing, whatever pace it sends at.
    fn follows(&mut self, now: Vec2) -> (Vec2, Vec2) {
        let filled = self.since.elapsed().as_secs_f32() * PLAUSIBLE_SPEED;
        self.since = Instant::now();
        self.allowance = (self.allowance + filled).min(SURVEY_SWEEP);

        let (from, run) = (self.at, self.at.distance(now));
        let reached = if run <= self.allowance {
            now
        } else {
            from + (now - from) * (self.allowance / run)
        };
        // Charged the whole run and not the part of it that was followed,
        // and floored at nothing: a jump costs everything in hand, or the far
        // end of one could be bought over and over for the price of the near
        // end.
        self.allowance = (self.allowance - run).max(0.0);
        self.at = reached;
        (from, reached)
    }
}

/// The corners of the box of chunks the sight test has to be asked about for
/// a way from `from` to `to` — what keeps that test a bounded number of
/// questions rather than one about the whole world.
///
/// It has to *contain* everything the test accepts, and containing it is not
/// the same as reaching the same distance. A chunk is measured by its square,
/// so the box starts a chunk further out than the point it is worked from:
/// where the reach lands exactly on a boundary, the chunk before it is still
/// in sight by its own far corner. The high side wants no such slack, a
/// chunk's own corner being the near one there. A box that only nearly
/// contains the rule loses a chunk at an edge — silently, and only sometimes,
/// which is the worst way for a survey to have a hole in it.
fn scanned_for_sight(from: Vec2, to: Vec2) -> (IVec2, IVec2) {
    (
        chunk_at(from.min(to) - SIGHT_RADIUS) - IVec2::ONE,
        chunk_at(from.max(to) + SIGHT_RADIUS),
    )
}

/// Takes a position report and gives this player everything the way to it
/// earns them: the ground that came within sight of it, and the cairns.
///
/// One entry point because it is one rule — what a voyage shows you — and
/// because both halves have to be worked off the same way. The [`Wake`] says
/// how much of a report is believed and must be asked exactly once per report:
/// asked twice it would charge one run's allowance twice over.
///
/// The way rather than its end, for both halves: a hull between two position
/// reports must slip past neither a coast nor a cairn.
fn follow_the_way(shared: &Shared, id: PlayerId, wake: &mut Wake, now: Vec2) {
    let (from, to) = wake.follows(now);
    survey_the_way(shared, id, from, to);
    sight_the_cairns(shared, id, from, to);
}

/// Surveys whatever came within sight along a way, adds it to this player's
/// survey and tells them what was found there.
///
/// The ground is worked out with no lock held, because working it out may mean
/// generating an island — tens to hundreds of milliseconds, on this
/// connection's own thread and outside the worker pool. A client that draws the
/// world never comes to that, its islands being in the cache already, but that
/// is a fact about a cooperative client and not a bound. What bounds it is the
/// [`Wake`]'s pace, already applied to the way handed in here.
///
/// The locks are taken around all that rather than across it, and the survey's
/// own is taken with the roster's let go — see [`Surveyed`].
fn survey_the_way(shared: &Shared, id: PlayerId, from: Vec2, to: Vec2) {
    let (least, most) = scanned_for_sight(from, to);

    let surveyed = {
        let players = shared.players.held();
        let Some(player) = players.get(&id) else {
            return;
        };
        player.surveyed.clone()
    };
    let fresh: Vec<IVec2> = {
        let known = surveyed.held();
        (least.y..=most.y)
            .flat_map(|z| (least.x..=most.x).map(move |x| IVec2::new(x, z)))
            .filter(|chunk| {
                // Held to the same reach the ground is: the world's file
                // refuses a surveyed chunk that fails this, so a survey
                // allowed to hold one would be a world that saved and then
                // could not be opened again. A legal report from just inside
                // the edge has chunks in sight whose corners are past it.
                in_the_world(*chunk) && !known.surveyed(*chunk) && in_sight_along(*chunk, from, to)
            })
            .collect()
    };
    if fresh.is_empty() {
        return;
    }

    let found: Vec<(IVec2, Soundings)> = fresh
        .into_iter()
        .map(|chunk| (chunk, survey_chunk(&shared.world, chunk)))
        .collect();

    {
        // Kept as well as sent: what closes a coastline is the world's own
        // answer, and a claim is settled off it — see [`Surveyed`].
        let mut surveyed = surveyed.held();
        for (chunk, ink) in &found {
            surveyed.record(*chunk, ink.clone());
        }
    }
    let players = shared.players.held();
    post_the_survey(&players, id, found);
}

/// Tells a returning player back the survey the world remembers them having
/// taken, worked out afresh from the ground.
///
/// On a thread of its own, and that is not tidiness. The chunks are wherever
/// this player has ever been, so the islands under them are not in the cache
/// and every one has to be grown again — seconds of work for a well-sailed
/// world, and on the connection's own thread those would be seconds in which
/// its requests for the ground underfoot went unread. So the ink arrives a
/// batch at a time while the world opens around them, which is also how it
/// reads: the chart fills in.
///
/// An hour's sailing is on the order of a thousand chunks re-derived, which at
/// this scale is fine; a world somebody has lived in for a season would want
/// the ink kept rather than re-earned.
fn tell_the_survey_so_far(shared: &Arc<Shared>, id: PlayerId) {
    let surveyed = {
        let players = shared.players.held();
        let Some(player) = players.get(&id) else {
            return;
        };
        player.surveyed.clone()
    };
    let known: Vec<IVec2> = {
        let mut known: Vec<IVec2> = surveyed.held().charted().collect();
        // Sorted so that two runs of one world hand the same chart back in
        // the same order — a hash set's order is nobody's business.
        known.sort_by_key(|chunk| (chunk.x, chunk.y));
        known
    };
    if known.is_empty() {
        return;
    }

    let shared = shared.clone();
    thread::spawn(move || {
        for slab in known.chunks(SURVEY_SLAB) {
            // Two ways there is nobody left to tell — a world that has ended
            // and a player who has hung up — and both have to end this loop
            // rather than only slow it. Neither is noticed by posting, which
            // quietly does nothing, while the growing of islands carries on:
            // a client reconnecting in a loop would stack a thread of these
            // per attempt, each holding the world open.
            if shared.stopping.load(Ordering::Relaxed) {
                return;
            }
            if !shared.players.held().contains_key(&id) {
                return;
            }
            let found: Vec<(IVec2, Soundings)> = slab
                .iter()
                .map(|&chunk| (chunk, survey_chunk(&shared.world, chunk)))
                .collect();
            {
                // Filled in behind the blank the door left — see the survey
                // this player was welcomed with. Until a slab lands here the
                // world knows this player has been to those chunks but not
                // what is on them, which is what a claim is settled by.
                let mut surveyed = surveyed.held();
                for (chunk, ink) in &found {
                    surveyed.record(*chunk, ink.clone());
                }
            }
            let players = shared.players.held();
            post_the_survey(&players, id, found);
        }
    });
}

/// The yaw that points a hull standing at `at` toward `toward` — the
/// client's own drawing convention, forward being -Z turned by the yaw
/// about the vertical. Zero when the two coincide, which names no direction
/// and leaves the bow pointing north.
fn aimed(at: Vec2, toward: Vec2) -> f32 {
    let along = toward - at;
    if along == Vec2::ZERO {
        return 0.0;
    }
    f32::atan2(-along.x, -along.y)
}

/// Waits out whatever is left of `pace` since something last paid it.
///
/// How the paced asks are rationed — see [`CAIRN_PACE`]. The asker is made to
/// wait rather than the ask thrown away: a dropped ask is indistinguishable
/// from one refused or one lost. Slowing the connection that asked costs
/// nobody else anything, this being its own thread and no lock held.
fn wait_out(since: Option<Instant>, pace: Duration) {
    if let Some(left) = since.and_then(|paid| pace.checked_sub(paid.elapsed())) {
        thread::sleep(left);
    }
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
pub(crate) fn in_the_world(chunk: IVec2) -> bool {
    reachable(chunk.as_vec2() * protocol::ground::CHUNK_METRES)
}

/// Whether an island's identity is one the world can keep: the [`in_the_world`]
/// test, asked of the chunk the identity's lattice point falls in — see
/// [`protocol::survey::chunk_of`].
///
/// Both ends of the world file ask exactly this, which is the point of it
/// being one function. A claim the reader refuses is not a claim lost but a
/// *world* lost: the file will not parse, the opening falls back to the copy
/// beside it, and the next save takes that too.
pub(crate) fn island_in_the_world(island: IVec2) -> bool {
    in_the_world(protocol::survey::chunk_of(island))
}

/// Puts a message in a player's outbox, or hangs up on them.
///
/// Never waits. Most of the sends here happen with the roster's lock held, and
/// a thread stalled on one client's full queue would hold the session still
/// for everybody. A queue that deep is a client that has stopped reading, so
/// their connection is shut down instead — their own thread then takes them
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
            // Cloned per recipient, which is cheap: nothing broadcast is ever
            // a chunk, ground being answered to the one player who asked.
            post(player, message.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::survey::{Coast, Mark};

    /// One chunk's ink of a chosen size: a coast of `marks` points, which is
    /// what a torn shore actually costs on the wire.
    fn ink(marks: usize) -> Soundings {
        Soundings {
            coast: vec![Coast::new(vec![Mark::unpack([1, 2]); marks], false)],
            shoal: Vec::new(),
        }
    }

    /// Whether a batch is one the wire will actually carry — the ceiling
    /// being protocol's own, and a message that outgrew it failing to write
    /// rather than arriving as garbage.
    fn goes(batch: &[(IVec2, Soundings)]) -> bool {
        (ToClient::Surveyed {
            found: batch.to_vec(),
        })
        .write(&mut Vec::new())
        .is_ok()
    }

    #[test]
    fn a_survey_too_big_for_one_message_is_cut_into_several() {
        // The budget doing its job. A voyage's worth of ink is more than one
        // frame holds, so it has to come apart somewhere, and every piece
        // has to be a piece the wire will carry.
        let found: Vec<(IVec2, Soundings)> =
            (0..32).map(|x| (IVec2::new(x, 0), ink(500))).collect();
        let batches = survey_batches(found.clone());
        assert!(
            batches.len() > 1,
            "{} chunks and {} bytes went in one message",
            found.len(),
            found
                .iter()
                .map(|(_, ink)| protocol::surveyed_bytes(ink))
                .sum::<usize>()
        );
        for batch in &batches {
            assert!(goes(batch), "a batch of {} would not send", batch.len());
        }

        // And nothing was lost or reordered on the way through: the batches
        // read end to end are the chunks that went in.
        let back: Vec<IVec2> = batches.iter().flatten().map(|(chunk, _)| *chunk).collect();
        assert_eq!(
            back,
            found.iter().map(|(chunk, _)| *chunk).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_chunk_too_torn_for_the_budget_still_travels_whole() {
        // The case the frame's ceiling is worked out from: a coast crossing
        // every facet of one chunk costs more than a whole batch, and it
        // must go alone rather than be cut in half — half a chunk's ink is
        // not soundings anybody can read.
        let alone = ink(6_000);
        assert!(protocol::surveyed_bytes(&alone) > SURVEY_BATCH_BYTES);
        let batches = survey_batches(vec![
            (IVec2::ZERO, ink(4)),
            (IVec2::new(1, 0), alone),
            (IVec2::new(2, 0), ink(4)),
        ]);
        assert_eq!(batches.iter().map(Vec::len).collect::<Vec<_>>(), [1, 1, 1]);
        for batch in &batches {
            assert!(goes(batch));
        }
    }

    #[test]
    fn the_box_holds_everything_the_sight_test_accepts() {
        // The scan is a box and the rule is a distance, and the box has to
        // hold the rule whole. The case worth naming is the second way here:
        // the reach landing exactly on a chunk boundary, where the chunk
        // before it is still in sight by its far corner and a box measured
        // to the same distance leaves it out.
        let ways = [
            (Vec2::ZERO, Vec2::ZERO),
            (Vec2::new(SIGHT_RADIUS, 64.0), Vec2::new(SIGHT_RADIUS, 64.0)),
            (
                Vec2::new(SIGHT_RADIUS, 64.0),
                Vec2::new(SIGHT_RADIUS + 3.0 * protocol::ground::CHUNK_METRES, 64.0),
            ),
            (Vec2::new(-77.0, 412.0), Vec2::new(913.0, -1_200.0)),
        ];
        for (from, to) in ways {
            let (least, most) = scanned_for_sight(from, to);
            for z in least.y - 4..=most.y + 4 {
                for x in least.x - 4..=most.x + 4 {
                    let chunk = IVec2::new(x, z);
                    assert!(
                        !in_sight_along(chunk, from, to)
                            || (chunk.cmpge(least).all() && chunk.cmple(most).all()),
                        "{chunk} is in sight of the way from {from} to {to}, \
                         and outside the box {least}..{most} that is scanned for it"
                    );
                }
            }
        }
    }

    #[test]
    fn a_cairn_is_measured_off_the_whole_way_and_not_its_ends() {
        // The rule this function exists for, and the only one an endpoint test
        // would not catch. A run the [`Wake`] believes can be most of
        // SURVEY_SWEEP long and CAIRN_SIGHT is shorter than that, so the case
        // that matters is a cairn passed close aboard somewhere in the middle
        // of one: far from where the report started and far from where it
        // ended, and seen all the same.
        let (from, to) = (Vec2::new(-4_000.0, 0.0), Vec2::new(4_000.0, 0.0));
        let passed = Vec2::new(0.0, 900.0);
        assert!(
            passed.distance(from) > CAIRN_SIGHT && passed.distance(to) > CAIRN_SIGHT,
            "the cairn is within sight of an end of the way and proves nothing"
        );
        assert!(
            off_the_way(passed, from, to) <= CAIRN_SIGHT,
            "a cairn passed at {} metres was not in sight of the way it was passed on",
            off_the_way(passed, from, to)
        );

        // The segment and not the line it lies on: a point beyond an end is off
        // the way by the whole of how far past it lies, or a run would sight
        // cairns lying ahead of anywhere anybody had got to.
        assert_eq!(off_the_way(Vec2::new(9_000.0, 0.0), from, to), 5_000.0);
        // Off the far end and off to the side with it, where the line the way
        // lies on would answer 3,000 and the way itself answers 5,000.
        assert_eq!(
            off_the_way(Vec2::new(-8_000.0, -3_000.0), from, to),
            5_000.0
        );

        // And a way that goes nowhere is its own nearest point, which is what
        // makes this a plain distance for a player standing still.
        assert_eq!(off_the_way(passed, from, from), from.distance(passed));
    }

    #[test]
    fn a_report_a_hull_could_have_made_is_followed_whole() {
        // Everything a boat actually does: a wake opens where the world put
        // somebody down, and the way to their next report is followed end to
        // end, allowance or no allowance.
        let mut wake = Wake::opening(Vec2::ZERO);
        let along = Vec2::new(120.0, 0.0);
        assert_eq!(wake.follows(along), (Vec2::ZERO, along));
        let further = along + Vec2::new(0.0, 90.0);
        assert_eq!(wake.follows(further), (along, further));

        // And standing still costs nothing, so a player who stops does not
        // spend their way towards being disbelieved.
        assert_eq!(wake.follows(further), (further, further));
    }

    #[test]
    fn a_jump_no_hull_could_make_is_followed_only_so_far() {
        // Two kilometres in the time between two reports is not a voyage,
        // and inking the whole of it would be a hundred chunks of ground
        // ordered for twelve bytes. It is followed as far as anybody could
        // have got, and the rest waits.
        let mut wake = Wake::opening(Vec2::ZERO);
        let far = Vec2::new(100_000.0, 0.0);
        let (from, reached) = wake.follows(far);
        assert_eq!(from, Vec2::ZERO);
        assert!(
            (reached.x - SURVEY_SWEEP).abs() < 1.0,
            "the survey followed a jump to {reached}"
        );

        // And the next one straight after it goes nowhere at all: the jump
        // cost everything that was in hand, so what a client can order is
        // paced by the clock rather than by how fast it can write.
        let (_, again) = wake.follows(far + Vec2::new(100_000.0, 0.0));
        assert!(
            again.distance(reached) < SURVEY_SWEEP / 4.0,
            "a second jump ran on to {again} from {reached}"
        );
    }
}
