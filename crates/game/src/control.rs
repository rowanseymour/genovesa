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
//! - `click <button>` — the menus. They are driven by the mouse and nothing
//!   else, and a windowless run has no window, so no cursor, so no pointer
//!   for Bevy's picking to work from: naming the button is not a shortcut
//!   chosen over a click, it is the only door there is. It goes through the
//!   same systems a click does, and skips only the hit-testing. What it buys
//!   is the screens nothing can otherwise reach — a controls row armed and
//!   waiting for a key, a display trial counting down — and the transitions
//!   between screens, which anything that opens a screen outright walks past.
//! - `zoom`, `yaw` — the view, which used to be two options that could each
//!   be said once. Where the *player* is is the server's `goto`, and the
//!   camera goes with them: it is pinned to whatever carries them, and snaps
//!   rather than eases when that jumps.
//! - `hold` — the clock, stopped, so that two pictures of one place differ in
//!   what they were taken to show and not in what hour it had got to. It
//!   waits for the sky to reach the hour the world is at before freezing it,
//!   for the reason everything here waits: `time` moves the *world's* clock,
//!   and the sky eases onto that rather than jumping, so a hold thrown on the
//!   frame `time` was answered would pin the hour the light was leaving.
//! - `quit` — the way out. A run that is hosting writes its world down as it
//!   goes (see [`crate::stopping`]), so a driver that ends by killing the
//!   process loses the last of the world it was making.
//!
//! **A line is answered when its work is done, and not before.** That is the
//! load-bearing property, and the reason this replaced a list of `--shot`s
//! rather than sitting beside it. `press forward 20` answers twenty seconds
//! later; a `goto` answers once the ground at the new place has arrived and
//! the picture has stopped moving; `shot` answers when the file is on disk.
//! So a pipe of lines is a script rather than a race — each one starts from
//! where the last left the world — and the settling that used to happen
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
//! command it. In one respect it is a wider door than the console ever was,
//! and it is worth saying plainly rather than leaving to be discovered:
//! `shot <path>` unlinks the file it is about to write (see [`shot`]), so
//! whatever can reach the port can delete any file this run's user can. It is
//! a development instrument and is off unless asked for.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender};
use std::sync::{Mutex, PoisonError};
use std::thread;

use bevy::asset::RenderAssetUsages;
use bevy::camera::RenderTarget;
use bevy::ecs::system::SystemParam;
use bevy::image::Image;
use bevy::input::InputSystems;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
use bevy::render::view::screenshot::{save_to_disk, Screenshot};

use crate::bindings::{self, Action, KeyBindings};
use crate::camera::{MapCamera, View, MAX_DISTANCE, MIN_DISTANCE};
use crate::console::{dispatch, Dispatch};
use crate::debug::Toggles;
use crate::menu::MenuButton;
use crate::net::Online;
use crate::settings;
use crate::terrain::{ChunkBuild, Ground};

/// What `help` says about this end of the grammar, before the server's own
/// answer is appended to it. The server's half is asked for rather than
/// guessed at, exactly as the console's tab completion asks — so one `help`
/// down the socket is the whole vocabulary, both sides of the wire.
const HELP: &str = "shot <path> — write a PNG of the view, once the ground has arrived\n\
                    press <control> [seconds] — hold a control down, or tap it if no time is given; \
                     `escape` too, which is nobody's control\n\
                    click <button> — press a menu button; `click` alone lists them\n\
                    zoom <m> — camera distance\n\
                    yaw <deg> — bearing to look from\n\
                    hold on|off — stop the clock where it stands, so the light keeps still\n\
                    quit — close the world and stop the game\n\
                    set … — this client's own switches; `set` alone lists them";

/// How tall a picture a windowless run writes until the console says
/// otherwise, in rows off [`crate::settings::LADDER`]. 1440 matches the shots
/// already in `screenshots/`, which came off a 1280x720 window on a doubled
/// display.
const DEFAULT_ROWS: u32 = 1440;

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
    /// Whether the world has put this player down since the line in hand was
    /// sent — see [`Control::put_down`].
    ///
    /// What lets a forwarded line that *moved* somebody wait for the ground
    /// without this end knowing which of the server's verbs move people. The
    /// grammar is the server's and is not parsed here (see
    /// [`crate::console`]); a put down is a fact on the wire, so anything the
    /// server ever adds that moves a player settles the same way `goto` does.
    put_down: bool,
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
    /// A menu button has been pressed, and the stand-in carrying the press
    /// has still to be taken away — see [`begin`]'s `click`.
    Clicking {
        stand_in: Entity,
        said: String,
        answer: SyncSender<String>,
    },
    /// `hold on`, waiting for the drawn hour to catch up with the world's
    /// before it freezes anything — see [`crate::sky::Sky::caught_up`].
    Holding {
        waited: u32,
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
    ///
    /// Whoever is waiting, not whoever asked: replies carry nothing to match
    /// them against, so a reply that arrives after its own line gave up
    /// waiting ([`ASK_FRAMES`]) is handed to the *next* line instead, and the
    /// driver reads it one answer late. Correlating properly would mean a
    /// token on the wire in both directions for the sake of a case that only
    /// arises when the server has already been declared silent, so this is
    /// written down rather than fixed.
    pub fn answered(&mut self, text: &str) {
        // Taken and put back rather than matched through a reference: sending
        // consumes the channel, and anything else in hand is another line
        // still being served, which this reply is none of.
        match self.doing.take() {
            Some(Doing::Asking { prefix, answer, .. }) => {
                let said = both_halves(&prefix, text);
                // A line that moved the player is not finished when the
                // server says it is: the ground where they now are has still
                // to arrive, and a `shot` on the next line would photograph
                // whatever had turned up. So such a line becomes a settle,
                // exactly as the view verbs are, and is answered when the
                // picture has stopped changing.
                self.doing = if std::mem::take(&mut self.put_down) {
                    settling(said, answer)
                } else {
                    let _ = answer.send(said);
                    None
                };
            }
            otherwise => self.doing = otherwise,
        }
    }

    /// Notes that the world has put this player down somewhere — called by
    /// [`crate::net::receive`] on every [`protocol::ToClient::PutDown`],
    /// which is the one word that moves this client's own carrier.
    pub fn put_down(&mut self) {
        self.put_down = true;
    }
}

/// This end's half of an answer and the far end's, as one reply.
///
/// Only `help` has a half of its own; every other forwarded line has nothing
/// to add, and an empty prefix must not become a leading blank line, which
/// would end the answer where it started — see [`serve`].
fn both_halves(prefix: &str, text: &str) -> String {
    if prefix.is_empty() {
        text.to_string()
    } else {
        format!("{prefix}\n{text}")
    }
}

/// Opens the socket and starts listening on it, or says why it could not be
/// opened. Called before the app exists, so that a port already in use is a
/// plain failure to start rather than an error deep in a running game.
pub fn open(port: u16) -> Result<Control, String> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
        .map_err(|why| format!("cannot listen on port {port}: {why}"))?;
    // Said out loud for the reason a run without a seed says which world it
    // picked. `--debug 0` asks the operating system for whatever port is free,
    // which is the sane thing for a driver starting several runs at once to
    // ask, and there would otherwise be no way of learning which it got — an
    // undriveable run with nothing said about why. A named port is worth
    // confirming for the same money.
    if let Ok(at) = listener.local_addr() {
        println!("debug socket on {at}");
    }
    let (orders, waiting) = channel();
    thread::spawn(move || listen(&listener, &orders));
    Ok(Control {
        orders: Mutex::new(waiting),
        doing: None,
        target: None,
        holding: false,
        put_down: false,
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
}

impl Plugin for ControlPlugin {
    fn build(&self, app: &mut App) {
        if self.headless {
            // The one place in the game that knows a run has no window is the
            // one that says how big its pictures are — see
            // [`crate::debug::Toggles::resolution`], where `set resolution`
            // finds it afterwards. The image itself is not made here: making
            // it is what [`dress_the_target`] does every time the answer
            // changes, and doing it once here as well would be the same size
            // worked out in two places.
            app.world_mut().resource_mut::<Toggles>().resolution = Some(DEFAULT_ROWS);
            app.add_systems(Update, dress_the_target);
        }
        // After the input plugin has filled the key state, so that a key this
        // module presses is not cleared by the same frame's real input, and is
        // seen by every system that reads it in `Update`.
        app.add_systems(PreUpdate, serve_orders.after(InputSystems))
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

/// Keeps the picture a windowless run draws into agreeing with `set
/// resolution` — and points the camera at it, there being no window for it to
/// draw into instead.
///
/// Every frame and compared before writing, the way [`crate::settings`]
/// dresses a window, because the answer can change under it: the console can
/// name another rung at any point, and a camera can turn up after the image
/// did. What it compares against is the image's own size rather than a note of
/// what was last asked for — the target either is the size the console named
/// or it is not, and nothing else has to be kept true.
fn dress_the_target(
    toggles: Res<Toggles>,
    mut control: ResMut<Control>,
    mut images: ResMut<Assets<Image>>,
    mut scale: ResMut<UiScale>,
    mut cameras: Query<&mut RenderTarget, With<MapCamera>>,
) {
    // Only ever `Some` in a run with no window, which is the only kind of run
    // this system is added to.
    let Some(rows) = toggles.resolution else {
        return;
    };
    let wanted = UVec2::new(settings::width_for(settings::WIDESCREEN, rows), rows);
    let already = control
        .target
        .as_ref()
        .and_then(|target| images.get(target))
        .is_some_and(|image| image.size() == wanted);
    if !already {
        control.target = Some(images.add(offscreen(wanted)));
        // The UI is drawn into these pictures — the readout and the compass
        // are in every one — and the system that fits it to a window has no
        // window to read, so it is fitted to the picture here instead.
        *scale = UiScale(settings::fitted_to(wanted.as_vec2()));
    }

    let Some(target) = &control.target else {
        return;
    };
    for mut camera in &mut cameras {
        if camera.as_image() != Some(target) {
            *camera = target.clone().into();
        }
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
    /// Read only to know whether it has caught up with the world's clock —
    /// `hold` is the one line that waits on the light rather than the ground.
    sky: Res<'w, crate::sky::Sky>,
    cameras: Query<'w, 's, &'static mut MapCamera>,
    /// The buttons the screen is showing, read to refuse a `click` at one it
    /// is not — a press nothing is listening for would otherwise be answered
    /// as though it had done something.
    buttons: Query<'w, 's, &'static MenuButton>,
    ground: GroundArriving<'w, 's>,
}

impl Hands<'_, '_> {
    /// Puts the view where a line asked for it. Only ever the *view* — where
    /// the player is is the server's to say, and the camera follows whatever
    /// carries them of its own accord.
    fn look(&mut self, wanted: View) {
        *self.view = wanted;
        for mut camera in &mut self.cameras {
            camera.snap_to(wanted);
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
    // Copied out and written back so that serving the line can borrow the
    // rest of the resource — the handle it photographs into, and the switch
    // `hold` throws. Both halves need it: a line in hand can be a `hold` that
    // has been waiting for the sky since the frame it arrived.
    let target = control.target.clone();
    let mut holding = control.holding;

    if let Some(doing) = control.doing.take() {
        control.doing = advance(
            doing,
            &mut commands,
            &mut hands,
            &time,
            Held {
                target,
                holding: &mut holding,
            },
        );
        control.holding = holding;
        return;
    }

    let order = {
        // A poisoned lock is taken anyway. Nothing under it can be left half
        // written — it is a [`Receiver`] and the panic would have to have
        // happened inside the channel — whereas declining would make every
        // frame after the panic return here in silence, and a driver would
        // wait out the rest of the run for an answer that is no longer coming
        // with nothing said about why.
        let orders = control
            .orders
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Ok(order) = orders.try_recv() else {
            return;
        };
        order
    };

    // Cleared as the line starts, so that a put down belonging to an earlier
    // line — or to joining the world at all — cannot make this one wait.
    control.put_down = false;
    control.doing = begin(
        order,
        &mut commands,
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
fn advance(
    doing: Doing,
    commands: &mut Commands,
    hands: &mut Hands,
    time: &Time,
    held: Held,
) -> Option<Doing> {
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
        Doing::Clicking {
            stand_in,
            said,
            answer,
        } => {
            // A frame late, and that is the whole of why this is a state
            // rather than one call: a stand-in left lying about still reads
            // as *freshly changed* to any system that has not run since it
            // was spawned, and the screen a click opens is exactly that. Its
            // systems sat out every frame until now, so a `back` pressed on
            // one screen would be read a second time by the screen it
            // returned to, which went back again. Real buttons cannot do it —
            // a screen takes its own down on the way out — and this is what
            // gives the stand-in the same manners. `menu`'s own tests take
            // theirs away for the same reason.
            if let Ok(mut entity) = commands.get_entity(stand_in) {
                entity.despawn();
            }
            settling(said, answer)
        }
        Doing::Holding { waited, answer } => {
            if hands.sky.caught_up() {
                *held.holding = true;
                let _ = answer.send("the clock is held where it stands".to_string());
                return None;
            }
            if waited >= PATIENCE {
                // Nothing is held: a driver told the clock was stopped, when
                // it is running and at an hour nobody asked for, would take
                // every picture after this one on trust.
                let _ = answer.send("the sky never caught up with the world's clock".to_string());
                return None;
            }
            Some(Doing::Holding {
                waited: waited + 1,
                answer,
            })
        }
        Doing::Asking {
            prefix,
            waited,
            answer,
        } => {
            if waited >= ASK_FRAMES {
                let _ = answer.send(both_halves(&prefix, "the server said nothing back"));
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
    commands: &mut Commands,
    hands: &mut Hands,
    exit: &mut MessageWriter<AppExit>,
    held: Held,
) -> Option<Doing> {
    let Order { line, answer } = order;
    let words: Vec<&str> = line.split_whitespace().collect();
    match words.split_first() {
        Some((&"shot", _)) => match shot(after_verb(&line)) {
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
            Ok((pressing, left)) => {
                // The key the control is *bound* to rather than one of this
                // module's choosing, so a rebound control is pressed where
                // the player put it and a press goes down the same path a
                // hand would.
                let key = pressing.key(&hands.bindings);
                hands.keys.press(key);
                Some(Doing::Pressing {
                    key,
                    left,
                    said: pressed(&pressing, left),
                    answer,
                })
            }
            Err(why) => refuse(why, answer),
        },
        Some((&"zoom", rest)) => match one(rest, "zoom", "<metres>", zoom) {
            Ok(distance) => {
                let wanted = View {
                    distance,
                    ..*hands.view
                };
                hands.look(wanted);
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
                hands.look(wanted);
                settling(format!("yaw {}", bearing.to_degrees().round()), answer)
            }
            Err(why) => refuse(why, answer),
        },
        Some((&"click", rest)) => match button(rest, &hands.buttons) {
            Ok(button) => {
                // A stand-in rather than the button itself. Every menu system
                // asks for `(&Interaction, &MenuButton)` and none of them
                // cares which entity carries it, so this goes through the
                // same code a click does — and the real button cannot be used
                // for it: `ui_focus_system` sets every *node* it finds
                // pressed back to `None` the moment the mouse is not down,
                // which in a run with no mouse is always. A bare entity is no
                // node, so nothing takes the press away but us.
                let stand_in = commands.spawn((button, Interaction::Pressed)).id();
                Some(Doing::Clicking {
                    stand_in,
                    said: format!("{} clicked", rest.join(" ")),
                    answer,
                })
            }
            Err(why) => refuse(why, answer),
        },
        // Letting go is done the moment it is said — there is nothing to wait
        // for in handing the hour back to the world. Taking hold is not.
        Some((&"hold", rest)) => match onoff(rest, "hold") {
            // No world here, and no word from one ever: there is no clock to
            // hold, and waiting for an hour that is never coming would stand
            // a scripted run still for the whole of `PATIENCE` before saying
            // so. A session that has simply not been told the hour *yet* is
            // the other case, and that one waits — the word is on its way.
            Ok(true) if hands.online.is_none() && !hands.sky.heard_the_hour() => refuse(
                "there is no world here whose clock could be held".to_string(),
                answer,
            ),
            Ok(true) => Some(Doing::Holding { waited: 0, answer }),
            Ok(false) => {
                *held.holding = false;
                let _ = answer.send("the clock runs again".to_string());
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
///
/// With no server there is still this end's half to give. It matters for one
/// line in particular: `help` down a socket that has not joined a world is
/// exactly when a driver most needs the words this module adds, and answering
/// only that nobody is serving would throw them away.
fn forward(line: &str, prefix: String, hands: &Hands, answer: SyncSender<String>) -> Option<Doing> {
    let Some(online) = &hands.online else {
        let _ = answer.send(both_halves(&prefix, "nobody is serving this world"));
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

/// The button a `click` names, if the screen is showing one.
///
/// Two refusals, and the second is the point: a name nothing answers to is a
/// typo, and a button the screen has not got is a line that would otherwise
/// be answered as though it had done something. Nothing listens for a `start`
/// on the main menu, so a press of one is silence — and silence answered
/// "start clicked" is the kind of lie a driver builds a whole script on.
fn button(words: &[&str], on_screen: &Query<&MenuButton>) -> Result<MenuButton, String> {
    let Some(wanted) = MenuButton::parse(words) else {
        return Err(format!(
            "`click` wants a button — `click new-world`\nbuttons: {}\n{}",
            MenuButton::names().join(", "),
            MenuButton::WANTS
        ));
    };
    if !on_screen.iter().any(|button| *button == wanted) {
        return Err(format!("there is no `{}` on this screen", words.join(" ")));
    }
    Ok(wanted)
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

/// Everything after the first word of a line, trimmed at both ends and
/// otherwise left exactly as it was typed. Only `shot` reads a line this way,
/// and only because a path is not a word: `shot /My Pictures/near.png` names
/// one file, and splitting it would refuse a perfectly good path for having a
/// space in a directory name somebody else chose.
fn after_verb(line: &str) -> &str {
    line.trim()
        .split_once(char::is_whitespace)
        .map_or("", |(_, rest)| rest.trim())
}

/// `shot <path>`: where the picture goes.
///
/// The file is removed first, so that its appearing is the proof this picture
/// was written and not the last one that happened to have the name.
///
/// The directory it goes in has to exist already; nothing here makes one. That
/// is checked now rather than left to fail later because a picture is not
/// written by this module — it is asked for, and the failure comes back as the
/// file never appearing, which is indistinguishable from a slow one. A typo in
/// a path would spend the whole of [`PATIENCE`] before saying so, when what it
/// wants is to be refused on the line that carried it.
fn shot(path: &str) -> Result<PathBuf, String> {
    if path.is_empty() {
        return Err("`shot` wants one path — `shot near.png`".to_string());
    }
    let path = PathBuf::from(path);
    // A bare name has a parent of `""`, which is this directory and always
    // there — only a path that names one is worth looking for.
    if let Some(parent) = path.parent().filter(|it| !it.as_os_str().is_empty()) {
        if !parent.is_dir() {
            return Err(format!(
                "cannot write {}: there is no directory {}",
                path.display(),
                parent.display()
            ));
        }
    }
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
fn press(args: &[&str]) -> Result<(Pressing, f32), String> {
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
    if let Some(key) = bindings::reserved_key(named) {
        return Ok((Pressing::Reserved(key, named.to_string()), seconds));
    }
    let Some(action) = Action::ALL.into_iter().find(|it| it.name() == named) else {
        return Err(format!("no control called `{named}`\n{}", actions()));
    };
    Ok((Pressing::Control(action), seconds))
}

/// What a `press` line named.
///
/// Two kinds because there are two: a control, which is pressed at whatever
/// key it is *bound* to so that a rebound one is found where the player put
/// it, and a key that is nobody's control and so has nothing to be bound to —
/// see [`crate::bindings::reserved_key`].
#[derive(Clone, PartialEq, Debug)]
enum Pressing {
    Control(Action),
    Reserved(KeyCode, String),
}

impl Pressing {
    /// The key to hold down. The bindings are asked for a control and not for
    /// a reserved key, which is the whole difference between them.
    fn key(&self, bindings: &KeyBindings) -> KeyCode {
        match self {
            Self::Control(action) => bindings.key(*action),
            Self::Reserved(key, _) => *key,
        }
    }

    /// What it is called in an answer — the word that was typed.
    fn name(&self) -> &str {
        match self {
            Self::Control(action) => action.name(),
            Self::Reserved(_, named) => named,
        }
    }
}

/// What a press answers with when it is over. A tap and a hold are told apart
/// because a driver that meant to sail and forgot the seconds gets a boat
/// that has not moved, and the answer is the only place that shows.
fn pressed(pressing: &Pressing, seconds: f32) -> String {
    if seconds > 0.0 {
        format!("{} held {seconds} seconds", pressing.name())
    } else {
        format!("{} tapped", pressing.name())
    }
}

/// The controls a `press` will take, as one line — read off [`Action::ALL`]
/// so a control added to the game is offered here without being listed twice,
/// with the one key that is nobody's control named after them.
fn actions() -> String {
    let names: Vec<&str> = Action::ALL.iter().map(|it| it.name()).collect();
    format!("controls: {}, escape", names.join(", "))
}

#[cfg(test)]
mod tests {
    use bevy::time::{TimePlugin, TimeUpdateStrategy};

    use super::*;
    use crate::testing::{run_frames, run_until, FRAME};

    /// The socket's own end of an order channel, and a headless app with the
    /// system that serves it.
    ///
    /// Built by hand rather than by adding [`ControlPlugin`]: what is under
    /// test is the state machine, and the plugin's other half points a camera
    /// at an off-screen image, which wants a render app there is none of here.
    /// The channel is the real one — a driver's line reaches [`serve_orders`]
    /// exactly this way, and its answer comes back down the same rendezvous
    /// [`serve`] blocks on, so a test reads what a driver would read.
    fn driven_app() -> (App, Sender<Order>) {
        let (orders, waiting) = channel();
        let mut app = App::new();
        app.add_plugins(TimePlugin)
            .insert_resource(TimeUpdateStrategy::ManualDuration(FRAME))
            .insert_resource(Control {
                orders: Mutex::new(waiting),
                doing: None,
                target: None,
                holding: false,
                put_down: false,
            })
            .init_resource::<Toggles>()
            // The hour, which `hold` waits on. A default one has heard
            // nothing from a server yet, which is exactly the state a run is
            // in for its first frames.
            .init_resource::<crate::sky::Sky>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<KeyBindings>()
            .init_resource::<View>()
            .add_systems(PreUpdate, serve_orders);
        (app, orders)
    }

    /// Sends a line the way the listening thread does, and hands back the end
    /// a driver reads its answer off. Nothing waits on it: whether a line has
    /// been answered *yet* is most of what these tests are asking.
    fn say(orders: &Sender<Order>, line: &str) -> Receiver<String> {
        let (answer, answered) = sync_channel(1);
        orders
            .send(Order {
                line: line.to_string(),
                answer,
            })
            .expect("the app is holding the other end");
        answered
    }

    fn keys(app: &App) -> &ButtonInput<KeyCode> {
        app.world().resource::<ButtonInput<KeyCode>>()
    }

    /// Clears the just-pressed flags the way the input plugin does at the top
    /// of every frame, so a key left down reads as held rather than as pressed
    /// afresh. Called between frames rather than before them only so that a
    /// test can look at the edge the frame just made.
    fn between_frames(app: &mut App) {
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .clear();
    }

    /// The bug this waiting exists for: `time` moves the world's clock and
    /// the sky *eases* onto the new hour, so a hold thrown on the frame the
    /// `time` was answered used to pin the hour the light was leaving — and
    /// pin it for good, since nothing revisits a hold once it has latched. A
    /// driver taking two comparable pictures of one place got both of them at
    /// an hour they had explicitly moved off.
    #[test]
    fn a_hold_waits_for_the_sky_to_reach_the_hour_before_freezing_it() {
        let (mut app, orders) = driven_app();
        // The real pair: the ease that closes on the server's hour, and the
        // system that freezes what it arrives at.
        app.add_systems(Update, (crate::sky::advance_the_day, hold_the_sky));

        // A world at one hour, and a server that has just named a very
        // different one — the shape `time 23:00` leaves behind.
        let mut sky = app.world_mut().resource_mut::<crate::sky::Sky>();
        sky.told(0.35);
        sky.told(0.95);
        assert!(
            !sky.caught_up(),
            "the sky arrived at the new hour without crossing to it"
        );

        let answered = say(&orders, "hold on");
        app.update();
        assert!(
            answered.try_recv().is_err(),
            "answered while the sky was still on its way"
        );
        assert!(
            !app.world().resource::<Control>().holding,
            "froze the hour the sky was leaving"
        );

        // Let it get there. The ease is exponential, so this is a few
        // seconds of frames rather than one.
        run_until(&mut app, "the sky has caught up", |app| {
            app.world().resource::<crate::sky::Sky>().caught_up()
        });
        app.update();

        assert_eq!(
            answered.try_recv(),
            Ok("the clock is held where it stands".to_string())
        );
        assert!(app.world().resource::<Control>().holding);

        // And what it froze is the hour that was asked for, not the one it
        // started from.
        let held = app.world().resource::<crate::sky::Sky>().phase();
        assert!(
            (held - 0.95)
                .rem_euclid(1.0)
                .min((0.95 - held).rem_euclid(1.0))
                < 0.01,
            "held {held} rather than the hour the world had run on to"
        );
    }

    /// Letting go has nothing to wait for, and says so at once. From a run
    /// that is actually holding, so that the switch being thrown is something
    /// this can fail on rather than the state it started in.
    #[test]
    fn letting_go_of_the_clock_is_answered_on_the_spot() {
        let (mut app, orders) = driven_app();
        app.world_mut().resource_mut::<Control>().holding = true;

        let answered = say(&orders, "hold off");
        app.update();
        assert_eq!(answered.try_recv(), Ok("the clock runs again".to_string()));
        assert!(!app.world().resource::<Control>().holding, "still holding");
    }

    /// And a run with no world to hold the clock of is told so on the line
    /// that asked, rather than after half a minute of waiting for a word that
    /// was never coming — `--debug` with `--state mainmenu` is a run like
    /// that, and a driver taking pictures of the menus is the one who would
    /// pay for it.
    #[test]
    fn holding_the_clock_of_no_world_is_refused_at_once() {
        let (mut app, orders) = driven_app();
        let answered = say(&orders, "hold on");
        app.update();

        let said = answered.try_recv().expect("answered on the spot");
        assert!(said.contains("no world"), "unhelpful: {said}");
        assert!(!app.world().resource::<Control>().holding);
    }

    /// `press escape` is the way to the pause menu, and there is no other:
    /// Escape is reserved from rebinding, so it is bound to no [`Action`] and
    /// nothing in [`Action::ALL`] can find it. Without this the pause menu
    /// and the three screens under it are reachable by a hand at a keyboard
    /// and by nothing else — `click` cannot open a screen no button opens.
    #[test]
    fn escape_is_pressable_though_it_is_nobodys_control() {
        assert_eq!(
            press(&["escape"]),
            Ok((
                Pressing::Reserved(KeyCode::Escape, "escape".to_string()),
                0.0
            ))
        );
        assert_eq!(
            press(&["escape", "0.5"]).map(|(_, seconds)| seconds),
            Ok(0.5)
        );

        // Named in what a missed control lists, so a driver finds it without
        // having to know it is a special case.
        assert!(actions().contains("escape"), "{}", actions());

        // And the other reserved keys are not offered: the arrows are a
        // second set of the movement controls and `press forward` says those
        // already, and the backquote opens the console this line came down.
        for key in ["arrowup", "up", "backquote", "`"] {
            assert!(press(&[key]).is_err(), "`{key}` should not be a control");
        }
    }

    /// And it goes down as the real key, so whatever reads Escape sees it —
    /// the pause menu's own `helm_keys` among them.
    #[test]
    fn a_pressed_escape_is_the_escape_key_itself() {
        let (mut app, orders) = driven_app();
        let answered = say(&orders, "press escape");

        app.update();
        assert!(
            keys(&app).just_pressed(KeyCode::Escape),
            "escape never went down"
        );
        between_frames(&mut app);

        app.update();
        assert!(
            !keys(&app).pressed(KeyCode::Escape),
            "escape was never let go"
        );
        assert_eq!(answered.try_recv(), Ok("escape tapped".to_string()));
    }

    /// A click presses a button the screen is showing, and is answered once
    /// what it opened has settled.
    #[test]
    fn a_click_presses_a_button_the_screen_is_showing() {
        let (mut app, orders) = driven_app();
        // A screen with one button on it, as a spawned menu leaves things.
        app.world_mut()
            .spawn((MenuButton::Options, Interaction::None));
        let answered = say(&orders, "click options");

        app.update();
        let pressed: Vec<MenuButton> = app
            .world_mut()
            .query::<(&MenuButton, &Interaction)>()
            .iter(app.world())
            .filter(|(_, interaction)| **interaction == Interaction::Pressed)
            .map(|(button, _)| *button)
            .collect();
        assert_eq!(
            pressed,
            vec![MenuButton::Options],
            "the press never reached a button the menus would read"
        );

        run_until(&mut app, "the click is answered", |app| {
            app.world().resource::<Control>().doing.is_none()
        });
        assert_eq!(answered.try_recv(), Ok("options clicked".to_string()));
    }

    /// And the stand-in carrying that press is taken away again before the
    /// screen it opened can read it. A button left lying about reads as
    /// freshly changed to systems that have not run since it was spawned —
    /// which is every system of the arriving screen — so a `back` would be
    /// read twice and go back twice. See [`advance`]'s `Clicking`.
    #[test]
    fn the_press_is_taken_away_before_the_next_screen_could_read_it() {
        let (mut app, orders) = driven_app();
        app.world_mut().spawn((MenuButton::Back, Interaction::None));
        let _answered = say(&orders, "click back");

        app.update();
        assert_eq!(
            app.world_mut()
                .query::<&MenuButton>()
                .iter(app.world())
                .count(),
            2,
            "the stand-in never appeared"
        );

        app.update();
        assert_eq!(
            app.world_mut()
                .query::<&MenuButton>()
                .iter(app.world())
                .count(),
            1,
            "the stand-in outlived the frame its press was read on"
        );
    }

    /// A button the screen has not got is refused rather than pressed into
    /// the void. Nothing listens for a `start` on a screen that has no Start,
    /// so answering "start clicked" would be a lie a script is built on.
    #[test]
    fn a_click_at_a_button_that_is_not_there_is_refused() {
        let (mut app, orders) = driven_app();
        app.world_mut()
            .spawn((MenuButton::Options, Interaction::None));

        let answered = say(&orders, "click start");
        app.update();
        let why = answered.try_recv().expect("answered on the spot");
        assert!(
            why.contains("no `start` on this screen"),
            "unhelpful: {why}"
        );

        // And a name nothing answers to lists what would.
        let answered = say(&orders, "click sideways");
        app.update();
        let why = answered.try_recv().expect("answered on the spot");
        assert!(why.contains("new-world"), "unhelpful: {why}");
    }

    /// Every button `click` offers can be clicked, and lands on the button it
    /// named. The listing is an index rather than the grammar — `parse` never
    /// reads it — so this is what holds the two to agreement, the way
    /// `server::console`'s `VERBS` is held to `interpret`.
    ///
    /// The four that carry something are given one here. A button that grew
    /// an argument and did not say so would show up as its bare name failing
    /// to parse.
    #[test]
    fn every_button_offered_is_a_button_that_can_be_clicked() {
        for name in MenuButton::names() {
            let line: Vec<&str> = match name {
                "open-kept" | "ask-discard" | "discard" => vec![name, "0"],
                "rebind" => vec![name, "forward"],
                "resolution" => vec![name, "native"],
                _ => vec![name],
            };
            let button = MenuButton::parse(&line)
                .unwrap_or_else(|| panic!("`click {}` is offered and refused", line.join(" ")));
            assert_eq!(
                button.name(),
                name,
                "`click {}` landed on another button",
                line.join(" ")
            );
        }
    }

    /// The buttons that carry something take it as a second word — a row
    /// number, a control, a rung of the display ladder.
    #[test]
    fn the_buttons_that_carry_something_read_it_off_the_line() {
        assert_eq!(
            MenuButton::parse(&["open-kept", "2"]),
            Some(MenuButton::OpenKept(2))
        );
        assert_eq!(
            MenuButton::parse(&["rebind", "chart"]),
            Some(MenuButton::Rebind(Action::Chart))
        );
        assert_eq!(
            MenuButton::parse(&["resolution", "1440"]),
            Some(MenuButton::PickResolution(
                crate::settings::Resolution::Rows(1440)
            ))
        );
        assert_eq!(
            MenuButton::parse(&["resolution", "native"]),
            Some(MenuButton::PickResolution(
                crate::settings::Resolution::Native
            ))
        );

        // And what none of them is.
        assert_eq!(MenuButton::parse(&["open-kept"]), None, "wants a row");
        assert_eq!(MenuButton::parse(&["open-kept", "last"]), None);
        assert_eq!(MenuButton::parse(&["rebind", "sideways"]), None);
        assert_eq!(MenuButton::parse(&["resolution", "1441"]), None, "no rung");
        // Read as rows and not as the label the button wears, so that
        // changing what the screen prints cannot quietly stop this parsing.
        assert_eq!(
            MenuButton::parse(&["resolution", "1080p"]),
            None,
            "the label is not the grammar"
        );
        assert_eq!(MenuButton::parse(&[]), None);
    }

    /// A tap is one frame of the key being down, which is the whole of what a
    /// control reading an edge ever sees. Nothing else in the game presses a
    /// key, so if this drifted the only sign would be `press chart` quietly
    /// doing nothing.
    #[test]
    fn a_tap_is_down_for_exactly_one_frame() {
        let (mut app, orders) = driven_app();
        let key = app.world().resource::<KeyBindings>().key(Action::Chart);
        let answered = say(&orders, "press chart");

        app.update();
        assert!(keys(&app).just_pressed(key), "the tap never went down");
        assert!(
            answered.try_recv().is_err(),
            "answered before the press was over"
        );
        between_frames(&mut app);

        app.update();
        assert!(!keys(&app).pressed(key), "the tap was never let go");
        assert_eq!(answered.try_recv(), Ok("chart tapped".to_string()));
    }

    /// A timed press stays down across frames and comes up when the time is
    /// spent — and the answer is what says so, which is how a script of
    /// presses reads as one move after another rather than as a race.
    #[test]
    fn a_timed_press_is_held_until_its_time_is_spent() {
        let (mut app, orders) = driven_app();
        let key = app
            .world()
            .resource::<KeyBindings>()
            .key(Action::MoveForward);
        let answered = say(&orders, "press forward 0.32");

        // Well inside the time asked for, so what this catches is the hold
        // itself and not the frame it happened to end on.
        for _ in 0..10 {
            app.update();
            assert!(keys(&app).pressed(key), "the press was let go early");
            assert!(answered.try_recv().is_err(), "answered mid-press");
            between_frames(&mut app);
        }

        run_until(&mut app, "the press is over", |app| {
            between_frames(app);
            !app.world().resource::<ButtonInput<KeyCode>>().pressed(key)
        });
        assert_eq!(
            answered.try_recv(),
            Ok("forward held 0.32 seconds".to_string())
        );
    }

    /// A line in flight, as [`forward`] leaves one, and the way to answer it.
    fn asking(app: &mut App) -> Receiver<String> {
        let (answer, answered) = sync_channel(1);
        app.world_mut().resource_mut::<Control>().doing = Some(Doing::Asking {
            prefix: String::new(),
            waited: 0,
            answer,
        });
        answered
    }

    /// A line that moved the player is answered when the world has caught up
    /// with where they now are, and not before. This is the load-bearing
    /// claim of the module, and it fails silently: a line answered early
    /// still answers, only with a world under it that has not finished
    /// arriving.
    ///
    /// It is the *put down* that says a line moved somebody, not the line —
    /// the server's grammar is not read on this side, so `goto` is nothing
    /// here but a line that happened to be followed by one.
    #[test]
    fn a_line_that_moved_the_player_waits_for_the_ground() {
        let (mut app, _orders) = driven_app();
        // Ground delivered and not yet handed to a mesh builder, which is
        // exactly what [`Ground::settled`] is false for.
        app.insert_resource(crate::testing::test_ground());
        let answered = asking(&mut app);

        {
            // The order `goto` puts them on the wire: the world moves the
            // player, and only then does the server say what it did.
            let mut control = app.world_mut().resource_mut::<Control>();
            control.put_down();
            control.answered("you are at 98 -317");
        }

        run_frames(&mut app, SETTLE_FRAMES as usize * 3);
        assert!(
            answered.try_recv().is_err(),
            "answered with the ground still arriving"
        );
        assert!(
            matches!(
                app.world().resource::<Control>().doing,
                Some(Doing::Settling { waited: 0, .. })
            ),
            "the settling ran while the ground was still coming, so the frames \
             it counted were not settling frames"
        );

        // The last of it lands, and only now do the settling frames count.
        app.insert_resource(Ground::default());
        run_until(&mut app, "the moved line is answered", |app| {
            app.world().resource::<Control>().doing.is_none()
        });
        assert_eq!(answered.try_recv(), Ok("you are at 98 -317".to_string()));
    }

    /// And a line that moved nobody is not made to wait for ground it has no
    /// reason to want — most lines are that, and a `weather gale` that waited
    /// on the terrain would be a toll on every one of them.
    #[test]
    fn a_line_that_moved_nobody_is_answered_as_soon_as_the_server_speaks() {
        let (mut app, _orders) = driven_app();
        app.insert_resource(crate::testing::test_ground());
        let answered = asking(&mut app);

        app.world_mut()
            .resource_mut::<Control>()
            .answered("the wind is ordered gale");

        assert_eq!(
            answered.try_recv(),
            Ok("the wind is ordered gale".to_string())
        );
        assert!(app.world().resource::<Control>().doing.is_none());
    }

    /// `help` down a socket that has not joined a world still lists the words
    /// this module adds. That is when a driver most needs them — a run held on
    /// a menu screen has nothing else to ask — and they are this end's to give
    /// whether or not anything is serving.
    #[test]
    fn help_gives_this_end_of_it_with_no_server_to_ask() {
        let (mut app, orders) = driven_app();
        let answered = say(&orders, "help");

        app.update();
        let reply = answered.try_recv().expect("nothing to wait for");
        assert!(reply.contains("shot <path>"), "{reply}");
        assert!(reply.contains("press <control>"), "{reply}");
        assert!(reply.contains("quit —"), "{reply}");
        assert!(reply.contains("nobody is serving this world"), "{reply}");
    }

    /// The grammar of a press, which is the one line here with anything much
    /// to get wrong: most of what this module takes it hands straight to the
    /// console's own [`dispatch`].
    #[test]
    fn press_reads_a_control_and_a_time() {
        assert_eq!(
            press(&["forward", "20"]),
            Ok((Pressing::Control(Action::MoveForward), 20.0))
        );
        assert_eq!(
            press(&["claim"]),
            Ok((Pressing::Control(Action::Claim), 0.0))
        );
        assert_eq!(
            press(&["view-left", "0.5"]),
            Ok((Pressing::Control(Action::TurnLeft), 0.5))
        );
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
            pressed(&Pressing::Control(Action::MoveForward), 20.0),
            "forward held 20 seconds"
        );
        assert_eq!(
            pressed(&Pressing::Control(Action::Chart), 0.0),
            "chart tapped"
        );
    }

    /// The view verbs, which took the place of `--zoom` and `--yaw` and have
    /// to refuse what those refused.
    #[test]
    fn the_view_verbs_read_what_the_options_used_to() {
        assert_eq!(zoom("120"), Ok(120.0));
        assert_eq!(yaw("90"), Ok(std::f32::consts::FRAC_PI_2));

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

    /// A path is whatever is left of the line, and a directory that is not
    /// there is refused on the spot. Both are about the same thing: a driver
    /// down a socket has no other way of being told it typed the path wrong,
    /// and the alternative is [`PATIENCE`] frames of waiting followed by
    /// "nothing was written", which reads like a broken game.
    #[test]
    fn a_shot_takes_the_whole_of_the_rest_of_the_line() {
        assert_eq!(
            after_verb("shot /My Pictures/near.png"),
            "/My Pictures/near.png"
        );
        assert_eq!(after_verb("shot"), "", "a verb on its own leaves nothing");

        let dir = std::env::temp_dir().join(format!("genovesa shots {}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("somewhere to pretend to write");
        let spaced = dir.join("near shore.png");
        assert_eq!(
            shot(spaced.to_str().expect("a path this test just made")),
            Ok(spaced.clone()),
            "a space in a directory name is not a second argument"
        );

        let missing = dir.join("nowhere").join("near.png");
        let why = shot(missing.to_str().expect("likewise")).expect_err("no such directory");
        assert!(why.contains("there is no directory"), "{why}");
        assert!(shot("").is_err(), "`shot` alone names no file");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_switch_is_on_or_off() {
        assert_eq!(onoff(&["on"], "hold"), Ok(true));
        assert_eq!(onoff(&["off"], "hold"), Ok(false));
        let why = onoff(&["maybe"], "hold").expect_err("not a switch");
        assert!(why.contains("`hold` is on or off"), "{why}");
    }
}
