//! The logbook: what this machine still keeps of a world between visits —
//! an interim home, and shrinking on purpose.
//!
//! The design this serves is *world stop and restart*, not save-and-load:
//! the server's world file is the world's continuity, and a client is meant
//! to keep nothing a rejoin cannot answer. Two things remain here on their
//! way to that. The **token** stays for good — it is the player's credential
//! for a world, the client's half of a secret, and has to live with the
//! client by definition. The **chart** stays only until claims land: the
//! server will then track each player's survey for its own reasons (a claim
//! has to be judged) and can serve the drawn coastline back on join, at
//! which point this file is a token and nothing else. The berth this book
//! once carried is already gone — boats are the server's entities now, told
//! over the wire like the players and the beasts.
//!
//! Books are keyed by the [`WorldId`] the server names in its handshake —
//! never by address, since a world moved to another host is meant to still
//! be the same world. Like the server's world file, the format is plain
//! versioned text, written whole beside the file and renamed over it, and a
//! build refuses a file it only half-understands — except the keys a past
//! format-1 build wrote and this one has outgrown, which are read past
//! rather than refused: an old book's chart is still a chart.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::PathBuf;

use bevy::prelude::*;

use protocol::survey::{Coast, Mark, Soundings};
use protocol::{Token, WorldId};

use crate::chart::Chart;
use crate::net::Session;
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
    /// The chart as of the last write — the working copy is the [`Chart`]
    /// resource, read back in here each time the book is written.
    soundings: Vec<(IVec2, Soundings)>,
    /// And the names the player has written on it, by island id: the other
    /// half of what the chart holds, and the half it would hurt most to
    /// lose.
    names: Vec<(IVec2, String)>,
}

impl Logbook {
    /// The chart this world's book holds, for the survey to continue from.
    pub fn charted(&self) -> Chart {
        Chart::from_entries(self.soundings.iter().cloned(), self.names.iter().cloned())
    }
}

/// The logbook for a session about to be entered, or `None` for a session
/// this machine does not remember: a world both ephemeral and our own, which
/// will never exist again once left.
///
/// The book on file is opened — or begun — under the world id the handshake
/// named, and the token the welcome dealt replaces whatever was held: the
/// server's answer is what next visit's papers must match.
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
            soundings: Vec::new(),
            names: Vec::new(),
        },
        // A book that exists and cannot be read is left exactly where it is
        // — no path, so nothing this session writes can land on it. The one
        // way to get here honestly is a file from a build later than this
        // one, and destroying that file would be this build's fault.
        Read::Refused => Logbook {
            path: None,
            token: connection.token,
            soundings: Vec::new(),
            names: Vec::new(),
        },
    };
    book.token = connection.token;
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
        Ok((token, soundings, names)) => Read::Book(Logbook {
            path: Some(path),
            token,
            soundings,
            names,
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
            keep_the_log.run_if(in_state(AppState::InWorld).and_then(resource_exists::<Logbook>)),
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
        (logbook.soundings, logbook.names) = chart.entries();
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
    // Sorted, so that one chart is one file, byte for byte, whatever order
    // the survey's map hands its chunks out in. Every surveyed chunk gets a
    // `coast` line even when it found nothing — surveyed-and-empty is worth
    // remembering, or the survey would do the chunk again next visit — and a
    // `shoal` line only when there is a shoal to draw.
    let mut soundings: Vec<&(IVec2, Soundings)> = logbook.soundings.iter().collect();
    soundings.sort_by_key(|(chunk, _)| (chunk.x, chunk.y));
    for (chunk, found) in soundings {
        let _ = write!(out, "coast {} {}", chunk.x, chunk.y);
        put_runs(&mut out, &found.coast);
        let _ = writeln!(out);
        if !found.shoal.is_empty() {
            let _ = write!(out, "shoal {} {}", chunk.x, chunk.y);
            put_runs(&mut out, &found.shoal);
            let _ = writeln!(out);
        }
    }
    let mut names: Vec<&(IVec2, String)> = logbook.names.iter().collect();
    names.sort_by_key(|(island, _)| (island.x, island.y));
    for (island, name) in names {
        // A name is one line of the book, so it must be one line of text:
        // anything a control character could do to the format is dropped
        // rather than written into it.
        let name: String = name.chars().filter(|c| !c.is_control()).collect();
        if !name.is_empty() {
            let _ = writeln!(out, "name {} {} {name}", island.x, island.y);
        }
    }
    out
}

fn put_runs(out: &mut String, runs: &[Coast]) {
    for run in runs {
        let _ = write!(out, " {}", if run.closed { 'c' } else { 'o' });
        for mark in &run.marks {
            let [x, z] = mark.pack();
            let _ = write!(out, "{x:02x}{z:02x}");
        }
    }
}

type Parsed = (Token, Vec<(IVec2, Soundings)>, Vec<(IVec2, String)>);

fn parse(text: &str) -> Result<Parsed, String> {
    let mut lines = text.lines();
    match lines.next() {
        Some(header) if header == format!("genovesa logbook {FORMAT}") => {}
        Some(other) => return Err(format!("not a logbook this build keeps: `{other}`")),
        None => return Err("an empty file".to_string()),
    }

    let mut token = None;
    let mut soundings: std::collections::HashMap<(i32, i32), Soundings> = Default::default();
    let mut names = Vec::new();
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
            // Keys an earlier format-1 build wrote and this one has outgrown
            // — the berth moved to the server with the boats. Read past, not
            // refused: an old book's chart is still a chart.
            "aboard" | "ashore" => {}
            "coast" => {
                let (chunk, runs) = runs_line(value)?;
                soundings.entry((chunk.x, chunk.y)).or_default().coast = runs;
            }
            "shoal" => {
                let (chunk, runs) = runs_line(value)?;
                soundings.entry((chunk.x, chunk.y)).or_default().shoal = runs;
            }
            "name" => {
                let mut fields = value.splitn(3, ' ');
                let island = IVec2::new(
                    whole(fields.next().ok_or("a name with no island")?)?,
                    whole(fields.next().ok_or("a name with half an island")?)?,
                );
                let text = fields.next().filter(|text| !text.is_empty());
                names.push((
                    island,
                    text.ok_or("a name with nothing written")?.to_string(),
                ));
            }
            other => return Err(format!("unknown key `{other}`")),
        }
    }
    Ok((
        token.ok_or("no token")?,
        soundings
            .into_iter()
            .map(|((x, y), found)| (IVec2::new(x, y), found))
            .collect(),
        names,
    ))
}

/// One line of survey — `coast` and `shoal` share the shape: the chunk's
/// coordinates, then each run as `c` (a ring) or `o` (open) followed by the
/// marks as hex pairs. A `coast` with no runs is still an entry — surveyed,
/// and found to be all water or all land.
fn runs_line(value: &str) -> Result<(IVec2, Vec<Coast>), String> {
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
                    (Ok(x), Ok(z)) => Ok(Mark::unpack([x, z])),
                    _ => Err("marks that are not hex".to_string()),
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        runs.push(Coast::new(marks, closed));
    }
    Ok((chunk, runs))
}

fn whole(value: &str) -> Result<i32, String> {
    value
        .parse::<i32>()
        .map_err(|_| format!("`{value}` is not a chunk coordinate"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_book() -> Logbook {
        Logbook {
            path: None,
            token: Token(0x00C0_FFEE_0000_0007),
            soundings: vec![
                (
                    IVec2::new(3, -2),
                    Soundings {
                        coast: vec![
                            Coast::new(
                                vec![Mark::unpack([0, 17]), Mark::unpack([255, 254])],
                                false,
                            ),
                            Coast::new(
                                vec![
                                    Mark::unpack([10, 10]),
                                    Mark::unpack([20, 10]),
                                    Mark::unpack([10, 20]),
                                ],
                                true,
                            ),
                        ],
                        shoal: vec![Coast::new(
                            vec![Mark::unpack([5, 5]), Mark::unpack([200, 5])],
                            false,
                        )],
                    },
                ),
                // Surveyed and found to hold no coast at all — still worth a
                // line, or the survey would do the chunk again next visit.
                (IVec2::new(-8, 4), Soundings::default()),
            ],
            names: vec![(IVec2::new(701, -512), "Windward Reach".to_string())],
        }
    }

    #[test]
    fn a_logbook_survives_the_round_trip() {
        let book = a_book();
        let (token, soundings, names) = parse(&compose(&book)).expect("parse what was composed");
        assert_eq!(token, book.token);
        // Composing sorts the entries — one chart, one file — so they come
        // back in that order whatever order they were held in; sorting both
        // sides makes the comparison about content alone.
        let sorted = |mut entries: Vec<(IVec2, Soundings)>| {
            entries.sort_by_key(|(chunk, _)| (chunk.x, chunk.y));
            entries
        };
        assert_eq!(sorted(soundings), sorted(book.soundings.clone()));
        assert_eq!(names, book.names);
    }

    #[test]
    fn an_old_books_berth_keys_are_read_past() {
        // A format-1 book from before the berth moved to the server: its
        // chart still counts, and the keys this build has outgrown are
        // passed over rather than refused.
        let (token, soundings, _names) =
            parse("genovesa logbook 1\ntoken 2a\nashore 1 2 3 4 5 6 7\naboard 1.5\ncoast 0 0\n")
                .expect("an old book still reads");
        assert_eq!(token, Token(0x2A));
        assert_eq!(soundings.len(), 1);
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
                "genovesa logbook 1\ntoken 1\nname 1 2\n",
                "a name with nothing written",
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
