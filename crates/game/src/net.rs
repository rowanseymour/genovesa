//! Playing in a served world: the game as a client, and — when this machine is
//! the one hosting — as the server's landlord too.
//!
//! Every world is a served world. The connection carries the session — who
//! else is in the world and where they are, drawn as marker capsules — and it
//! carries the world itself, a chunk of ground at a time: this module puts the
//! requests [`crate::terrain::Ground`] wants onto the wire and the answers
//! back into it. The welcome names no seed, because a client has nothing to
//! generate; what it names is where this player stands and which way to look.
//!
//! A [`Connection`] is made either from the command line, before the app
//! exists, or from a menu screen with the frame loop already running — the
//! second of which is why dialling has [`Dialing`], a thread and a channel,
//! rather than being a function the menu could simply call. Once made, a
//! reader thread turns the socket into a channel the schedule drains once a
//! frame ([`receive`]), and the player's own movements trickle back the other
//! way ([`report_position`]).
//!
//! Hosting is the same picture with a server behind it: the `server` crate is
//! headless and engine-free, so a game opening a world runs one on a thread
//! and then joins it over the loopback like anybody else. That is true of
//! *every* world started from this machine, shared or not — see [`Reach`].
//! There is no second, quieter implementation of a session to play alone
//! against, and no way at all to be in a world without a server making it.

use std::collections::{HashMap, HashSet};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use protocol::ground::ChunkPayload;
use protocol::survey::Soundings;
use protocol::{
    BeastId, BeastKind, BoatId, BoatKind, PlayerId, ToClient, ToServer, Token, WorldId,
    DEFAULT_PORT, PROTOCOL_VERSION,
};
use server::{Host, Server};

use crate::player::PlayerPlace;
use crate::sea;
use crate::terrain::Ground;
use crate::told::{eased_onto, Told};
use crate::{matte, AppState};

/// Seconds between position reports, at least. Ten a second reads as
/// continuous once markers ease between them, and keeps an idle wire quiet.
const REPORT_INTERVAL: f32 = 0.1;

/// Metres of movement below which nothing is reported — a player standing
/// still costs the wire nothing.
const REPORT_THRESHOLD: f32 = 0.25;

/// Radians of swing below which nothing is reported — a hull holding its
/// course costs the wire nothing either. Small, because a bearing is what
/// another client draws a whole ship along.
const REPORT_SWING: f32 = 0.02;

/// How long a report may spend trying to reach the server. Reports are written
/// straight from the schedule, so this is time the player would spend watching
/// a frozen frame: a server that stopped reading its socket would otherwise
/// hold the game still the moment the send buffer filled. Kept short because a
/// lost report costs nothing — positions are absolute, not steps, and the next
/// one supersedes it a tenth of a second later.
const REPORT_TIMEOUT: Duration = Duration::from_millis(100);

/// How quickly a marker eases towards where the server last put its player,
/// in e-foldings per second — see [`crate::eased`]. Positions arrive a few times a
/// second, so the easing is what turns the steps back into movement.
const MARKER_SMOOTHING: f32 = 8.0;

/// Radius of the capsule another player appears as.
const MARKER_RADIUS: f32 = 0.8;
/// Length of the capsule's straight middle. With its two caps it stands
/// about four metres tall — oversized for a person, but a marker has to be
/// found from a camera hundreds of metres up.
const MARKER_LENGTH: f32 = 2.4;

/// The line to a server, held open for the life of the app.
pub struct Connection {
    /// The socket, written to straight from the schedule: a position report
    /// is a dozen bytes down a no-delay socket, not worth a thread.
    stream: TcpStream,
    /// What the reader thread has heard. Behind a mutex only because a Bevy
    /// resource must be `Sync`; nothing but [`Connection::drain`] locks it.
    incoming: Mutex<Receiver<ToClient>>,
    /// Who the server says we are. Nothing uses it yet, but a session where
    /// the client doesn't know its own name would be a strange one.
    pub id: PlayerId,
    /// Where the server puts arriving players down.
    pub spawn: Vec2,
    /// A ground point the opening view is turned towards — the island the
    /// spawn stands off. Equal to the spawn when the server had nothing in
    /// particular to offer, which names no direction and leaves the bearing
    /// alone.
    pub facing: Vec2,
    /// Which world this is — what the client's own files about the world are
    /// keyed by. See [`protocol::WorldId`].
    pub world: WorldId,
    /// The token this player now holds the world by, for the logbook to
    /// keep and the next visit to present.
    pub token: Token,
    /// The boat whose helm this player enters at, if any — `None` is a
    /// player entering on their own feet. The hull itself arrives as a
    /// telling; this is only how entry knows not to spawn a walker.
    pub aboard: Option<BoatId>,
}

impl Connection {
    /// Dials a server and speaks the handshake, blocking until the welcome
    /// arrives. `addr` is `host` or `host:port`; a bare host gets
    /// [`DEFAULT_PORT`]. The errors are strings because they are for the
    /// player, not for matching on.
    pub fn join(addr: &str) -> Result<Self, String> {
        let addr = dialled_as(addr);
        let stream =
            TcpStream::connect(&addr).map_err(|error| format!("cannot reach `{addr}`: {error}"))?;
        // Position reports matter now or not at all — see the server's twin
        // of this line.
        let _ = stream.set_nodelay(true);
        // The handshake is bounded: a host that accepts and then says nothing
        // would otherwise hang the game before it opened. The session itself
        // has no deadline — going quiet is what an idle server sounds like —
        // so the welcome lifts the timeout again below.
        let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));

        (ToServer::Hello {
            version: PROTOCOL_VERSION,
        })
        .write(&mut &stream)
        .map_err(|error| format!("`{addr}` hung up mid-greeting: {error}"))?;

        // The server names which world this is, and the papers answer: the
        // token the logbook holds for that world, or nothing — being new
        // here. The refusal a version mismatch earns arrives here, in the
        // world's place.
        let world = match ToClient::read(&mut &stream) {
            Ok(ToClient::World { id }) => id,
            Ok(ToClient::Refused { version }) => {
                return Err(format!(
                    "`{addr}` speaks protocol {version}, this build speaks {PROTOCOL_VERSION}"
                ))
            }
            Ok(other) => return Err(format!("`{addr}` is talking nonsense: {other:?}")),
            Err(error) => return Err(format!("no answer from `{addr}`: {error}")),
        };
        let presented = crate::logbook::token_for(world);
        (ToServer::Papers { token: presented })
            .write(&mut &stream)
            .map_err(|error| format!("`{addr}` hung up mid-greeting: {error}"))?;

        match ToClient::read(&mut &stream) {
            Ok(ToClient::Welcome {
                id,
                spawn,
                facing,
                token,
                aboard,
            }) => {
                let _ = stream.set_read_timeout(None);
                let _ = stream.set_write_timeout(Some(REPORT_TIMEOUT));
                // From here the socket splits: this thread reads it forever,
                // the schedule writes it. The channel closing on either side
                // — the game dropping the connection, the server hanging up
                // — quietly ends the other.
                let (heard, incoming) = mpsc::channel();
                let mut reader = stream
                    .try_clone()
                    .map_err(|error| format!("cannot split the connection: {error}"))?;
                thread::spawn(move || {
                    while let Ok(message) = ToClient::read(&mut reader) {
                        if heard.send(message).is_err() {
                            break;
                        }
                    }
                });
                Ok(Self {
                    stream,
                    incoming: Mutex::new(incoming),
                    id,
                    spawn,
                    facing,
                    world,
                    token,
                    aboard,
                })
            }
            Ok(other) => Err(format!("`{addr}` is talking nonsense: {other:?}")),
            Err(error) => Err(format!("no welcome from `{addr}`: {error}")),
        }
    }

    /// Tells the server where the player is.
    ///
    /// A write that failed — the server gone, or [`REPORT_TIMEOUT`] spent
    /// waiting on one that has stopped reading — may have left half a frame on
    /// the wire, and nothing sent after that could be read as a message. So
    /// the line is finished off here rather than limped along: the reader
    /// thread ends with it, and [`drain`] is where the loss is surfaced, once.
    ///
    /// [`drain`]: Connection::drain
    fn report(&self, position: Vec2) {
        self.say(ToServer::Move { position });
    }

    /// Asks for one chunk of ground.
    ///
    /// Fire and forget, like a position report: the answer arrives through the
    /// reader thread whenever the server gets to it, and a request that never
    /// reached the wire is a connection that has ended.
    fn ask_for(&self, chunk: IVec2) {
        self.say(ToServer::WantChunk { chunk });
    }

    /// Says that this player is at anchor and would like the night over.
    ///
    /// Public where the first two are not, because unlike the ground and the
    /// player's position this is not something this module decides to send:
    /// [`crate::sky`] owns the night and the key that waits it out, and all
    /// that belongs here is the wire.
    pub fn want_dawn(&self) {
        self.say(ToServer::WantDawn);
    }

    /// Puts a console line on the wire, verbatim — [`crate::console`] owns
    /// the typing and decides what crosses; the answer comes back through
    /// [`receive`] as a [`ToClient::Reply`]. Public on the same terms as
    /// [`Connection::want_dawn`].
    pub fn command(&self, line: String) {
        self.say(ToServer::Command { line });
    }

    /// Asks for a boat's helm — [`crate::player`] owns the key and judges
    /// the reach; the answer comes back as a [`protocol::ToClient::Boat`]
    /// telling, believed when it lands rather than assumed when it is sent.
    /// Public on the terms of [`Connection::want_dawn`].
    pub fn board(&self, boat: protocol::BoatId) {
        self.say(ToServer::Board { boat });
    }

    /// Steps off the helm we hold, landing at `position` — the spot whose
    /// footing [`crate::player`] already judged. Public on the same terms.
    pub fn disembark(&self, position: Vec2) {
        self.say(ToServer::Disembark { position });
    }

    /// Lowers the ship's boat at `position` — the berth [`crate::player`]
    /// already chose alongside — and asks to be seated in it; the answer
    /// comes back as the pair of boat tellings the wire promises, believed
    /// when they land. Public on the same terms.
    pub fn lower(&self, position: Vec2, heading: f32) {
        self.say(ToServer::Lower { position, heading });
    }

    /// Claims the island the player is standing on, which the server reads
    /// from where they stand and grants against its own survey or refuses —
    /// see [`crate::player`], which owns the key. What comes back is a cairn,
    /// word that the survey is unfinished, or nothing at all.
    pub fn claim(&self) {
        self.say(ToServer::Claim);
    }

    /// Writes a name on an island this player holds. The name is the world's
    /// to accept or refuse; nothing is drawn until it says so — see
    /// [`crate::chart::write_through`].
    pub fn christen(&self, island: IVec2, name: &str) {
        self.say(ToServer::Name {
            island,
            name: name.to_string(),
        });
    }

    fn say(&self, message: ToServer) {
        if message.write(&mut &self.stream).is_err() {
            let _ = self.stream.shutdown(Shutdown::Both);
        }
    }

    /// Everything heard since the last frame, and whether the line is still
    /// up.
    fn drain(&self) -> (Vec<ToClient>, bool) {
        let incoming = self.incoming.lock().expect("no poisoned lock");
        let mut messages = Vec::new();
        loop {
            match incoming.try_recv() {
                Ok(message) => messages.push(message),
                Err(TryRecvError::Empty) => return (messages, true),
                Err(TryRecvError::Disconnected) => return (messages, false),
            }
        }
    }
}

/// Hanging up, and meaning it.
///
/// Letting the socket drop is not enough on its own: the reader thread holds a
/// clone of the same connection, blocked in a read, and a connection with a
/// second owner stays open. The server would go on holding a phantom player,
/// still relaying their last position to everyone else, for as long as this
/// process lived — and the reader thread would never end. Shutting down closes
/// it for both owners at once, which ends the thread and lets the server hear
/// the departure it is entitled to.
impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

/// Where this machine keeps the worlds it opens from the menu — what
/// [`Session::open`] files them into and the set-sail screen lists. `None`
/// on a machine with nowhere to keep anything.
pub fn worlds_dir() -> Option<PathBuf> {
    Some(server::data_dir()?.join("worlds"))
}

/// What a server named as `host` or `host:port` is dialled as: a bare name
/// gets [`DEFAULT_PORT`], which is the whole of what a player has to be told
/// to join somebody.
fn dialled_as(address: &str) -> String {
    if address.contains(':') {
        address.to_string()
    } else {
        format!("{address}:{DEFAULT_PORT}")
    }
}

/// A session in hand, and the world behind it when this machine is the one
/// hosting.
pub struct Session {
    pub connection: Connection,
    /// The server serving this session, when it is our own. Held for the life
    /// of the match — see [`Hosting`].
    pub hosting: Option<Host>,
    /// Whether the hosted world outlives this session — kept in the game's
    /// worlds directory, to be offered again from the menu. Always false for
    /// a joined session, whose keeping is the far host's business; what it
    /// decides here is whether this machine opens a logbook (see
    /// [`crate::logbook::for_session`]) — a world both ephemeral and our own
    /// is not worth remembering, since it will never exist again.
    pub kept: bool,
}

impl Session {
    /// Joins somebody else's world, blocking until the welcome arrives.
    pub fn joining(address: &str) -> Result<Self, String> {
        Ok(Self {
            connection: Connection::join(address)?,
            hosting: None,
            kept: false,
        })
    }

    /// Opens a world on this machine and joins it, blocking likewise.
    ///
    /// Every world started here goes through this, invited guests or not: the
    /// ground comes from a server, so playing alone means running one and
    /// talking to it over the loopback. [`Reach`] is the only difference
    /// between the two, and it is a question about the network rather than
    /// about the session.
    ///
    /// Joined over the loopback whatever it is bound to: whoever opened the
    /// world gets there the short way.
    ///
    /// `opening` is the hour of its day the world starts at, as a phase —
    /// [`server::OPENING`] for a world nobody asked anything particular of.
    /// `keep` files the world in this machine's worlds directory, to be
    /// offered again from the menu: what the menu asks and the command line
    /// does not, a `--seed` run being a world to look at rather than one to
    /// live in.
    pub fn open(seed: u32, reach: Reach, opening: f32, keep: bool) -> Result<Self, String> {
        let mut server = Server::bind(reach.bound_to(), seed)
            .map_err(|error| format!("cannot open a world: {error}"))?
            .opening_at(opening);
        if keep {
            let worlds = worlds_dir()
                .ok_or_else(|| "this machine has nowhere to keep a world".to_string())?;
            server = server
                .keeping_in(&worlds)
                .map_err(|error| format!("cannot keep the world: {error}"))?;
        }
        Self::hosting(server, keep)
    }

    /// Reopens a kept world and joins it — the same world, aged only by the
    /// time it was actually open, with this player where it last saw them.
    pub fn reopening(path: PathBuf, reach: Reach) -> Result<Self, String> {
        let server = Server::reopen(reach.bound_to(), &path)
            .map_err(|error| format!("cannot reopen the world: {error}"))?;
        Self::hosting(server, true)
    }

    fn hosting(server: Server, kept: bool) -> Result<Self, String> {
        let host = server
            .spawn()
            .map_err(|error| format!("cannot open a world: {error}"))?;
        Ok(Self {
            connection: Connection::join(&format!("127.0.0.1:{}", host.addr().port()))?,
            hosting: Some(host),
            kept,
        })
    }
}

/// Who can reach a world this machine opens.
///
/// The whole of the difference between keeping a world and sharing it. A world
/// is served either way — there is no such thing here as a world without a
/// server — so this is a question about the network and not about the session:
/// whether the listener is reachable from other machines, and on a port they
/// could be told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// This machine only, on whatever port happens to be free. Nothing outside
    /// can connect, and nothing outside needs to know a port was used.
    Alone,
    /// Every interface, on [`DEFAULT_PORT`] — which is what the people being
    /// shared with have to be able to guess.
    Shared,
}

impl Reach {
    fn bound_to(self) -> SocketAddr {
        match self {
            Self::Alone => ([127, 0, 0, 1], 0).into(),
            Self::Shared => ([0, 0, 0, 0], DEFAULT_PORT).into(),
        }
    }

    /// What to call this dial if it dies without saying anything — see
    /// [`Dialing::what`]. Player-facing, so it describes the place rather than
    /// the socket.
    fn described(self) -> String {
        match self {
            Self::Alone => "a world of your own".into(),
            Self::Shared => format!("a world for others to join, port {DEFAULT_PORT}"),
        }
    }
}

/// A session being made, on a thread of its own.
///
/// Dialling blocks: a name to look up, a connection to make, a handshake to
/// wait for, and — for a host that accepts and then says nothing — ten seconds
/// of that. A run started with `--join` can afford to do it before the app
/// exists, since what it learns is what the app is built from. A menu screen
/// cannot: it has to go on drawing, and taking a button click means being able
/// to take the one that gives up too. So the dialling happens on a thread and
/// the screen asks after it once a frame.
#[derive(Resource)]
pub struct Dialing {
    /// The channel until the dial has answered on it, and `None` from then
    /// on — see [`Dialing::outcome`], which is the only thing that touches it.
    /// (The mutex is only because a Bevy resource must be `Sync`.)
    outcome: Mutex<Option<Receiver<Result<Session, String>>>>,
    /// What is being dialled, for the one thing that has to name it: a dial
    /// whose thread died says so, and "dialling came to nothing" would leave
    /// the player nothing to check. The screens write their own waiting line
    /// from the address they already hold, so nothing outside reads this.
    what: String,
}

impl Dialing {
    /// Starts dialling a server named as `host` or `host:port`.
    pub fn to(address: &str) -> Self {
        let address = address.to_string();
        Self::on(address.clone(), move || Session::joining(&address))
    }

    /// Starts opening a world on this machine — see [`Session::open`], which
    /// this is the off-the-frame-loop way to reach.
    pub fn opening(seed: u32, reach: Reach, opening: f32, keep: bool) -> Self {
        Self::on(reach.described(), move || {
            Session::open(seed, reach, opening, keep)
        })
    }

    /// Starts reopening a kept world — see [`Session::reopening`].
    pub fn reopening(path: PathBuf, reach: Reach) -> Self {
        Self::on(reach.described(), move || Session::reopening(path, reach))
    }

    fn on(what: String, dial: impl FnOnce() -> Result<Session, String> + Send + 'static) -> Self {
        let (outcome, waiting) = mpsc::channel();
        thread::spawn(move || {
            // A screen that has given up drops the receiver, and this send
            // fails with the session still in it — which drops the connection,
            // and the server behind it if this was a world being hosted. A
            // world nobody waited for is a world nobody is in.
            let _ = outcome.send(dial());
        });
        Self {
            outcome: Mutex::new(Some(waiting)),
            what,
        }
    }

    /// What came of it, or `None` while it is still ringing — and `None` ever
    /// after, once it has answered: whoever takes the outcome owns the
    /// session, so a screen asks until it gets something and then drops this
    /// resource.
    pub fn outcome(&self) -> Option<Result<Session, String>> {
        let mut waiting = self.outcome.lock().expect("no poisoned lock");
        let outcome = match waiting.as_ref()?.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return None,
            // The thread sends whatever the dial came to, success or failure,
            // so a channel that closed without one is a thread that died —
            // this crate's bug, but a screen that says so can still be left.
            Err(TryRecvError::Disconnected) => {
                Err(format!("dialling {} came to nothing", self.what))
            }
        };
        // Letting go of the channel is what makes the answer final, and it has
        // to be done here rather than left to the caller's dropping the
        // resource. A channel kept past its answer has no way to tell the
        // thread's ordinary end from its death: the dial thread drops its
        // sender when it ends, which is *after* the send, so a second ask read
        // an answered dial as nothing at all for as long as that took and as a
        // thread that died with nothing to say from then on. Neither is true,
        // and which one came back depended on how the threads were scheduled.
        *waiting = None;
        Some(outcome)
    }
}

/// The world this machine is hosting, for as long as the player is in it.
///
/// Mostly, holding it *is* what it does: dropping the handle stops the server
/// and hangs up on everyone in the world, so it lives exactly as long as the
/// host's own visit — inserted when the world is entered, removed on the way
/// out by [`disconnect`]. It is also the one thing in a match that knows which
/// world this is, which is why the debug readout asks it for the seed.
#[derive(Resource)]
pub struct Hosting(pub Host);

impl Hosting {
    /// Whether anyone off this machine could be in this world — which is the
    /// difference between leaving a world of one's own and shutting one on
    /// other people.
    ///
    /// Read off the address actually bound rather than remembered from the
    /// dial. A world of one's own is served too, over the loopback, so merely
    /// hosting says nothing about who can reach it; being bound somewhere a
    /// stranger could dial is the whole of what makes a world shared.
    pub fn shared(&self) -> bool {
        !self.0.addr().ip().is_loopback()
    }
}

/// The joined session. Present only in a run that is playing in a served
/// world; every system here conditions on it, so a local world pays nothing.
#[derive(Resource)]
pub struct Online {
    /// The line itself. Public because a module that owns a piece of the
    /// world — [`crate::sky`] and the night it waits out — has something of
    /// its own to say on it, and what may be said is [`Connection`]'s few
    /// public methods rather than the socket.
    pub connection: Connection,
    /// The marker standing in for each player the server has introduced.
    ///
    /// Kept, rather than found by looking through the markers themselves,
    /// because a marker is spawned through `Commands` and so does not exist
    /// until the system that spawned it has ended. Two messages about one
    /// player in a single frame's drain — an arrival and a departure, an
    /// arrival and a move — would otherwise be searching for an entity that
    /// is still only a queued command: the departure would find nothing and
    /// leave a marker nobody can ever remove, and the move would be dropped.
    ///
    /// Living in this resource is what keeps it from going stale. Markers are
    /// `DespawnOnExit(AppState::InWorld)` and this resource is removed on the
    /// same exit, so the map cannot outlive the entities it names.
    markers: HashMap<PlayerId, Entity>,
}

impl Online {
    /// A session with nobody in it yet — the server introduces the players it
    /// already has as its first messages after the welcome.
    pub fn new(connection: Connection) -> Self {
        Self {
            connection,
            markers: HashMap::new(),
        }
    }
}

/// Marks another player's marker. Where the server last put them is the
/// [`Told`] beside this, which every other told thing on screen carries too.
#[derive(Component)]
pub struct RemotePlayer {
    pub id: PlayerId,
}

pub struct NetPlugin;

impl Plugin for NetPlugin {
    fn build(&self, app: &mut App) {
        // The words themselves, and then the two sets that give them their
        // order: everything reading them runs after the drain that wrote
        // them, so a word lands on the frame it arrived rather than the one
        // after. The run condition is here rather than repeated on every
        // reader, which is most of what a set is for — and an app built
        // without this plugin leaves the sets unconfigured, which is what a
        // module's own lean tests want.
        app.add_message::<GroundArrived>()
            .add_message::<CoastSurveyed>()
            .add_message::<WindChanged>()
            .add_message::<HourTold>()
            .add_message::<BeastSeen>()
            .add_message::<BeastGone>()
            .add_message::<HullTold>()
            .add_message::<HullGone>()
            .add_message::<PutDown>()
            .add_message::<CairnSeen>()
            .add_message::<Uncharted>()
            .add_message::<ServerReplied>()
            .add_message::<VocabularyTaught>()
            .configure_sets(
                Update,
                (Wire::Heard, Wire::Read)
                    .chain()
                    .run_if(in_state(AppState::InWorld)),
            )
            // What `place_markers` floats on. Also initialised by the plugins
            // that draw and ride the sea; initialising a resource twice is
            // free, and each plugin's tests run it alone.
            .init_resource::<sea::SeaConditions>()
            // And the fleet, which this module both reports through and
            // shades markers by.
            .init_resource::<crate::boat::Fleet>()
            .add_systems(
                OnEnter(AppState::InWorld),
                enter_afoot.run_if(resource_exists::<Online>),
            )
            .add_systems(
                Update,
                receive
                    .in_set(Wire::Heard)
                    .run_if(resource_exists::<Online>),
            )
            // The markers, which are the one word this module reads itself.
            //
            // After the hulls, and that is not tidiness: a marker is hidden
            // while its player is at a helm, and which helms are held is
            // something a `Boat` telling this same frame may have changed.
            .add_systems(
                Update,
                // Shading before placing, so that a marker coming back out
                // from under a hull is put where it belongs before the same
                // frame eases and stands it on the ground there.
                (shade_markers, place_markers)
                    .chain()
                    .after(crate::boat::take_the_hulls)
                    .in_set(Wire::Read)
                    .run_if(resource_exists::<Online>),
            )
            // And what goes back the other way, after every word of this
            // frame has been acted on. `report_position` says whether this
            // player is at a helm, which the frame's own tellings decide, so
            // reporting before they are read would report the helm they held
            // a frame ago.
            .add_systems(
                Update,
                (ask_for_ground, report_position)
                    .after(Wire::Read)
                    .run_if(in_state(AppState::InWorld).and_then(resource_exists::<Online>)),
            )
            .add_systems(
                OnExit(AppState::InWorld),
                // The fleet forgotten here as well as by the boat plugin,
                // because this module holds one too — the helm decides which
                // way a position is reported and which markers are shaded.
                // Forgetting twice is clearing an empty map twice, and an app
                // with this plugin and not that one must still not carry one
                // world's hulls into the next.
                (disconnect, crate::boat::scuttle),
            );
    }
}

/// Leaving the world ends the session, whichever end of it we were.
///
/// Dropping the connection shuts our socket down, which is how the server
/// hears us go. Dropping the host — if the world was ours — then ends it for
/// everyone else, in that order, so that the guests hear the departure before
/// the world it was from stops existing. Neither may outlive the visit: the
/// next world entered from the menu is a different one, where the players of
/// this one would be strangers standing on ground that isn't theirs.
///
/// The markers need no clearing up here, being `DespawnOnExit` of the state
/// this is the exit from.
fn disconnect(mut commands: Commands) {
    commands.remove_resource::<Online>();
    commands.remove_resource::<Hosting>();
}

/// A colour of their own for each player, spread around the wheel by the
/// golden angle so any handful of ids lands well apart — no hand-picked
/// roster of colours to run out of.
fn marker_color(id: PlayerId) -> Color {
    Color::hsl((id.0 as f32 * 137.508) % 360.0, 0.65, 0.55)
}

// ---------------------------------------------------------------------------
// The words of a session, as this app's own
//
// One per thing a server can say that is not the session's own bookkeeping,
// written by `receive` and read by whichever module owns the thing it is
// about. Nothing below interprets anything: a message is the wire's sentence
// with its bytes already read and its numbers already vetted, and what it
// *means* belongs to the module that draws the thing.
//
// The arrangement is worth the extra names. `receive` used to apply every word
// itself, which made this module the one place that knew how a beast is drawn,
// what a chart records, where a hull is moored and what the console prints —
// twelve modules reached into from one system, and a thirteenth waiting for
// the next word a server learns. Now a module hears what is its own, one word
// can be heard by two modules that owe each other nothing (a cairn is a stone
// in the world *and* a letter on the chart), and the wire knows about none of
// them.
//
// Believing is still done here, once, in `receive`: a message exists only if
// its numbers are ones this machine can safely draw with for the rest of the
// session. See the notes there — every reader may take what it is handed at
// face value.

/// One chunk of ground, or the open water that is the absence of it — see
/// [`protocol::ToClient::Chunk`]. Read by [`crate::terrain`], which asked.
#[derive(Message)]
pub struct GroundArrived {
    pub chunk: IVec2,
    /// How much of the wind reaches this chunk, or `None` where nothing
    /// shelters it — see [`protocol::ToClient::Chunk`]. Beside the ground
    /// rather than inside it because a chunk with no ground at all may still
    /// lie in a headland's lee.
    pub shelter: Option<Vec<protocol::ground::Exposure>>,
    pub ground: Option<ChunkPayload>,
}

/// Coast this player has now surveyed — see [`protocol::ToClient::Surveyed`].
/// Read by [`crate::chart`], which is where ink lands.
#[derive(Message)]
pub struct CoastSurveyed {
    pub found: Vec<(IVec2, Soundings)>,
}

/// The wind over the whole world. Read by [`crate::sea`], which wears it.
#[derive(Message)]
pub struct WindChanged {
    pub wind: Vec2,
}

/// Where the world's day stands, as a phase. Read by [`crate::sky`], which
/// runs the clock on between tellings.
#[derive(Message)]
pub struct HourTold {
    pub phase: f32,
}

/// A beast, wherever it has got to — introduction and movement in one word,
/// as the wire has it. Read by [`crate::beasts`].
#[derive(Message)]
pub struct BeastSeen {
    pub id: BeastId,
    pub kind: BeastKind,
    pub position: Vec2,
    pub velocity: Vec2,
    pub surfaced: bool,
}

/// The server has stopped minding a beast. Read by [`crate::beasts`].
#[derive(Message)]
pub struct BeastGone {
    pub id: BeastId,
}

/// A boat, wherever it lies and in whosever hands. Read by [`crate::boat`].
#[derive(Message)]
pub struct HullTold {
    pub id: BoatId,
    pub kind: BoatKind,
    pub position: Vec2,
    pub heading: f32,
    pub occupant: Option<PlayerId>,
}

/// A boat is out of the world. Read by [`crate::boat`].
#[derive(Message)]
pub struct HullGone {
    pub id: BoatId,
}

/// The world has moved this player, whatever they thought — see
/// [`protocol::ToClient::PutDown`], which is the one word that overrules a
/// client about its own place.
///
/// Read by [`crate::player`], which moves whatever is carrying them, and by
/// [`crate::control`], where a driver may be waiting on the ground at the far
/// end. It must be read *after* [`HullTold`]: a player seated at a helm in the
/// same breath is carried by that hull, so the seating has to have been heard
/// before this is acted on. See [`Wire`].
#[derive(Message)]
pub struct PutDown {
    pub position: Vec2,
    pub heading: Option<f32>,
}

/// A cairn: seen from a distance, come near enough to read, or just raised.
///
/// Read by two modules that owe each other nothing — [`crate::cairn`] stands
/// the stone in the world, [`crate::chart`] letters the sheet — which is the
/// arrangement that made messages worth having.
#[derive(Message)]
pub struct CairnSeen {
    pub island: IVec2,
    pub at: Vec2,
    /// What the claim covers, in world metres — see
    /// [`protocol::ToClient::Cairn`]'s `covers`. The sheet's, the standing
    /// stone having no use for it.
    pub covers: Rect,
    pub name: String,
    pub yours: bool,
}

/// The answer to a claim from ground whose island the asker has not finished
/// surveying — see [`protocol::ToClient::Uncharted`]. Read by
/// [`crate::notice`], which is the one line it becomes.
#[derive(Message)]
pub struct Uncharted;

/// What the server said to a console line. Read by [`crate::console`], which
/// prints it, and by [`crate::control`], where a driver may be waiting on it.
#[derive(Message)]
pub struct ServerReplied {
    pub text: String,
}

/// The server's console vocabulary, for tab completion. Read by
/// [`crate::console`].
#[derive(Message)]
pub struct VocabularyTaught {
    pub phrases: Vec<String>,
}

/// The two halves of a frame's worth of wire.
///
/// A module reading the words that are its own puts its system in
/// [`Wire::Read`] and is then ordered after the drain without having to know
/// what does the draining. Nothing is chained across the whole of `Read` —
/// most of the words are about different things and may land in any order —
/// except the one pair that has to hold: see [`PutDown`].
///
/// [`NetPlugin`] is what gives these sets their order and their run
/// condition. An app built without it — a module's own lean tests — leaves
/// them unconfigured, which is a set that simply runs, and is what those
/// tests want.
///
/// A reader that forgets [`Wire::Read`] is the one mistake here that does not
/// announce itself: it still hears every word, a frame late, for ever. (A
/// word nobody registered, by contrast, panics the first time it is written.)
/// So the set is not decoration on a system that would work without it.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Wire {
    /// Draining the socket into the words above: [`receive`], alone.
    Heard,
    /// Acting on them, each module on its own.
    Read,
}

/// Turns everything the server said since last frame into this app's own
/// words, and applies the one part of it that is nobody else's: the markers
/// other players stand as.
///
/// Believing happens here and only here. The three checks are all the same
/// check — a number this machine will ease towards, and go on easing towards,
/// every frame from now on. One non-finite telling is a marker, a hull, a
/// beast or a sky at NaN for the rest of the session, and no later good word
/// mends it. The ceilings sit far above anything honest rather than at it, so
/// a server that learns to blow harder is not silently ignored.
///
/// A word that fails is dropped whole, which leaves the world exactly as this
/// client last heard it — the same answer a lost packet gives, and the only
/// one that is safe without knowing what the word was for.
fn receive(
    mut commands: Commands,
    mut online: ResMut<Online>,
    mut kit: crate::boat::HullKit,
    mut said: Words,
    mut lost: Local<bool>,
) {
    let (messages, connected) = online.connection.drain();
    if !connected && !*lost {
        // Once, not every frame. The markers simply stand where they were —
        // losing the server needn't end what is still a walkable world.
        *lost = true;
        warn!("lost the server — other players will stand where they were");
    }

    for message in messages {
        match message {
            ToClient::Joined { id, position } => {
                let marker = commands
                    .spawn((
                        Name::new(id.to_string()),
                        RemotePlayer { id },
                        told_marker(position),
                        DespawnOnExit(AppState::InWorld),
                        Mesh3d(kit.meshes.add(Capsule3d::new(MARKER_RADIUS, MARKER_LENGTH))),
                        MeshMaterial3d(kit.materials.add(matte(marker_color(id)))),
                        // Spelled out rather than left to the mesh's required
                        // components, because [`shade_markers`] writes it —
                        // a helmsman's capsule goes dark under their hull.
                        Visibility::default(),
                        // On the ground plane for now — `place_markers` owns
                        // the height from the next frame on.
                        Transform::from_xyz(position.x, 0.0, position.y),
                    ))
                    .id();
                online.markers.insert(id, marker);
            }
            ToClient::Moved { id, position } => {
                if let Some(&marker) = online.markers.get(&id) {
                    // Written as a command rather than through a query for the
                    // reason the map itself exists: a player who arrived
                    // earlier in this same drain has no component to reach for
                    // yet, only a spawn queued ahead of this. Overwriting is
                    // the whole of the update — where the server last put a
                    // player is all a marker knows about them.
                    commands.entity(marker).insert(told_marker(position));
                }
            }
            ToClient::Left { id } => {
                if let Some(marker) = online.markers.remove(&id) {
                    commands.entity(marker).despawn();
                }
            }
            ToClient::Chunk {
                chunk,
                shelter,
                ground: sent,
            } => {
                said.ground.write(GroundArrived {
                    chunk,
                    shelter,
                    ground: sent,
                });
            }
            ToClient::Surveyed { found } => {
                // Nothing is checked — a mark is two bytes and cannot be
                // non-finite, and what a coast *means* is the wire's own
                // arithmetic rather than something a client re-derives and
                // could disagree about.
                said.coast.write(CoastSurveyed { found });
            }
            ToClient::Weather { wind } => {
                // A target, not an order: the drawn sea eases towards it —
                // see [`crate::sea::settle_conditions`] — so the server's
                // occasional quantised updates arrive as weather rather than
                // as steps. Which is also why it is vetted: a non-finite
                // wind, once eased into the conditions, is NaN for good.
                if wind.is_finite() && wind.length() < 100.0 {
                    said.wind.write(WindChanged { wind });
                }
            }
            ToClient::Daylight { phase } => {
                // An hour outside the day is a broken or hostile server, and
                // a non-finite one eased into the clock would leave this
                // machine with no time of day at all, for good.
                if phase.is_finite() && (0.0..1.0).contains(&phase) {
                    said.hour.write(HourTold { phase });
                }
            }
            ToClient::Beast {
                id,
                kind,
                position,
                velocity,
                surfaced,
            } => {
                if position.is_finite() && velocity.is_finite() && velocity.length() < 50.0 {
                    said.beast.write(BeastSeen {
                        id,
                        kind,
                        position,
                        velocity,
                        surfaced,
                    });
                }
            }
            ToClient::BeastGone { id } => {
                said.beast_gone.write(BeastGone { id });
            }
            ToClient::Reply { text } => {
                said.reply.write(ServerReplied { text });
            }
            ToClient::Vocabulary { phrases } => {
                said.vocabulary.write(VocabularyTaught { phrases });
            }
            ToClient::Boat {
                id,
                kind,
                position,
                heading,
                occupant,
            } => {
                if position.is_finite() && heading.is_finite() {
                    said.hull.write(HullTold {
                        id,
                        kind,
                        position,
                        heading,
                        occupant,
                    });
                }
            }
            ToClient::BoatGone { id } => {
                said.hull_gone.write(HullGone { id });
            }
            ToClient::PutDown { position, heading } => {
                if position.is_finite() && heading.is_none_or(f32::is_finite) {
                    said.put_down.write(PutDown { position, heading });
                }
            }
            ToClient::Cairn {
                island,
                at,
                covers,
                name,
                yours,
            } => {
                if at.is_finite() && covers.0.is_finite() && covers.1.is_finite() {
                    said.cairn.write(CairnSeen {
                        island,
                        at,
                        covers: Rect::from_corners(covers.0, covers.1),
                        name,
                        yours,
                    });
                }
            }
            ToClient::Uncharted => {
                said.uncharted.write(Uncharted);
            }
            // The handshake consumed its own messages; a stray one now is a
            // server bug, not something to end a match over.
            ToClient::Welcome { .. } | ToClient::Refused { .. } | ToClient::World { .. } => {}
        }
    }
}

/// Every word [`receive`] can say, in one hand.
///
/// Together because they are one thing — the vocabulary of a session — and
/// not merely to keep a system's parameter list under the sixteen Bevy allows,
/// though twelve writers and three other parameters would have been at that
/// wall too. Adding a word to the wire is a field here and an arm there, and
/// nothing else in this module moves.
#[derive(SystemParam)]
struct Words<'w> {
    ground: MessageWriter<'w, GroundArrived>,
    coast: MessageWriter<'w, CoastSurveyed>,
    wind: MessageWriter<'w, WindChanged>,
    hour: MessageWriter<'w, HourTold>,
    beast: MessageWriter<'w, BeastSeen>,
    beast_gone: MessageWriter<'w, BeastGone>,
    hull: MessageWriter<'w, HullTold>,
    hull_gone: MessageWriter<'w, HullGone>,
    put_down: MessageWriter<'w, PutDown>,
    cairn: MessageWriter<'w, CairnSeen>,
    uncharted: MessageWriter<'w, Uncharted>,
    reply: MessageWriter<'w, ServerReplied>,
    vocabulary: MessageWriter<'w, VocabularyTaught>,
}

/// How a marker is drawn closing on the last word about its player. No
/// bearing: a walker's own is never reported, a capsule looks the same from
/// every side, and a `facing` of zero would be the world spinning somebody
/// north for no reason.
fn told_marker(at: Vec2) -> Told {
    Told {
        at,
        facing: None,
        closing: MARKER_SMOOTHING,
        swinging: MARKER_SMOOTHING,
    }
}

/// Puts the chunk requests the ground is waiting on onto the wire.
///
/// The streaming policy lives in [`crate::terrain`] and knows nothing about
/// connections; this knows nothing about which chunks are worth wanting. All
/// that passes between them is a list of coordinates.
fn ask_for_ground(online: Res<Online>, ground: Option<ResMut<Ground>>) {
    let Some(mut ground) = ground else {
        return;
    };
    for chunk in ground.take_requests() {
        online.connection.ask_for(chunk);
    }
}

/// How far apart two bearings are, in radians, the short way round the
/// circle — never more than half a turn.
///
/// Taken this way because a yaw comes off `atan2` and is cut at due south,
/// where +π and -π are the same bearing: a raw difference between two of
/// them is up to a whole turn for a hull that never moved its helm, and the
/// hull holding the one course that straddles the cut reported itself
/// turning at every interval of the voyage.
fn swing(from: f32, to: f32) -> f32 {
    let round = (to - from).rem_euclid(std::f32::consts::TAU);
    round.min(std::f32::consts::TAU - round)
}

/// Tells the server where the player is: where whatever carries them is —
/// the boat they are aboard, or one day their own feet — resolved through
/// [`PlayerPlace`] so this system never learns which.
fn report_position(
    time: Res<Time>,
    online: Res<Online>,
    fleet: Res<crate::boat::Fleet>,
    player: PlayerPlace,
    mut last: Local<Option<(f32, Vec2, f32)>>,
) {
    let (Some(position), Some(heading)) = (player.on_the_map(), player.heading()) else {
        return;
    };
    // The carrier's bearing as the wire's yaw — how the hull is pointed,
    // which the other clients draw and a capsule marker never needed.
    let yaw = f32::atan2(-heading.x, -heading.y);
    let now = time.elapsed_secs();

    if let Some((reported_at, reported, reported_yaw)) = *last {
        // A hull turning in place is moving news even though it goes
        // nowhere: the heading is drawn, so it reports on the same terms as
        // the position.
        let turned = swing(reported_yaw, yaw) > REPORT_SWING;
        if now - reported_at < REPORT_INTERVAL
            || (reported.distance(position) < REPORT_THRESHOLD && !turned)
        {
            return;
        }
    }
    // At a helm the report is the boat's — the server carries the rider
    // with the vehicle — and afoot it is the walker's own.
    match fleet.helmed {
        Some(_) => online.connection.say(ToServer::Helm {
            position,
            heading: yaw,
        }),
        None => online.connection.report(position),
    }
    *last = Some((now, position, yaw));
}

/// Enters the world on foot, for the player the welcome seated at no helm:
/// their boat lies wherever they left it, one telling among the rest, and
/// what entry owes them is a walker standing where the server said they
/// stand. The height starts at sea level and [`crate::player`] settles it
/// onto the ground once the ground has streamed in.
pub(crate) fn enter_afoot(
    mut commands: Commands,
    online: Res<Online>,
    view: Option<Res<crate::camera::View>>,
) {
    if online.connection.aboard.is_some() {
        return;
    }
    // The view only aims the walker — a headless test with no camera at all
    // gets one facing north, which nothing there looks at.
    let yaw = view.map_or(0.0, |view| view.yaw);
    let at = online.connection.spawn;
    commands.spawn((
        Name::new("Player"),
        crate::player::Player,
        crate::player::Unsettled,
        DespawnOnExit(AppState::InWorld),
        Transform::from_xyz(at.x, 0.0, at.y).with_rotation(Quat::from_rotation_y(yaw)),
        Visibility::default(),
    ));
}

/// Hides the capsule of anyone at a helm: their boat is their marker, told
/// and drawn in full, and a capsule riding its deck would be clutter over
/// the one thing on screen that already says who is where.
///
/// And puts a marker back where its player is the moment it comes out from
/// under the hull again, rather than letting [`place_markers`] ease it
/// there. Nothing is reported afoot while a player is at a helm — their
/// position crosses as the boat's — so a hidden marker's target sits at the
/// spot they boarded at, however far they then sailed. Eased, a player
/// stepping ashore after a voyage would be a capsule skating across the
/// water from the far side of it; the crossing is not a movement anybody
/// made, so it is not one to animate.
fn shade_markers(
    fleet: Res<crate::boat::Fleet>,
    mut markers: Query<(&RemotePlayer, Ref<Told>, &mut Visibility, &mut Transform)>,
    // Who has been under a hull and is not yet standing where they stepped
    // off. A player who leaves the world while still at one stays in here,
    // which is a handful of bytes for as long as the session lasts and
    // nothing else: ids are dealt once each, so the name never comes round
    // again to be wrongly snapped.
    mut adrift: Local<HashSet<PlayerId>>,
) {
    for (player, told, mut visibility, mut transform) in &mut markers {
        if fleet.crewed(player.id) {
            *visibility = Visibility::Hidden;
            adrift.insert(player.id);
            continue;
        }
        if adrift.contains(&player.id) {
            transform.translation.x = told.at.x;
            transform.translation.z = told.at.y;
            // Held until a word about the player themself lands, because
            // the two arrive as two: the helm is told free and the step
            // ashore follows, and a frame that read only the first would
            // snap to the boarding point and then glide the whole voyage
            // anyway. Until then the snap is to a target the marker is
            // already standing on, which costs nothing.
            if told.is_changed() {
                adrift.remove(&player.id);
            }
        }
        *visibility = Visibility::Inherited;
    }
}

/// Walks each marker towards where the server last put its player, and
/// stands it on the ground there.
fn place_markers(
    time: Res<Time>,
    ground: Option<Res<Ground>>,
    sea: Res<sea::SeaConditions>,
    mut markers: Query<(&Told, &mut Transform), With<RemotePlayer>>,
) {
    let (dt, elapsed) = (time.delta_secs(), time.elapsed_secs_wrapped());

    for (told, mut transform) in &mut markers {
        let at = eased_onto(&mut transform, told, dt);

        // Standing on the surface, capsule half-height above it, so a player
        // crossing open ocean is sailing it rather than walking the seabed —
        // and riding the swell the way the boat itself does, or a marker
        // crossing open water would stand still in a sea everything else is
        // bobbing on. Ground still generating keeps the last height, exactly
        // as the boat and the camera's own focus do.
        if let Some(surface) = sea.surface_over(ground.as_deref(), at, elapsed) {
            transform.translation.y = surface + MARKER_LENGTH * 0.5 + MARKER_RADIUS;
        }
    }
}

/// A pretend server on a loopback port: accepts one client, answers the
/// handshake, and hands the test the socket to keep speaking with. Lives out
/// here rather than in this module's tests because the menu's tests, which
/// join servers by clicking on things, want one too.
#[cfg(test)]
pub(crate) fn fake_server(spawn: Vec2, facing: Vec2) -> (String, Receiver<TcpStream>) {
    use std::net::TcpListener;

    // Joining reads (and playing on would write) logbooks, and a test's must
    // not be the player's.
    crate::testing::quarantine_data_dir();
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    let (handover, socket) = mpsc::channel();
    thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept");
        match ToServer::read(&mut &stream).expect("a first message") {
            ToServer::Hello { version } => assert_eq!(version, PROTOCOL_VERSION),
            other => panic!("expected a hello, got {other:?}"),
        }
        // An id no real world will ever mint — they are drawn from the
        // clock — so the joining client's logbook lookup finds nothing and
        // every fake join is a fresh arrival.
        (ToClient::World {
            id: protocol::WorldId(42),
        })
        .write(&mut &stream)
        .expect("world");
        match ToServer::read(&mut &stream).expect("papers") {
            ToServer::Papers { .. } => {}
            other => panic!("expected papers, got {other:?}"),
        }
        (ToClient::Welcome {
            id: PlayerId(1),
            spawn,
            facing,
            token: Token(7),
            // Afoot, so a test app entering this world spawns a walker and
            // owes the fake server no boat tellings.
            aboard: None,
        })
        .write(&mut &stream)
        .expect("welcome");
        let _ = handover.send(stream);
    });
    (addr, socket)
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    use bevy::state::app::StatesPlugin;
    use bevy::time::TimePlugin;

    use super::*;
    use crate::testing::run_until;
    use crate::Helm;

    /// A headless app with the net systems running in a match, and no
    /// terrain — markers then keep their height, which these tests ignore.
    ///
    /// The listeners are stood up by hand rather than by adding the plugins
    /// that carry them, and that is what these tests are: the wire end to
    /// end, from a real socket to the hulls, stones, ink and scrollback the
    /// words are about. Their own modules test what each does with a word;
    /// nothing but this tests that the word arrives at all, in the right
    /// order, off a socket somebody else is writing to. Adding the real
    /// plugins would drag a renderer, a UI tree and a boat's whole rig in to
    /// prove it.
    fn test_app(connection: Connection) -> App {
        let mut app = App::new();
        app.add_plugins((
            // The asset machinery because `receive` carries a HullKit now —
            // a telling can spawn a hull, and a hull is meshes.
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
            TimePlugin,
            StatesPlugin,
            NetPlugin,
        ))
        .init_state::<AppState>()
        .add_sub_state::<Helm>()
        // Registered with the asset server, not merely inserted: a boat
        // telling loads its hull's meshes through it — and a rowboat's,
        // being rigged, arrive as a whole scene.
        .init_asset::<Mesh>()
        .init_asset::<bevy::world_serialization::WorldAsset>()
        .init_resource::<Assets<StandardMaterial>>()
        // What the listeners below keep, which their own plugins would have
        // brought.
        .init_resource::<crate::cairn::Cairns>()
        .init_resource::<crate::console::Console>()
        .insert_resource(Online::new(connection))
        .add_systems(
            Update,
            (
                // The hulls before the put down, which is the one order the
                // wire fixes — see [`PutDown`].
                crate::boat::take_the_hulls,
                crate::boat::lose_the_hulls,
                crate::player::take_the_put_down,
                crate::cairn::raise_the_cairns,
                crate::console::hear_the_server,
                crate::console::learn_the_vocabulary,
            )
                .chain()
                .in_set(Wire::Read),
        )
        // The sheet is a world's, not a run's: the tests that want one insert
        // it themselves, exactly as entering a world does.
        .add_systems(
            Update,
            (crate::chart::ink_the_coast, crate::chart::letter_the_sheet)
                .in_set(Wire::Read)
                .run_if(resource_exists::<crate::chart::Chart>),
        );
        app.update();
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::InWorld);
        app.update();
        app
    }

    fn markers(app: &mut App) -> Vec<(PlayerId, Vec2)> {
        app.world_mut()
            .query::<(&RemotePlayer, &Told)>()
            .iter(app.world())
            .map(|(player, told)| (player.id, told.at))
            .collect()
    }

    /// Waits for a dial to land. It crosses real sockets and a thread, so a
    /// moment of patience is legitimate — five seconds of it is a failure.
    ///
    /// The quarantining is the caller's, done before the dial is started: a
    /// dial's handshake reads the data directory for its papers, and by the
    /// time there is a dial to wait on, the thread that will read it is
    /// already running.
    fn settle(dialing: &Dialing) -> Result<Session, String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(outcome) = dialing.outcome() {
                return outcome;
            }
            thread::sleep(Duration::from_millis(2));
        }
        panic!("the dial never landed");
    }

    #[test]
    fn opening_a_world_is_a_session_like_any_other() {
        // A world of one's own runs the very same server a dedicated one runs,
        // and then joins it: what comes back is an ordinary welcome. Bound to
        // the loopback on a port of the machine's choosing, which is what
        // `Reach::Alone` is — a test run cannot collide with a real server on
        // this machine, and neither can a player.
        crate::testing::quarantine_data_dir();
        let dialing = Dialing::opening(77, Reach::Alone, server::OPENING, false);
        let session = settle(&dialing).expect("the world should be opened and joined");

        assert!(
            session.hosting.is_some(),
            "the world was joined without anything serving it"
        );
        // And it is a real world: the server put this player down somewhere in
        // it and named the land to look at.
        assert_ne!(session.connection.spawn, session.connection.facing);
    }

    #[test]
    fn the_seed_asked_for_is_the_world_that_opens() {
        crate::testing::quarantine_data_dir();
        let first = settle(&Dialing::opening(77, Reach::Alone, server::OPENING, false))
            .expect("a world should open");
        let second = settle(&Dialing::opening(78, Reach::Alone, server::OPENING, false))
            .expect("a world should open");
        // A host can ask its own server which world it made — that is where
        // the debug readout's seed comes from.
        assert_eq!(first.hosting.as_ref().expect("hosting").seed(), 77);
        // But the seed never reaches a *client*, so that the chosen one
        // actually reached the generator shows only in two of them being two
        // places.
        assert_ne!(
            first.connection.spawn, second.connection.spawn,
            "two seeds opened onto the same patch of water"
        );
    }

    #[test]
    fn a_hosted_world_is_one_other_people_can_join() {
        // The point of sharing: the world the host is standing in is reachable
        // from outside, and whoever arrives is somebody else in the same
        // world rather than the host again.
        crate::testing::quarantine_data_dir();
        let dialing = Dialing::opening(3, Reach::Alone, server::OPENING, false);
        let session = settle(&dialing).expect("the world should be opened and joined");
        let port = session.hosting.as_ref().expect("hosting").addr().port();

        let guest = Connection::join(&format!("127.0.0.1:{port}")).expect("a guest should get in");
        assert_eq!(
            guest.facing, session.connection.facing,
            "the guest was let into a different world"
        );
        assert_ne!(guest.id, session.connection.id, "two players, one id");
    }

    #[test]
    fn a_dial_that_finds_nobody_reports_it() {
        // Port 1, where nothing has ever listened. The failure has to come
        // back as an outcome the screen can show, not as a hang.
        crate::testing::quarantine_data_dir();
        let dialing = Dialing::to("127.0.0.1:1");
        let error = settle(&dialing).err().expect("nobody home");
        assert!(
            error.contains("127.0.0.1:1"),
            "the error does not name what was dialled: {error}"
        );
    }

    #[test]
    fn a_dial_answers_once() {
        // An outcome is taken, not read: a second answer would be a second
        // session for one dial, or a failure reported twice over.
        crate::testing::quarantine_data_dir();
        let dialing = Dialing::to("127.0.0.1:1");
        settle(&dialing).err().expect("nobody home");
        assert!(
            dialing.outcome().is_none(),
            "a dial that has answered answered again"
        );
    }

    #[test]
    fn joining_learns_where_it_is_from_the_welcome() {
        let (addr, _socket) = fake_server(Vec2::new(100.0, -200.0), Vec2::new(100.0, -400.0));
        let connection = Connection::join(&addr).expect("join");
        assert_eq!(connection.id, PlayerId(1));
        assert_eq!(connection.spawn, Vec2::new(100.0, -200.0));
        assert_eq!(connection.facing, Vec2::new(100.0, -400.0));
    }

    #[test]
    fn a_bare_host_gets_the_default_port() {
        // Asked of the naming rather than of a dial: a dial could only show
        // this by failing to reach the port, and the machine a test runs on is
        // exactly the machine somebody might be hosting a world from.
        assert_eq!(
            dialled_as("example.com"),
            format!("example.com:{DEFAULT_PORT}")
        );
        assert_eq!(dialled_as("example.com:4000"), "example.com:4000");
    }

    #[test]
    fn a_refusal_is_a_readable_error() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr").to_string();
        thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            let _ = ToServer::read(&mut &stream);
            (ToClient::Refused { version: 9 })
                .write(&mut &stream)
                .expect("refuse");
        });

        let error = Connection::join(&addr).err().expect("should be refused");
        assert!(
            error.contains("protocol 9") && error.contains(&PROTOCOL_VERSION.to_string()),
            "the error does not explain the mismatch: {error}"
        );
    }

    #[test]
    fn leaving_is_heard_at_the_other_end() {
        let (addr, socket) = fake_server(Vec2::ZERO, Vec2::ZERO);
        let connection = Connection::join(&addr).expect("join");
        let server = socket.recv().expect("the fake server keeps its socket");
        // So that a connection which is not really closed fails this test
        // instead of hanging it.
        server
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("set timeout");

        drop(connection);

        // End of file, not a timeout: the reader thread holds a clone of this
        // same connection, so without an explicit shutdown the server would go
        // on waiting for a player who has gone.
        let error = ToServer::read(&mut &server).expect_err("the line should be closed");
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::UnexpectedEof,
            "the server never heard the departure: {error}"
        );
    }

    #[test]
    fn a_put_down_moves_whatever_is_carrying_the_player() {
        let (addr, socket) = fake_server(Vec2::ZERO, Vec2::ZERO);
        let connection = Connection::join(&addr).expect("join");
        let server = socket.recv().expect("the fake server keeps its socket");
        let mut app = test_app(connection);

        // Afoot, this world having been entered on foot: the walker is the
        // carrier, and a put down stands them somewhere else outright. At
        // sea level and owing the ground a footing, exactly as they arrived
        // — the ground at the far end of a jump has not been asked for yet.
        let ashore = Vec2::new(1_204.0, -880.0);
        (ToClient::PutDown {
            position: ashore,
            heading: None,
        })
        .write(&mut &server)
        .expect("put down");
        run_until(&mut app, "the walker is put down", |app| {
            app.world_mut()
                .query_filtered::<&Transform, With<crate::player::Player>>()
                .single(app.world())
                .is_ok_and(|place| place.translation.xz() == ashore)
        });
        let (standing, unsettled) = app
            .world_mut()
            .query_filtered::<(&Transform, Has<crate::player::Unsettled>), With<crate::player::Player>>()
            .single(app.world())
            .expect("the player");
        assert_eq!(standing.translation.y, 0.0, "put down at a stale height");
        assert!(unsettled, "the walker was left with no ground to find");

        // Now at a helm: the hull is the carrier, and the same word moves it
        // instead — the one telling this client believes about its own hull,
        // every other word about it being its own reports echoed back.
        (ToClient::Boat {
            id: BoatId(4),
            kind: protocol::BoatKind::Sloop,
            position: ashore,
            heading: 0.0,
            occupant: Some(PlayerId(1)),
        })
        .write(&mut &server)
        .expect("boat");
        run_until(&mut app, "the helm is taken", |app| {
            app.world().resource::<crate::boat::Fleet>().helmed == Some(BoatId(4))
        });

        // And making way when the word comes, which is the state a jump has
        // to end — you were taken there, you did not sail there. A ship left
        // drawing would close the beach it was brought to look at in
        // seconds; see [`protocol::ToClient::PutDown`].
        app.world_mut()
            .query_filtered::<&mut crate::boat::Boat, With<crate::boat::HullId>>()
            .single_mut(app.world_mut())
            .expect("the hull")
            .hoist();

        let afloat = Vec2::new(-3_000.0, 512.0);
        (ToClient::PutDown {
            position: afloat,
            heading: Some(1.25),
        })
        .write(&mut &server)
        .expect("put down afloat");
        run_until(&mut app, "the hull is put down", |app| {
            app.world_mut()
                .query_filtered::<&Transform, With<crate::boat::HullId>>()
                .single(app.world())
                .is_ok_and(|place| place.translation.xz() == afloat)
        });
        let lying = app
            .world_mut()
            .query_filtered::<&Transform, With<crate::boat::HullId>>()
            .single(app.world())
            .copied()
            .expect("the hull");
        let forward = lying.forward();
        assert!(
            (f32::atan2(-forward.x, -forward.z) - 1.25).abs() < 1e-3,
            "the hull was put down pointing somewhere else"
        );
        let rest = app
            .world_mut()
            .query_filtered::<&crate::boat::Boat, With<crate::boat::HullId>>()
            .single(app.world())
            .expect("the hull");
        assert!(!rest.sails_set(), "the hull arrived with its sails drawing");
        assert!(rest.at_rest(), "the hull arrived still making way");
    }

    #[test]
    fn a_helm_taken_out_of_order_stands_the_player_off_the_deck() {
        // The out-of-order defence in `Fleet::told`: a telling says the hull
        // we hold the helm of is nobody's, arriving in an order this
        // client's own disembark would never have made. The helm goes back
        // to its moorings — and the player has to come off the deck with it.
        // Left aboard, they would be a passenger on somebody else's boat:
        // the hull is moored on that same telling's word and `moor` eases it
        // there, carrying anything parented to it along.
        let (addr, socket) = fake_server(Vec2::ZERO, Vec2::ZERO);
        let connection = Connection::join(&addr).expect("join");
        let server = socket.recv().expect("the fake server keeps its socket");
        let mut app = test_app(connection);

        let ashore = Vec2::new(120.0, -40.0);
        let seated = |occupant| ToClient::Boat {
            id: BoatId(4),
            kind: protocol::BoatKind::Sloop,
            position: ashore,
            heading: 0.0,
            occupant,
        };
        seated(Some(PlayerId(1)))
            .write(&mut &server)
            .expect("the seating");
        run_until(&mut app, "the player is aboard", |app| {
            app.world_mut()
                .query_filtered::<&ChildOf, With<crate::player::Player>>()
                .single(app.world())
                .is_ok()
        });

        // And now the helm is nobody's, said out of the order the client's
        // own disembark would have made it in.
        seated(None).write(&mut &server).expect("the hand back");
        run_until(&mut app, "the player is off the deck", |app| {
            app.world_mut()
                .query_filtered::<&ChildOf, With<crate::player::Player>>()
                .single(app.world())
                .is_err()
        });
        assert!(
            app.world()
                .resource::<crate::boat::Fleet>()
                .helmed
                .is_none(),
            "the helm was not given back"
        );

        let (standing, unsettled, kept) = app
            .world_mut()
            .query_filtered::<(
                &Transform,
                Has<crate::player::Unsettled>,
                Has<DespawnOnExit<crate::AppState>>,
            ), With<crate::player::Player>>()
            .single(app.world())
            .map(|(place, unsettled, kept)| (*place, unsettled, kept))
            .expect("the player");
        assert!(
            standing.translation.xz().distance(ashore) < 1.0,
            "the player was left at {} rather than where the hull lay",
            standing.translation.xz()
        );
        assert_eq!(
            standing.translation.y, 0.0,
            "the player kept a height that was the deck's to hold"
        );
        assert!(
            unsettled,
            "the player was left standing at sea level with no ground to find"
        );
        assert!(
            kept,
            "the player came off the deck without the despawn the hierarchy \
             was holding for them"
        );
    }

    #[test]
    fn a_helm_given_and_taken_in_one_breath_stands_the_player_where_it_was_told() {
        // The same standing off, in the drain where the scene graph has
        // nothing to offer it. A grant seats us in a boat this client has
        // never seen — so the hull is spawned by that very telling, and its
        // spawn is a queued command with no pose in the scene yet — and the
        // telling that takes the helm away again lands in the same drain,
        // before anything has flushed. Reading the hull's place off the
        // scene comes up empty, and the player must not be let go with
        // nothing for it: unparented while still carrying the deck-local
        // offset the helm gave them, that offset is read as a map coordinate
        // and stands them a stride from the world origin, and without
        // `Unsettled` no ground will ever come to claim them. The telling's
        // own word for where the hull lies is the answer instead, which is
        // the same word the hull is about to be moored on.
        //
        // Driven through the systems rather than over the socket for the
        // reason `a_seating_and_a_put_down_in_one_breath_move_the_hull_just_seated`
        // is: a wire cannot be asked to deliver two words in one frame, and a
        // test that hoped for it would quietly pass on the easy case.
        let (addr, socket) = fake_server(Vec2::ZERO, Vec2::ZERO);
        let connection = Connection::join(&addr).expect("join");
        let server = socket.recv().expect("the fake server keeps its socket");
        let mut app = test_app(connection);

        // A player in the world first, standing at a helm we were granted in
        // the ordinary way. Their own entity has to exist before the drain
        // under test, entry aboard being the one case that spawns it — see
        // the early return in `boat::stand_off`.
        (ToClient::Boat {
            id: BoatId(4),
            kind: protocol::BoatKind::Sloop,
            position: Vec2::new(-20.0, 15.0),
            heading: 0.0,
            occupant: Some(PlayerId(1)),
        })
        .write(&mut &server)
        .expect("the seating");
        run_until(&mut app, "the player is aboard", |app| {
            app.world_mut()
                .query_filtered::<&ChildOf, With<crate::player::Player>>()
                .single(app.world())
                .is_ok()
        });

        let afloat = Vec2::new(120.0, -40.0);
        let mut once = false;
        app.add_systems(
            Update,
            move |mut commands: Commands,
                  mut kit: crate::boat::HullKit,
                  mut fleet: ResMut<crate::boat::Fleet>,
                  players: crate::player::Players,
                  poses: Query<&Transform, With<crate::boat::Vessel>>| {
                if std::mem::replace(&mut once, true) {
                    return;
                }
                for occupant in [Some(PlayerId(1)), None] {
                    fleet.told(
                        &mut commands,
                        &mut kit,
                        &players,
                        &poses,
                        PlayerId(1),
                        BoatId(7),
                        protocol::BoatKind::Sloop,
                        afloat,
                        0.0,
                        occupant,
                    );
                }
            },
        );
        run_until(&mut app, "the player is off the deck", |app| {
            app.world_mut()
                .query_filtered::<&ChildOf, With<crate::player::Player>>()
                .single(app.world())
                .is_err()
        });

        let (standing, unsettled) = app
            .world_mut()
            .query_filtered::<(&Transform, Has<crate::player::Unsettled>), With<crate::player::Player>>()
            .single(app.world())
            .map(|(place, unsettled)| (*place, unsettled))
            .expect("the player");
        assert!(
            standing.translation.xz().distance(afloat) < 1.0,
            "the player was left at {} rather than where the telling said the \
             hull lay — a deck-local offset read as a map coordinate",
            standing.translation.xz()
        );
        assert!(
            unsettled,
            "the player was unparented owing the ground a footing they will \
             never be asked to find"
        );
    }

    #[test]
    fn a_seating_and_a_put_down_in_one_breath_move_the_hull_just_seated() {
        // The case `put_down`'s doc is written about, and the reason it reads
        // the fleet's book at all: a `Boat` telling that seats this player
        // and a put down about the same jump arrive in the same drain, so the
        // parentage the seating asked for is still a queued command and the
        // scene graph cannot answer what carries them. The book can — it is
        // written the instant the telling is read — and the command queue
        // keeps the order, so the seating's transform is written first and
        // the put down has the last word about where the hull lies.
        //
        // Driven through the systems rather than over the socket, because
        // "the same drain" is exactly what a wire cannot be asked to
        // guarantee: the reader thread queues on its own schedule, and a test
        // that hoped for both words in one frame would quietly pass on the
        // easy case whenever it got two.
        let (addr, socket) = fake_server(Vec2::ZERO, Vec2::ZERO);
        let connection = Connection::join(&addr).expect("join");
        let _server = socket.recv().expect("the fake server keeps its socket");
        let mut app = test_app(connection);

        let afloat = Vec2::new(-820.0, 640.0);
        let mut once = false;
        app.add_systems(
            Update,
            move |mut commands: Commands,
                  mut kit: crate::boat::HullKit,
                  mut fleet: ResMut<crate::boat::Fleet>,
                  players: crate::player::Players,
                  poses: Query<&Transform, With<crate::boat::Vessel>>| {
                if std::mem::replace(&mut once, true) {
                    return;
                }
                fleet.told(
                    &mut commands,
                    &mut kit,
                    &players,
                    &poses,
                    PlayerId(1),
                    BoatId(4),
                    protocol::BoatKind::Sloop,
                    Vec2::new(30.0, 30.0),
                    0.0,
                    Some(PlayerId(1)),
                );
                crate::player::put_down(&mut commands, &fleet, &players, afloat, Some(-0.75));
            },
        );
        run_until(&mut app, "the hull is seated and put down", |app| {
            app.world_mut()
                .query_filtered::<&Transform, With<crate::boat::HullId>>()
                .single(app.world())
                .is_ok_and(|place| place.translation.xz() == afloat)
        });

        let (hull, lying) = app
            .world_mut()
            .query_filtered::<(Entity, &Transform), With<crate::boat::HullId>>()
            .single(app.world())
            .map(|(hull, place)| (hull, *place))
            .expect("the hull");
        let forward = lying.forward();
        assert!(
            (f32::atan2(-forward.x, -forward.z) + 0.75).abs() < 1e-3,
            "the hull was put down pointing somewhere else"
        );
        let (aboard, unsettled) = app
            .world_mut()
            .query_filtered::<(&ChildOf, Has<crate::player::Unsettled>), With<crate::player::Player>>()
            .single(app.world())
            .map(|(aboard, unsettled)| (aboard.parent(), unsettled))
            .expect("the player should be aboard the hull they were seated at");
        assert_eq!(aboard, hull, "the player was left off the hull that moved");
        assert!(
            !unsettled,
            "a player on a deck was left owing the ground a footing"
        );
    }

    #[test]
    fn a_boat_telling_raises_a_hull_and_hides_its_helmsman() {
        let (addr, socket) = fake_server(Vec2::ZERO, Vec2::ZERO);
        let connection = Connection::join(&addr).expect("join");
        let server = socket.recv().expect("the fake server keeps its socket");
        let mut app = test_app(connection);

        // Somebody else is here, at a helm: their hull is raised from the
        // telling, and their capsule goes dark — the boat is their marker.
        (ToClient::Joined {
            id: PlayerId(9),
            position: Vec2::new(4.0, 5.0),
        })
        .write(&mut &server)
        .expect("joined");
        (ToClient::Boat {
            id: BoatId(3),
            kind: protocol::BoatKind::Sloop,
            position: Vec2::new(4.0, 5.0),
            heading: 0.5,
            occupant: Some(PlayerId(9)),
        })
        .write(&mut &server)
        .expect("boat");
        run_until(&mut app, "the hull is raised", |app| {
            app.world_mut()
                .query::<&crate::boat::HullId>()
                .iter(app.world())
                .count()
                == 1
        });
        run_until(&mut app, "the helmsman's capsule goes dark", |app| {
            app.world_mut()
                .query::<(&RemotePlayer, &Visibility)>()
                .single(app.world())
                .is_ok_and(|(_, visibility)| *visibility == Visibility::Hidden)
        });

        // They sail away and step ashore: the helm is told free, and the
        // capsule stands again — a walker is a capsule, having no hull to be
        // — where they now are rather than where they boarded. Nothing was
        // reported afoot for the whole voyage, their position having crossed
        // as the boat's, so a marker eased towards this would come skating
        // across half a sea nobody walked.
        let ashore = Vec2::new(604.0, -195.0);
        (ToClient::Boat {
            id: BoatId(3),
            kind: protocol::BoatKind::Sloop,
            position: Vec2::new(600.0, -200.0),
            heading: 0.5,
            occupant: None,
        })
        .write(&mut &server)
        .expect("boat freed");
        (ToClient::Moved {
            id: PlayerId(9),
            position: ashore,
        })
        .write(&mut &server)
        .expect("moved");
        // Waited for by the step ashore rather than by the freeing of the
        // helm, the two being two words: the frame this lands on is the
        // frame the marker must already be standing on the beach.
        run_until(&mut app, "the step ashore is heard", |app| {
            markers(app) == [(PlayerId(9), ashore)]
        });
        let (visibility, standing) = app
            .world_mut()
            .query_filtered::<(&Visibility, &Transform), With<RemotePlayer>>()
            .single(app.world())
            .expect("the one marker");
        assert_eq!(
            *visibility,
            Visibility::Inherited,
            "the walker's capsule never stood again"
        );
        assert_eq!(
            standing.translation.xz(),
            ashore,
            "the capsule is gliding in from where they boarded"
        );
    }

    /// The hull this client's player is riding, by its wire name.
    fn aboard_hull(app: &mut App) -> Option<BoatId> {
        let hull = app
            .world_mut()
            .query_filtered::<&ChildOf, With<crate::player::Player>>()
            .single(app.world())
            .ok()?
            .parent();
        app.world()
            .entity(hull)
            .get::<crate::boat::HullId>()
            .map(|named| named.0)
    }

    #[test]
    fn helm_grants_cross_the_player_between_hulls_and_a_hoist_retires_one() {
        // The whole online shape of going ashore by boat, as tellings: a
        // ship granted, then a lower grant — the rowboat first with us
        // aboard, then our ship left at anchor — then the boarding back and
        // the tender's going. The player is re-seated by each grant, and
        // whatever helm they held goes back to its moorings.
        let (addr, socket) = fake_server(Vec2::ZERO, Vec2::ZERO);
        let connection = Connection::join(&addr).expect("join");
        let server = socket.recv().expect("the fake server keeps its socket");
        let mut app = test_app(connection);
        // What the fake server's welcome deals.
        let me = PlayerId(1);

        (ToClient::Boat {
            id: BoatId(1),
            kind: protocol::BoatKind::Sloop,
            position: Vec2::ZERO,
            heading: 0.0,
            occupant: Some(me),
        })
        .write(&mut &server)
        .expect("ship granted");
        run_until(&mut app, "the player is seated at the ship's helm", |app| {
            aboard_hull(app) == Some(BoatId(1))
        });

        (ToClient::Boat {
            id: BoatId(2),
            kind: protocol::BoatKind::Rowboat,
            position: Vec2::new(3.0, 0.0),
            heading: 0.5,
            occupant: Some(me),
        })
        .write(&mut &server)
        .expect("tender granted");
        (ToClient::Boat {
            id: BoatId(1),
            kind: protocol::BoatKind::Sloop,
            position: Vec2::ZERO,
            heading: 0.0,
            occupant: None,
        })
        .write(&mut &server)
        .expect("ship at anchor");
        run_until(&mut app, "the player crosses to the tender", |app| {
            aboard_hull(app) == Some(BoatId(2))
        });
        // The ship went back to its moorings: the sailing systems came off
        // with the grant that took its crew.
        let ship = app
            .world_mut()
            .query::<(Entity, &crate::boat::HullId)>()
            .iter(app.world())
            .find(|(_, named)| named.0 == BoatId(1))
            .map(|(hull, _)| hull)
            .expect("the ship's hull is still in the world");
        assert!(
            app.world()
                .entity(ship)
                .get::<crate::boat::Boat>()
                .is_none(),
            "the ship kept its sailing systems after the helm was given up"
        );

        (ToClient::Boat {
            id: BoatId(1),
            kind: protocol::BoatKind::Sloop,
            position: Vec2::ZERO,
            heading: 0.0,
            occupant: Some(me),
        })
        .write(&mut &server)
        .expect("ship granted back");
        (ToClient::BoatGone { id: BoatId(2) })
            .write(&mut &server)
            .expect("tender hoisted");
        run_until(&mut app, "the player crosses back to the ship", |app| {
            aboard_hull(app) == Some(BoatId(1))
        });
        run_until(&mut app, "the tender is out of the world", |app| {
            app.world_mut()
                .query::<&crate::boat::HullId>()
                .iter(app.world())
                .count()
                == 1
        });
    }

    #[test]
    fn a_hull_taken_from_under_a_player_leaves_them_where_it_lay() {
        // The order this client is not supposed to have to handle: a hull
        // told gone while the fleet still has us aboard it, with no word
        // first about where we went. It cannot happen on this wire — the
        // seating is always said before the going — which is exactly why the
        // answer to it wants pinning: a player is posed against their hull,
        // so one taken away without a pose of their own stands at the world
        // origin, an ocean from wherever they were.
        let (addr, socket) = fake_server(Vec2::ZERO, Vec2::ZERO);
        let connection = Connection::join(&addr).expect("join");
        let server = socket.recv().expect("the fake server keeps its socket");
        let mut app = test_app(connection);
        let me = PlayerId(1);
        let afloat = Vec2::new(60.0, -20.0);

        (ToClient::Boat {
            id: BoatId(2),
            kind: protocol::BoatKind::Rowboat,
            position: afloat,
            heading: 0.0,
            occupant: Some(me),
        })
        .write(&mut &server)
        .expect("tender granted");
        run_until(&mut app, "the player is seated in the boat", |app| {
            aboard_hull(app) == Some(BoatId(2))
        });

        (ToClient::BoatGone { id: BoatId(2) })
            .write(&mut &server)
            .expect("tender hoisted");
        run_until(&mut app, "the boat is out from under them", |app| {
            aboard_hull(app).is_none()
        });

        let (place, unsettled) = app
            .world_mut()
            .query_filtered::<(&Transform, Has<crate::player::Unsettled>), With<crate::player::Player>>()
            .single(app.world())
            .expect("the player went with the hull");
        assert_eq!(
            place.translation.xz(),
            afloat,
            "the player was left at the world's origin rather than where the boat lay"
        );
        assert!(
            unsettled,
            "the player was put down without being left to find the ground"
        );
    }

    #[test]
    fn a_telling_of_the_survey_reaches_the_chart() {
        // The client has no survey of its own: coast reaches the sheet by
        // being told, and this is the whole of that path.
        use protocol::survey::{Coast, Mark, Soundings};

        let (addr, socket) = fake_server(Vec2::ZERO, Vec2::ZERO);
        let connection = Connection::join(&addr).expect("join");
        let server = socket.recv().expect("the fake server keeps its socket");
        let mut app = test_app(connection);
        app.insert_resource(crate::chart::Chart::default());

        let shore = IVec2::new(3, -2);
        (ToClient::Surveyed {
            found: vec![
                (
                    shore,
                    Soundings {
                        coast: vec![Coast::new(
                            vec![Mark::unpack([0, 17]), Mark::unpack([255, 254])],
                            false,
                        )],
                        shoal: Vec::new(),
                    },
                ),
                // And open water, which is surveyed and blank.
                (IVec2::new(4, -2), Soundings::default()),
            ],
        })
        .write(&mut &server)
        .expect("surveyed");

        run_until(&mut app, "the ink lands on the sheet", |app| {
            app.world().resource::<crate::chart::Chart>().surveys() == 2
        });
        assert!(app
            .world()
            .resource::<crate::chart::Chart>()
            .surveyed(shore));
    }

    #[test]
    fn a_telling_of_a_cairn_stands_it_in_the_world_and_letters_the_sheet() {
        // A cairn is the one thing that reaches both at once: the world gets
        // the stones, and the sheet gets the name — which arrives no other
        // way now that a name rides the claim it is written on.
        let (addr, socket) = fake_server(Vec2::ZERO, Vec2::ZERO);
        let connection = Connection::join(&addr).expect("join");
        let server = socket.recv().expect("the fake server keeps its socket");
        let mut app = test_app(connection);
        app.insert_resource(crate::chart::Chart::default());

        let island = IVec2::new(76, 255);
        let at = Vec2::new(120.0, -40.0);
        (ToClient::Cairn {
            island,
            at,
            covers: (Vec2::new(-128.0, -256.0), Vec2::new(512.0, 128.0)),
            name: "Isla Genovesa".to_string(),
            yours: true,
        })
        .write(&mut &server)
        .expect("cairn");

        run_until(&mut app, "the cairn is heard of", |app| {
            app.world()
                .resource::<crate::chart::Chart>()
                .claim(island)
                .is_some()
        });

        // On the paper: the name, and whose it is.
        let chart = app.world().resource::<crate::chart::Chart>();
        assert_eq!(chart.name(island), Some("Isla Genovesa"));
        let claimed = chart.claim(island).expect("a claim was told");
        assert_eq!(claimed.at, at);
        assert!(claimed.yours, "the player's own cairn read as a stranger's");

        // And standing in the world, at the point it was told of — with no
        // height yet, this app having no ground for it to settle onto.
        let mut standing = app
            .world_mut()
            .query::<(&crate::cairn::Cairn, &Transform)>();
        let (cairn, place) = standing
            .iter(app.world())
            .next()
            .expect("a cairn was raised");
        assert_eq!(cairn.island, island);
        assert_eq!(place.translation.x, at.x);
        assert_eq!(place.translation.z, at.y);
    }

    #[test]
    fn a_bearing_is_never_more_than_half_a_turn_from_another() {
        use std::f32::consts::{PI, TAU};

        // The cut: a hull holding a course a hair either side of due south
        // has barely moved, whatever the two numbers look like.
        assert!(swing(PI - 0.001, -PI + 0.001) < 0.01);
        assert!(swing(-PI + 0.001, PI - 0.001) < 0.01);
        // A real swing is still a real swing, either way round...
        assert!((swing(0.0, 1.0) - 1.0).abs() < 1e-5);
        assert!((swing(1.0, 0.0) - 1.0).abs() < 1e-5);
        // ...and half a turn is as far apart as two bearings get.
        assert!((swing(0.0, PI) - PI).abs() < 1e-5);
        assert!(swing(0.3, 0.3 + TAU) < 1e-5);
    }

    #[test]
    fn other_players_come_move_and_go_as_markers() {
        let (addr, socket) = fake_server(Vec2::ZERO, Vec2::ZERO);
        let connection = Connection::join(&addr).expect("join");
        let server = socket.recv().expect("the fake server keeps its socket");
        let mut app = test_app(connection);

        // Someone is out there: a marker appears where the server put them.
        (ToClient::Joined {
            id: PlayerId(9),
            position: Vec2::new(64.0, -32.0),
        })
        .write(&mut &server)
        .expect("joined");
        run_until(&mut app, "the marker appears", |app| {
            !markers(app).is_empty()
        });
        assert_eq!(markers(&mut app), [(PlayerId(9), Vec2::new(64.0, -32.0))]);

        // They move: the marker's destination follows.
        (ToClient::Moved {
            id: PlayerId(9),
            position: Vec2::new(80.0, 0.0),
        })
        .write(&mut &server)
        .expect("moved");
        run_until(&mut app, "the marker retargets", |app| {
            markers(app) == [(PlayerId(9), Vec2::new(80.0, 0.0))]
        });

        // They leave: the marker goes with them.
        (ToClient::Left { id: PlayerId(9) })
            .write(&mut &server)
            .expect("left");
        run_until(&mut app, "the marker despawns", |app| {
            markers(app).is_empty()
        });
    }

    #[test]
    fn a_whole_frames_worth_of_news_about_one_player_is_applied_in_order() {
        // A marker is spawned through `Commands` and so is not there to be
        // found until the frame ends, which is why the session keeps a map of
        // them: two messages about one player in a single drain used to leave
        // a marker nobody could remove, and lose a move outright.
        let (addr, socket) = fake_server(Vec2::ZERO, Vec2::ZERO);
        let connection = Connection::join(&addr).expect("join");
        let server = socket.recv().expect("the fake server keeps its socket");
        let mut app = test_app(connection);

        for message in [
            ToClient::Joined {
                id: PlayerId(9),
                position: Vec2::new(64.0, -32.0),
            },
            ToClient::Moved {
                id: PlayerId(9),
                position: Vec2::new(80.0, 0.0),
            },
            ToClient::Joined {
                id: PlayerId(10),
                position: Vec2::new(8.0, 8.0),
            },
            ToClient::Left { id: PlayerId(10) },
        ] {
            message.write(&mut &server).expect("a message");
        }

        // Nothing has drawn a frame since, so this is the reader thread being
        // given time to put all four in the channel — a slow machine that
        // spread them over two frames would let the test pass without asking
        // the question, but no machine can make it fail spuriously.
        thread::sleep(Duration::from_millis(100));
        run_until(&mut app, "the news is applied", |app| {
            !markers(app).is_empty()
        });

        assert_eq!(
            markers(&mut app),
            [(PlayerId(9), Vec2::new(80.0, 0.0))],
            "the one who left is still standing there, or the move was lost"
        );
    }

    #[test]
    fn a_console_line_crosses_the_wire_and_its_reply_lands_on_the_console() {
        let (addr, socket) = fake_server(Vec2::ZERO, Vec2::ZERO);
        let connection = Connection::join(&addr).expect("join");
        let server = socket.recv().expect("the fake server keeps its socket");
        let mut app = test_app(connection);

        // The line goes out exactly as typed — the client does not parse a
        // word of the server's vocabulary, so nothing of it can be lost here.
        app.world()
            .resource::<Online>()
            .connection
            .command("spawn shark".to_string());
        server
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("set timeout");
        // Read past the walker's own position reports — entry stands a
        // player up now, and a player reports — to the line itself.
        let line = loop {
            match ToServer::read(&mut &server).expect("the command should arrive") {
                ToServer::Move { .. } | ToServer::Helm { .. } => continue,
                message => break message,
            }
        };
        assert_eq!(
            line,
            ToServer::Command {
                line: "spawn shark".to_string()
            }
        );

        // And the answer lands where the question was typed.
        (ToClient::Reply {
            text: "a shark rises 62 m away".to_string(),
        })
        .write(&mut &server)
        .expect("reply");
        run_until(&mut app, "the reply reaches the console", |app| {
            app.world()
                .resource::<crate::console::Console>()
                .said("a shark rises 62 m away")
        });
    }

    #[test]
    fn the_taught_vocabulary_reaches_the_console() {
        let (addr, socket) = fake_server(Vec2::ZERO, Vec2::ZERO);
        let connection = Connection::join(&addr).expect("join");
        let server = socket.recv().expect("the fake server keeps its socket");
        let mut app = test_app(connection);

        (ToClient::Vocabulary {
            phrases: vec!["spawn".to_string()],
        })
        .write(&mut &server)
        .expect("vocabulary");
        run_until(&mut app, "the console is taught", |app| {
            app.world()
                .resource::<crate::console::Console>()
                .knows("spawn")
        });
    }

    /// Pausing holds the player still; it does not hang up on anyone.
    ///
    /// This is what [`Helm`] is for. Hanging up here would mean the other
    /// boats vanished the moment somebody opened a menu, and — for whoever was
    /// hosting — that the world itself shut on everyone else in it.
    #[test]
    fn pausing_keeps_the_session_and_leaving_ends_it() {
        let (addr, socket) = fake_server(Vec2::ZERO, Vec2::ZERO);
        let connection = Connection::join(&addr).expect("join");
        let _server = socket.recv().expect("the fake server keeps its socket");
        let mut app = test_app(connection);
        app.insert_resource(Hosting(
            Server::bind("127.0.0.1:0", 7)
                .expect("bind")
                .spawn()
                .expect("spawn"),
        ));

        set_helm(&mut app, Helm::Paused);
        assert!(
            app.world().contains_resource::<Online>(),
            "pausing hung up the connection"
        );
        assert!(
            app.world().contains_resource::<Hosting>(),
            "pausing shut the world on everyone else in it"
        );

        // Leaving still costs the whole world, which is the other half of the
        // bargain — a pause that never let go would leak the port and the
        // thread behind it.
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::MainMenu);
        app.update();
        assert!(!app.world().contains_resource::<Online>());
        assert!(!app.world().contains_resource::<Hosting>());
        // And the helm goes with the world it belonged to.
        assert!(app.world().get_resource::<State<Helm>>().is_none());
    }

    fn set_helm(app: &mut App, helm: Helm) {
        app.world_mut().resource_mut::<NextState<Helm>>().set(helm);
        app.update();
    }
}
