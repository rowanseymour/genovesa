//! Playing in a served world: the game as a client, and — when this machine is
//! the one hosting — as the server's landlord too.
//!
//! Terrain never arrives over the wire. The server's welcome names the seed,
//! and the seed *is* the world — the `world` crate regenerates it here, bit
//! for bit, exactly as a local run would have. What the connection carries is
//! the session: who else is in the world and where they are, drawn as marker
//! capsules standing on the same ground every machine is generating for
//! itself.
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
//! headless and engine-free, so a game that shares a world runs one on a
//! thread and then joins it over the loopback like anybody else. There is no
//! second, quieter implementation of a session for the host to play against.

use std::collections::HashMap;
use std::net::{Shutdown, TcpStream};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use bevy::prelude::*;

use protocol::{PlayerId, ToClient, ToServer, DEFAULT_PORT, PROTOCOL_VERSION};
use server::{Host, Server};

use crate::camera::MapCamera;
use crate::terrain::{WorldConfig, WorldTerrain};
use crate::AppState;

/// Seconds between position reports, at least. Ten a second reads as
/// continuous once markers ease between them, and keeps an idle wire quiet.
const REPORT_INTERVAL: f32 = 0.1;

/// Metres of movement below which nothing is reported — a player standing
/// still costs the wire nothing.
const REPORT_THRESHOLD: f32 = 0.25;

/// How long a report may spend trying to reach the server. Reports are written
/// straight from the schedule, so this is time the player would spend watching
/// a frozen frame: a server that stopped reading its socket would otherwise
/// hold the game still the moment the send buffer filled. Kept short because a
/// lost report costs nothing — positions are absolute, not steps, and the next
/// one supersedes it a tenth of a second later.
const REPORT_TIMEOUT: Duration = Duration::from_millis(100);

/// How quickly a marker eases towards where the server last put its player.
/// Positions arrive a few times a second, so the easing is what turns the
/// steps back into movement.
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
    /// The world being served — the whole of it, this being Genovesa.
    pub seed: u32,
    /// Where the server puts arriving players down.
    pub spawn: Vec2,
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

        match ToClient::read(&mut &stream) {
            Ok(ToClient::Welcome { id, seed, spawn }) => {
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
                    seed,
                    spawn,
                })
            }
            Ok(ToClient::Refused { version }) => Err(format!(
                "`{addr}` speaks protocol {version}, this build speaks {PROTOCOL_VERSION}"
            )),
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
        if (ToServer::Move { position })
            .write(&mut &self.stream)
            .is_err()
        {
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
    /// What is being dialled, for the screen to say while it waits.
    pub what: String,
}

impl Dialing {
    /// Starts dialling a server named as `host` or `host:port`.
    pub fn to(address: &str) -> Self {
        let address = address.to_string();
        let what = address.clone();
        Self::on(what, move || {
            Connection::join(&address).map(|connection| Session {
                connection,
                hosting: None,
            })
        })
    }

    /// Starts hosting a world on this machine, and joins it.
    ///
    /// Bound on every interface, because sharing a world means being reachable
    /// from another machine — the menu asks for [`DEFAULT_PORT`], which is
    /// what the people being shared with have to be able to guess. Joined over
    /// the loopback whatever address they use: the host is a player in their
    /// own world and gets to it the short way.
    pub fn hosting(config: WorldConfig, port: u16) -> Self {
        Self::on(format!("a world of your own, port {port}"), move || {
            let server = Server::bind(("0.0.0.0", port), config)
                .map_err(|error| format!("cannot host on port {port}: {error}"))?;
            let host = server
                .spawn()
                .map_err(|error| format!("cannot host: {error}"))?;
            let connection = Connection::join(&format!("127.0.0.1:{}", host.addr().port()))?;
            Ok(Session {
                connection,
                hosting: Some(host),
            })
        })
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
/// Nothing reads it: holding it *is* what it does. Dropping the handle stops
/// the server and hangs up on everyone in the world, so it lives exactly as
/// long as the host's own visit — inserted when the world is entered, removed
/// on the way out by [`disconnect`].
#[derive(Resource)]
pub struct Hosting(pub Host);

/// The joined session. Present only in a run that is playing in a served
/// world; every system here conditions on it, so a local world pays nothing.
#[derive(Resource)]
pub struct Online {
    connection: Connection,
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
        app.add_systems(
            Update,
            (receive, report_position, place_markers)
                .chain()
                .run_if(in_state(AppState::InWorld).and_then(resource_exists::<Online>)),
        )
        .add_systems(OnExit(AppState::InWorld), disconnect);
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

/// Applies what the server said since last frame: players joining, moving
/// and leaving, as markers coming, easing and going.
fn receive(
    mut commands: Commands,
    mut online: ResMut<Online>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
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
                        Mesh3d(meshes.add(Capsule3d::new(MARKER_RADIUS, MARKER_LENGTH))),
                        // Matte, like everything else in this look.
                        MeshMaterial3d(materials.add(StandardMaterial {
                            base_color: marker_color(id),
                            perceptual_roughness: 1.0,
                            metallic: 0.0,
                            reflectance: 0.0,
                            ..default()
                        })),
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
            // The handshake consumed its own messages; a stray one now is a
            // server bug, not something to end a match over.
            ToClient::Welcome { .. } | ToClient::Refused { .. } => {}
        }
    }
}

/// Tells the server where the player is — which, until there is an avatar to
/// walk around, means where their view is focused.
fn report_position(
    time: Res<Time>,
    online: Res<Online>,
    cameras: Query<&MapCamera>,
    mut last: Local<Option<(f32, Vec2)>>,
) {
    let Ok(camera) = cameras.single() else {
        return;
    };
    let position = Vec2::new(camera.focus.x, camera.focus.z);
    let now = time.elapsed_secs();

    if let Some((reported_at, reported)) = *last {
        if now - reported_at < REPORT_INTERVAL || reported.distance(position) < REPORT_THRESHOLD {
            return;
        }
    }
    online.connection.report(position);
    *last = Some((now, position));
}

/// Walks each marker towards where the server last put its player, and
/// stands it on the ground there.
fn place_markers(
    time: Res<Time>,
    terrain: Option<Res<WorldTerrain>>,
    mut markers: Query<(&RemotePlayer, &mut Transform)>,
) {
    // The same frame-rate-independent easing as the camera's.
    let t = 1.0 - (-MARKER_SMOOTHING * time.delta_secs()).exp();

    for (player, mut transform) in &mut markers {
        let at = Vec2::new(transform.translation.x, transform.translation.z);
        let at = at.lerp(player.target, t);

        // Feet on the ground where it has streamed in, and on the surface
        // where the ground is under water — a player crossing open ocean is
        // sailing it, not walking the seabed. Ground still generating keeps
        // the last height, exactly as the camera's own focus does.
        let mut height = transform.translation.y;
        if let Some(ground) = terrain.as_ref().and_then(|t| t.0.ready_height(at.x, at.y)) {
            height = ground.max(0.0) + MARKER_LENGTH * 0.5 + MARKER_RADIUS;
        }
        transform.translation = Vec3::new(at.x, height, at.y);
    }
}

/// A pretend server on a loopback port: accepts one client, answers the
/// handshake, and hands the test the socket to keep speaking with. Lives out
/// here rather than in this module's tests because the menu's tests, which
/// join servers by clicking on things, want one too.
#[cfg(test)]
pub(crate) fn fake_server(seed: u32, spawn: Vec2) -> (String, Receiver<TcpStream>) {
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    let (handover, socket) = mpsc::channel();
    thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept");
        match ToServer::read(&mut &stream).expect("a first message") {
            ToServer::Hello { version } => assert_eq!(version, PROTOCOL_VERSION),
            other => panic!("expected a hello, got {other:?}"),
        }
        (ToClient::Welcome {
            id: PlayerId(1),
            seed,
            spawn,
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

    /// A headless app with the net systems running in a match, and no
    /// terrain — markers then keep their height, which these tests ignore.
    fn test_app(connection: Connection) -> App {
        let mut app = App::new();
        app.add_plugins((TimePlugin, StatesPlugin, NetPlugin))
            .init_state::<AppState>()
            .init_resource::<Assets<Mesh>>()
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
    fn hosting_a_world_is_a_session_like_any_other() {
        // Sharing a world runs the very same server a dedicated one runs, and
        // then joins it: what comes back is an ordinary welcome, and the seed
        // in it is the world that was asked for. On a port of the machine's
        // choosing, so that a test run cannot collide with a real server on
        // this machine; the menu asks for the well-known one.
        let dialing = Dialing::hosting(WorldConfig { seed: 77 }, 0);
        let session = settle(&dialing).expect("the world should be hosted and joined");

        assert_eq!(session.connection.seed, 77);
        assert!(
            session.hosting.is_some(),
            "the world was joined without anything hosting it"
        );
    }

    #[test]
    fn a_hosted_world_is_one_other_people_can_join() {
        // The point of sharing: the world the host is standing in is reachable
        // from outside, and whoever arrives is somebody else in the same
        // world rather than the host again.
        let dialing = Dialing::hosting(WorldConfig { seed: 3 }, 0);
        let session = settle(&dialing).expect("the world should be hosted and joined");
        let port = session.hosting.as_ref().expect("hosting").addr().port();

        let guest = Connection::join(&format!("127.0.0.1:{port}")).expect("a guest should get in");
        assert_eq!(guest.seed, 3, "the guest was let into a different world");
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
    fn joining_learns_the_world_from_the_welcome() {
        let (addr, _socket) = fake_server(42, Vec2::new(100.0, -200.0));
        let connection = Connection::join(&addr).expect("join");
        assert_eq!(connection.id, PlayerId(1));
        assert_eq!(connection.seed, 42);
        assert_eq!(connection.spawn, Vec2::new(100.0, -200.0));
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
        let (addr, socket) = fake_server(1, Vec2::ZERO);
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
    fn other_players_come_move_and_go_as_markers() {
        let (addr, socket) = fake_server(1, Vec2::ZERO);
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
        let (addr, socket) = fake_server(1, Vec2::ZERO);
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
}
