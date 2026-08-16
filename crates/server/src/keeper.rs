//! Keeping a world: the file it survives in, and the lock that keeps it one
//! world at a time.
//!
//! Nearly everything a session shows is not in the file, because nearly
//! everything a session shows is not state: the terrain, the palms and the
//! weather are functions of the seed and the world's age, and the beasts are
//! raised around whoever is present. What is left — what cannot be re-derived
//! and so is the whole of what a world *is* beyond its seed — fits in a few
//! lines: which world this is, how old it is, what it is called, and where it
//! last saw each player it has dealt papers to. The seed is the geography;
//! this file is the history.
//!
//! The format is plain text, one `key value` per line under a versioned
//! header, written sorted so that two saves of one state are byte-identical.
//! Text because the whole file is smaller than one chunk of ground and will
//! be looked at by people — moved between machines, backed up, read when
//! something seems wrong — and a format version rather than tolerant parsing
//! because a build that half-understands a file should refuse it whole, not
//! quietly drop the half it never heard of.
//!
//! Writes are atomic — composed beside the file and renamed over it — and the
//! previous version survives as `.old`, which [`load`] falls back to: the
//! failure being bought off is a machine losing power mid-write, which must
//! never cost the world. The lock is the OS's own advisory file lock, held on
//! a `.lock` sibling for the life of the [`Keeper`]: it dies with the process,
//! so a crash cannot leave a world wrongly barred, and it is on a sibling
//! rather than the file itself because the file is replaced at every save and
//! a lock rides the file it was taken on, not the name. What it prevents is
//! two processes hosting one world at once, each saving over the other —
//! two histories written to one name, which is the quiet way to lose one.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use glam::Vec2;
use protocol::{Token, WorldId};

/// The format this build writes, named in the file's first line. A file
/// carrying a different number is refused whole rather than guessed at —
/// see the module doc for why.
const FORMAT: u32 = 1;

/// The extension a kept world's file carries, so a directory of them can be
/// told from whatever else ends up alongside. Public through
/// [`kept_worlds`], which is the reader; the game names files through
/// [`crate::Server::keeping_in`], which is the writer.
const EXTENSION: &str = "world";

/// Everything a world is beyond its seed: the state a session accumulates
/// that no seed can re-derive. What [`load`] reads and [`Keeper::save`]
/// writes, and — through `Shared` — the shape a running session summarises
/// itself into.
pub(crate) struct WorldRecord {
    pub id: WorldId,
    pub seed: u32,
    /// What the world is called on the screens that list worlds. Empty for a
    /// world nobody has named, which is every world a dedicated server makes.
    pub name: String,
    /// The phase of the day the clock read at age zero — see
    /// [`crate::OPENING`].
    pub opening: f32,
    /// World-seconds lived, over all the world's sessions: time served, plus
    /// every night run off and every hour the console skipped. The clock and
    /// the weather are both functions of it, which is what makes it the one
    /// number that keeps a reopened world mid-story: the sun stands where it
    /// stood, and a gale quit out of is a gale returned to.
    pub age: f32,
    /// Where the world last saw each player it has dealt papers to.
    pub players: HashMap<Token, Vec2>,
}

impl WorldRecord {
    /// A world that has just been made and has no history yet.
    pub(crate) fn fresh(seed: u32) -> Self {
        Self {
            id: WorldId(mint()),
            seed,
            name: String::new(),
            opening: crate::OPENING,
            age: 0.0,
            players: HashMap::new(),
        }
    }
}

/// A held world file: the path saves go to, and the lock that says this
/// process is the one world writing there. Dropping it releases the lock,
/// as does dying — it is the OS's, not a file that could go stale — but a
/// session's end does not wait for the drop: see [`Keeper::release`].
pub(crate) struct Keeper {
    path: PathBuf,
    /// The lock rides this handle; holding the handle is holding the lock.
    lock: File,
}

impl Keeper {
    /// Takes hold of the world at `path` — creating the directories on the
    /// way to it if the path is new — or says who cannot: a lock already held
    /// means another process is hosting this world right now, and two hosts
    /// would write two histories to one name.
    pub(crate) fn hold(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let lock = OpenOptions::new()
            .create(true)
            .write(true)
            .open(sibling(path, "lock"))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "this world is already being hosted",
                ));
            }
            Err(TryLockError::Error(error)) => return Err(error),
        }
        Ok(Self {
            path: path.to_path_buf(),
            lock,
        })
    }

    /// Lets the world go before the keeper itself is gone. The keeper lives
    /// in the session's shared state, which the worker and sky threads keep
    /// alive for a beat past the session's end — and a player who leaves a
    /// world and immediately reopens it must not lose that race to their own
    /// session's shadow. Called after the closing save, inside what a
    /// leaving host waits for; nothing saves after it.
    pub(crate) fn release(&self) {
        let _ = self.lock.unlock();
    }

    /// Writes the world down: composed whole beside the file, made durable,
    /// and renamed into place, with the version being replaced surviving as
    /// `.old`. At no instant is the name pointing at half a world.
    pub(crate) fn save(&self, record: &WorldRecord) -> io::Result<()> {
        let fresh = sibling(&self.path, "new");
        {
            let mut file = File::create(&fresh)?;
            io::Write::write_all(&mut file, compose(record).as_bytes())?;
            // Made durable before it is named, or the rename could land a
            // file whose bytes were still nowhere when the power went.
            file.sync_all()?;
        }
        match fs::rename(&self.path, sibling(&self.path, "old")) {
            Ok(()) => {}
            // The first save has nothing to back up.
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        fs::rename(&fresh, &self.path)
    }
}

/// Reads the world at `path`, falling back to its `.old` if the file itself
/// is missing or will not parse — the crash being recovered from is one
/// mid-[`Keeper::save`], where the newest good version is the backup.
pub(crate) fn load(path: &Path) -> io::Result<WorldRecord> {
    let newest = fs::read_to_string(path)
        .map_err(|error| error.to_string())
        .and_then(|text| parse(&text));
    match newest {
        Ok(record) => Ok(record),
        Err(why) => match fs::read_to_string(sibling(path, "old")) {
            Ok(text) => parse(&text).map_err(corrupt),
            // The backup being no help, the original complaint is the
            // useful one.
            Err(_) => Err(corrupt(why)),
        },
    }
}

/// One world in a directory listing: what a screen offering kept worlds
/// shows, and the path that reopens the one chosen.
pub struct KeptWorld {
    pub path: PathBuf,
    /// Empty for a world nobody has named.
    pub name: String,
    /// World-seconds lived — see [`WorldRecord::age`]. What a listing turns
    /// into a day count, that being the one fact about a world's story its
    /// file can tell without naming any machinery.
    pub age: f32,
    /// When the world was last written — which, saves being periodic and one
    /// closing the session, is when it was last sailed.
    pub kept: SystemTime,
}

/// The worlds kept in `dir`, newest-sailed first. A directory that does not
/// exist holds no worlds, and a file that will not parse is passed over
/// rather than sinking the list — the world it fails to name is still there
/// for a reopen to complain about properly.
pub fn kept_worlds(dir: &Path) -> Vec<KeptWorld> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut worlds: Vec<KeptWorld> = entries
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension().is_none_or(|ext| ext != EXTENSION) {
                return None;
            }
            let record = load(&path).ok()?;
            let kept = fs::metadata(&path)
                .and_then(|meta| meta.modified())
                .unwrap_or(UNIX_EPOCH);
            Some(KeptWorld {
                path,
                name: record.name,
                age: record.age,
                kept,
            })
        })
        .collect();
    worlds.sort_by(|a, b| b.kept.cmp(&a.kept));
    worlds
}

/// Where this machine keeps what outlives a run: the platform's own place
/// for an application's data, with the game's name on it. `None` on a
/// machine so bare it cannot say where home is, which callers treat as
/// "nothing can be kept" rather than inventing a directory.
///
/// `GENOVESA_DATA` overrides the lot — which is how tests keep their worlds
/// out of the player's real directory, and how a deployment that wants its
/// files somewhere particular asks.
pub fn data_dir() -> Option<PathBuf> {
    if let Some(overridden) = std::env::var_os("GENOVESA_DATA") {
        if !overridden.is_empty() {
            return Some(PathBuf::from(overridden));
        }
    }
    #[cfg(target_os = "macos")]
    return std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Library/Application Support/Genovesa"));

    #[cfg(target_os = "windows")]
    return std::env::var_os("APPDATA").map(|appdata| PathBuf::from(appdata).join("Genovesa"));

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
            if !xdg.is_empty() {
                return Some(PathBuf::from(xdg).join("genovesa"));
            }
        }
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share/genovesa"))
    }
}

/// A fresh 64-bit identity: what world ids and player tokens are minted
/// from. The clock counted into rather than trusted — not every platform's
/// has nanoseconds in it, and two mints in one tick must still differ — and
/// the whole thing stirred so that consecutive draws share no visible
/// pattern. Tokens double as proof of being a returning player, and this is
/// as much unguessability as that is owed in a game among people who chose
/// each other: a stranger cannot stumble into one, and a friend determined
/// to impersonate a friend has easier ways in.
pub(crate) fn mint() -> u64 {
    /// Weyl increment: odd, so successive draws differ in every draw, and
    /// the constant splitmix64 was published with.
    static DRAWN: AtomicU64 = AtomicU64::new(0);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos() as u64)
        .unwrap_or(0);
    let counted = DRAWN.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
    // The process id as well, because the counter only separates mints
    // within one process and not every platform's clock has nanoseconds in
    // it: two servers started in the same coarse tick must still make two
    // worlds. Shifted up so it perturbs bits the tick's own arithmetic
    // leaves quiet.
    let process = (std::process::id() as u64) << 32;

    // splitmix64's finaliser, which spreads a difference in any input bit
    // over the whole answer.
    let mut mixed = nanos.wrapping_add(counted) ^ process;
    mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    mixed ^ (mixed >> 31)
}

/// `path` with a further extension on it: `foo.world` → `foo.world.old`.
/// Appended rather than swapped so every file of one world sorts and greps
/// together.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".");
    name.push(suffix);
    PathBuf::from(name)
}

fn compose(record: &WorldRecord) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "genovesa world {FORMAT}");
    let _ = writeln!(out, "id {}", record.id);
    let _ = writeln!(out, "seed {}", record.seed);
    if !record.name.is_empty() {
        let _ = writeln!(out, "name {}", record.name);
    }
    let _ = writeln!(out, "opening {}", record.opening);
    let _ = writeln!(out, "age {}", record.age);
    // Sorted, so that one state is one file, byte for byte, whatever order
    // a map hands its entries out in.
    let mut players: Vec<_> = record.players.iter().collect();
    players.sort_by_key(|(token, _)| token.0);
    for (token, position) in players {
        let _ = writeln!(out, "player {:016x} {} {}", token.0, position.x, position.y);
    }
    out
}

fn parse(text: &str) -> Result<WorldRecord, String> {
    let mut lines = text.lines();
    match lines.next() {
        Some(header) if header == format!("genovesa world {FORMAT}") => {}
        Some(other) => return Err(format!("not a world this build keeps: `{other}`")),
        None => return Err("an empty file".to_string()),
    }

    let (mut id, mut seed, mut opening, mut age) = (None, None, None, None);
    let mut name = String::new();
    let mut players = HashMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (key, value) = line
            .split_once(' ')
            .ok_or_else(|| format!("a line with no value: `{line}`"))?;
        match key {
            "id" => id = Some(WorldId(hex(value)?)),
            "seed" => {
                seed = Some(
                    value
                        .parse::<u32>()
                        .map_err(|_| format!("`{value}` is not a seed"))?,
                )
            }
            "name" => name = value.to_string(),
            "opening" => opening = Some(finite(value)?),
            "age" => age = Some(finite(value)?),
            "player" => {
                let mut fields = value.split(' ');
                let (token, x, y) = (
                    fields.next().ok_or("a player with no token")?,
                    fields.next().ok_or("a player with no position")?,
                    fields.next().ok_or("a player with half a position")?,
                );
                if fields.next().is_some() {
                    return Err(format!("too much about one player: `{line}`"));
                }
                let position = Vec2::new(finite(x)?, finite(y)?);
                if !crate::reachable(position) {
                    return Err(format!("nobody was ever at {position}"));
                }
                players.insert(Token(hex(token)?), position);
            }
            other => return Err(format!("unknown key `{other}`")),
        }
    }

    let opening = opening.ok_or("no opening")?;
    let age = age.ok_or("no age")?;
    if !(0.0..1.0).contains(&opening) {
        return Err(format!("{opening} is not a phase of the day"));
    }
    if age < 0.0 {
        return Err(format!("a world cannot be {age} seconds old"));
    }
    Ok(WorldRecord {
        id: id.ok_or("no id")?,
        seed: seed.ok_or("no seed")?,
        name,
        opening,
        age,
        players,
    })
}

fn hex(value: &str) -> Result<u64, String> {
    u64::from_str_radix(value, 16).map_err(|_| format!("`{value}` is not sixteen hex digits"))
}

fn finite(value: &str) -> Result<f32, String> {
    value
        .parse::<f32>()
        .ok()
        .filter(|parsed| parsed.is_finite())
        .ok_or_else(|| format!("`{value}` is not a number"))
}

fn corrupt(why: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of this test's own under the system's temporary space,
    /// so parallel tests cannot see each other's worlds.
    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("genovesa-keeper-{:016x}", mint()));
        fs::create_dir_all(&dir).expect("temp space");
        dir
    }

    fn a_record() -> WorldRecord {
        WorldRecord {
            id: WorldId(0x00C0_FFEE_0000_0001),
            seed: 20_040_112,
            name: "Windward Reach".to_string(),
            opening: 0.35,
            age: 1234.5,
            players: HashMap::from([
                (Token(7), Vec2::new(12.5, -340.25)),
                (Token(0xFFFF_0000_0000_0002), Vec2::new(-0.125, 9000.0)),
            ]),
        }
    }

    #[test]
    fn a_world_survives_the_round_trip() {
        let record = a_record();
        let read = parse(&compose(&record)).expect("parse what was composed");
        assert_eq!(read.id, record.id);
        assert_eq!(read.seed, record.seed);
        assert_eq!(read.name, record.name);
        assert_eq!(read.opening, record.opening);
        assert_eq!(read.age, record.age);
        assert_eq!(read.players, record.players);
    }

    #[test]
    fn one_state_is_one_file() {
        // Byte for byte, however the map orders itself today: a save is
        // diffable against another save of the same moment.
        assert_eq!(compose(&a_record()), compose(&a_record()));
    }

    #[test]
    fn what_is_not_a_world_is_refused() {
        for (text, what) in [
            ("", "an empty file"),
            ("genovesa world 999\n", "a format from some other year"),
            ("genovesa world 1\nseed 7\nopening 0.35\nage 0\n", "no id"),
            (
                "genovesa world 1\nid 1\nseed 7\nopening 0.35\nage 0\nfuture stuff\n",
                "a key this build has never heard of",
            ),
            (
                "genovesa world 1\nid 1\nseed 7\nopening 2.5\nage 0\n",
                "an opening past the day",
            ),
            (
                "genovesa world 1\nid 1\nseed 7\nopening 0.35\nage -4\n",
                "a negative age",
            ),
            (
                "genovesa world 1\nid 1\nseed 7\nopening 0.35\nage NaN\n",
                "an age that is not a number",
            ),
            (
                "genovesa world 1\nid 1\nseed 7\nopening 0.35\nage 0\nplayer 1 1e30 0\n",
                "a player past where the world resolves",
            ),
        ] {
            assert!(parse(text).is_err(), "swallowed {what}");
        }
    }

    #[test]
    fn a_save_interrupted_is_a_world_recovered() {
        let dir = scratch();
        let path = dir.join("one.world");
        let keeper = Keeper::hold(&path).expect("hold");
        let mut record = a_record();
        keeper.save(&record).expect("first save");
        record.age = 9999.0;
        keeper.save(&record).expect("second save");

        // The newest version is what loads...
        assert_eq!(load(&path).expect("load").age, 9999.0);

        // ...and a file half-written when the power went — any torn byte
        // will do — falls back to the version before it.
        fs::write(&path, "genovesa wor").expect("tear");
        let recovered = load(&path).expect("the backup stands in");
        assert_eq!(recovered.age, 1234.5);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_world_is_kept_by_one_process_at_a_time() {
        let dir = scratch();
        let path = dir.join("one.world");
        let held = Keeper::hold(&path).expect("first hold");
        let refused = Keeper::hold(&path);
        assert!(
            refused.is_err(),
            "two keepers would write two histories to one name"
        );

        // And letting go is enough — no stale lock outlives a session.
        drop(held);
        assert!(Keeper::hold(&path).is_ok(), "the lock outlived its keeper");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_directory_lists_its_worlds_and_nothing_else() {
        let dir = scratch();
        for (file, age) in [("a.world", 1.0), ("b.world", 2.0)] {
            let keeper = Keeper::hold(&dir.join(file)).expect("hold");
            keeper
                .save(&WorldRecord { age, ..a_record() })
                .expect("save");
        }
        fs::write(dir.join("notes.txt"), "not a world").expect("write");
        fs::write(dir.join("torn.world"), "genovesa wor").expect("write");

        let listed = kept_worlds(&dir);
        assert_eq!(listed.len(), 2, "the listing miscounted the worlds");
        assert!(listed.iter().all(|world| world.name == "Windward Reach"));

        // And nowhere at all is simply no worlds.
        assert!(kept_worlds(&dir.join("nowhere")).is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn mints_do_not_repeat() {
        let minted: Vec<u64> = (0..64).map(|_| mint()).collect();
        for (i, coin) in minted.iter().enumerate() {
            assert!(!minted[i + 1..].contains(coin), "two mints came out alike");
        }
    }
}
