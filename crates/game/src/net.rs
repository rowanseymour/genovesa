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

use protocol::{
    BoatId, PlayerId, ToClient, ToServer, Token, WorldId, DEFAULT_PORT, PROTOCOL_VERSION,
};
use server::{Host, Server, WorldConfig};

use crate::player::PlayerPlace;
use crate::sea;
use crate::terrain::Ground;
use crate::{eased, matte, AppState};

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
/// in e-foldings per second — see [`eased`]. Positions arrive a few times a
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
    /// Every world started here goes through this, whether anyone else is
    /// invited or not: the ground comes from a server, so playing alone means
    /// running one and talking to it over the loopback. [`Reach`] is the only
    /// difference between the two, and it is a question about the network
    /// rather than about the session — a world of one's own is served exactly
    /// as a shared one is, and the player is a client in it exactly as a guest
    /// would be.
    ///
    /// Joined over the loopback whatever it is bound to: whoever opened the
    /// world is a player in it and gets there the short way.
    ///
    /// `opening` is the hour of its day the world starts at, as a phase —
    /// [`server::OPENING`] for a world nobody asked anything particular of.
    /// `keep` files the world in this machine's worlds directory, to be
    /// offered again from the menu: what the menu asks and the command line
    /// does not, a `--seed` run being a world to look at rather than one to
    /// live in.
    pub fn open(
        config: WorldConfig,
        reach: Reach,
        opening: f32,
        keep: bool,
    ) -> Result<Self, String> {
        let mut server = Server::bind(reach.bound_to(), config)
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
    /// Behind a mutex only because a Bevy resource must be `Sync`; nothing but
    /// [`Dialing::outcome`] locks it.
    outcome: Mutex<Receiver<Result<Session, String>>>,
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
    pub fn opening(config: WorldConfig, reach: Reach, opening: f32, keep: bool) -> Self {
        Self::on(reach.described(), move || {
            Session::open(config, reach, opening, keep)
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
            outcome: Mutex::new(waiting),
            what,
        }
    }

    /// What came of it, or `None` while it is still ringing. Answers once:
    /// whoever takes the outcome owns the session, so a screen asks until it
    /// gets something and then drops this resource.
    pub fn outcome(&self) -> Option<Result<Session, String>> {
        match self.outcome.lock().expect("no poisoned lock").try_recv() {
            Ok(outcome) => Some(outcome),
            Err(TryRecvError::Empty) => None,
            // The thread sends whatever the dial came to, success or failure,
            // so a channel that closed without one is a thread that died —
            // this crate's bug, but a screen that says so can still be left.
            Err(TryRecvError::Disconnected) => {
                Some(Err(format!("dialling {} came to nothing", self.what)))
            }
        }
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

/// Marks another player's marker, and where the server last put them.
#[derive(Component)]
pub struct RemotePlayer {
    pub id: PlayerId,
    /// Where they are heading — eased towards, like the camera's own focus.
    target: Vec2,
}

pub struct NetPlugin;

impl Plugin for NetPlugin {
    fn build(&self, app: &mut App) {
        // What `receive` writes the weather to and `place_markers` floats
        // on. Also initialised by the plugins that draw and ride the sea;
        // initialising a resource twice is free, and each plugin's tests
        // run it alone.
        app.init_resource::<sea::Forecast>()
            .init_resource::<sea::SeaConditions>()
            .init_resource::<crate::sky::Sky>()
            .init_resource::<crate::beasts::Beasts>()
            .init_resource::<crate::boat::Fleet>()
            .init_resource::<crate::console::Console>()
            .add_systems(
                OnEnter(AppState::InWorld),
                enter_afoot.run_if(resource_exists::<Online>),
            )
            .add_systems(
                Update,
                (
                    receive,
                    ask_for_ground,
                    report_position,
                    // Shading before placing, so that a marker coming back
                    // out from under a hull is put where it belongs before
                    // the same frame stands it on the ground there.
                    shade_markers,
                    place_markers,
                )
                    .chain()
                    .run_if(in_state(AppState::InWorld).and_then(resource_exists::<Online>)),
            )
            .add_systems(
                OnExit(AppState::InWorld),
                // The fleet forgotten here as well as by the boat plugin —
                // scuttling twice is writing a default twice, and an app
                // with only one of the two plugins (the lean net tests')
                // must still not carry one world's hulls into the next.
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

/// Where a server's word lands when it is not an entity: the wind the sea is
/// drawn under, the hour the world is lit at, the beasts in its water, and
/// the console a reply is printed on. Resources belonging to four other
/// modules, taken together because [`receive`] is the one place any of them
/// is written and none is this module's to interpret.
#[derive(SystemParam)]
struct Told<'w> {
    forecast: ResMut<'w, sea::Forecast>,
    sky: ResMut<'w, crate::sky::Sky>,
    beasts: ResMut<'w, crate::beasts::Beasts>,
    console: ResMut<'w, crate::console::Console>,
    fleet: ResMut<'w, crate::boat::Fleet>,
}

/// Applies what the server said since last frame: players joining, moving
/// and leaving, as markers coming, easing and going — and the boats, whose
/// assets travel in the [`crate::boat::HullKit`] the markers' own meshes
/// and materials now come through too, one system not being allowed two
/// hands on one store.
fn receive(
    mut commands: Commands,
    mut online: ResMut<Online>,
    mut ground: Option<ResMut<Ground>>,
    mut kit: crate::boat::HullKit,
    mut told: Told,
    walkers: Query<Entity, (With<crate::player::Player>, Without<ChildOf>)>,
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
                        RemotePlayer {
                            id,
                            target: position,
                        },
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
                    commands.entity(marker).insert(RemotePlayer {
                        id,
                        target: position,
                    });
                }
            }
            ToClient::Left { id } => {
                if let Some(marker) = online.markers.remove(&id) {
                    commands.entity(marker).despawn();
                }
            }
            ToClient::Chunk {
                chunk,
                ground: sent,
            } => {
                // Only while a world is open. A chunk answered after leaving
                // one is about a world that no longer exists here, and there
                // is nothing left for it to be part of.
                if let Some(ground) = ground.as_mut() {
                    ground.deliver(chunk, sent);
                }
            }
            ToClient::Weather { wind } => {
                // A target, not an order: the drawn sea eases towards it —
                // see [`sea::settle_conditions`] — so the server's occasional
                // quantised updates arrive as weather rather than as steps.
                //
                // Believed only within reason. The server is the authority on
                // the sky, but a hostile or broken one must not get to poison
                // the arithmetic every vertex of the sea runs on — a
                // non-finite wind, once eased into the conditions, is NaN for
                // good. The ceiling sits far above any honest gale rather
                // than at it, so a server that learns to blow harder is not
                // silently ignored here.
                if wind.is_finite() && wind.length() < 100.0 {
                    told.forecast.wind = Some(wind);
                }
            }
            ToClient::Daylight { phase } => {
                // Believed within the same reason as the weather: an hour
                // outside the day is a broken or hostile server, and a
                // non-finite one eased into the clock would leave this
                // machine with no time of day at all, for good.
                if phase.is_finite() && (0.0..1.0).contains(&phase) {
                    told.sky.told(phase);
                }
            }
            ToClient::Beast {
                id,
                kind,
                position,
                velocity,
                surfaced,
            } => {
                // Believed within the same reason as the sky: a beast is
                // eased towards and drawn out of this arithmetic every
                // frame, and one telling of a non-finite place would be a
                // shark at NaN for good. The pace ceiling sits far above any
                // honest beast rather than at it.
                if position.is_finite() && velocity.is_finite() && velocity.length() < 50.0 {
                    told.beasts
                        .seen(&mut commands, id, kind, position, velocity, surfaced);
                }
            }
            ToClient::BeastGone { id } => told.beasts.gone(&mut commands, id),
            // Whatever the server said to a console line, said where it was
            // typed. Only ever sent asked-for, so a quiet session pays
            // nothing here.
            ToClient::Reply { text } => told.console.say(&text),
            // The server's verbs, for tab at the console — see
            // [`crate::console`], which owns what completion means and
            // still sends every line verbatim.
            ToClient::Vocabulary { verbs } => told.console.teach(verbs),
            ToClient::Boat {
                id,
                position,
                heading,
                occupant,
                ..
            } => {
                // Believed within the sky's reason: a hull is eased towards
                // and drawn every frame, and one telling of a non-finite
                // pose would moor it at NaN for good.
                if position.is_finite() && heading.is_finite() {
                    told.fleet.told(
                        &mut commands,
                        &mut kit,
                        &walkers,
                        online.connection.id,
                        id,
                        position,
                        heading,
                        occupant,
                    );
                }
            }
            // The handshake consumed its own messages; a stray one now is a
            // server bug, not something to end a match over.
            ToClient::Welcome { .. } | ToClient::Refused { .. } | ToClient::World { .. } => {}
        }
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
fn enter_afoot(
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
    mut markers: Query<(Ref<RemotePlayer>, &mut Visibility, &mut Transform)>,
    // Who has been under a hull and is not yet standing where they stepped
    // off. A player who leaves the world while still at one stays in here,
    // which is a handful of bytes for as long as the session lasts and
    // nothing else: ids are dealt once each, so the name never comes round
    // again to be wrongly snapped.
    mut adrift: Local<HashSet<PlayerId>>,
) {
    for (player, mut visibility, mut transform) in &mut markers {
        if fleet.crewed(player.id) {
            *visibility = Visibility::Hidden;
            adrift.insert(player.id);
            continue;
        }
        if adrift.contains(&player.id) {
            transform.translation.x = player.target.x;
            transform.translation.z = player.target.y;
            // Held until a word about the player themself lands, because
            // the two arrive as two: the helm is told free and the step
            // ashore follows, and a frame that read only the first would
            // snap to the boarding point and then glide the whole voyage
            // anyway. Until then the snap is to a target the marker is
            // already standing on, which costs nothing.
            if player.is_changed() {
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
    mut markers: Query<(&RemotePlayer, &mut Transform)>,
) {
    let t = eased(MARKER_SMOOTHING, time.delta_secs());

    for (player, mut transform) in &mut markers {
        let at = Vec2::new(transform.translation.x, transform.translation.z);
        let at = at.lerp(player.target, t);

        // Standing on the surface, capsule half-height above it, so a player
        // crossing open ocean is sailing it rather than walking the seabed —
        // and riding the swell the way the boat itself does, or a marker
        // crossing open water would stand still in a sea everything else is
        // bobbing on. Ground still generating keeps the last height, exactly
        // as the boat and the camera's own focus do.
        let mut height = transform.translation.y;
        if let Some(standing) = ground.as_ref().and_then(|g| g.height(at.x, at.y)) {
            let water = sea.water_over(ground.as_deref(), at, time.elapsed_secs_wrapped());
            height = standing.max(water) + MARKER_LENGTH * 0.5 + MARKER_RADIUS;
        }
        transform.translation = Vec3::new(at.x, height, at.y);
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
        // telling loads its hull's meshes through it.
        .init_asset::<Mesh>()
        .init_resource::<Assets<StandardMaterial>>()
        .insert_resource(Online::new(connection));
        app.update();
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::InWorld);
        app.update();
        app
    }

    fn markers(app: &mut App) -> Vec<(PlayerId, Vec2)> {
        app.world_mut()
            .query::<&RemotePlayer>()
            .iter(app.world())
            .map(|player| (player.id, player.target))
            .collect()
    }

    /// Waits for a dial to land. It crosses real sockets and a thread, so a
    /// moment of patience is legitimate — five seconds of it is a failure.
    fn settle(dialing: &Dialing) -> Result<Session, String> {
        // Every dial's handshake reads the data directory for its papers,
        // and a test's must not be the player's.
        crate::testing::quarantine_data_dir();
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
        let dialing = Dialing::opening(
            WorldConfig { seed: 77 },
            Reach::Alone,
            server::OPENING,
            false,
        );
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
        let first = settle(&Dialing::opening(
            WorldConfig { seed: 77 },
            Reach::Alone,
            server::OPENING,
            false,
        ))
        .expect("a world should open");
        let second = settle(&Dialing::opening(
            WorldConfig { seed: 78 },
            Reach::Alone,
            server::OPENING,
            false,
        ))
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
        let dialing = Dialing::opening(
            WorldConfig { seed: 3 },
            Reach::Alone,
            server::OPENING,
            false,
        );
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
        let dialing = Dialing::to("127.0.0.1:1");
        let error = settle(&dialing).err().expect("nobody home");
        assert!(
            error.contains("127.0.0.1:1"),
            "the error does not name what was dialled: {error}"
        );
    }

    #[test]
    fn a_dial_answers_once() {
        let dialing = Dialing::to("127.0.0.1:1");
        settle(&dialing).err().expect("nobody home");
        assert!(
            dialing.outcome().is_some_and(|outcome| outcome.is_err()),
            "a dial already given up should not report success"
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
            verbs: vec!["spawn".to_string()],
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
            Server::bind("127.0.0.1:0", WorldConfig::default())
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
