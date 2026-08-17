//! The logbook: the one thing this machine keeps of a world between visits.
//!
//! The design this serves is *world stop and restart*, not save-and-load: the
//! server's world file is the world's continuity, and a client keeps nothing a
//! rejoin cannot answer. What is left here is the **token** alone — the
//! player's credential for a world, the client's half of a secret, and the one
//! thing that has to live on this side by definition, since a world that
//! handed it back on request would be a world where anybody could be anybody.
//!
//! Everything else has gone the same way in turn: the berth, then the survey,
//! and now the island names, which went when a name stopped being a private
//! note and started riding with the claim it is written on. All of it is the
//! world's, told over the wire like the players, the boats and the beasts.
//! This module is what is left when a client is finally only a client, and it
//! is not expected to shrink further.
//!
//! Books are keyed by the [`WorldId`] the server names in its handshake —
//! never by address, since a world moved to another host is meant to still
//! be the same world. Like the server's world file, the format is plain
//! versioned text, written whole beside the file and renamed over it, and a
//! build refuses a file it only half-understands — except the keys a past
//! format-1 build wrote and this one has outgrown, which are read past
//! rather than refused: whatever else an old book holds, its token is still
//! this player's name in that world, and losing it would make them a
//! stranger where they have a history.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::PathBuf;

use bevy::prelude::*;

use protocol::{Token, WorldId};

use crate::net::Session;
use crate::AppState;

/// The format this build writes, named in the file's first line.
const FORMAT: u32 = 1;

/// This machine's memory of the world the player is in. Present exactly
/// while a remembered world is being played: inserted and written on the way
/// in (see [`for_session`] and [`open_the_log`]), removed on the way out — a
/// test's world, or an ephemeral one opened from the command line, never has
/// one.
#[derive(Resource)]
pub struct Logbook {
    /// Where this logbook lives, or `None` on a machine with nowhere to keep
    /// one — then the book lives and dies with the session.
    path: Option<PathBuf>,
    /// The token this player holds the world by — presented next visit.
    token: Token,
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
        },
        // A book that exists and cannot be read is left exactly where it is
        // — no path, so nothing this session writes can land on it. The one
        // way to get here honestly is a file from a build later than this
        // one, and destroying that file would be this build's fault.
        Read::Refused => Logbook {
            path: None,
            token: connection.token,
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

/// Throws this machine's book for `world` away, for a world being discarded:
/// the token is the player's name in a place, and a place that no longer
/// exists has nobody to be named to. Quiet about a book that was never
/// written — a world sailed on another machine and only listed here has none.
pub fn forget(world: WorldId) {
    let Some(path) = place_for(world) else {
        return;
    };
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => warn!("the logbook for a discarded world could not be thrown away: {error}"),
    }
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
        Ok(token) => Read::Book(Logbook {
            path: Some(path),
            token,
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
        // Written on the way in rather than on a beat. There is one thing in
        // a logbook now and it is settled before the world opens, so a timer
        // would be a timer spent writing the same bytes over the same bytes —
        // and writing it at once is what makes a crashed first session still
        // leave the player their name in that world.
        app.add_systems(
            OnEnter(AppState::InWorld),
            open_the_log.run_if(resource_exists::<Logbook>),
        )
        // A world nobody remembers has no book to close, and leaving one must
        // not be an error.
        .add_systems(
            OnExit(AppState::InWorld),
            close_the_log.run_if(resource_exists::<Logbook>),
        );
    }
}

/// The one write: the token this world dealt, kept where the next visit's
/// handshake will look for it.
fn open_the_log(logbook: Res<Logbook>) {
    write_down(&logbook);
}

/// The book put away: the next world is a different book, and finding this one
/// still on the shelf would write one world's papers against another's id.
///
/// Nothing is written here. There is one thing in a logbook and it was written
/// on the way in — see [`open_the_log`] — so a closing write would be the same
/// bytes over the same bytes, and a session that gained something to say would
/// have to say it here anyway.
fn close_the_log(mut commands: Commands) {
    commands.remove_resource::<Logbook>();
}

fn write_down(logbook: &Logbook) {
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
    out
}

fn parse(text: &str) -> Result<Token, String> {
    let mut lines = text.lines();
    match lines.next() {
        Some(header) if header == format!("genovesa logbook {FORMAT}") => {}
        Some(other) => return Err(format!("not a logbook this build keeps: `{other}`")),
        None => return Err("an empty file".to_string()),
    }

    let mut token = None;
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
            // — the berth went to the server with the boats, the survey after
            // it, and the names with the claims they now ride on. Read past,
            // not refused: a book's token is this player's name in that
            // world, and refusing the file over what the world can hand back
            // would make them a stranger where they have a history.
            "aboard" | "ashore" | "coast" | "shoal" | "name" => {}
            other => return Err(format!("unknown key `{other}`")),
        }
    }
    token.ok_or_else(|| "no token".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_book() -> Logbook {
        Logbook {
            path: None,
            token: Token(0x00C0_FFEE_0000_0007),
        }
    }

    #[test]
    fn a_logbook_survives_the_round_trip() {
        let book = a_book();
        let token = parse(&compose(&book)).expect("parse what was composed");
        assert_eq!(token, book.token);
    }

    #[test]
    fn the_keys_of_an_older_book_are_read_past() {
        // A format-1 book from before the berth moved to the server, before
        // the survey followed it, and before a name became something a claim
        // carries: what this build has outgrown is passed over rather than
        // refused, because the token on the second line is this player's name
        // in that world and losing it would make them a stranger there.
        let token = parse(concat!(
            "genovesa logbook 1\ntoken 2a\nashore 1 2 3 4 5 6 7\naboard 1.5\n",
            "coast 0 0 o0011fffe\nshoal 0 0 c0a0b\nname 3 4 Skull Rock\n",
        ))
        .expect("an old book still reads");
        assert_eq!(token, Token(0x2A));
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
        ] {
            assert!(parse(text).is_err(), "swallowed {what}");
        }
    }

    #[test]
    fn a_forgotten_world_leaves_no_papers() {
        // What a discarded world costs on this side: the token is the player's
        // name in a place, and the place is gone.
        crate::testing::quarantine_data_dir();
        let world = WorldId(0x0BAD_0F00_D000_0001);
        let book = Logbook {
            path: place_for(world),
            token: Token(7),
        };
        write_down(&book);
        let path = place_for(world).expect("a place to keep it");
        assert!(path.exists(), "the book was never written");

        forget(world);
        assert!(!path.exists(), "the book outlived the world");
        // And a world nobody kept a book for is forgotten quietly.
        forget(WorldId(0x0BAD_0F00_D000_0002));
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
