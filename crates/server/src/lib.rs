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
use protocol::ground::{chunk_at, dequantize};
use protocol::survey::{in_sight_along, Soundings, Survey, SIGHT_RADIUS};
use protocol::{
    BeastKind, BoatId, BoatKind, PlayerId, ToClient, ToServer, Token, WorldId, PROTOCOL_VERSION,
    SURVEY_BATCH_BYTES,
};
use world::archipelago::Archipelago;

pub use keeper::{data_dir, discard, kept_worlds, KeptWorld};
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
/// altogether is noticed rather than buffered forever.
///
/// What a full queue costs is worth stating in the worst case, because the
/// worst case has grown: a message is now as long as the longest survey batch
/// rather than as long as a chunk of ground — the better part of sixty
/// kilobytes — so a queue full of those is some fifteen megabytes for one
/// player. Nothing sends anywhere near it (a batch is filled to eight
/// kilobytes, and the ceiling stands where a coast crossing every facet of a
/// chunk would put it), but the number to size this against is the ceiling,
/// not the ordinary. It is still the right trade: a queue this deep is
/// several seconds of a client saying nothing, which is not "briefly behind"
/// — see [`post`], which hangs up on it rather than holding the megabytes.
///
/// The arrival's burst is also one word per boat in the world, the whole
/// fleet being introduced at the door, so this is the ceiling any future
/// sizing of the fleet has to be read against: a world holding boats in the
/// hundreds would be a world whose newcomers arrive into a full outbox.
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

/// How far from where an arrival is being put down a free hull nobody has
/// ever touched may lie and still be handed to them rather than a new one
/// minted, in metres — see [`BoatState::virgin`].
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
/// see [`protocol::survey::in_sight_along`], which is why. That only holds
/// while the two are a report apart: a client says where it is several times
/// a second, and no hull here makes two kilometres in that. A jump longer
/// than this is not a voyage, it is a client that stopped talking or one that
/// is lying, and running the survey along the whole of it would be inking
/// water nobody crossed — and, since each new chunk is ground to be worked
/// out, would be work a client could order by the megametre.
///
/// So this is the ceiling, and [`PLAUSIBLE_SPEED`] is the rate: what a player
/// has in hand is that speed times the time since they were last believed,
/// and it never banks past this. Waiting therefore buys a whole sweep and not
/// a metre more, which is what keeps a client that says nothing for an hour
/// from arriving with an hour's worth of work to order.
///
/// Public so a test of a jump no hull could make can pin how far it is
/// followed without repeating the number.
pub const SURVEY_SWEEP: f32 = 2_048.0;

/// How fast a player may be believed to have travelled, in metres per second,
/// as far as the survey is concerned.
///
/// The survey is where somebody has *been*, so what it costs to keep has an
/// honest bound in the world: nobody sailed two kilometres in fifty
/// milliseconds. Without one, a client that never asks for a chunk can order
/// a hundred chunks of ground worked out per twelve-byte `Move` — on the
/// connection thread, outside the worker pool and past every piece of
/// backpressure the chunk path has — as fast as it can write. Bounding the
/// way *per second* rather than per message is what makes that arithmetic
/// about the world instead of about how fast a socket can be fed.
///
/// Twenty-odd times what the fastest hull here makes, and it wants to stay
/// absurd: the number this is guarding against is a client asking for
/// kilometres, and every metre of headroom is a lag spike, a stall on some
/// far machine, or a legitimate catch-up that this must never clip. A voyage
/// that touched it would be a voyage nobody sailed.
///
/// Public so that a test which has to sail a real coast can wait for its
/// allowance at the rate the survey actually fills it, rather than sleeping on
/// a guess that would rot the moment this number moved.
pub const PLAUSIBLE_SPEED: f32 = 256.0;

/// How many chunks of a returning player's survey are worked out between one
/// look at whether there is still anybody to tell — see
/// [`tell_the_survey_so_far`].
///
/// Not a size on the wire: that budget is [`SURVEY_BATCH_BYTES`], spent in
/// exactly one place. This is a unit of *work*, because each of these chunks
/// may mean growing an island, and it is how long the backfill can carry on
/// for somebody who has already left. The chunks come sorted, so a slab is
/// usually a stretch of one coast and the islands under it are grown once
/// between them; sixty-four is milliseconds of that in the ordinary case and
/// under a second in the worst.
const SURVEY_SLAB: usize = 64;

/// How near a cairn a player has to be to be told about it, in metres.
///
/// A cairn is a thing standing in the world rather than an announcement, so it
/// is told to whoever could be looking at it: everybody nearby when one is
/// raised or renamed, and everybody put down beside one when they join. Narrow
/// enough that who holds what is something a player finds out by going there,
/// and wide enough to be a *sighting* — the range at which a pale pillar on a
/// headland is a thing you could pick out, rather than the range at which the
/// server happens to be willing to say so.
///
/// It does not gate a player's own claims, which they are told on joining
/// wherever they stand — see [`tell_the_cairns_about`], where the difference
/// is argued. Nothing is leaked by telling somebody what they already hold.
///
/// What it earns is [`Knowing::Sighted`] and no more: that there is a cairn,
/// and where. What the island is *called* costs a landing — see
/// [`CAIRN_VISIT`].
///
/// It was half as wide again when a cairn was a banner on a twenty-metre staff
/// — that carried a mile, and the radius was cut to match when the mark became
/// a stone pillar at head height (see `game::cairn` for why it did). A kilometre
/// is about as far as a pale speck on a green headland is a thing anybody can
/// honestly claim to have seen, so this is the far tier meaning what it says:
/// [`Knowing::Sighted`] is earned where a hull could have seen the stones, not
/// merely where the server knows they are.
///
/// It now falls just inside the radius a client streams terrain over, where it
/// used to sit well outside it. That ordering is not load-bearing — a cairn
/// waits unfooted and unseen until there is ground under it either way, which
/// is `game::cairn`'s business — and the way round it is now is the easier one:
/// the ground a mark stands on has usually been asked for before the mark is
/// told of, so the wait is a few frames rather than half a kilometre of
/// sailing.
///
/// Public with [`CAIRN_VISIT`] so that a test which has to stand at a chosen
/// remove from a cairn can work out where that is, rather than writing a
/// distance down twice and having one of them rot.
pub const CAIRN_SIGHT: f32 = 1_000.0;

/// How near a cairn a player has to come to read what is written on it, in
/// metres.
///
/// Nearly eight times narrower than [`CAIRN_SIGHT`], and the whole difference between
/// the two tiers of knowing. Stone standing on a headland says only *somebody
/// is here*, and says it to anyone who passes. A name is lettering, and
/// lettering is read by walking up to it. So a passing hull learns an island is
/// spoken for and has to land to learn whose word is on it.
///
/// Short enough that no honest voyage collects a name in passing, and long
/// enough that a player who has beached and walked up the shore is not left
/// hunting for a spot: this is a stone's throw, not a doorstep. A cairn on a
/// headland can be read from a boat lying right off it, which is the claimant's
/// own doing — build inland and the name stays inland with it.
///
/// Public for the reason [`CAIRN_SIGHT`] is.
pub const CAIRN_VISIT: f32 = 128.0;

/// How often one player may have a cairn raised or rewritten, at most.
///
/// Two different costs, one pace, because both are bought with the same word
/// from the same client and a cairn is hand-carved either way: a quarter of a
/// second between asks is nothing to a player and everything to a pestering
/// one.
///
/// Settling a claim means walking every coastline that player has surveyed.
/// That is arithmetic rather than generation — the soundings are already in
/// hand, see [`Surveyed`] — but it grows with the voyage, and a client can ask
/// as fast as it can write.
///
/// Christening one costs the walk nothing and costs everybody else something:
/// a granted name is told to every player within [`CAIRN_SIGHT`] of the cairn,
/// and an outbox that fills is a connection shut down (see [`post`]). Unpaced,
/// one client alternating two perfectly good names could hang up on every
/// player standing near its island.
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

/// One player's survey, held behind a lock of its own rather than inside the
/// roster's — see [`Player::surveyed`].
///
/// An hour's sailing is thousands of chunks, and the two things that want the
/// whole of it — the periodic save and the departure — would otherwise copy
/// it with the roster held, which is every other player's `Move` and `Helm`
/// stopped for the length of the copy. The chunk path was built never to work
/// under that lock and this is the same rule: what the roster hands out is a
/// pointer, and the copying happens where nobody is waiting on it.
///
/// Lock order: this is a leaf, and the way it stays one is that everybody
/// takes the roster, clones the handle, lets the roster go, and only then
/// locks this. No thread ever holds both, so there is no order for the two to
/// disagree about.
///
/// The ink is kept and not only the coordinates, which costs a few megabytes
/// for a player who has called at a thousand islands and buys the one thing a
/// bare list of chunks cannot answer: whether a coastline closes. A claim is
/// settled by asking exactly that — see [`settle_a_claim`] — and asking it of
/// a list would mean working every chunk of a whole voyage's ground out again,
/// per ask, on the connection's own thread. The soundings are already in hand
/// where they are recorded, so keeping them is free.
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
    /// token it dealt them — and whether they were at a helm. What
    /// [`ToServer::Papers`] is answered from, and — merged with the roster,
    /// which holds the players who are here — what a save writes down.
    /// Loaded from the world's file when there is one, and kept regardless,
    /// so leaving and rejoining works even in a world nobody is keeping.
    remembered: Mutex<HashMap<Token, keeper::PlayerRecord>>,
    /// Every boat in the world, by its lasting name — the vehicles, which
    /// are entities of the world and never anybody's appendage: they
    /// outlive visits, lie at anchor while unoccupied, and change hands by
    /// [`ToServer::Board`].
    ///
    /// Lock order: a thread holding [`Shared::players`] may take this — the
    /// welcome and the helm words work on both at once — and nothing
    /// holding this ever reaches for the roster; takers with no roster
    /// business (the saves) take it alone. That one-way rule is why the
    /// pair cannot deadlock.
    pub(crate) boats: Mutex<HashMap<BoatId, BoatState>>,
    /// Every island anybody has claimed, by the identity of the ring that is
    /// it — see [`protocol::survey::Island::id`]. What a cairn stands for, and
    /// the whole of who may name what.
    ///
    /// Lock order: a leaf, like a player's survey, and held alone. Everything
    /// a grant needs from the roster — where the asker is, whether they are
    /// afoot — is read and let go before this is taken, and everything anybody
    /// is told is posted after it has been let go again. So the one nesting
    /// this session allows, the roster over the boats, has nothing to say
    /// about this lock and cannot be got into an argument with it.
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
    /// [`survey_the_way`], which is the only thing that adds to it, and
    /// [`protocol::ToClient::Surveyed`], which is how they are told.
    ///
    /// The world's, not the client's: a claim is judged against a coastline
    /// somebody has actually closed, so which ground that is has to be a fact
    /// the server holds rather than one a client reports. Loaded from the
    /// world's memory of this token at the door and filed back there on the
    /// way out, so a voyage outlives the visit that made it.
    ///
    /// Behind a lock of its own, and see [`Surveyed`] for why: the roster is
    /// held by everything a player does, and this is the one thing on it that
    /// is big enough to be worth not copying there.
    surveyed: Surveyed,
    /// What this player has found out about other people's claims, by the
    /// island each stands for — see [`Knowing`]. Two things deepen it and
    /// nothing else does: [`sight_the_cairns`], as a voyage brings a cairn
    /// within reach, and [`tell_the_cairn`], where stones that have just
    /// changed are pushed to whoever is standing by to watch it happen.
    ///
    /// Small where the survey is huge — one entry per cairn this player has
    /// ever come near, against thousands of chunks — so it lives under the
    /// roster's own lock rather than behind one of its own. There is nothing
    /// here worth not copying.
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
/// third of the three things this world keeps per person. The other two are
/// [`Player::surveyed`], which is where they have been, and [`Shared::claims`],
/// which is what they hold; this is what they have found out about everybody
/// else's. They are deliberately three and not one: only a survey earns a
/// claim, so what a player has *seen of a coast* and what they have *heard
/// about it* must never be able to stand in for each other. A chart that let
/// hearsay close a ring would let a player claim an island by being told about
/// it.
///
/// It only ever grows. Sighting a cairn and sailing away does not unsee it —
/// the chart is a record of the voyage, and what a chart records it keeps — so
/// there is no going back down these steps and no forgetting them at the end of
/// a session. That is why the world's file carries them (see
/// [`keeper::PlayerRecord::known`]) and why the ordering here is the whole of
/// the merge: what somebody knows is the deeper of what they knew and what
/// standing where they are earns them.
///
/// There is no third step for *surveyed*, and there should not be. A player who
/// has been round the coast has the ring in their own survey already, which is
/// a stronger thing than knowing about it and reached by a different road.
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
/// and whether the hull has ever been sailed at all.
pub(crate) struct BoatState {
    pub(crate) kind: BoatKind,
    pub(crate) position: Vec2,
    pub(crate) heading: f32,
    pub(crate) occupant: Option<PlayerId>,
    /// Whether nobody has ever done anything with this hull — a boat minted
    /// for an arrival who never sailed it, never boarded it and never
    /// stepped off it. A virgin hull lying free is offered to the next
    /// arrival instead of minting another, which is what bounds the fleet
    /// against a client that joins and hangs up in a loop: the loop is
    /// handed the same boat every time. Session-local — a loaded boat is not
    /// virgin, its being in the file at all meaning somebody's story touched
    /// it.
    ///
    /// Cleared by all three of sailing, boarding and stepping off, because
    /// any of them makes the hull somebody's: a boat a live player parked on
    /// a beach and walked away from must not be handed out from under them
    /// on the strength of never having been *reported* moved. The loop this
    /// bounds does none of the three — it joins and hangs up — so the bound
    /// costs nothing.
    pub(crate) virgin: bool,
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
                                    virgin: false,
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
    /// path through them — and the surveys are copied out after the roster
    /// has been let go, the roster's hold being nothing but the pointers.
    /// A save that copied thousands of chunks per player under it would be a
    /// save that stopped everyone else's `Move` for the length of the file.
    fn record(&self) -> keeper::WorldRecord {
        // Everything about a present player the roster itself can say, and
        // beside it the handle to the one thing it cannot — the survey, whose
        // own lock is taken below with this one let go.
        let present: Vec<(Token, keeper::PlayerRecord, Surveyed)> = {
            let players = self.players.lock().expect("no poisoned lock");
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
                record.surveyed = surveyed
                    .lock()
                    .expect("no poisoned lock")
                    .charted()
                    .collect();
                (token, record)
            })
            .collect();
        let mut players = self.remembered.lock().expect("no poisoned lock").clone();
        players.extend(here);
        let boats = {
            let boats = self.boats.lock().expect("no poisoned lock");
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
            let claims = self.claims.lock().expect("no poisoned lock");
            claims
                .iter()
                // Nothing the file could not be read back saying, on the terms
                // the beasts are held to — see `beasts::Flock::records`, and
                // [`island_in_the_world`] for what it costs to get this wrong.
                // Nothing granted since the fix can fail this; it is the
                // guarantee rather than the fix, and it is the guarantee that
                // matters, because a claim the reader refuses takes the whole
                // world with it.
                .filter(|(island, claim)| island_in_the_world(**island) && reachable(claim.at))
                .map(|(island, claim)| keeper::ClaimRecord {
                    island: *island,
                    by: claim.by,
                    at: claim.at,
                    name: claim.name.clone(),
                })
                .collect()
        };
        let beasts = self.beasts.lock().expect("no poisoned lock").clone();
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
        // over the first seconds of their visit rather than at the door, which
        // is the same window in which their client has no chart drawn either:
        // an island claimed in it is refused, and claimable a moment later.
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
            // Including what the papers had been anywhere: a stranger has
            // seen nothing, and the survey belongs to whoever is still
            // holding those papers rather than to both of them. Replaced
            // rather than emptied, so that nothing locks a survey with the
            // roster in hand — see [`Surveyed`]. Nobody else holds this one
            // yet anyway, the player not being on the roster until below.
            player.surveyed = Arc::new(Mutex::new(Survey::default()));
            player.known.clear();
            returning = None;
        }

        // Where this player enters, and at whose helm — the boats being
        // world entities with keepers rather than owners. A newcomer's
        // story starts aboard: the world mints them a sloop on the spawn.
        // A returner who left at a helm is seated back into that boat only
        // if it still lies free where they left it; otherwise somebody has
        // taken it up or sailed it off in the meantime, and the world
        // minting a fresh hull where the returner stood is the interim
        // answer until there is any other way to be on open water. A
        // returner who left ashore enters on their own feet, their old
        // boat — wherever it now lies — being one of the tellings below.
        //
        // Inside the roster's hold, with the boats' lock nested under it —
        // the one nesting [`Shared::boats`]'s order allows — so the seat is
        // taken before anyone can be told about the boat it claims.
        // The way the hull a returner is seated at lies — what their view
        // is opened along, there being no entry island to face them at.
        let mut bow = None;
        {
            let mut boats = shared.boats.lock().expect("no poisoned lock");
            let fresh_hull = |boats: &mut HashMap<BoatId, BoatState>, at: Vec2| {
                // A virgin hull lying free where this arrival is being put
                // down is theirs before any new one is minted — see
                // [`BoatState::virgin`] and [`SPARE_BERTH`]:
                // it is what keeps a join-and-hang-up loop from growing the
                // fleet, and it reads as the world having a boat ready
                // rather than conjuring one.
                let handed_down = boats.iter_mut().find(|(_, boat)| {
                    boat.virgin
                        && boat.occupant.is_none()
                        && boat.position.distance(at) <= SPARE_BERTH
                });
                if let Some((&boat, state)) = handed_down {
                    state.occupant = Some(id);
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
                        virgin: true,
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
                    None => None,
                    Some(kept) => match boats.get_mut(&kept) {
                        Some(boat)
                            if boat.occupant.is_none()
                                && boat.position.distance(record.position) <= KEPT_BERTH =>
                        {
                            boat.occupant = Some(id);
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
            let boats = shared.boats.lock().expect("no poisoned lock");
            for (boat, state) in boats.iter() {
                post(
                    newcomer,
                    ToClient::Boat {
                        id: *boat,
                        kind: state.kind,
                        position: state.position,
                        heading: state.heading,
                        occupant: state.occupant,
                    },
                );
            }
            if let Some(boat) = seated {
                let state = &boats[&boat];
                broadcast(
                    &players,
                    id,
                    ToClient::Boat {
                        id: boat,
                        kind: state.kind,
                        position: state.position,
                        heading: state.heading,
                        occupant: state.occupant,
                    },
                );
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
        let players = shared.players.lock().expect("no poisoned lock");
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
    // and nothing shared has business with them. Two clocks rather than one,
    // because the two rules about what pays are not the same rule and each
    // reads where it is applied. Nothing yet, so the first ask of a session is
    // answered at once.
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
                    let mut players = shared.players.lock().expect("no poisoned lock");
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
                    let mut players = shared.players.lock().expect("no poisoned lock");
                    // The rider goes with the vehicle: one report moves both.
                    // Quietly ignored from a player occupying nothing — see the
                    // wire's own doc for how that happens honestly.
                    let steering = players
                        .get_mut(&id)
                        .and_then(|player| player.aboard.inspect(|_| player.position = position));
                    if let Some(boat) = steering {
                        let kind = {
                            let mut boats = shared.boats.lock().expect("no poisoned lock");
                            let state = boats.get_mut(&boat).expect("a boat once boarded exists");
                            state.position = position;
                            state.heading = heading;
                            // Sailed, so no longer the spare hull the spawn
                            // hands to arrivals — see [`BoatState::virgin`].
                            state.virgin = false;
                            state.kind
                        };
                        broadcast(
                            &players,
                            id,
                            ToClient::Boat {
                                id: boat,
                                kind,
                                position,
                                heading,
                                occupant: Some(id),
                            },
                        );
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
                let mut players = shared.players.lock().expect("no poisoned lock");
                let Some(player) = players.get_mut(&id) else {
                    break;
                };
                let (answer, granted) = {
                    let mut boats = shared.boats.lock().expect("no poisoned lock");
                    // A boat this world never made is a broken or hostile
                    // client, and there is no state to answer with.
                    let Some(state) = boats.get_mut(&boat) else {
                        break;
                    };
                    // Granted only to somebody on their own feet beside an
                    // empty helm; anything else leaves the boat as it was,
                    // and the state is the whole of the answer either way.
                    let granted = player.aboard.is_none()
                        && state.occupant.is_none()
                        && state.position.distance(player.position) <= BOARD_GRANT;
                    if granted {
                        state.occupant = Some(id);
                        // Taken up, so no longer the spare hull the spawn
                        // hands to arrivals — see [`BoatState::virgin`].
                        state.virgin = false;
                        player.aboard = Some(boat);
                        player.position = state.position;
                    }
                    (
                        ToClient::Boat {
                            id: boat,
                            kind: state.kind,
                            position: state.position,
                            heading: state.heading,
                            occupant: state.occupant,
                        },
                        granted,
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
                let mut players = shared.players.lock().expect("no poisoned lock");
                if let Some(player) = players.get_mut(&id) {
                    // Ignored when not aboard, on Helm's terms.
                    if let Some(boat) = player.aboard.take() {
                        stepped = true;
                        player.position = position;
                        let told = {
                            let mut boats = shared.boats.lock().expect("no poisoned lock");
                            let state = boats.get_mut(&boat).expect("a boat once boarded exists");
                            state.occupant = None;
                            // Somebody's, and left where they left it: a hull
                            // parked ashore is not spare, whether or not it
                            // was ever sailed — see [`BoatState::virgin`].
                            state.virgin = false;
                            ToClient::Boat {
                                id: boat,
                                kind: state.kind,
                                position: state.position,
                                heading: state.heading,
                                occupant: None,
                            }
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
            Ok(ToServer::Claim { island }) => {
                // Paced rather than answered as fast as it is asked — see
                // [`CAIRN_PACE`] — by waiting out what is left of it rather
                // than by dropping the ask. A client that asks too soon is
                // answered late; it is never answered with silence, which
                // would leave an honest one unable to tell a refusal from a
                // message that went nowhere. The waiting is done on this
                // connection's own thread, so the only session it slows is
                // the one asking.
                //
                // Only an ask that actually walked the coastlines pays the
                // pace, which is why the clock is set from what came back
                // rather than from having asked. What is being rationed is
                // that walk; an ask the session answered off the roster or the
                // claims alone — from somebody at a helm, or after an island
                // somebody already holds — cost nothing, and charging it would
                // mean a player who asked from the deck and then stepped
                // ashore waited for no reason.
                wait_out(asked_to_claim, CAIRN_PACE);
                if settle_a_claim(&shared, id, island) {
                    asked_to_claim = Some(Instant::now());
                }
            }
            Ok(ToServer::Name { island, name }) => {
                // Paced on the claim's terms and for the same reason, one word
                // of it being as cheap to say as the other — see
                // [`CAIRN_PACE`], which is where the two costs are written up.
                // What pays is a christening that was actually granted, that
                // being the one that goes to everybody near the cairn; a
                // refusal reaches its asker and nobody else, and is answered
                // as promptly as the last grant allows.
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
    let last_seen = {
        let players = shared.players.lock().expect("no poisoned lock");
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
                surveyed: surveyed
                    .lock()
                    .expect("no poisoned lock")
                    .charted()
                    .collect(),
                known,
            },
        )
    });
    if let Some((token, record)) = &leaving {
        shared
            .remembered
            .lock()
            .expect("no poisoned lock")
            .insert(*token, record.clone());
    }
    {
        let mut players = shared.players.lock().expect("no poisoned lock");
        players.remove(&id);
        broadcast(&players, id, ToClient::Left { id });
        // The helm they held is anyone's now: an offline player's boat lies
        // at anchor, visible and takeable, and being seated back into it on
        // return is a memory rather than a hold. Told under the same hold
        // as the departure, so nobody hears of a free boat before its
        // keeper has left.
        if let Some(boat) = leaving.and_then(|(_, record)| record.aboard) {
            let told = {
                let mut boats = shared.boats.lock().expect("no poisoned lock");
                boats.get_mut(&boat).map(|state| {
                    state.occupant = None;
                    ToClient::Boat {
                        id: boat,
                        kind: state.kind,
                        position: state.position,
                        heading: state.heading,
                        occupant: None,
                    }
                })
            };
            if let Some(told) = told {
                broadcast_all(&players, told);
            }
        }
    }
    (shared.report)(&format!("{id} left"));
}

/// Settles a claim: grants it if the world's own record says this player has
/// earned the island, and tells whoever can see the answer either way.
///
/// The three things a grant wants are all facts the server holds. The claimant
/// must be afoot, because a cairn is built by somebody standing on the ground
/// and not by somebody sailing past. The server's survey *for that player*
/// must answer [`Survey::island_under`] with the island they named, which is
/// both halves of the rule in one question — an unclosed coast rings nothing,
/// and a point outside a ring is somewhere else. And nobody may hold it
/// already, first asker taking it.
///
/// A refusal is posted to the asker alone and never broadcast: the world is
/// otherwise exactly as they last heard it, and a refusal every other player's
/// outbox carried would be a client able to pester the whole roster. Where the
/// refusal is that somebody got there first, what goes back is that cairn —
/// the same idiom as a refused boarding, where the boat's own state is the
/// whole of the answer — provided the asker is standing near enough to be
/// looking at it, which is [`tell_the_asker`]'s business. Where there is no
/// state to send, or nobody near enough to be sent it, the answer is silence.
///
/// The claims are asked *before* the coastlines are walked, and that order is
/// the whole of what stops a pestering client. An island somebody already holds
/// is settled by the claims alone, and asking after one is exactly what a
/// hostile client would repeat: the walk is what the pace exists to ration, so
/// an ask that cannot possibly need it must not buy one. It also keeps the
/// survey's own lock — which the saves and the backfill queue behind — held for
/// the shortest time the question allows.
///
/// The locks are taken one at a time and in this order for the reason written
/// on [`Shared::claims`]: the roster, let go; the claims, let go; the survey,
/// let go; the claims again, let go; and only then the roster, to tell people.
/// The claims being asked twice is not a race: the second hold is the one that
/// decides, so two players walking the same shore at once still leave one
/// cairn, whichever of them reaches the insertion first.
///
/// Says whether the survey was walked, which is the expensive half and the
/// only half worth pacing — see the read loop, which is where the pacing is.
fn settle_a_claim(shared: &Shared, id: PlayerId, island: IVec2) -> bool {
    let Some((token, at, afoot, surveyed)) = ({
        let players = shared.players.lock().expect("no poisoned lock");
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
    let held = shared
        .claims
        .lock()
        .expect("no poisoned lock")
        .get(&island)
        .cloned();
    if let Some(claim) = held {
        let players = shared.players.lock().expect("no poisoned lock");
        tell_the_asker(&players, id, island, &claim);
        return false;
    }
    // Nor for somebody who could not be building a cairn wherever they are:
    // one is built by a player standing on the ground, not sailing past.
    if !afoot {
        return false;
    }

    let stands_on = surveyed
        .lock()
        .expect("no poisoned lock")
        .island_under(at)
        .is_some_and(|found| found.id == island);

    let (told, granted) = {
        let mut claims = shared.claims.lock().expect("no poisoned lock");
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
        let mut players = shared.players.lock().expect("no poisoned lock");
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
/// A name is the world's now rather than one client's notebook: it rides with
/// the cairn, so everybody who passes reads the same word. Which is why it is
/// earned the way the island was — only the claimant may write it, and a name
/// the wire will not carry (see [`protocol::island_name`]) is a refusal rather
/// than an erasure, leaving the cairn saying whatever it said before.
///
/// An island nobody has claimed cannot be named at all, and there is no state
/// to answer with, so that ask is met with silence.
///
/// Says whether the christening was granted, which is the half that reaches
/// anybody but the asker and so the half worth pacing — see the read loop.
fn christen(shared: &Shared, id: PlayerId, island: IVec2, name: &str) -> bool {
    let Some(token) = ({
        let players = shared.players.lock().expect("no poisoned lock");
        players.get(&id).map(|player| player.token)
    }) else {
        return false;
    };

    let (told, granted) = {
        let mut claims = shared.claims.lock().expect("no poisoned lock");
        let Some(claim) = claims.get_mut(&island) else {
            return false;
        };
        match protocol::island_name(name) {
            // The name it already had is not news, so nobody nearby is told of
            // it: this is a client's word, and a client repeating itself should
            // not read as the cairn changing. It suppresses a repetition and
            // nothing more — a client alternating two good names passes it
            // every time — so what actually holds the rate down is
            // [`CAIRN_PACE`], applied where the ask is read.
            Some(written) if claim.by == token && written != claim.name => {
                claim.name = written;
                (claim.clone(), true)
            }
            // Somebody else's island, or nothing anybody could call a name:
            // the cairn goes back to the asker exactly as it stands.
            _ => (claim.clone(), false),
        }
    };

    let mut players = shared.players.lock().expect("no poisoned lock");
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
/// cairns is [`Knowing`]'s business and was earned by going there in some
/// earlier hour of the world; it comes back exactly as it was left, because a
/// chart does not forget over a night ashore. A player's own claim is theirs to
/// know outright: it is on the wire as `yours`, they are the only one who may
/// write on it, and there is no other way for them to hear of it again. A
/// client that is not told is a client whose sheet draws its own island blank
/// and will not open the pen on it — their island, unnameable, with nothing to
/// say why — for as long as they stay away from it.
///
/// What this deliberately does *not* do is look at where the player has been
/// put down. Standing somewhere is worth exactly what standing there is worth
/// to anybody, and [`sight_the_cairns`] is where that is decided; the join runs
/// it on the spot, the same way it runs the survey, so an arrival beside a
/// stranger's cairn sees it for the reason anyone sees it.
///
/// The locks are taken one at a time in the order [`Shared::claims`] sets: the
/// roster for the token and the knowing, let go; the claims, let go; and the
/// roster again to post.
fn tell_the_cairns_about(shared: &Shared, id: PlayerId) {
    let Some((token, known)) = ({
        let players = shared.players.lock().expect("no poisoned lock");
        players
            .get(&id)
            .map(|player| (player.token, player.known.clone()))
    }) else {
        return;
    };
    let worth_telling: Vec<(IVec2, Claim, Knowing)> = {
        let claims = shared.claims.lock().expect("no poisoned lock");
        claims
            .iter()
            .filter_map(|(island, claim)| {
                // Their own read as fully known however far off they stand,
                // and whatever the file happens to say — see [`Player::known`],
                // where that deliberate belt-and-braces is argued.
                let knowing = if claim.by == token {
                    Some(Knowing::Visited)
                } else {
                    known.get(island).copied()
                };
                knowing.map(|knowing| (*island, claim.clone(), knowing))
            })
            .collect()
    };
    if worth_telling.is_empty() {
        return;
    }
    let players = shared.players.lock().expect("no poisoned lock");
    let Some(player) = players.get(&id) else {
        return;
    };
    for (island, claim, knowing) in worth_telling {
        post(player, cairn_told_to(player, island, &claim, Some(knowing)));
    }
}

/// Brings one player's knowledge of one cairn up to what being `near` metres
/// from it earns, and says what they know of it now and whether that is any
/// deeper than what they knew a moment ago.
///
/// The merge is [`Knowing`]'s ordering and nothing else: the deeper of what
/// they knew and what they have just earned, never the newer. Sailing away from
/// a cairn does not unlearn it, so this only ever climbs.
///
/// Saying whether it climbed is what spares the caller looking the entry up
/// itself to find out. [`sight_the_cairns`] tells a player only when something
/// is news, and it asks this of every claim in the world on every position
/// report, so one lookup here rather than one here and one there is worth the
/// second half of the answer.
///
/// A claimant knows their own outright wherever they stand, and it is not
/// written down: it is held by token in [`Shared::claims`] and is true whether
/// or not this map says so — see [`Player::known`]. Nothing was written, so
/// nothing deepened, and that is the honest answer for it. Answering without
/// recording is why [`sight_the_cairns`] passes over a player's own cairns
/// rather than finding one perpetually newsworthy.
fn learns(player: &mut Player, island: IVec2, claim: &Claim, near: f32) -> (Option<Knowing>, bool) {
    if claim.by == player.token {
        return (Some(Knowing::Visited), false);
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
    let deeper = now > held;
    if deeper {
        player
            .known
            .insert(island, now.expect("deeper than nothing is something"));
    }
    (now, deeper)
}

/// Takes down whatever cairns the way from `from` to `to` brought within reach,
/// and tells this player about the ones that are news to them.
///
/// The other half of what a position report earns, alongside the survey, and
/// run on the same terms: over the way rather than its end, because a hull
/// between two reports must no more slip past a cairn than past a coast. A run
/// the [`Wake`] believes can be most of [`SURVEY_SWEEP`] long and
/// [`CAIRN_SIGHT`] is shorter than that, so an endpoint test really would let a
/// cairn passed at speed go unseen — silently, and only sometimes.
///
/// Told only when it is news, and that is the whole reason [`Player::known`]
/// exists rather than this being a distance check per report. A player standing
/// beside a cairn reports their position ten times a second; without a record
/// of what they have already been told, each of those would be a message about
/// a pillar of stone that has not moved.
///
/// A walk of every claim in the world per report, bounded by how many islands
/// anybody has claimed rather than by how far anybody has sailed, which is what
/// makes it affordable to do this often. What it costs once a cairn is in reach
/// is a `Vec` and a copy of every claim in it, carved name and all — so for a
/// player parked beside one that is ten times a second, forever, news or no
/// news. The copy is the price of the lock order rather than sloppiness: what
/// counts as news is [`Player::known`]'s business, `known` lives under the
/// roster's lock, and reaching for the roster with the claims still held is
/// exactly the order [`Shared::claims`] forbids. So the claims are read, let
/// go, and only then judged.
///
/// The locks are taken one at a time in the order [`Shared::claims`] sets: the
/// claims, let go; then the roster, which is where the knowing lives and where
/// the telling goes.
fn sight_the_cairns(shared: &Shared, id: PlayerId, from: Vec2, to: Vec2) {
    let within_reach: Vec<(IVec2, Claim, f32)> = {
        let claims = shared.claims.lock().expect("no poisoned lock");
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

    let mut players = shared.players.lock().expect("no poisoned lock");
    let Some(player) = players.get_mut(&id) else {
        return;
    };
    let theirs = player.token;
    for (island, claim, near) in within_reach {
        // Their own passed over: there is nothing about it they can learn by
        // going near it, they were told of it when they raised it and again at
        // every door since, and [`learns`] deliberately writes nothing down for
        // it — so without this it would be news on every report they ever make.
        if claim.by == theirs {
            continue;
        }
        let (now, news) = learns(player, island, &claim, near);
        if news {
            post(player, cairn_told_to(player, island, &claim, now));
        }
    }
}

/// What one player's knowledge of the cairns goes into the world's file as.
///
/// Nothing the file could not be read back saying, on the terms the claims
/// themselves are held to — see [`Shared::record`], where the same filter is
/// applied to the claims, and [`island_in_the_world`] for what it costs to get
/// this wrong. These name islands exactly the way a claim does and so have
/// exactly the same edge to fall off.
///
/// Nothing that gets into a player's knowing can fail it, every entry being
/// about a claim that passed the same test in order to exist at all. It is the
/// guarantee rather than the fix, and the guarantee is the part that matters:
/// one line the reader refuses is not a fact lost but a *world* lost.
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
/// cairn is how a picture corrects itself for somebody standing there and
/// finding it already taken; ungated it would be a client with an island's
/// identity in hand reading off that the place is spoken for and where the
/// stones stand, from anywhere in the world. Sailing round a coast is
/// what earns an identity, so the leak is small — and who holds what is
/// something a player is meant to find out by going there.
///
/// Answered every time and not only when it is news, unlike
/// [`sight_the_cairns`]: an ask is owed an answer, or an honest client could
/// not tell a refusal from a message that went nowhere.
///
/// But answered at the depth already held, and no deeper: this reads
/// [`Player::known`] and never writes to it. What standing somewhere is worth
/// is worked out along the way the [`Wake`] believes in and nowhere else, and a
/// position report is a client's own word for where it is. Deepening here would
/// hand a client the whole of that rule for the asking — an ask after an island
/// somebody already holds is deliberately unpaced, so a jump onto the stones
/// and a claim in the same breath would buy back the very name the sighting
/// walk had just declined to give. An honest client loses nothing: their report
/// ran [`sight_the_cairns`] before they ever got to ask, and whatever standing
/// there earns is written down by then.
fn tell_the_asker(
    players: &HashMap<PlayerId, Player>,
    asker: PlayerId,
    island: IVec2,
    claim: &Claim,
) {
    if let Some(player) = players.get(&asker) {
        if player.position.distance(claim.at) <= CAIRN_SIGHT {
            let knowing = if claim.by == player.token {
                Some(Knowing::Visited)
            } else {
                player.known.get(&island).copied()
            };
            post(player, cairn_told_to(player, island, claim, knowing));
        }
    }
}

/// Tells everyone near enough to see a cairn about it, and the `asker`
/// whether they are near it or not — a player who names an island from the
/// other side of the world still hears what became of their asking.
///
/// Told to everyone in reach whether or not it is news to them, and that is the
/// difference from [`sight_the_cairns`]: the cairn itself has just changed, so
/// a player standing beside one they already knew is exactly the player who
/// needs to hear it. What the world keeps of a player is how well they know a
/// cairn and not a copy of the word they read off it, so a rename is pushed to
/// whoever is in reach to watch it happen and everybody else reads the new word
/// the next time they are told of that cairn at all — at the door, or on coming
/// back into sight of it. Keeping the word per player instead would be a copy
/// in the world's file for every stranger who ever landed, and charts left
/// quietly disagreeing with the world for nothing.
///
/// Unlike [`sight_the_cairns`], the gate here is the position last *reported*
/// rather than the way the [`Wake`] followed to it. There is no way to work it
/// off: the stones may have come into existence a moment ago, and a run that
/// was believed before they stood there says nothing about them. What holds it
/// down is that this only runs at all when somebody's ask was granted, and
/// granting is what [`CAIRN_PACE`] rations.
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
        let (knowing, _) = learns(player, island, claim, near);
        post(player, cairn_told_to(player, island, claim, knowing));
    }
}

/// One cairn as one player hears it, at the depth they know it.
///
/// The message is built per hearer because both of the things it says beyond
/// the stones themselves are facts about the hearer. `yours` is what a client
/// needs in order to know which cairns are its own player's doing — and what it
/// must never be sent is the [`Token`] that actually settles it, tokens being
/// credentials that go nowhere but to the client holding them.
///
/// And the name is withheld from anybody who has not been up to the cairn to
/// read it — see [`Knowing`]. On the wire that is indistinguishable from an
/// island nobody has christened, and deliberately so: a hearer who has only
/// sighted the stones has no business being able to tell *there is a name here
/// you may not read* from *there is no name here*. Both are a cairn with
/// nothing written on it as far as they can see, which is the truth of standing
/// a mile off.
fn cairn_told_to(
    player: &Player,
    island: IVec2,
    claim: &Claim,
    known: Option<Knowing>,
) -> ToClient {
    ToClient::Cairn {
        island,
        at: claim.at,
        name: match known {
            Some(Knowing::Visited) => claim.name.clone(),
            _ => String::new(),
        },
        yours: player.token == claim.by,
    }
}

/// One chunk of the world, surveyed exactly as a client would survey it.
///
/// Through [`Archipelago::chunk_payload`] and back out of it — quantised
/// heights and all — rather than off the generator's own `f32`s, and that is
/// the whole point of this function existing. A client surveys what it was
/// *sent*, and a waterline traced across heights the wire has rounded is not
/// quite the waterline traced across the heights before it: a corner a
/// centimetre either side of the sea puts the crossing somewhere else, and
/// two ends that disagree about a coastline disagree about whether it closes.
/// So the server asks the same question of the same numbers.
///
/// Open water carries no ground, and comes back surveyed and blank — which is
/// exactly what a client makes of an ocean answer, and is worth recording:
/// water somebody has crossed is not water nobody has.
fn survey_chunk(world: &Archipelago, chunk: IVec2) -> Soundings {
    match world.chunk_payload(chunk) {
        None => Soundings::default(),
        Some(payload) => {
            let heights: Vec<f32> = payload.heights.iter().copied().map(dequantize).collect();
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
/// A connection thread's own local, not a field on the roster. A player's
/// reports are read by exactly one thread, and this is about the rate that
/// thread is being asked to work at — nothing shared has any business with
/// it, and on the roster it would only be another thing taken under that
/// lock.
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
    /// any real hull ever makes — a boat at ten metres a second, reporting
    /// ten times a second, spends a metre of an allowance filling twenty-five
    /// times as fast. One beyond it is neither called a lie nor thrown away:
    /// the survey simply follows at the speed it believes in, and the rest of
    /// the way is there to be had once the allowance has filled again. So a
    /// client hopping about the world orders ground worked out at the pace of
    /// somebody sailing, whatever pace it sends at.
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
/// because both halves have to be worked off the same way. The [`Wake`] is what
/// says how much of a report is believed, and it must be asked exactly once per
/// report: asked twice it would charge one run's allowance twice over, and the
/// second answer would be a way from where the first one got to.
///
/// The way rather than its end, for both halves and for the same reason: a hull
/// between two position reports must slip past neither a coast nor a cairn. See
/// [`protocol::survey::in_sight_along`] for the one rule and [`off_the_way`]
/// for the other.
fn follow_the_way(shared: &Shared, id: PlayerId, wake: &mut Wake, now: Vec2) {
    let (from, to) = wake.follows(now);
    survey_the_way(shared, id, from, to);
    sight_the_cairns(shared, id, from, to);
}

/// Surveys whatever came within sight along a way, adds it to this player's
/// survey and tells them what was found there.
///
/// The ground is worked out with no lock held, because working it out may
/// mean generating an island — tens to hundreds of milliseconds, on this
/// connection's own thread and outside the worker pool. For a client that
/// draws the world it never comes to that: a chunk within [`SIGHT_RADIUS`] is
/// one the client streamed the ground of long ago, so the island is in the
/// world's cache and this is arithmetic over a grid. That is a fact about a
/// cooperative client, though, and not a bound — a client that asks for no
/// ground at all and only reports positions would be ordering islands grown
/// from nothing. What bounds it is the [`Wake`]'s pace, which is about the
/// world rather than about how fast a socket can be fed — and which
/// [`follow_the_way`] has already applied to the way handed in here.
///
/// The locks are taken around all that rather than across it, and the
/// survey's own is taken with the roster's let go — see [`Surveyed`].
fn survey_the_way(shared: &Shared, id: PlayerId, from: Vec2, to: Vec2) {
    let (least, most) = scanned_for_sight(from, to);

    let surveyed = {
        let players = shared.players.lock().expect("no poisoned lock");
        let Some(player) = players.get(&id) else {
            return;
        };
        player.surveyed.clone()
    };
    let fresh: Vec<IVec2> = {
        let known = surveyed.lock().expect("no poisoned lock");
        (least.y..=most.y)
            .flat_map(|z| (least.x..=most.x).map(move |x| IVec2::new(x, z)))
            .filter(|chunk| {
                // Held to the same reach the ground is, and that is not
                // tidiness: the world's own file refuses a surveyed chunk
                // that fails this, so a survey allowed to hold one would be
                // a world that saved and then could not be opened again. A
                // legal report from just inside the edge has chunks in sight
                // whose corners are past it, which is how that happens
                // honestly.
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
        let mut surveyed = surveyed.lock().expect("no poisoned lock");
        for (chunk, ink) in &found {
            surveyed.record(*chunk, ink.clone());
        }
    }
    let players = shared.players.lock().expect("no poisoned lock");
    post_the_survey(&players, id, found);
}

/// Tells a returning player back the survey the world remembers them having
/// taken, worked out afresh from the ground.
///
/// On a thread of its own, and that is not tidiness. The chunks are wherever
/// this player has ever been, which is nowhere near where they are standing
/// now, so the islands under them are not in the world's cache and every one
/// has to be grown again — seconds of work for a well-sailed world. Done on
/// the connection's own thread it would be seconds in which that client's
/// requests for the ground under its feet went unread, at exactly the moment
/// it has nothing to draw. So the ink arrives a batch at a time while the
/// world opens around them, which is also how it reads: the chart fills in.
///
/// The one number here worth revisiting first is how much is re-derived. An
/// hour's sailing is on the order of a thousand chunks and a few hundred
/// kilobytes on the wire, which at this scale is fine; a world somebody has
/// lived in for a season would want the ink kept rather than re-earned.
fn tell_the_survey_so_far(shared: &Arc<Shared>, id: PlayerId) {
    let surveyed = {
        let players = shared.players.lock().expect("no poisoned lock");
        let Some(player) = players.get(&id) else {
            return;
        };
        player.surveyed.clone()
    };
    let known: Vec<IVec2> = {
        let mut known: Vec<IVec2> = surveyed
            .lock()
            .expect("no poisoned lock")
            .charted()
            .collect();
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
            // Two ways there is nobody left to tell, and both have to end
            // this loop rather than only slow it: a world that has ended, and
            // a player who has hung up. Neither is noticed by posting — that
            // quietly does nothing — while the growing of islands carries on
            // regardless, so a client reconnecting in a loop would stack a
            // thread of these per attempt, each holding the world open and
            // each filling its cache with the coast of everywhere that
            // player had ever been.
            if shared.stopping.load(Ordering::Relaxed) {
                return;
            }
            if !shared
                .players
                .lock()
                .expect("no poisoned lock")
                .contains_key(&id)
            {
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
                let mut surveyed = surveyed.lock().expect("no poisoned lock");
                for (chunk, ink) in &found {
                    surveyed.record(*chunk, ink.clone());
                }
            }
            let players = shared.players.lock().expect("no poisoned lock");
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
/// How the paced asks are rationed — see [`CAIRN_PACE`]. A rate is held down
/// here by making the asker wait rather than by throwing the ask away: a
/// dropped ask is indistinguishable, from the far end of a socket, from one
/// that was refused or one that was lost, and a client cannot be expected to
/// tell those apart. Slowing the connection that asked is honest about what is
/// happening and costs nobody else anything, this being that connection's own
/// thread and no lock held while it sleeps.
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
/// Both ends of the world file ask exactly this, and that is the point of it
/// being one function. Nothing that fails it is granted or written down (see
/// [`settle_a_claim`] and [`Shared::record`]), and nothing that fails it is
/// read back (see `keeper::parse`) — because a claim the reader refuses is not
/// a claim lost but a *world* lost: the file will not parse, the opening falls
/// back to the copy beside it, and the next save takes that too.
pub(crate) fn island_in_the_world(island: IVec2) -> bool {
    in_the_world(protocol::survey::chunk_of(island))
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
