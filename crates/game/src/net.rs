//! Playing in a served world: the game as a client.
//!
//! Terrain never arrives over the wire. The server's welcome names the seed,
//! and the seed *is* the world — the `world` crate regenerates it here, bit
//! for bit, exactly as a local run would have. What the connection carries is
//! the session: who else is in the world and where they are, drawn as marker
//! capsules standing on the same ground every machine is generating for
//! itself.
//!
//! [`Connection::join`] speaks the handshake synchronously, before the app is
//! built, because what it learns is what the app gets built *from*. After
//! that a reader thread turns the socket into a channel the schedule drains
//! once a frame ([`receive`]), and the player's own movements trickle back
//! the other way ([`report_position`]).

use std::collections::HashMap;
use std::net::{Shutdown, TcpStream};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use bevy::prelude::*;

use protocol::{PlayerId, ToClient, ToServer, DEFAULT_PORT, PROTOCOL_VERSION};

use crate::camera::MapCamera;
use crate::terrain::WorldTerrain;
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
        let addr = if addr.contains(':') {
            addr.to_string()
        } else {
            format!("{addr}:{DEFAULT_PORT}")
        };
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

/// The joined session. Present only when the run was started with `--join`;
/// every system here conditions on it, so offline runs pay nothing.
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
        .add_systems(
            OnExit(AppState::InWorld),
            disconnect.run_if(resource_exists::<Online>),
        );
    }
}

/// Leaving the world ends the session: dropping the connection shuts the
/// socket down, which is how the server hears it. A fresh world entered from
/// the menu is a local one, where the served players would be strangers. The
/// markers go with the resource, being `DespawnOnExit` of the same state.
fn disconnect(mut commands: Commands) {
    commands.remove_resource::<Online>();
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

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    use bevy::state::app::StatesPlugin;
    use bevy::time::TimePlugin;

    use super::*;

    /// A pretend server on a loopback port: accepts one client, answers the
    /// handshake, and hands the test the socket to keep speaking with.
    fn fake_server(seed: u32, spawn: Vec2) -> (String, mpsc::Receiver<TcpStream>) {
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

    /// Runs frames until the condition holds. The messages cross a real
    /// socket and a channel, so a frame or two of patience is legitimate —
    /// five seconds of it is a failure.
    fn run_until(app: &mut App, what: &str, mut done: impl FnMut(&mut App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            app.update();
            if done(app) {
                return;
            }
            thread::sleep(Duration::from_millis(2));
        }
        panic!("timed out waiting until {what}");
    }

    fn markers(app: &mut App) -> Vec<(PlayerId, Vec2)> {
        app.world_mut()
            .query::<&RemotePlayer>()
            .iter(app.world())
            .map(|player| (player.id, player.target))
            .collect()
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
        // Nothing is listening there, so all that can be checked is that the
        // error names the port the dial actually used.
        let error = Connection::join("127.0.0.1").err().expect("nobody home");
        assert!(
            error.contains(&format!(":{DEFAULT_PORT}")),
            "the default port was not applied: {error}"
        );
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
