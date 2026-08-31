//! The debug socket: the console, on a port instead of a keyboard.
//!
//! `--debug <port>` puts a listener on the loopback that takes the same lines
//! the console takes, one per line of the connection, and writes back what
//! they answer. It is the whole of how this game is debugged from outside
//! itself, and it replaced a second vocabulary on the command line: an option
//! can only say what a run should do *before* it starts, where a socket says
//! it at any point and as often as it likes.
//!
//! Nothing here invents a grammar. A word this end has no [`Verb`] for goes
//! to [`crate::console::dispatch`] exactly as a typed one would — so `client
//! haze off` doctors this client's picture and `world weather gale` crosses
//! the wire to the server, and a verb added to either side is reachable from
//! here the day it is added. What this module adds is what a *keyboard* never
//! needed words for: `shot`, `press`, `click`, `hold` and `quit`. The view
//! used to be here too, as `zoom` and `yaw`; it is `client zoom` and `client
//! yaw` now, which is where a variable of this machine belongs — the socket's
//! part in it is the waiting, not the word.
//!
//! Those are [`VERBS`], a row apiece — the word, what `help` says of it, and
//! one function from the words after it to what should be begun. The three
//! vocabularies in this game are all shaped that way now, and for the same
//! reason: what `help` says is a fold over the table rather than a listing
//! beside it, so it cannot come to advertise a word nothing serves.
//!
//! Two of those are less obvious than they look. `press` works the bound key
//! through the real bindings rather than commanding the world, because
//! sailing happens at the client — [`protocol::ToServer`] carries `Move` and
//! `Helm`, a client *telling* a server where it got to, so no amount of
//! ordering the world about from the server's side makes a boat sail. And
//! `click` is not a shortcut chosen over a mouse: a windowless run has no
//! cursor for Bevy's picking to work from, so naming the button is the only
//! door to the menus there is.
//!
//! **A line is answered when its work is done, and not before.** That is the
//! load-bearing property. `press forward 20` answers twenty seconds later; a
//! `goto` or a `client zoom` answers once the ground has arrived and the
//! picture has stopped moving; `shot` answers when the file is on disk. So a pipe of
//! lines is a script rather than a race, and the settling that used to happen
//! silently in frames nobody could see is now the thing an answer means.
//!
//! Answers are line-based and end with a blank line, which is what makes the
//! socket usable from a shell with nothing in between:
//!
//! ```sh
//! printf 'world weather gale\npress forward 20\nshot gale.png\nquit\n' | nc 127.0.0.1 7777
//! ```
//!
//! A blank line inside an answer would end it early, so blank lines are
//! dropped from answers rather than escaped. One connection is served through
//! to its end before another is accepted, there being one of everything two
//! drivers would both be moving.
//!
//! It is loopback TCP rather than a Unix socket to keep `#[cfg(unix)]` out of
//! a module the game has to build without. Worth saying plainly: `shot <path>`
//! unlinks the file it is about to write (see [`shot`]), so whatever can reach
//! the port can delete any file this run's user can. It is a development
//! instrument and is off unless asked for.
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
use bevy::render::view::screenshot::Screenshot;

use crate::bindings::{self, Action, KeyBindings};
use crate::camera::MapCamera;
use crate::console::{dispatch, Dispatch};
use crate::debug::{Machine, Toggles};
use crate::menu::MenuButton;
use crate::net::Online;
use crate::settings;
use crate::shots::save_stamped;
use crate::terrain::{ChunkBuild, Ground};

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

/// A line that cannot be answered on the frame it arrived, and the way back
/// to whoever sent it. The channel is here rather than in every kind of work
/// because every kind of work ends the same way: something to say, said once.
struct Doing {
    answer: SyncSender<String>,
    work: Work,
}

/// What a line is waiting for.
///
/// Two counters run through most of these and they are not the same thing.
/// `waited` is frames since the thing being waited *for* last moved, held at
/// zero while ground is still arriving — see [`Hands::waited`] — and `age` is
/// frames since the line was taken, which is what runs out against
/// [`PATIENCE`].
enum Work {
    /// A key is down. Released when the time runs out, and answered then —
    /// the answer is what says the press is over.
    Pressing {
        key: KeyCode,
        left: f32,
        said: String,
    },
    /// Waiting for the world to catch up with something that moved it.
    ///
    /// `stand_in` is a menu button's press, taken away on the first frame
    /// here; every other line arrives with none. See [`begin`]'s `click` for
    /// why it cannot be left lying about.
    Settling {
        waited: u32,
        age: u32,
        said: String,
        stand_in: Option<Entity>,
    },
    /// A picture is on its way, somewhere in [`Stage`].
    Shooting {
        path: PathBuf,
        target: Option<Handle<Image>>,
        stage: Stage,
        waited: u32,
        age: u32,
    },
    /// `hold on`, waiting for the drawn hour to catch up with the world's
    /// before it freezes anything — see [`crate::sky::Sky::caught_up`].
    Holding { age: u32 },
    /// A line has gone to the server; waiting for what it says back. The
    /// prefix is this end's own half of the answer, which `help` has and
    /// nothing else does.
    Asking { prefix: String, age: u32 },
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
    /// [`take_the_answer`] on every [`crate::net::ServerReplied`], which may
    /// well be answering a line somebody typed at the console instead — so a
    /// reply arriving while nothing is waiting is dropped here and printed
    /// there, rather than being anybody's answer.
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
            Some(Doing {
                answer,
                work: Work::Asking { prefix, .. },
            }) => {
                let said = both_halves(&prefix, text);
                // A line that moved the player is not finished when the
                // server says it is: the ground where they now are has still
                // to arrive, and a `shot` on the next line would photograph
                // whatever had turned up. So such a line becomes a settle,
                // exactly as the view verbs are, and is answered when the
                // picture has stopped changing.
                self.doing = if std::mem::take(&mut self.put_down) {
                    Some(Doing {
                        answer,
                        work: settling(said),
                    })
                } else {
                    let _ = answer.send(said);
                    None
                };
            }
            otherwise => self.doing = otherwise,
        }
    }

    /// Notes that the world has put this player down somewhere — called by
    /// [`note_the_put_down`] on every [`crate::net::PutDown`], which is the
    /// one word that moves this client's own carrier.
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
            // [`crate::debug::Toggles::resolution`], where `client resolution`
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
        app.add_message::<crate::net::ServerReplied>()
            .add_message::<crate::net::PutDown>()
            .add_systems(PreUpdate, serve_orders.after(InputSystems))
            .add_systems(Update, hold_the_sky)
            // Chained, and that order is the wire's rather than a preference:
            // a `goto` is answered with the put down first and the reply
            // after — see the server's own console — and
            // [`Control::answered`] reads what [`Control::put_down`] set. Read
            // the other way round, a line that moved the player would answer
            // before the ground at the far end had arrived, which is the whole
            // thing a settle exists to wait for.
            .add_systems(
                Update,
                (note_the_put_down, take_the_answer)
                    .chain()
                    .in_set(crate::net::Wire::Read),
            );
    }
}

/// Notes a world that has moved this player, for the line that asked it to.
///
/// The same word [`crate::player`] acts on; what a driver wants from it is
/// only that it happened, the ground at the far end being what it is really
/// waiting for.
fn note_the_put_down(mut control: ResMut<Control>, mut moved: MessageReader<crate::net::PutDown>) {
    for _ in moved.read() {
        control.put_down();
    }
}

/// Hands the server's replies to whoever down the socket is waiting for one.
/// The same word the console prints — see [`crate::console`].
fn take_the_answer(
    mut control: ResMut<Control>,
    mut replies: MessageReader<crate::net::ServerReplied>,
) {
    for reply in replies.read() {
        control.answered(&reply.text);
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
    } else {
        // And lets it go the frame the hold is lifted. The hold is the only
        // thing that commands an hour, so releasing here is what makes
        // `hold off` mean what it says — the sky used to stay pinned at the
        // first held hour for the rest of the run.
        sky.release();
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
    /// The switches and the view a `client` line reaches, which this module
    /// only passes along — see [`crate::console::dispatch`].
    machine: Machine<'w, 's>,
    keys: ResMut<'w, ButtonInput<KeyCode>>,
    bindings: Res<'w, KeyBindings>,
    /// Absent until a world is joined, and on the menu screens.
    online: Option<Res<'w, Online>>,
    /// Read only to know whether it has caught up with the world's clock —
    /// `hold` is the one line that waits on the light rather than the ground.
    sky: Res<'w, crate::sky::Sky>,
    /// The buttons the screen is showing, read to refuse a `click` at one it
    /// is not — a press nothing is listening for would otherwise be answered
    /// as though it had done something.
    buttons: Query<'w, 's, &'static MenuButton>,
    ground: GroundArriving<'w, 's>,
}

impl Hands<'_, '_> {
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
    // rest of the resource. A line in hand can be a `hold` that has been
    // waiting for the sky since the frame it arrived, so both paths below
    // write it.
    let mut holding = control.holding;

    if let Some(mut doing) = control.doing.take() {
        // Answered here rather than in each kind of work, every one of them
        // ending the same way: something to say, said once.
        match doing.advance(&mut commands, &mut hands, &time, &mut holding) {
            Some(said) => {
                let _ = doing.answer.send(said);
            }
            None => control.doing = Some(doing),
        }
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
            // Only a starting line photographs anything; one already in hand
            // carries the handle it was given.
            target: control.target.clone(),
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
impl Doing {
    /// One frame of the work. `Some` is what to answer with, and the end of
    /// it; `None` is another frame of waiting.
    fn advance(
        &mut self,
        commands: &mut Commands,
        hands: &mut Hands,
        time: &Time,
        holding: &mut bool,
    ) -> Option<String> {
        match &mut self.work {
            Work::Pressing { key, left, said } => {
                *left -= time.delta_secs();
                if *left > 0.0 {
                    return None;
                }
                // Released rather than left down: the next line starts from a
                // world with nothing held, which is what makes a script of
                // presses read as a sequence of separate moves.
                hands.keys.release(*key);
                Some(std::mem::take(said))
            }
            Work::Settling {
                waited,
                age,
                said,
                stand_in,
            } => {
                // A frame late, and that is the whole of why a click waits
                // here rather than answering outright: a stand-in left lying
                // about still reads as *freshly changed* to any system that
                // has not run since it was spawned, and the screen a click
                // opens is exactly that. Left there, a `back` would be read a
                // second time by the screen it returned to, which went back
                // again. Real buttons cannot do it — a screen takes its own
                // down on the way out — and this gives the stand-in the same
                // manners.
                if let Some(entity) = stand_in.take() {
                    if let Ok(mut entity) = commands.get_entity(entity) {
                        entity.despawn();
                    }
                }
                if hands.settled(*waited) {
                    return Some(std::mem::take(said));
                }
                if *age >= PATIENCE {
                    return Some(format!("{said}, with ground still arriving"));
                }
                *waited = hands.waited(*waited);
                *age += 1;
                None
            }
            Work::Shooting {
                path,
                target,
                stage,
                waited,
                age,
            } => {
                if *age >= PATIENCE {
                    return Some(match stage {
                        Stage::Arriving => {
                            format!("the ground never finished arriving for {}", path.display())
                        }
                        _ => format!("nothing was written to {}", path.display()),
                    });
                }
                (*stage, *waited) = match stage {
                    Stage::Arriving if hands.settled(*waited) => {
                        // A window is photographed as a window; a run without
                        // one is photographed off the image its camera draws
                        // into.
                        let mut asked = match &target {
                            Some(target) => commands.spawn(Screenshot::image(target.clone())),
                            None => commands.spawn(Screenshot::primary_window()),
                        };
                        // Read here rather than when the line arrived: a
                        // `shot` waits for the ground, and the view it waited
                        // out is the one the picture is of.
                        asked.observe(save_stamped(path.clone(), hands.machine.stamp()));
                        (Stage::Asked, 0)
                    }
                    Stage::Arriving => (Stage::Arriving, hands.waited(*waited)),
                    Stage::Asked if path.exists() => (Stage::Written, 0),
                    Stage::Asked => (Stage::Asked, *waited + 1),
                    Stage::Written if *waited >= SHOT_SETTLE => {
                        return Some(format!("shot {}", path.display()))
                    }
                    Stage::Written => (Stage::Written, *waited + 1),
                };
                *age += 1;
                None
            }
            Work::Holding { age } => {
                if hands.sky.caught_up() {
                    *holding = true;
                    return Some("the clock is held where it stands".to_string());
                }
                if *age >= PATIENCE {
                    // Nothing is held: a driver told the clock was stopped,
                    // when it is running and at an hour nobody asked for,
                    // would take every picture after this one on trust.
                    return Some("the sky never caught up with the world's clock".to_string());
                }
                *age += 1;
                None
            }
            Work::Asking { prefix, age } => {
                if *age >= ASK_FRAMES {
                    return Some(both_halves(prefix, "the server said nothing back"));
                }
                *age += 1;
                None
            }
        }
    }
}

/// Starts a line. Answers it outright where it can, and hands back what to go
/// on doing where it cannot.
/// One line this end serves: the word that reaches it, what `help` says about
/// it, and what saying it begins.
///
/// A table for the reason `server::console`'s is one — [`help`] is a fold
/// over it, so the listing and the grammar cannot come apart — and for one
/// more that is this module's own. Every arm of the match this replaced ended
/// in the same four lines: parse the words, make a [`Doing`] out of what came
/// back, and answer the refusal otherwise. A verb now says what it wants
/// begun and nothing about how a line is answered, which is [`begin`]'s.
struct Verb {
    word: &'static str,
    /// What `help` says, each line printed after the word.
    usage: &'static [&'static str],
    run: fn(Spoken<'_, '_, '_>) -> Result<Begun, String>,
}

/// What a verb is handed: the words after it, the line they were cut from,
/// and this machine to work on.
struct Spoken<'a, 'w, 's> {
    args: &'a [&'a str],
    /// The line as typed, whole — for `shot`, whose argument is a path and
    /// may carry spaces. See [`after_verb`].
    line: &'a str,
    hands: &'a mut Hands<'w, 's>,
    held: Held<'a>,
}

/// What a verb has begun, which is one of five things and never a socket
/// answer: the answering is [`begin`]'s, and writing it once there is the
/// point of the table.
enum Begun {
    /// Work that outlives the line. The answer comes when it is done —
    /// see [`Doing::advance`].
    Working(Work),
    /// A button pressed on the screen. The stand-in that carries the press is
    /// spawned by [`begin`], which is where a [`Commands`] is.
    Clicking(MenuButton),
    /// An answer on the spot, and nothing left to wait for.
    Said(String),
    /// The server's line: put on the wire, its answer given after this much
    /// of ours. See [`forward`].
    Asking(String),
    /// The world closed and the game stopped.
    Quitting,
}

/// Every line this end serves, in the order `help` prints them.
const VERBS: [Verb; 7] = [
    Verb {
        word: "shot",
        usage: &["<path> — write a PNG of the view, once the ground has arrived"],
        run: shooting,
    },
    Verb {
        word: "press",
        usage: &[
            "<control> [seconds] — hold a control down, or tap it if no time is given; \
             `escape` too, which is nobody's control",
        ],
        run: pressing,
    },
    Verb {
        word: "click",
        usage: &["<button> — press a menu button; `click` alone lists them"],
        run: clicking,
    },
    Verb {
        word: "hold",
        usage: &["on|off — stop the clock where it stands, so the light keeps still"],
        run: holding,
    },
    Verb {
        word: "quit",
        usage: &["— close the world and stop the game"],
        run: quitting,
    },
    // Listed by nothing, unlike every other row: the answer to `help` is
    // both halves of the vocabulary at once, and the server's own line for it
    // is already in the second half. Saying it here would say it twice.
    Verb {
        word: "help",
        usage: &[],
        run: |_| Ok(Begun::Asking(help())),
    },
    // Served by the fall-through, which is where the rule about what stays on
    // this machine lives. Listed all the same: a player at the socket has no
    // other way to learn the word, and `help` is the documentation.
    Verb {
        word: "client",
        usage: &["… — this machine's own switches and view; `client` alone lists them"],
        run: elsewhere,
    },
];

/// What `help` says about this end of the grammar, before the server's own
/// answer is appended to it. The server's half is asked for rather than
/// guessed at, exactly as the console's tab completion asks — so one `help`
/// down the socket is the whole vocabulary, both sides of the wire.
fn help() -> String {
    VERBS
        .iter()
        .flat_map(|verb| {
            verb.usage
                .iter()
                .map(move |line| format!("{} {line}", verb.word))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every word this end serves, for [`crate::cli`]'s prose to be held to — the
/// `--help` text names them and no table can generate a sentence.
#[cfg(test)]
pub(crate) fn verbs() -> impl Iterator<Item = &'static str> {
    VERBS.iter().map(|verb| verb.word)
}

fn begin(
    order: Order,
    commands: &mut Commands,
    hands: &mut Hands,
    exit: &mut MessageWriter<AppExit>,
    held: Held,
) -> Option<Doing> {
    let Order { line, answer } = order;
    let words: Vec<&str> = line.split_whitespace().collect();
    let (word, args) = words
        .split_first()
        .map_or(("", &[][..]), |(word, args)| (*word, args));

    let run = VERBS.iter().find(|verb| verb.word == word).map_or(
        elsewhere as fn(Spoken<'_, '_, '_>) -> Result<Begun, String>,
        |verb| verb.run,
    );
    let begun = run(Spoken {
        args,
        line: &line,
        hands,
        held,
    });

    match begun {
        Err(why) => refuse(why, answer),
        Ok(Begun::Working(work)) => Some(Doing { answer, work }),
        Ok(Begun::Clicking(button)) => {
            // A stand-in rather than the button itself. Every menu system
            // asks for `(&Interaction, &MenuButton)` and none of them cares
            // which entity carries it, so this goes through the same code a
            // click does — and the real button cannot be used for it:
            // `ui_focus_system` sets every *node* it finds pressed back to
            // `None` the moment the mouse is not down, which in a run with no
            // mouse is always. A bare entity is no node, so nothing takes the
            // press away but us.
            let stand_in = commands.spawn((button, Interaction::Pressed)).id();
            Some(Doing {
                answer,
                work: Work::Settling {
                    waited: 0,
                    age: 0,
                    said: format!("{} clicked", args.join(" ")),
                    stand_in: Some(stand_in),
                },
            })
        }
        Ok(Begun::Said(said)) => {
            let _ = answer.send(said);
            None
        }
        Ok(Begun::Asking(prefix)) => forward(&line, prefix, hands, answer),
        Ok(Begun::Quitting) => {
            let _ = answer.send("closing the world".to_string());
            exit.write(AppExit::Success);
            None
        }
    }
}

/// `shot <path>`: a picture of the view, once there is a view to take one of.
fn shooting(spoken: Spoken) -> Result<Begun, String> {
    let path = shot(after_verb(spoken.line))?;
    Ok(Begun::Working(Work::Shooting {
        path,
        target: spoken.held.target,
        stage: Stage::Arriving,
        waited: 0,
        age: 0,
    }))
}

/// `press <control> [seconds]`: a control held down, or tapped.
fn pressing(spoken: Spoken) -> Result<Begun, String> {
    let (control, left) = press(spoken.args)?;
    // The key the control is *bound* to rather than one of this module's
    // choosing, so a rebound control is pressed where the player put it and a
    // press goes down the same path a hand would.
    let key = control.key(&spoken.hands.bindings);
    spoken.hands.keys.press(key);
    Ok(Begun::Working(Work::Pressing {
        key,
        left,
        said: pressed(control, left),
    }))
}

/// `click <button>`: a button of the screen pressed.
fn clicking(spoken: Spoken) -> Result<Begun, String> {
    Ok(Begun::Clicking(button(spoken.args, &spoken.hands.buttons)?))
}

/// `hold on|off`: the clock stopped where it stands, or let go of.
///
/// Letting go is done the moment it is said — there is nothing to wait for in
/// handing the hour back to the world. Taking hold is not.
fn holding(spoken: Spoken) -> Result<Begun, String> {
    match onoff(spoken.args, "hold")? {
        // No world here, and no word from one ever: there is no clock to
        // hold, and waiting for an hour that is never coming would stand a
        // scripted run still for the whole of `PATIENCE` before saying so. A
        // session that has simply not been told the hour *yet* is the other
        // case, and that one waits — the word is on its way.
        true if spoken.hands.online.is_none() && !spoken.hands.sky.heard_the_hour() => {
            Err("there is no world here whose clock could be held".to_string())
        }
        true => Ok(Begun::Working(Work::Holding { age: 0 })),
        false => {
            *spoken.held.holding = false;
            Ok(Begun::Said("the clock runs again".to_string()))
        }
    }
}

/// `quit`: the world closed and the game stopped.
fn quitting(spoken: Spoken) -> Result<Begun, String> {
    match spoken.args {
        [] => Ok(Begun::Quitting),
        _ => Err("`quit` takes nothing but itself".to_string()),
    }
}

/// Everything this end has no verb for: a `client` line, which never leaves
/// the machine, or the server's, which crosses verbatim. The rule about which
/// is which is [`dispatch`]'s and is asked for here rather than repeated —
/// one grammar, whichever mouth speaks it.
fn elsewhere(spoken: Spoken) -> Result<Begun, String> {
    // The view as it stands before the line runs, so that a line which moved
    // it can be told from one that did not. `client zoom 120` is answered
    // when the picture has stopped changing, exactly as it was when `zoom`
    // was a verb of this module's own — the wait belongs to the socket, which
    // promises a line is answered when its work is done, and not to the
    // grammar, which is the same grammar a keyboard types.
    let before = spoken.hands.machine.seen();
    let line = spoken.line;
    let answered = dispatch(line, &mut spoken.hands.machine.picture());

    Ok(match answered {
        Dispatch::Remote => Begun::Asking(String::new()),
        Dispatch::Local(Ok(reply) | Err(reply)) => {
            if spoken.hands.machine.seen() == before {
                Begun::Said(reply)
            } else {
                Begun::Working(settling(reply))
            }
        }
    })
}

/// Says no, and has done with the line.
fn refuse(why: String, answer: SyncSender<String>) -> Option<Doing> {
    let _ = answer.send(why);
    None
}

/// The work of waiting for the world to catch up with a view that has just
/// moved, and saying what was asked for once it has.
fn settling(said: String) -> Work {
    Work::Settling {
        waited: 0,
        age: 0,
        said,
        stand_in: None,
    }
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
    Some(Doing {
        answer,
        work: Work::Asking { prefix, age: 0 },
    })
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
        return Ok((Pressing::Reserved(key), seconds));
    }
    let Some(action) = Action::ALL.into_iter().find(|it| it.name() == named) else {
        return Err(format!("no control called `{named}`\n{}", actions()));
    };
    Ok((Pressing::Control(action), seconds))
}

/// What a `press` line named: a control, pressed at whatever key it is
/// *bound* to so a rebound one is found where the player put it, or a key
/// that is nobody's control and so has nothing to be bound to.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Pressing {
    Control(Action),
    Reserved(KeyCode),
}

impl Pressing {
    fn key(self, bindings: &KeyBindings) -> KeyCode {
        match self {
            Self::Control(action) => bindings.key(action),
            Self::Reserved(key) => key,
        }
    }

    /// What it is called in an answer, which is the word that was typed —
    /// both arms derive it from what they hold rather than carrying it.
    fn name(self) -> &'static str {
        match self {
            Self::Control(action) => action.name(),
            Self::Reserved(key) => bindings::NAMED
                .iter()
                .find(|(_, named)| *named == key)
                .map_or("that key", |(name, _)| name),
        }
    }
}

/// What a press answers with when it is over. A tap and a hold are told apart
/// because a driver that meant to sail and forgot the seconds gets a boat
/// that has not moved, and the answer is the only place that shows.
fn pressed(pressing: Pressing, seconds: f32) -> String {
    if seconds > 0.0 {
        format!("{} held {seconds} seconds", pressing.name())
    } else {
        format!("{} tapped", pressing.name())
    }
}

/// The controls a `press` will take, as one line — read off [`Action::ALL`]
/// and [`bindings::NAMED`] so neither is listed twice.
fn actions() -> String {
    let names = Action::ALL
        .iter()
        .map(|it| it.name())
        .chain(bindings::NAMED.iter().map(|(name, _)| *name))
        .collect::<Vec<_>>();
    format!("controls: {}", names.join(", "))
}

#[cfg(test)]
mod tests {
    use bevy::time::{TimePlugin, TimeUpdateStrategy};

    use super::*;
    use crate::camera::View;
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
        // In a world, which is what a driver's lines are about — and what
        // `Machine` asks about before it offers a view to read or move.
        app.add_plugins((bevy::state::app::StatesPlugin, TimePlugin))
            .insert_state(crate::AppState::InWorld)
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
        // different one — the shape `world time 23:00` leaves behind.
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
    /// was never coming. A run given no seed is on the menu, and a driver
    /// taking pictures of the menus is the one who would pay for it.
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
            Ok((Pressing::Reserved(KeyCode::Escape), 0.0))
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

    /// A `client` line that moved the picture waits for it to stop moving,
    /// exactly as `zoom` did while it was a verb of this module's own — and
    /// one that only read it is answered on the frame it arrives.
    ///
    /// This is what the socket adds to a grammar it shares with the keyboard:
    /// the words are the console's, the promise that an answer means the work
    /// is done is this module's, and the two meet by watching whether the view
    /// moved rather than by keeping a list of the lines that move it.
    #[test]
    fn a_client_line_that_moved_the_view_settles_before_it_answers() {
        let (mut app, orders) = driven_app();
        app.world_mut().spawn(MapCamera::looking(View {
            distance: 240.0,
            yaw: -std::f32::consts::PI,
            ..default()
        }));

        // A reading changes nothing, so there is nothing to wait for. The
        // bearing comes back folded — what the camera holds runs unbounded,
        // and a driver that read `-540` and wrote it back would be turning
        // the camera rather than leaving it.
        let asked = say(&orders, "client yaw");
        app.update();
        assert_eq!(asked.try_recv().expect("nothing to wait for"), "yaw 180");

        // A setting does move it, and is not answered on the spot.
        let asked = say(&orders, "client zoom 120");
        app.update();
        assert!(
            asked.try_recv().is_err(),
            "the picture was declared still on the frame it started moving"
        );
        run_frames(&mut app, SETTLE_FRAMES as usize + 2);
        assert_eq!(asked.try_recv().expect("the picture settled"), "zoom 120");

        // And it took: the camera is where the line put it, not merely the
        // `View` a camera would be spawned from.
        let camera = app
            .world_mut()
            .query::<&MapCamera>()
            .single(app.world())
            .expect("the camera this test spawned");
        assert_eq!(camera.distance, 120.0);
    }

    /// Every word `help` offers is answered by this end, on the frame it
    /// arrives — a refusal for the ones that want an argument, which is still
    /// this end answering rather than the server.
    ///
    /// What `help` says is now a fold over [`VERBS`], so a word it offers and
    /// nothing serves is no longer a thing that can be written. What is still
    /// worth holding is the *promptness*: `client` is served through the same
    /// fall-through that puts a line on the wire, and a verb that quietly
    /// stopped being one would go on being answered — by the server, a
    /// network round trip later, with a refusal.
    #[test]
    fn every_word_help_offers_is_a_word_this_end_serves() {
        let (mut app, orders) = driven_app();
        for verb in VERBS.iter().map(|verb| verb.word) {
            let answered = say(&orders, verb);
            app.update();

            let said = answered
                .try_recv()
                .unwrap_or_else(|_| panic!("`{verb}` is offered by `help` and answered by nobody"));
            assert_ne!(
                said, "nobody is serving this world",
                "`{verb}` is offered by `help` and served only by the server"
            );
        }
    }

    /// Every button `click` offers can be clicked, and lands on the button it
    /// named. The listing is an index rather than the grammar — `parse` never
    /// reads it — so this is what holds the two to agreement. It is the last
    /// of this game's vocabularies that needs holding: the other two are
    /// tables now, and their listings are folds over them.
    ///
    /// The five that carry something are given one here. A button that grew
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
        app.world_mut().resource_mut::<Control>().doing = Some(Doing {
            answer,
            work: Work::Asking {
                prefix: String::new(),
                age: 0,
            },
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
                Some(Doing {
                    work: Work::Settling { waited: 0, .. },
                    ..
                })
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
    /// reason to want — most lines are that, and a `world weather gale` that
    /// waited on the terrain would be a toll on every one of them.
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
            pressed(Pressing::Control(Action::MoveForward), 20.0),
            "forward held 20 seconds"
        );
        assert_eq!(
            pressed(Pressing::Control(Action::Chart), 0.0),
            "chart tapped"
        );
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
