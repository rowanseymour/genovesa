//! The debug socket: the console, on a port instead of a keyboard.
//!
//! `--debug <port>` puts a listener on the loopback address that takes the
//! same lines the console takes, one per line of the connection, and writes
//! back what they answer. It is the whole of how this game is debugged from
//! outside itself. The command line used to carry a second vocabulary for
//! that — a view to open on, a list of pictures to take — and every option in
//! it could only ever say what the run should do *before* it started. A
//! socket says it at any point, as often as it likes, which is strictly more,
//! so those options went and this took their place.
//!
//! Nothing here invents a grammar. A line arrives, and it goes to
//! [`crate::console::dispatch`] exactly as a typed one would — so `set haze
//! off` doctors this client's picture and `weather gale` crosses the wire to
//! the server, by the same one syntactic rule, and a verb added to either
//! side is reachable from here the day it is added. What this module adds is
//! what a *keyboard* never needed words for, because a person at a window
//! already has them:
//!
//! - `shot <path>` — the eyes. A driver that can order a gale and not see it
//!   is not debugging anything.
//! - `press <action> [seconds]` — the hands. The client is where sailing
//!   happens: [`protocol::ToServer`] carries `Move` and `Helm`, which is a
//!   client *telling* a server where it got to, so no amount of commanding
//!   the world from the server's side can make a boat sail. Pressing the
//!   bound key is the honest way in — the same key the player's own hand
//!   would find, through the same bindings, so a press exercises the real
//!   control and not a shortcut past it.
//! - `focus`, `zoom`, `yaw` — the view, which used to be three options that
//!   could each be said once.
//! - `hold` — the clock, stopped, so that two pictures of one place differ in
//!   what they were taken to show and not in what hour it had got to.
//! - `quit` — the way out. A run that is hosting writes its world down as it
//!   goes (see [`crate::stopping`]), so a driver that ends by killing the
//!   process loses the last of the world it was making.
//!
//! **A line is answered when its work is done, and not before.** That is the
//! load-bearing property, and the reason this replaced a list of `--shot`s
//! rather than sitting beside it. `press forward 20` answers twenty seconds
//! later; `focus` answers once the ground at the new place has arrived and
//! the picture has stopped moving; `shot` answers when the file is on disk.
//! So a pipe of lines is a script rather than a race — each one starts from
//! where the last left the world — and the settling a capture run used to do
//! silently, in frames nobody could see, is now the thing an answer means.
//!
//! Answers are line-based and end with a blank line, which is what makes the
//! socket usable from a shell with nothing in between:
//!
//! ```sh
//! printf 'weather gale\npress forward 20\nshot gale.png\nquit\n' | nc 127.0.0.1 7777
//! ```
//!
//! A blank line inside an answer would end it early, so blank lines are
//! dropped from answers rather than escaped: nothing either grammar says
//! carries meaning in one.
//!
//! The socket serves one connection through to its end before accepting
//! another. There is no reason for two drivers at once — the world has one of
//! everything they would both be moving.
//!
//! It is loopback TCP rather than a Unix socket for the plainest of reasons —
//! a Unix socket would put `#[cfg(unix)]` through this module and leave the
//! game unbuildable on a platform that only ever wanted to compile it. A port
//! on `127.0.0.1` is reachable by anything else on this machine, which is the
//! same footing the console already stands on: anyone in a session may
//! command it. It is a development instrument and is off unless asked for.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender};
use std::sync::Mutex;
use std::thread;

use args::{metres, pair};
use bevy::asset::RenderAssetUsages;
use bevy::camera::RenderTarget;
use bevy::ecs::system::SystemParam;
use bevy::image::Image;
use bevy::input::InputSystems;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
use bevy::render::view::screenshot::{save_to_disk, Screenshot};

use crate::bindings::{Action, KeyBindings};
use crate::camera::{MapCamera, View, MAX_DISTANCE, MIN_DISTANCE};
use crate::console::{dispatch, Dispatch};
use crate::debug::Toggles;
use crate::net::Online;
use crate::player::PlayerSweep;
use crate::terrain::{ChunkBuild, Ground};

/// What `help` says about this end of the grammar, before the server's own
/// answer is appended to it. The server's half is asked for rather than
/// guessed at, exactly as the console's tab completion asks — so one `help`
/// down the socket is the whole vocabulary, both sides of the wire.
const HELP: &str = "shot <path> — write a PNG of the view, once the ground has arrived\n\
                    press <action> [seconds] — hold a control down, or tap it if no time is given\n\
                    focus <x,z> — put the player down at a map point, in metres\n\
                    zoom <m> — camera distance\n\
                    yaw <deg> — bearing to look from\n\
                    hold on|off — stop the clock where it stands, so the light keeps still\n\
                    quit — close the world and stop the game\n\
                    set … — this client's own switches; `set` alone lists them";

/// The longest a single `press` will hold a key. A driver that meant to sail
/// for twenty seconds and typed twenty thousand should get an answer rather
/// than a socket that never speaks again — the line is held until the press
/// ends, so an absurd one is indistinguishable from a hang.
const LONGEST_PRESS: f32 = 600.0;

/// Frames to let the picture stand still once the ground has stopped
/// arriving. The camera snaps rather than eases, so this only has to cover
/// dropping the focus back onto the ground and redrawing the shadow maps.
const SETTLE_FRAMES: u32 = 8;

/// Frames to let the ground reach the screen, or a picture reach the disk,
/// before giving up. Generous, because the ground comes from a server and an
/// answer is the proof a line's work is done — but bounded, because a driver
/// waiting forever cannot tell a slow world from a dead one.
const PATIENCE: u32 = 1800;

/// Frames to leave a picture alone after the file first appears. It is
/// created before it is written, so existing is not the same as being
/// finished, and a driver that read it on the instant could read half a PNG.
const SHOT_SETTLE: u32 = 2;

/// Frames to wait for a server's reply before answering without one. A
/// command that crosses the wire is fire-and-forget — see
/// [`crate::net::Connection::command`] — so nothing here can distinguish a
/// slow answer from one that is never coming, and the socket must not hang on
/// the difference.
const ASK_FRAMES: u32 = 600;

/// The socket, and the one line it has in hand.
///
/// Public because [`crate::net`] answers into it: a reply from the server
/// belongs to whoever asked, and if that was the socket then the socket is
/// what has been waiting for it.
#[derive(Resource)]
pub struct Control {
    /// Lines arriving from the listening thread. Behind a mutex because a
    /// [`Receiver`] is `Send` but not `Sync`, and a resource must be both —
    /// uncontended in practice, one reader and one writer.
    orders: Mutex<Receiver<Order>>,
    /// The line being served, if one is. At most one, ever: the thread blocks
    /// on each answer before reading the next line.
    doing: Option<Doing>,
    /// Where a picture renders when the run has no window. `None` in a
    /// windowed run, where the window itself is what gets photographed.
    target: Option<Handle<Image>>,
    /// Whether the clock is being held where it stands — see
    /// [`hold_the_sky`], which does the holding every frame because the
    /// server keeps saying what hour it really is.
    holding: bool,
}

/// One line, and the way back to whoever sent it.
struct Order {
    line: String,
    answer: SyncSender<String>,
}

/// A line that cannot be answered on the frame it arrived.
enum Doing {
    /// A key is down. Released when the time runs out, and answered then —
    /// the answer is what says the press is over.
    Pressing {
        key: KeyCode,
        left: f32,
        said: String,
        answer: SyncSender<String>,
    },
    /// The view has been moved; waiting for the world to catch up with it.
    Settling {
        waited: u32,
        patience: u32,
        said: String,
        answer: SyncSender<String>,
    },
    /// A picture is on its way, somewhere in [`Stage`].
    Shooting {
        path: PathBuf,
        target: Option<Handle<Image>>,
        stage: Stage,
        waited: u32,
        patience: u32,
        answer: SyncSender<String>,
    },
    /// A line has gone to the server; waiting for what it says back. The
    /// prefix is this end's own half of the answer, which `help` has and
    /// nothing else does.
    Asking {
        prefix: String,
        waited: u32,
        answer: SyncSender<String>,
    },
}

/// How far along a picture is.
#[derive(Clone, Copy)]
enum Stage {
    /// Waiting for the ground to arrive and the view to stand still. What
    /// makes a shot a picture of the world rather than of however much of it
    /// had turned up.
    Arriving,
    /// The picture has been asked for; waiting for the file to appear.
    Asked,
    /// The file is there; leaving it alone long enough to be finished.
    Written,
}

impl Control {
    /// Hands a server's reply to whoever is waiting for one. Called by
    /// [`crate::net::receive`] on every [`protocol::ToClient::Reply`], which
    /// may well be answering a line somebody typed at the console instead —
    /// so a reply arriving while nothing is waiting is dropped here and
    /// printed there, rather than being anybody's answer.
    pub fn answered(&mut self, text: &str) {
        if !matches!(self.doing, Some(Doing::Asking { .. })) {
            return;
        }
        let Some(Doing::Asking { prefix, answer, .. }) = self.doing.take() else {
            return;
        };
        let reply = if prefix.is_empty() {
            text.to_string()
        } else {
            format!("{prefix}\n{text}")
        };
        let _ = answer.send(reply);
    }
}

/// Opens the socket and starts listening on it, or says why it could not be
/// opened. Called before the app exists, so that a port already in use is a
/// plain failure to start rather than an error deep in a running game.
pub fn open(port: u16) -> Result<Control, String> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
        .map_err(|why| format!("cannot listen on port {port}: {why}"))?;
    let (orders, waiting) = channel();
    thread::spawn(move || listen(&listener, &orders));
    Ok(Control {
        orders: Mutex::new(waiting),
        doing: None,
        target: None,
        holding: false,
    })
}

/// Serves connections, one through to its end before the next.
fn listen(listener: &TcpListener, orders: &Sender<Order>) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        // An error is this connection's, not the listener's: a driver that
        // hangs up mid-line ends its own session and no more than that.
        if serve(stream, orders).is_err() {
            continue;
        }
    }
}

/// Reads lines off one connection, and writes back what each is answered
/// with. Blocks on every answer, which is the back-pressure: the driver
/// cannot get ahead of the world.
fn serve(stream: TcpStream, orders: &Sender<Order>) -> std::io::Result<()> {
    let mut out = stream.try_clone()?;
    for line in BufReader::new(stream).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        // A rendezvous of one, so the game can hand over its answer and move
        // on without waiting to be read.
        let (answer, answered) = sync_channel(1);
        if orders.send(Order { line, answer }).is_err() {
            // The game has gone. Nothing more will ever be answered, so let
            // the driver see the connection end rather than wait on it.
            return Ok(());
        }
        let Ok(reply) = answered.recv() else {
            return Ok(());
        };
        for said in reply.lines().filter(|said| !said.trim().is_empty()) {
            writeln!(out, "{said}")?;
        }
        writeln!(out)?;
        out.flush()?;
    }
    Ok(())
}

/// Serves the socket, and — where the run has no window — gives the camera
/// somewhere to draw so that `shot` has something to photograph.
pub struct ControlPlugin {
    /// Whether this run has a window. Without one the camera has to be
    /// pointed at an image instead.
    pub headless: bool,
    /// Size of that image, and so of every picture `shot` writes. Ignored in
    /// a windowed run, where a picture is the size of the window.
    pub resolution: UVec2,
}

impl Plugin for ControlPlugin {
    fn build(&self, app: &mut App) {
        if self.headless {
            let target = app
                .world_mut()
                .resource_mut::<Assets<Image>>()
                .add(offscreen(self.resolution));
            app.world_mut().resource_mut::<Control>().target = Some(target);
            // The UI is drawn into these pictures — the readout and the
            // compass are in every one — and the system that fits it to a
            // window has no window to read, so it is fitted to the picture
            // here instead. Once, because unlike a window an image cannot be
            // dragged to another size.
            app.insert_resource(UiScale(crate::settings::fitted_to(
                self.resolution.as_vec2(),
            )));
            // After `Startup`, so the camera the map plugin spawns there
            // exists to be pointed somewhere.
            app.add_systems(PostStartup, render_off_screen);
        }
        // The console's own switches, which `set` lines write. Initialised
        // here as well as by the console, each plugin standing up what it
        // reads.
        app.init_resource::<Toggles>()
            // After the input plugin has filled the key state, so that a key
            // this module presses is not cleared by the same frame's real
            // input, and is seen by every system that reads it in `Update`.
            .add_systems(PreUpdate, serve_orders.after(InputSystems))
            .add_systems(Update, hold_the_sky);
    }
}

/// An image for the camera to draw into. `COPY_SRC` is what lets the finished
/// picture be read back off the GPU.
fn offscreen(resolution: UVec2) -> Image {
    let size = Extent3d {
        width: resolution.x,
        height: resolution.y,
        depth_or_array_layers: 1,
    };
    let mut image = Image::new_fill(
        size,
        TextureDimension::D2,
        &[0, 0, 0, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.texture_descriptor.usage =
        TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC | TextureUsages::TEXTURE_BINDING;
    image
}

/// Points the camera at the off-screen image. Runs after the camera has been
/// spawned rather than as part of spawning it, so that the camera itself
/// knows nothing about being photographed.
fn render_off_screen(
    control: Res<Control>,
    mut cameras: Query<&mut RenderTarget, With<MapCamera>>,
) {
    let Some(target) = &control.target else {
        return;
    };
    for mut camera in &mut cameras {
        *camera = target.clone().into();
    }
}

/// Holds the day where it stands while `hold` is on.
///
/// Every frame, because the server keeps saying what hour it really is and
/// the hold is this client's own overriding of that — see
/// [`crate::sky::Sky::hold`]. Stopping the clock is what makes two pictures
/// of one place comparable: without it they differ in the light as well as in
/// whatever they were taken to show.
fn hold_the_sky(control: Res<Control>, mut sky: ResMut<crate::sky::Sky>) {
    if control.holding {
        sky.hold();
    }
}

/// Whether any of the ground being looked at has yet to reach the screen.
///
/// Both halves matter, and neither can see the other's: a chunk that has been
/// asked for and not answered has no entity for the query to find, and one
/// that has been answered and not meshed has nothing on screen for the
/// resource to know about.
#[derive(SystemParam)]
struct GroundArriving<'w, 's> {
    ground: Option<Res<'w, Ground>>,
    building: Query<'w, 's, (), With<ChunkBuild>>,
}

impl GroundArriving<'_, '_> {
    fn still_coming(&self) -> bool {
        !self.building.is_empty() || self.ground.as_ref().is_some_and(|it| !it.settled())
    }
}

/// What serving a line needs to reach. Gathered because the work is one job —
/// take a line, answer it — and splitting it across systems would only mean
/// threading the line between them.
#[derive(SystemParam)]
struct Hands<'w, 's> {
    toggles: ResMut<'w, Toggles>,
    keys: ResMut<'w, ButtonInput<KeyCode>>,
    bindings: Res<'w, KeyBindings>,
    /// Absent until a world is joined, and on the menu screens.
    online: Option<Res<'w, Online>>,
    view: ResMut<'w, View>,
    cameras: Query<'w, 's, &'static mut MapCamera>,
    player: PlayerSweep<'w, 's>,
    ground: GroundArriving<'w, 's>,
}

impl Hands<'_, '_> {
    /// Puts the view where a line asked for it, and moves the player under it
    /// when the point itself moved. The camera is pinned to the player, so a
    /// focus is a sweep of whatever carries them — teleported, there being
    /// nobody to watch it sail there.
    fn look(&mut self, wanted: View, moved: bool) {
        *self.view = wanted;
        for mut camera in &mut self.cameras {
            camera.snap_to(wanted);
        }
        if moved {
            self.player
                .teleport(Vec2::new(wanted.focus.x, wanted.focus.z));
        }
    }

    /// Whether the picture has stopped changing: the ground here has all
    /// arrived, and enough frames have passed for the shadows to agree with
    /// it. The one notion of settled, shared by every line that waits.
    fn settled(&self, waited: u32) -> bool {
        !self.ground.still_coming() && waited >= SETTLE_FRAMES
    }

    /// How long a wait has run, held at zero while ground is still on its
    /// way — so the frames that follow are all settling and none of them
    /// waiting on a server.
    fn waited(&self, waited: u32) -> u32 {
        if self.ground.still_coming() {
            0
        } else {
            waited + 1
        }
    }
}

/// Finishes the line in hand, or takes the next one. Never both in a frame: a
/// press that ends this frame has had its key down for every frame of its
/// time, and the line after it starts from a world that has stopped moving.
fn serve_orders(
    mut commands: Commands,
    mut control: ResMut<Control>,
    mut hands: Hands,
    time: Res<Time>,
    mut exit: MessageWriter<AppExit>,
) {
    if let Some(doing) = control.doing.take() {
        control.doing = advance(doing, &mut commands, &mut hands, &time);
        return;
    }

    let order = {
        let Ok(orders) = control.orders.lock() else {
            return;
        };
        let Ok(order) = orders.try_recv() else {
            return;
        };
        order
    };

    // Copied out and written back so that serving the line can borrow the
    // rest of the resource — the handle it photographs into, and the switch
    // `hold` throws.
    let target = control.target.clone();
    let mut holding = control.holding;
    control.doing = begin(
        order,
        &mut hands,
        &mut exit,
        Held {
            target,
            holding: &mut holding,
        },
    );
    control.holding = holding;
}

/// The pieces of [`Control`] a line may read or write while it is being
/// served, lifted out so the rest of the resource stays borrowable.
struct Held<'a> {
    target: Option<Handle<Image>>,
    holding: &'a mut bool,
}

/// Carries a line that could not be finished at once one frame further, or
/// answers it and has done.
fn advance(doing: Doing, commands: &mut Commands, hands: &mut Hands, time: &Time) -> Option<Doing> {
    match doing {
        Doing::Pressing {
            key,
            left,
            said,
            answer,
        } => {
            let left = left - time.delta_secs();
            if left > 0.0 {
                return Some(Doing::Pressing {
                    key,
                    left,
                    said,
                    answer,
                });
            }
            // Released rather than left down: the next line starts from a
            // world with nothing held, which is what makes a script of
            // presses read as a sequence of separate moves.
            hands.keys.release(key);
            let _ = answer.send(said);
            None
        }
        Doing::Settling {
            waited,
            patience,
            said,
            answer,
        } => {
            if hands.settled(waited) {
                let _ = answer.send(said);
                return None;
            }
            if patience >= PATIENCE {
                let _ = answer.send(format!("{said}, with ground still arriving"));
                return None;
            }
            Some(Doing::Settling {
                waited: hands.waited(waited),
                patience: patience + 1,
                said,
                answer,
            })
        }
        Doing::Shooting {
            path,
            target,
            stage,
            waited,
            patience,
            answer,
        } => {
            if patience >= PATIENCE {
                let _ = answer.send(match stage {
                    Stage::Arriving => {
                        format!("the ground never finished arriving for {}", path.display())
                    }
                    _ => format!("nothing was written to {}", path.display()),
                });
                return None;
            }
            let (stage, waited) = match stage {
                Stage::Arriving if hands.settled(waited) => {
                    // A window is photographed as a window; a run without one
                    // is photographed off the image its camera draws into.
                    let mut asked = match &target {
                        Some(target) => commands.spawn(Screenshot::image(target.clone())),
                        None => commands.spawn(Screenshot::primary_window()),
                    };
                    asked.observe(save_to_disk(path.clone()));
                    (Stage::Asked, 0)
                }
                Stage::Arriving => (Stage::Arriving, hands.waited(waited)),
                Stage::Asked if path.exists() => (Stage::Written, 0),
                Stage::Asked => (Stage::Asked, waited + 1),
                Stage::Written if waited >= SHOT_SETTLE => {
                    let _ = answer.send(format!("shot {}", path.display()));
                    return None;
                }
                Stage::Written => (Stage::Written, waited + 1),
            };
            Some(Doing::Shooting {
                path,
                target,
                stage,
                waited,
                patience: patience + 1,
                answer,
            })
        }
        Doing::Asking {
            prefix,
            waited,
            answer,
        } => {
            if waited >= ASK_FRAMES {
                let unanswered = "the server said nothing back";
                let reply = if prefix.is_empty() {
                    unanswered.to_string()
                } else {
                    format!("{prefix}\n{unanswered}")
                };
                let _ = answer.send(reply);
                return None;
            }
            Some(Doing::Asking {
                prefix,
                waited: waited + 1,
                answer,
            })
        }
    }
}

/// Starts a line. Answers it outright where it can, and hands back what to go
/// on doing where it cannot.
fn begin(
    order: Order,
    hands: &mut Hands,
    exit: &mut MessageWriter<AppExit>,
    held: Held,
) -> Option<Doing> {
    let Order { line, answer } = order;
    let words: Vec<&str> = line.split_whitespace().collect();
    match words.split_first() {
        Some((&"shot", rest)) => match shot(rest) {
            Ok(path) => Some(Doing::Shooting {
                path,
                target: held.target,
                stage: Stage::Arriving,
                waited: 0,
                patience: 0,
                answer,
            }),
            Err(why) => refuse(why, answer),
        },
        Some((&"press", rest)) => match press(rest) {
            Ok((action, left)) => {
                // The key the action is *bound* to rather than one of this
                // module's choosing, so a rebound control is pressed where
                // the player put it and a press goes down the same path a
                // hand would.
                let key = hands.bindings.key(action);
                hands.keys.press(key);
                Some(Doing::Pressing {
                    key,
                    left,
                    said: pressed(action, left),
                    answer,
                })
            }
            Err(why) => refuse(why, answer),
        },
        Some((&"focus", rest)) => match one(rest, "focus", "<x,z>", focus) {
            Ok(point) => {
                let wanted = View {
                    focus: point,
                    ..*hands.view
                };
                hands.look(wanted, true);
                settling(format!("focus {},{}", point.x, point.z), answer)
            }
            Err(why) => refuse(why, answer),
        },
        Some((&"zoom", rest)) => match one(rest, "zoom", "<metres>", zoom) {
            Ok(distance) => {
                let wanted = View {
                    distance,
                    ..*hands.view
                };
                hands.look(wanted, false);
                settling(format!("zoom {distance}"), answer)
            }
            Err(why) => refuse(why, answer),
        },
        Some((&"yaw", rest)) => match one(rest, "yaw", "<degrees>", yaw) {
            Ok(bearing) => {
                let wanted = View {
                    yaw: bearing,
                    ..*hands.view
                };
                hands.look(wanted, false);
                settling(format!("yaw {}", bearing.to_degrees().round()), answer)
            }
            Err(why) => refuse(why, answer),
        },
        Some((&"hold", rest)) => match onoff(rest, "hold") {
            Ok(on) => {
                *held.holding = on;
                let _ = answer.send(if on {
                    "the clock is held where it stands".to_string()
                } else {
                    "the clock runs again".to_string()
                });
                None
            }
            Err(why) => refuse(why, answer),
        },
        Some((&"quit", [])) => {
            let _ = answer.send("closing the world".to_string());
            exit.write(AppExit::Success);
            None
        }
        // Both halves of the vocabulary in one answer: this end's, which the
        // server has never heard of, and the server's, which is the only
        // place its own verbs are written down.
        Some((&"help", [])) => forward(&line, HELP.to_string(), hands, answer),
        _ => match dispatch(&line, &mut hands.toggles) {
            Dispatch::Local(reply) => {
                let _ = answer.send(reply);
                None
            }
            Dispatch::Remote => forward(&line, String::new(), hands, answer),
        },
    }
}

/// Says no, and has done with the line.
fn refuse(why: String, answer: SyncSender<String>) -> Option<Doing> {
    let _ = answer.send(why);
    None
}

/// Waits for the world to catch up with a view that has just moved, and says
/// what was asked for once it has.
fn settling(said: String, answer: SyncSender<String>) -> Option<Doing> {
    Some(Doing::Settling {
        waited: 0,
        patience: 0,
        said,
        answer,
    })
}

/// Puts a line on the wire and waits for what the server says back.
fn forward(line: &str, prefix: String, hands: &Hands, answer: SyncSender<String>) -> Option<Doing> {
    let Some(online) = &hands.online else {
        let _ = answer.send("nobody is serving this world".to_string());
        return None;
    };
    online.connection.command(line.to_string());
    Some(Doing::Asking {
        prefix,
        waited: 0,
        answer,
    })
}

/// The verbs that take exactly one value, read the same way: the shape of the
/// refusal is the same for all of them, so it is written once.
fn one<T>(
    args: &[&str],
    verb: &str,
    wants: &str,
    read: impl Fn(&str) -> Result<T, String>,
) -> Result<T, String> {
    match args {
        [value] => read(value),
        _ => Err(format!("`{verb}` wants one value — `{verb} {wants}`")),
    }
}

/// An `on` or an `off`, which is what a switch down this socket looks like.
fn onoff(args: &[&str], verb: &str) -> Result<bool, String> {
    match args {
        ["on"] => Ok(true),
        ["off"] => Ok(false),
        _ => Err(format!("`{verb}` is on or off — `{verb} on`")),
    }
}

/// A map point in metres, as `x,z`. The height is left at zero: the camera
/// puts itself down on the ground on its first frame.
fn focus(value: &str) -> Result<Vec3, String> {
    let (x, z) = pair(value, ',', metres, "an x,z point in metres, e.g. 98,-317")?;
    Ok(Vec3::new(x, 0.0, z))
}

/// A camera distance, inside what the camera will actually go to.
fn zoom(value: &str) -> Result<f32, String> {
    let distance: f32 = value
        .parse()
        .map_err(|_| format!("`{value}` is not a distance in metres"))?;
    if !(MIN_DISTANCE..=MAX_DISTANCE).contains(&distance) {
        return Err(format!(
            "`zoom` is between {MIN_DISTANCE} and {MAX_DISTANCE} metres, not {distance}"
        ));
    }
    Ok(distance)
}

/// A bearing in degrees, kept as radians the way the view holds it.
fn yaw(value: &str) -> Result<f32, String> {
    let degrees: f32 = value
        .parse()
        .map_err(|_| format!("`{value}` is not a bearing in degrees"))?;
    if !degrees.is_finite() {
        return Err(format!("`{value}` is not a bearing in degrees"));
    }
    Ok(degrees.to_radians())
}

/// `shot <path>`: where the picture goes.
///
/// The file is removed first, so that its appearing is the proof this picture
/// was written and not the last one that happened to have the name.
fn shot(args: &[&str]) -> Result<PathBuf, String> {
    let [path] = args else {
        return Err("`shot` wants one path — `shot near.png`".to_string());
    };
    let path = PathBuf::from(path);
    if let Err(why) = fs::remove_file(&path) {
        if why.kind() != std::io::ErrorKind::NotFound {
            return Err(format!("cannot write {}: {why}", path.display()));
        }
    }
    Ok(path)
}

/// `press <action> [seconds]`: which control, and for how long.
///
/// Parsing only — the pressing is [`begin`]'s, which has the bindings and the
/// key state. Split that way so what a line is allowed to say can be tested
/// without standing up a world to say it in.
fn press(args: &[&str]) -> Result<(Action, f32), String> {
    let (named, seconds) = match args {
        // No time given is a tap: the key goes down now and comes up on the
        // next frame, which is exactly one frame of `just_pressed` for the
        // controls that read an edge rather than a hold.
        [named] => (*named, 0.0),
        [named, seconds] => {
            let seconds: f32 = seconds
                .parse()
                .map_err(|_| format!("`{seconds}` is not a number of seconds"))?;
            if !seconds.is_finite() || seconds < 0.0 {
                return Err(format!("`{seconds}` is not a length of time"));
            }
            if seconds > LONGEST_PRESS {
                return Err(format!(
                    "`press` will hold a key for up to {LONGEST_PRESS:.0} seconds, not {seconds}"
                ));
            }
            (*named, seconds)
        }
        _ => {
            return Err(format!(
                "`press` wants a control and a time — `press forward 20`\n{}",
                actions()
            ))
        }
    };
    let Some(action) = Action::ALL.into_iter().find(|it| it.name() == named) else {
        return Err(format!("no control called `{named}`\n{}", actions()));
    };
    Ok((action, seconds))
}

/// What a press answers with when it is over. A tap and a hold are told apart
/// because a driver that meant to sail and forgot the seconds gets a boat
/// that has not moved, and the answer is the only place that shows.
fn pressed(action: Action, seconds: f32) -> String {
    if seconds > 0.0 {
        format!("{} held {seconds} seconds", action.name())
    } else {
        format!("{} tapped", action.name())
    }
}

/// The controls a `press` will take, as one line — read off [`Action::ALL`]
/// so a control added to the game is offered here without being listed twice.
fn actions() -> String {
    let names: Vec<&str> = Action::ALL.iter().map(|it| it.name()).collect();
    format!("controls: {}", names.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The grammar of a press, which is the one line here with anything much
    /// to get wrong: most of what this module takes it hands straight to the
    /// console's own [`dispatch`].
    #[test]
    fn press_reads_a_control_and_a_time() {
        assert_eq!(press(&["forward", "20"]), Ok((Action::MoveForward, 20.0)));
        assert_eq!(press(&["claim"]), Ok((Action::Claim, 0.0)));
        assert_eq!(press(&["view-left", "0.5"]), Ok((Action::TurnLeft, 0.5)));
    }

    /// Every refusal names what was wrong with the line. A driver reads these
    /// down a socket with no other way of asking what it should have said, so
    /// the answer has to carry it.
    #[test]
    fn press_refuses_what_it_cannot_do() {
        for (line, expected) in [
            (vec!["sideways"], "no control called `sideways`"),
            (vec!["forward", "soon"], "`soon` is not a number of seconds"),
            (vec!["forward", "-3"], "not a length of time"),
            (vec!["forward", "99999"], "will hold a key for up to"),
            (vec![], "wants a control and a time"),
            (vec!["forward", "1", "2"], "wants a control and a time"),
        ] {
            let why = press(&line).expect_err(&format!("`press {line:?}` should be refused"));
            assert!(why.contains(expected), "`{why}` does not say `{expected}`");
        }
    }

    /// A refusal to name a control lists the ones there are, because the
    /// names are this module's invention and nothing else says them.
    #[test]
    fn a_missed_control_lists_them() {
        let why = press(&["sideways"]).expect_err("no such control");
        for action in Action::ALL {
            assert!(
                why.contains(action.name()),
                "`{why}` omits {}",
                action.name()
            );
        }
    }

    /// A tap and a hold say different things, so that a driver that meant to
    /// sail for twenty seconds and left the number off can see that it did.
    #[test]
    fn a_press_says_which_kind_it_was() {
        assert_eq!(
            pressed(Action::MoveForward, 20.0),
            "forward held 20 seconds"
        );
        assert_eq!(pressed(Action::Chart, 0.0), "chart tapped");
    }

    /// The view verbs, which took the place of `--focus`, `--zoom` and
    /// `--yaw` and have to refuse what those refused.
    #[test]
    fn the_view_verbs_read_what_the_options_used_to() {
        assert_eq!(focus("98,-317"), Ok(Vec3::new(98.0, 0.0, -317.0)));
        assert_eq!(zoom("120"), Ok(120.0));
        assert_eq!(yaw("90"), Ok(std::f32::consts::FRAC_PI_2));

        assert!(focus("98").is_err(), "needs both axes");
        assert!(focus("north,south").is_err());
        assert!(zoom("5").is_err(), "closer than the camera goes");
        assert!(zoom("5000").is_err(), "further than it goes");
        assert!(yaw("sideways").is_err());
    }

    /// A verb that takes one value says so when it is given none or several,
    /// naming itself and what it wanted.
    #[test]
    fn a_verb_wanting_one_value_says_which_it_is() {
        let why = one(&[], "zoom", "<metres>", zoom).expect_err("no value");
        assert!(why.contains("`zoom` wants one value"), "{why}");
        assert!(why.contains("zoom <metres>"), "{why}");
        assert!(one(&["1", "2"], "zoom", "<metres>", zoom).is_err());
    }

    #[test]
    fn a_switch_is_on_or_off() {
        assert_eq!(onoff(&["on"], "hold"), Ok(true));
        assert_eq!(onoff(&["off"], "hold"), Ok(false));
        let why = onoff(&["maybe"], "hold").expect_err("not a switch");
        assert!(why.contains("`hold` is on or off"), "{why}");
    }
}
