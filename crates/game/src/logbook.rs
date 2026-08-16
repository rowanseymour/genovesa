//! The logbook: what this machine remembers of a world between visits.
//!
//! The server keeps the world — its clock, its players' positions, one file
//! per world wherever it is hosted. What it deliberately does not keep is
//! this client's own memory of the place: the chart is a record of what *one
//! player's client* has seen (see `chart`), the boat has never crossed the
//! wire at all, and the token is the client's half of a secret. So each
//! world a player sails gets a logbook on this machine, keyed by the
//! [`WorldId`] the server names in its handshake — never by address, since a
//! world moved to another host is meant to still be the same world.
//!
//! A logbook holds three things: the token this player holds the world by,
//! where they left off — aboard, or ashore with the boat lying at anchor —
//! and the chart. The token is presented on the next visit (see
//! [`crate::net::Connection`]), the server answers with where this player
//! was, and the berth and the chart fill in the rest of the picture the wire
//! does not carry.
//!
//! The berth is trusted only when the server recognised the papers. A world
//! that greets us as a stranger — its file lost, its memory of us gone — is
//! putting us on the spawn, and a boat restored to some far anchorage would
//! be a boat the player can never reach. The chart survives either way: it
//! is a record of coasts seen, the world id says they are this world's
//! coasts, and being forgotten by the server does not unsee them.
//!
//! Like the server's world file, the format is plain versioned text, written
//! whole beside the file and renamed over it, and a build refuses a file it
//! only half-understands. Unlike the world file there is no lock and no
//! backup: one game process per machine is the ordinary case, and the worst
//! a lost logbook costs is a chart — the world itself is the server's.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::PathBuf;

use bevy::prelude::*;

use protocol::{Token, WorldId};

use crate::boat::Boat;
use crate::chart::{Chart, Coast};
use crate::net::Session;
use crate::player::Player;
use crate::AppState;

/// The format this build writes, named in the file's first line.
const FORMAT: u32 = 1;

/// Seconds between writes while in the world. The closing write on the way
/// out is the one that matters; these are insurance against never reaching
/// it, priced by what a crash costs: half a minute of survey.
const KEEP_INTERVAL: f32 = 30.0;

/// This machine's memory of the world the player is in. Present exactly
/// while a remembered world is being played: inserted on the way in (see
/// [`for_session`]), written and removed on the way out — a test's world, or
/// an ephemeral one opened from the command line, never has one.
#[derive(Resource)]
pub struct Logbook {
    /// Where this logbook lives, or `None` on a machine with nowhere to keep
    /// one — then the book lives and dies with the session.
    path: Option<PathBuf>,
    /// The token this player holds the world by — presented next visit.
    token: Token,
    /// Where the player left off. `None` when this visit started fresh —
    /// nothing to restore, and the entry ceremony is the ordinary one.
    pub berth: Option<Berth>,
    /// The chart as of the last write — the working copy is the [`Chart`]
    /// resource, read back in here each time the book is written.
    coasts: Vec<(IVec2, Vec<Coast>)>,
}

/// Where a player left off, in the terms the wire does not carry: the server
/// remembers where they *were*, and this says what that position meant —
/// at the helm, or on their own feet with the boat lying somewhere else.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Berth {
    /// At the helm, the hull heading this way (a yaw about the vertical,
    /// the world's usual convention).
    Aboard { heading: f32 },
    /// On their own feet, standing `at` — which must agree with where the
    /// server says they are, see [`BERTH_SLACK`] — with the boat at anchor
    /// at `boat`. `height` and `facing` are the walker's own, kept because
    /// the ground may not have streamed in yet when they are put back.
    Ashore {
        at: Vec2,
        boat: Vec2,
        heading: f32,
        height: f32,
        facing: f32,
    },
}

/// How far the book's idea of where the player stood may differ from the
/// server's before the berth is disbelieved, in metres.
///
/// The two records are written by different machines on different clocks, and
/// a session that *crashed* can leave them telling different moments: the
/// server's last save from before the player anchored and went ashore, the
/// logbook's from after. Trusting the stale half would stand the player on
/// open water with the boat at an anchorage they cannot reach. A clean close
/// writes both within the same instant, so agreement is the ordinary case
/// and the slack only has to cover rounding, not drift.
const BERTH_SLACK: f32 = 8.0;

impl Logbook {
    /// The chart this world's book holds, for the survey to continue from.
    pub fn charted(&self) -> Chart {
        Chart::from_entries(self.coasts.iter().cloned())
    }
}

/// The logbook for a session about to be entered, or `None` for a session
/// this machine does not remember: a world both ephemeral and our own, which
/// will never exist again once left.
///
/// The book on file is opened — or begun — under the world id the handshake
/// named, and the token the welcome dealt replaces whatever was held: the
/// server's answer is what next visit's papers must match. The berth
/// survives only a *recognised* return that agrees with the server about
/// where the player stands (see the module doc and [`BERTH_SLACK`]); the
/// chart survives any.
pub fn for_session(session: &Session) -> Option<Logbook> {
    if session.hosting.is_some() && !session.kept {
        return None;
    }
    let connection = &session.connection;
    let mut book = match read(connection.world) {
        Read::Book(book) => book,
        Read::Missing => Logbook {
            path: place_for(connection.world),
            token: connection.token,
            berth: None,
            coasts: Vec::new(),
        },
        // A book that exists and cannot be read is left exactly where it is
        // — no path, so nothing this session writes can land on it. The one
        // way to get here honestly is a file from a build later than this
        // one, and destroying that file would be this build's fault.
        Read::Refused => Logbook {
            path: None,
            token: connection.token,
            berth: None,
            coasts: Vec::new(),
        },
    };
    book.token = connection.token;
    if !connection.resumed {
        book.berth = None;
    }
    if let Some(Berth::Ashore { at, .. }) = book.berth {
        if at.distance(connection.spawn) > BERTH_SLACK {
            book.berth = None;
        }
    }
    Some(book)
}

/// The token this machine holds `world` by, if it has sailed there before —
/// what the handshake presents, read before any app exists.
pub fn token_for(world: WorldId) -> Option<Token> {
    let path = place_for(world)?;
    let text = fs::read_to_string(path).ok()?;
    let token = text.lines().find_map(|line| line.strip_prefix("token "))?;
    Some(Token(u64::from_str_radix(token, 16).ok()?))
}

/// Where `world`'s logbook lives on this machine.
fn place_for(world: WorldId) -> Option<PathBuf> {
    Some(
        server::data_dir()?
            .join("logbooks")
            .join(format!("{world}.logbook")),
    )
}

/// What looking a world's book up can come to. Missing and refused are kept
/// apart because they earn different paths: a missing book is begun, where a
/// refused one must never be written over — see [`for_session`].
enum Read {
    Book(Logbook),
    Missing,
    Refused,
}

fn read(world: WorldId) -> Read {
    let Some(path) = place_for(world) else {
        return Read::Missing;
    };
    let Ok(text) = fs::read_to_string(&path) else {
        return Read::Missing;
    };
    match parse(&text) {
        Ok((token, berth, coasts)) => Read::Book(Logbook {
            path: Some(path),
            token,
            berth,
            coasts,
        }),
        Err(why) => {
            // A book this build cannot read is left where it is, unwritten
            // rather than overwritten: refusing to understand a file is no
            // licence to destroy it. The session runs bookless.
            warn!("cannot read the logbook for this world: {why}");
            Read::Refused
        }
    }
}

pub struct LogbookPlugin;

impl Plugin for LogbookPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (note_berth, keep_the_log)
                .chain()
                .run_if(in_state(AppState::InWorld).and_then(resource_exists::<Logbook>)),
        )
        // Closed before the chart is stowed, or there would be nothing left
        // to write down. Conditioned like the systems above: a world nobody
        // remembers has no book to close, and leaving one must not be an
        // error.
        .add_systems(
            OnExit(AppState::InWorld),
            close_the_log
                .run_if(resource_exists::<Logbook>)
                .before(crate::chart::stow_the_chart),
        );
    }
}

/// Keeps the book's berth current, every frame: reading it is two queries,
/// and a berth mirrored continuously is one that is right whenever the book
/// happens to be written — including at an exit racing the despawn of the
/// very entities it describes.
fn note_berth(
    mut logbook: ResMut<Logbook>,
    players: Query<(&Transform, Option<&ChildOf>), With<Player>>,
    boats: Query<&Transform, With<Boat>>,
) {
    let (Ok((walker, aboard)), Ok(boat)) = (players.single(), boats.single()) else {
        return;
    };
    logbook.berth = Some(match aboard {
        Some(_) => Berth::Aboard {
            heading: yaw_of(boat),
        },
        None => Berth::Ashore {
            at: walker.translation.xz(),
            boat: boat.translation.xz(),
            heading: yaw_of(boat),
            height: walker.translation.y,
            facing: yaw_of(walker),
        },
    });
}

/// The yaw a transform was aimed with: the inverse of
/// `Quat::from_rotation_y`, read off the forward it produces.
fn yaw_of(transform: &Transform) -> f32 {
    let forward = transform.forward();
    f32::atan2(-forward.x, -forward.z)
}

/// Writes the book on a slow beat — insurance, not the record; see
/// [`KEEP_INTERVAL`].
fn keep_the_log(
    time: Res<Time>,
    mut logbook: ResMut<Logbook>,
    chart: Option<Res<Chart>>,
    mut last: Local<f32>,
) {
    if time.elapsed_secs() - *last < KEEP_INTERVAL {
        return;
    }
    *last = time.elapsed_secs();
    write_down(&mut logbook, chart.as_deref());
}

/// The closing write, and the book put away: the next world is a different
/// book, and finding this one still on the shelf would write one world's
/// coasts against another's id.
fn close_the_log(mut commands: Commands, mut logbook: ResMut<Logbook>, chart: Option<Res<Chart>>) {
    write_down(&mut logbook, chart.as_deref());
    commands.remove_resource::<Logbook>();
}

fn write_down(logbook: &mut Logbook, chart: Option<&Chart>) {
    if let Some(chart) = chart {
        logbook.coasts = chart.entries();
    }
    let Some(path) = &logbook.path else {
        return;
    };
    if let Err(error) = keep(path, logbook) {
        warn!("the logbook could not be kept: {error}");
    }
}

/// Composed whole beside the file and renamed over it, like the server's
/// world file: at no instant is the name pointing at half a book.
fn keep(path: &PathBuf, logbook: &Logbook) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let fresh = path.with_extension("logbook.new");
    fs::write(&fresh, compose(logbook))?;
    fs::rename(&fresh, path)
}

fn compose(logbook: &Logbook) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "genovesa logbook {FORMAT}");
    let _ = writeln!(out, "token {:016x}", logbook.token.0);
    match logbook.berth {
        None => {}
        Some(Berth::Aboard { heading }) => {
            let _ = writeln!(out, "aboard {heading}");
        }
        Some(Berth::Ashore {
            at,
            boat,
            heading,
            height,
            facing,
        }) => {
            let _ = writeln!(
                out,
                "ashore {} {} {} {} {heading} {height} {facing}",
                at.x, at.y, boat.x, boat.y
            );
        }
    }
    // Sorted, so that one chart is one file, byte for byte, whatever order
    // the survey's map hands its chunks out in.
    let mut coasts: Vec<&(IVec2, Vec<Coast>)> = logbook.coasts.iter().collect();
    coasts.sort_by_key(|(chunk, _)| (chunk.x, chunk.y));
    for (chunk, runs) in coasts {
        let _ = write!(out, "coast {} {}", chunk.x, chunk.y);
        for run in runs {
            let _ = write!(out, " {}", if run.closed { 'c' } else { 'o' });
            for mark in &run.marks {
                let [x, z] = mark.pack();
                let _ = write!(out, "{x:02x}{z:02x}");
            }
        }
        let _ = writeln!(out);
    }
    out
}

type Parsed = (Token, Option<Berth>, Vec<(IVec2, Vec<Coast>)>);

fn parse(text: &str) -> Result<Parsed, String> {
    let mut lines = text.lines();
    match lines.next() {
        Some(header) if header == format!("genovesa logbook {FORMAT}") => {}
        Some(other) => return Err(format!("not a logbook this build keeps: `{other}`")),
        None => return Err("an empty file".to_string()),
    }

    let mut token = None;
    let mut berth = None;
    let mut coasts = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (key, value) = line
            .split_once(' ')
            .ok_or_else(|| format!("a line with no value: `{line}`"))?;
        match key {
            "token" => {
                token = Some(Token(
                    u64::from_str_radix(value, 16)
                        .map_err(|_| format!("`{value}` is not a token"))?,
                ))
            }
            "aboard" => {
                berth = Some(Berth::Aboard {
                    heading: finite(value)?,
                })
            }
            "ashore" => {
                let fields: Vec<f32> = value.split(' ').map(finite).collect::<Result<_, _>>()?;
                let [ax, ay, bx, by, heading, height, facing] = fields[..] else {
                    return Err(format!("half a berth: `{line}`"));
                };
                berth = Some(Berth::Ashore {
                    at: Vec2::new(ax, ay),
                    boat: Vec2::new(bx, by),
                    heading,
                    height,
                    facing,
                });
            }
            "coast" => coasts.push(coast(value)?),
            other => return Err(format!("unknown key `{other}`")),
        }
    }
    Ok((token.ok_or("no token")?, berth, coasts))
}

/// One surveyed chunk's line: its coordinates, then each run as `c` (a ring)
/// or `o` (open) followed by the marks as hex pairs. A chunk with no runs is
/// still an entry — surveyed, and found to be all water or all land.
fn coast(value: &str) -> Result<(IVec2, Vec<Coast>), String> {
    let mut fields = value.split(' ');
    let chunk = IVec2::new(
        whole(fields.next().ok_or("a coast with no chunk")?)?,
        whole(fields.next().ok_or("a coast with half a chunk")?)?,
    );
    let mut runs = Vec::new();
    for field in fields {
        // Refused before it is sliced: `parse` is total everywhere else, and
        // a stray double space or a non-ASCII byte must come back as a
        // refusal rather than a slice out of char bounds.
        if field.is_empty() || !field.is_ascii() {
            return Err(format!("a run that is not a run: `{field}`"));
        }
        let (shape, marks) = field.split_at(1);
        let closed = match shape {
            "c" => true,
            "o" => false,
            other => return Err(format!("a run of unknown shape `{other}`")),
        };
        if marks.len() % 4 != 0 || marks.is_empty() {
            return Err(format!("a run of {} hex digits", marks.len()));
        }
        let marks = (0..marks.len() / 4)
            .map(|i| {
                let pair = |at: usize| u8::from_str_radix(&marks[at..at + 2], 16);
                match (pair(i * 4), pair(i * 4 + 2)) {
                    (Ok(x), Ok(z)) => Ok(crate::chart::Mark::unpack([x, z])),
                    _ => Err("marks that are not hex".to_string()),
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        runs.push(Coast::new(marks, closed));
    }
    Ok((chunk, runs))
}

fn finite(value: &str) -> Result<f32, String> {
    value
        .parse::<f32>()
        .ok()
        .filter(|parsed| parsed.is_finite())
        .ok_or_else(|| format!("`{value}` is not a number"))
}

fn whole(value: &str) -> Result<i32, String> {
    value
        .parse::<i32>()
        .map_err(|_| format!("`{value}` is not a chunk coordinate"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::Mark;
    use crate::testing::{enter_world, world_app_ashore_of_entry};

    fn a_book() -> Logbook {
        Logbook {
            path: None,
            token: Token(0x00C0_FFEE_0000_0007),
            berth: Some(Berth::Ashore {
                at: Vec2::new(118.0, -30.5),
                boat: Vec2::new(120.5, -33.25),
                heading: 1.25,
                height: 2.5,
                facing: -0.75,
            }),
            coasts: vec![
                (
                    IVec2::new(3, -2),
                    vec![
                        Coast::new(vec![Mark::unpack([0, 17]), Mark::unpack([255, 254])], false),
                        Coast::new(
                            vec![
                                Mark::unpack([10, 10]),
                                Mark::unpack([20, 10]),
                                Mark::unpack([10, 20]),
                            ],
                            true,
                        ),
                    ],
                ),
                // Surveyed and found to hold no coast at all — still worth a
                // line, or the survey would do the chunk again next visit.
                (IVec2::new(-8, 4), Vec::new()),
            ],
        }
    }

    #[test]
    fn a_logbook_survives_the_round_trip() {
        let book = a_book();
        let (token, berth, coasts) = parse(&compose(&book)).expect("parse what was composed");
        assert_eq!(token, book.token);
        assert_eq!(berth, book.berth);
        // Composing sorts the chunks — one chart, one file — so the entries
        // come back in that order whatever order they were held in.
        let mut held = book.coasts.clone();
        held.sort_by_key(|(chunk, _)| (chunk.x, chunk.y));
        assert_eq!(coasts, held);
    }

    #[test]
    fn a_book_with_no_berth_still_reads() {
        let mut book = a_book();
        book.berth = None;
        let (_, berth, _) = parse(&compose(&book)).expect("parse");
        assert_eq!(berth, None);

        book.berth = Some(Berth::Aboard { heading: 2.5 });
        let (_, berth, _) = parse(&compose(&book)).expect("parse");
        assert_eq!(berth, book.berth);
    }

    /// A book with only a berth in it, for the entry tests.
    fn moored(berth: Berth) -> Logbook {
        Logbook {
            path: None,
            token: Token(1),
            berth: Some(berth),
            coasts: Vec::new(),
        }
    }

    #[test]
    fn a_berth_ashore_puts_the_boat_at_anchor_and_the_player_on_their_feet() {
        let mut app = world_app_ashore_of_entry();
        // Where the server put the player down — their own walking position,
        // for a visit that left off ashore.
        app.world_mut().resource_mut::<crate::camera::View>().focus = Vec3::new(10.0, 0.0, 5.0);
        app.world_mut().insert_resource(moored(Berth::Ashore {
            at: Vec2::new(10.0, 5.0),
            boat: Vec2::new(50.0, -20.0),
            heading: 0.5,
            height: 2.0,
            facing: 1.0,
        }));
        enter_world(&mut app);

        // The boat lies at its anchorage, not at the player.
        let boat = *app
            .world_mut()
            .query_filtered::<&Transform, With<Boat>>()
            .single(app.world())
            .expect("a boat");
        assert_eq!(boat.translation.x, 50.0);
        assert_eq!(boat.translation.z, -20.0);

        // And the player stands on their own feet where the server said,
        // at the height the book remembered — not aboard.
        let (walker, aboard) = app
            .world_mut()
            .query_filtered::<(&Transform, Option<&ChildOf>), With<Player>>()
            .single(app.world())
            .expect("a player");
        assert!(aboard.is_none(), "the player was put back aboard");
        assert_eq!(walker.translation, Vec3::new(10.0, 2.0, 5.0));
    }

    #[test]
    fn a_berth_aboard_holds_the_hull_on_its_heading() {
        let mut app = world_app_ashore_of_entry();
        app.world_mut().resource_mut::<crate::camera::View>().focus = Vec3::new(10.0, 0.0, 5.0);
        app.world_mut()
            .insert_resource(moored(Berth::Aboard { heading: 1.25 }));
        enter_world(&mut app);

        // The boat is where the player is — the server's position — but on
        // the heading it was left on, not the view's.
        let boat = *app
            .world_mut()
            .query_filtered::<&Transform, With<Boat>>()
            .single(app.world())
            .expect("a boat");
        assert_eq!(boat.translation.x, 10.0);
        assert_eq!(boat.translation.z, 5.0);
        assert!(
            (yaw_of(&boat) - 1.25).abs() < 1e-5,
            "the hull came back on yaw {} rather than its own",
            yaw_of(&boat)
        );

        // And the player is aboard, exactly as an ordinary entry leaves them.
        let (_, aboard) = app
            .world_mut()
            .query_filtered::<(&Transform, Option<&ChildOf>), With<Player>>()
            .single(app.world())
            .expect("a player");
        assert!(aboard.is_some(), "the player was left standing on the sea");
    }

    #[test]
    fn what_is_not_a_logbook_is_refused() {
        for (text, what) in [
            ("", "an empty file"),
            ("genovesa logbook 999\ntoken 1\n", "a format from later"),
            ("genovesa logbook 1\n", "no token"),
            (
                "genovesa logbook 1\ntoken 1\nfuture stuff\n",
                "a key this build has never heard of",
            ),
            (
                "genovesa logbook 1\ntoken 1\ncoast 0 0 c123\n",
                "a run of one and a half marks",
            ),
            (
                "genovesa logbook 1\ntoken 1\ncoast 0 0  c0a0b\n",
                "the empty run a doubled space makes",
            ),
            (
                "genovesa logbook 1\ntoken 1\ncoast 0 0 c\u{20ac}1\n",
                "marks that are not even ASCII",
            ),
            (
                "genovesa logbook 1\ntoken 1\nashore 1 2 3\n",
                "half a berth",
            ),
        ] {
            assert!(parse(text).is_err(), "swallowed {what}");
        }
    }

    #[test]
    fn leaving_an_unremembered_world_is_uneventful() {
        // An ephemeral world — a `--seed` run, a test's — opens no book, and
        // leaving it must not be an error: every system here has to sit out
        // a session with no Logbook resource, the closing one included.
        use bevy::state::app::StatesPlugin;
        let mut app = App::new();
        app.add_plugins((StatesPlugin, LogbookPlugin))
            .init_state::<AppState>();
        app.update();
        for state in [AppState::InWorld, AppState::MainMenu] {
            app.world_mut()
                .resource_mut::<NextState<AppState>>()
                .set(state);
            app.update();
        }
    }
}
