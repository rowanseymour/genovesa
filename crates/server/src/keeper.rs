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
use protocol::{BeastKind, BoatId, BoatKind, Token, WorldId};

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
    /// What the world knows of each player it has dealt papers to — see
    /// [`PlayerRecord`].
    pub players: HashMap<Token, PlayerRecord>,
    /// Every boat there is. Boats are world entities with lasting names —
    /// see [`protocol::BoatId`] — so unlike the beasts this is not a
    /// summary of a performance but the roster itself.
    pub boats: Vec<BoatRecord>,
    /// The beasts alive when the world was last written — see
    /// [`BeastRecord`], and `beasts` for how the warden takes them back up.
    pub beasts: Vec<BeastRecord>,
}

/// Where the world last saw one player, and whether they were at a helm.
///
/// `aboard` names a boat in [`WorldRecord::boats`], and it is a memory
/// rather than a hold: boats have keepers, not owners, so on return the
/// player is seated back only if the boat still lies free where they left
/// it — see the join in `lib.rs` for what happens when it does not.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PlayerRecord {
    pub position: Vec2,
    pub aboard: Option<BoatId>,
}

/// One boat, as the file keeps it. No occupant: who is aboard is session
/// state — everyone aboard anything steps out of the record when the world
/// stops, and where they step back in is the players' own records' business.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BoatRecord {
    pub id: BoatId,
    pub kind: BoatKind,
    pub position: Vec2,
    pub heading: f32,
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
            boats: Vec::new(),
            beasts: Vec::new(),
        }
    }
}

/// One beast, as the file remembers it: what it is, where it stood, where it
/// was going if it was going anywhere, and how much life it had left.
///
/// Deliberately no more than that. Everything else about a beast — its
/// stance toward the boats around it, its breath, its wander — is re-derived
/// every beat and reads the same re-derived, so writing it down would couple
/// the file to the warden's internals for nothing anyone could see. What
/// *can* be seen is a shark still patrolling the shallows a player quit out
/// of, and that is the fact this record exists to keep: nothing with
/// consequence may be escapable by relogging.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BeastRecord {
    pub kind: BeastKind,
    pub position: Vec2,
    /// The one place it was going — `None` for a beast that had arrived and
    /// was living where it stood.
    pub goal: Option<Vec2>,
    /// Beats of life it had left. A restored beast starts its count afresh
    /// from this, so a life is one life however many sessions it spans.
    pub left: u32,
}

/// A kind as the file spells it — and [`kind_of`] reads it back. Words
/// rather than the wire's bytes because this file is read by people.
fn word_of(kind: BeastKind) -> &'static str {
    match kind {
        BeastKind::Shark => "shark",
        BeastKind::Dolphins => "dolphins",
        BeastKind::Whale => "whale",
    }
}

fn kind_of(word: &str) -> Option<BeastKind> {
    match word {
        "shark" => Some(BeastKind::Shark),
        "dolphins" => Some(BeastKind::Dolphins),
        "whale" => Some(BeastKind::Whale),
        _ => None,
    }
}

/// The boats' own spellings, on the beasts' terms.
fn hull_word_of(kind: BoatKind) -> &'static str {
    match kind {
        BoatKind::Sloop => "sloop",
    }
}

fn hull_kind_of(word: &str) -> Option<BoatKind> {
    match word {
        "sloop" => Some(BoatKind::Sloop),
        _ => None,
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
    for (token, player) in players {
        let _ = write!(
            out,
            "player {:016x} {} {}",
            token.0, player.position.x, player.position.y
        );
        if let Some(boat) = player.aboard {
            let _ = write!(out, " {:016x}", boat.0);
        }
        let _ = writeln!(out);
    }
    let mut boats = record.boats.clone();
    boats.sort_by_key(|boat| boat.id.0);
    for boat in boats {
        let _ = writeln!(
            out,
            "boat {:016x} {} {} {} {}",
            boat.id.0,
            hull_word_of(boat.kind),
            boat.position.x,
            boat.position.y,
            boat.heading
        );
    }
    // Sorted likewise — by kind and then by where they stood, the bits
    // standing in for an order nobody reads but everybody can reproduce.
    let mut beasts = record.beasts.clone();
    beasts.sort_by_key(|beast| {
        (
            word_of(beast.kind),
            beast.position.x.to_bits(),
            beast.position.y.to_bits(),
        )
    });
    for beast in beasts {
        let _ = write!(
            out,
            "beast {} {} {} {}",
            word_of(beast.kind),
            beast.position.x,
            beast.position.y,
            beast.left
        );
        if let Some(goal) = beast.goal {
            let _ = write!(out, " {} {}", goal.x, goal.y);
        }
        let _ = writeln!(out);
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
    let mut boats = Vec::new();
    let mut beasts = Vec::new();
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
                let aboard = fields.next().map(hex).transpose()?.map(BoatId);
                if fields.next().is_some() {
                    return Err(format!("too much about one player: `{line}`"));
                }
                let position = Vec2::new(finite(x)?, finite(y)?);
                if !crate::reachable(position) {
                    return Err(format!("nobody was ever at {position}"));
                }
                players.insert(Token(hex(token)?), PlayerRecord { position, aboard });
            }
            "boat" => {
                let mut fields = value.split(' ');
                let (id, kind, x, y, heading) = (
                    fields.next().ok_or("a boat with no name")?,
                    fields.next().ok_or("a boat of no kind")?,
                    fields.next().ok_or("a boat with no position")?,
                    fields.next().ok_or("a boat with half a position")?,
                    fields.next().ok_or("a boat with no heading")?,
                );
                if fields.next().is_some() {
                    return Err(format!("too much about one boat: `{line}`"));
                }
                let position = Vec2::new(finite(x)?, finite(y)?);
                if !crate::reachable(position) {
                    return Err(format!("no boat ever lay at {position}"));
                }
                boats.push(BoatRecord {
                    id: BoatId(hex(id)?),
                    kind: hull_kind_of(kind).ok_or_else(|| format!("no such boat as a {kind}"))?,
                    position,
                    heading: finite(heading)?,
                });
            }
            "beast" => {
                let fields: Vec<&str> = value.split(' ').collect();
                let ([kind, x, y, left], goal) = (
                    fields
                        .get(..4)
                        .and_then(|head| <[&str; 4]>::try_from(head).ok())
                        .ok_or_else(|| format!("half a beast: `{line}`"))?,
                    &fields[4.min(fields.len())..],
                );
                let goal = match goal {
                    [] => None,
                    [gx, gy] => Some(Vec2::new(finite(gx)?, finite(gy)?)),
                    _ => return Err(format!("too much about one beast: `{line}`")),
                };
                let position = Vec2::new(finite(x)?, finite(y)?);
                for spot in goal.iter().chain([&position]) {
                    if !crate::reachable(*spot) {
                        return Err(format!("no beast ever swam at {spot}"));
                    }
                }
                beasts.push(BeastRecord {
                    kind: kind_of(kind).ok_or_else(|| format!("no such beast as a {kind}"))?,
                    position,
                    goal,
                    left: left
                        .parse::<u32>()
                        .map_err(|_| format!("`{left}` is not a count of beats"))?,
                });
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
        boats,
        beasts,
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
                (
                    Token(7),
                    PlayerRecord {
                        position: Vec2::new(12.5, -340.25),
                        aboard: Some(BoatId(0xB0A7)),
                    },
                ),
                (
                    Token(0xFFFF_0000_0000_0002),
                    PlayerRecord {
                        position: Vec2::new(-0.125, 9000.0),
                        aboard: None,
                    },
                ),
            ]),
            boats: vec![
                BoatRecord {
                    id: BoatId(0xB0A7),
                    kind: BoatKind::Sloop,
                    position: Vec2::new(12.5, -340.25),
                    heading: 1.5,
                },
                BoatRecord {
                    id: BoatId(0xDEAD),
                    kind: BoatKind::Sloop,
                    position: Vec2::new(64.0, 8.0),
                    heading: -2.25,
                },
            ],
            beasts: vec![
                // One living where it stands, one still bound somewhere.
                BeastRecord {
                    kind: BeastKind::Shark,
                    position: Vec2::new(40.0, -12.5),
                    goal: None,
                    left: 900,
                },
                BeastRecord {
                    kind: BeastKind::Whale,
                    position: Vec2::new(-800.0, 2_000.0),
                    goal: Some(Vec2::new(200.0, 1_500.0)),
                    left: 4_000,
                },
            ],
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
        // Composing sorts the boats and the beasts, so compare content
        // rather than order.
        let boats_sorted = |mut boats: Vec<BoatRecord>| {
            boats.sort_by_key(|boat| boat.id.0);
            boats
        };
        assert_eq!(boats_sorted(read.boats), boats_sorted(record.boats.clone()));
        let sorted = |mut beasts: Vec<BeastRecord>| {
            beasts.sort_by_key(|beast| (word_of(beast.kind), beast.position.x.to_bits()));
            beasts
        };
        assert_eq!(sorted(read.beasts), sorted(record.beasts.clone()));
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
            (
                "genovesa world 1\nid 1\nseed 7\nopening 0.35\nage 0\nbeast shark 1 2\n",
                "half a beast",
            ),
            (
                "genovesa world 1\nid 1\nseed 7\nopening 0.35\nage 0\nbeast kraken 1 2 3\n",
                "a beast of a kind nothing keeps",
            ),
            (
                "genovesa world 1\nid 1\nseed 7\nopening 0.35\nage 0\nbeast shark 1 2 3 4\n",
                "half a goal",
            ),
            (
                "genovesa world 1\nid 1\nseed 7\nopening 0.35\nage 0\nbeast whale 1e30 0 5\n",
                "a beast past where the world resolves",
            ),
            (
                "genovesa world 1\nid 1\nseed 7\nopening 0.35\nage 0\nboat 1 sloop 1 2\n",
                "a boat with no heading",
            ),
            (
                "genovesa world 1\nid 1\nseed 7\nopening 0.35\nage 0\nboat 1 canoe 1 2 3\n",
                "a boat of a kind nothing sails",
            ),
            (
                "genovesa world 1\nid 1\nseed 7\nopening 0.35\nage 0\nplayer 1 1 2 nothex\n",
                "an aboard that is not a boat's name",
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
