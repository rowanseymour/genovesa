//! Keeping a world: the file it survives in, and the lock that keeps it one
//! world at a time.
//!
//! Nearly everything a session shows is not in the file, because nearly
//! everything a session shows is not state: the terrain, the palms and the
//! weather are functions of the seed and the world's age, and the beasts are
//! raised around whoever is present. What is left — what cannot be re-derived
//! and so is the whole of what a world *is* beyond its seed — is short: which
//! world this is, how old it is, what it is called, where it last saw each
//! player it has dealt papers to, which ground each of them has been near
//! enough to survey, and which islands have been claimed and by whom. The seed
//! is the geography; this file is the history.
//!
//! A survey is the one part of it with any size to it — a voyage is thousands
//! of chunks — and only the *coordinates* are written. What was found on them
//! is ink, and ink is derived: the ground is a function of the seed, so the
//! world can work the coastline out again in the time it takes to read the
//! file. What no seed can say is where somebody went.
//!
//! The format is plain text, one `key value` per line under a versioned
//! header, written sorted so that two saves of one state are byte-identical.
//! Text because the file is small — a well-sailed world is tens of kilobytes,
//! against a single chunk of ground's sixteen — and will be looked at by
//! people: moved between machines, backed up, read when something seems
//! wrong. A format version rather than tolerant parsing, because a build that
//! half-understands a file should refuse it whole, not quietly drop the half
//! it never heard of.
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

use glam::{IVec2, Vec2};
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
    /// Every island anybody has claimed — see [`ClaimRecord`]. Of everything
    /// in this file it is the part that has to be kept most carefully: an
    /// island claimed is a thing another player is barred from, and a world
    /// that forgot one would hand somebody's place to the next comer.
    pub claims: Vec<ClaimRecord>,
    /// The beasts alive when the world was last written — see
    /// [`BeastRecord`], and `beasts` for how the warden takes them back up.
    pub beasts: Vec<BeastRecord>,
}

/// Where the world last saw one player, whether they were at a helm, and what
/// ground they have surveyed.
///
/// `aboard` names a boat in [`WorldRecord::boats`], and it is a memory
/// rather than a hold: boats have keepers, not owners, so on return the
/// player is seated back only if the boat still lies free where they left
/// it — see the join in `lib.rs` for what happens when it does not.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PlayerRecord {
    pub position: Vec2,
    pub aboard: Option<BoatId>,
    /// Every chunk this player has been near enough to look at — the record
    /// of where they have been, and the whole of what a claim will one day
    /// be judged against. Coordinates only: what is *on* them the world works
    /// out again on the way in — see the module doc.
    pub surveyed: Vec<IVec2>,
    /// What this player has found out about other people's claims, by the
    /// island each stands for — see [`crate::Knowing`].
    ///
    /// Kept whole where the survey is kept as coordinates, and that is not an
    /// inconsistency. A survey can be re-earned from the ground it was taken
    /// over; knowing that somebody's cairn stands on a headland is a fact about
    /// a *visit*, and there is nowhere to work it out from again. Lose it and
    /// the player has to go back and look.
    ///
    /// A map and not a list of pairs, because the file cannot say two things
    /// about one island and neither may the record it is written from — see
    /// [`compose`], which writes a line per depth. A shape that could hold the
    /// contradiction would be a shape a save could carry into a world that
    /// then would not open.
    pub known: HashMap<IVec2, crate::Knowing>,
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

/// One island claimed, as the file keeps it: which island, whose it is, where
/// their cairn stands, and what they have christened it.
///
/// The island is named by the identity of its ring — see
/// [`protocol::survey::Island::id`] — which is a point on the mark lattice and
/// so a pair of whole numbers that mean the same thing in every session of
/// this world. `by` is a [`Token`], the same one the player's own line is
/// filed under: a claim belongs to whoever holds those papers, and outlives
/// every visit.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ClaimRecord {
    pub island: IVec2,
    pub by: Token,
    pub at: Vec2,
    /// Empty for an island nobody has christened. Free text, which is why it
    /// goes last on its line — see [`compose`].
    pub name: String,
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
            claims: Vec::new(),
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
            // Not truncated: the file is a lock and nothing more, so its
            // contents are nobody's business — least of all this process's,
            // which may be about to learn another process holds it.
            .truncate(false)
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
    /// Which world this is — the name the client keeps its own papers for the
    /// place under, so that discarding a world can take them with it.
    pub id: WorldId,
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
                id: record.id,
                name: record.name,
                age: record.age,
                kept,
            })
        })
        .collect();
    worlds.sort_by_key(|world| std::cmp::Reverse(world.kept));
    worlds
}

/// Throws a kept world away: the file, the backup behind it, and the lock
/// beside it, so nothing of the place is left to list or to recover.
///
/// Held before it is deleted, and for the same reason hosting holds it: a
/// world open in another process is a history still being written, and taking
/// the file out from under it would leave that session saving to a name
/// nobody will ever read again. A world being hosted therefore refuses to be
/// discarded, in the words [`Keeper::hold`] refuses in.
///
/// The lock goes last, after the hold is dropped: on Windows a file still
/// open cannot be deleted, and the hold is what has it open.
pub fn discard(path: &Path) -> io::Result<()> {
    let held = Keeper::hold(path)?;
    let mut outcome = remove(path);
    for suffix in ["old", "new"] {
        outcome = outcome.and(remove(&sibling(path, suffix)));
    }
    drop(held);
    outcome.and(remove(&sibling(path, "lock")))
}

/// Deletes a file, counting one that was never there as done — a world saved
/// only once has no `.old`, and a discard must not complain about the file it
/// was spared.
fn remove(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        outcome => outcome,
    }
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
        // A line of its own, because a voyage is thousands of chunks and a
        // player's own line should stay a line somebody can read. Sorted, on
        // the terms everything else here is: one state, one file.
        //
        // Written as columns — `x:z` and then how far north each further
        // chunk of that column stands from the one before it — because what
        // a survey actually is is a swath, and a swath sorted this way is
        // runs of neighbours: nearly every step is a `1`. It costs a couple
        // of characters a chunk where naming each in full costs a dozen, and
        // it is still a line a person can read. Deliberately one number per
        // chunk rather than first-and-last: a range would let a short file
        // ask for an enormous survey, and nothing here should be able to
        // grow in the reading.
        if !player.surveyed.is_empty() {
            let mut surveyed = player.surveyed.clone();
            surveyed.sort_by_key(|chunk| (chunk.x, chunk.y));
            surveyed.dedup();
            let _ = write!(out, "surveyed {:016x}", token.0);
            let mut column: Option<IVec2> = None;
            for chunk in surveyed {
                match column {
                    Some(before) if before.x == chunk.x => {
                        let _ = write!(out, ",{}", chunk.y - before.y);
                    }
                    _ => {
                        let _ = write!(out, " {}:{}", chunk.x, chunk.y);
                    }
                }
                column = Some(chunk);
            }
            let _ = writeln!(out);
        }
        // And what they know of other people's cairns, a line to each depth of
        // knowing — see [`crate::Knowing`]. Two keys rather than one line with
        // a word against every island, because there are only ever two answers
        // and a file that says `sighted` and `visited` in so many words needs
        // no key to read it by. Sorted, on the terms everything else here is.
        for (key, depth) in [
            ("sighted", crate::Knowing::Sighted),
            ("visited", crate::Knowing::Visited),
        ] {
            let mut known: Vec<IVec2> = player
                .known
                .iter()
                .filter(|(_, knowing)| **knowing == depth)
                .map(|(island, _)| *island)
                .collect();
            if known.is_empty() {
                continue;
            }
            known.sort_by_key(|island| (island.x, island.y));
            let _ = write!(out, "{key} {:016x}", token.0);
            for island in known {
                let _ = write!(out, " {}:{}", island.x, island.y);
            }
            let _ = writeln!(out);
        }
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
    // Sorted by the island claimed, which is the one field of a claim that
    // cannot repeat: an island is claimed once or not at all.
    let mut claims = record.claims.clone();
    claims.sort_by_key(|claim| (claim.island.x, claim.island.y));
    for claim in claims {
        let _ = write!(
            out,
            "claim {} {} {:016x} {} {}",
            claim.island.x, claim.island.y, claim.by.0, claim.at.x, claim.at.y
        );
        // The name last, and filtered on the way out as it is on the way in:
        // it is the one field here somebody typed, and a line of this file has
        // to stay a line. Nothing that reaches this should need the filter —
        // the wire refuses a name with a control character in it — but the
        // format's promise that it can be read back should not rest on
        // somebody else's check.
        let name = filtered(&claim.name);
        if !name.is_empty() {
            let _ = write!(out, " {name}");
        }
        let _ = writeln!(out);
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
    // Held aside rather than written straight into the record they belong to,
    // because a file may name a player's survey before it names the player —
    // this one never does, but nothing else here depends on line order and
    // this must not be the exception.
    let mut surveys: HashMap<Token, Vec<IVec2>> = HashMap::new();
    // And likewise what each of them knows of other people's cairns, gathered
    // across the two lines that can say it — see [`compose`].
    let mut knowings: HashMap<Token, HashMap<IVec2, crate::Knowing>> = HashMap::new();
    let mut boats = Vec::new();
    let mut claims: Vec<ClaimRecord> = Vec::new();
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
                // Once each, on the same terms the surveys are: a file
                // naming one player twice cannot say which of the two the
                // world went on with.
                let told = players.insert(
                    Token(hex(token)?),
                    PlayerRecord {
                        position,
                        aboard,
                        surveyed: Vec::new(),
                        known: HashMap::new(),
                    },
                );
                if told.is_some() {
                    return Err(format!("one player told twice: `{line}`"));
                }
            }
            // Columns of chunks, each `x:z` and then the steps north from
            // there — see [`compose`] for the shape and why. Read strictly:
            // the columns ascend, the steps never stand still or go back,
            // and so a file that reads at all says every chunk once and
            // says them in the one order a save would have written.
            "surveyed" => {
                let (token, columns) = value
                    .split_once(' ')
                    .ok_or_else(|| format!("a survey of nowhere: `{line}`"))?;
                let mut surveyed = Vec::new();
                let mut previous: Option<i32> = None;
                for column in columns.split(' ') {
                    let (x, steps) = column
                        .split_once(':')
                        .ok_or_else(|| format!("`{column}` is not a column of survey"))?;
                    let x = whole(x)?;
                    if previous.is_some_and(|before| x <= before) {
                        return Err(format!("a survey out of order at `{column}`"));
                    }
                    previous = Some(x);

                    let mut z: Option<i32> = None;
                    for step in steps.split(',') {
                        let step = whole(step)?;
                        let here = match z {
                            None => step,
                            Some(_) if step < 1 => {
                                return Err(format!("a survey that steps nowhere: `{column}`"));
                            }
                            Some(before) => before
                                .checked_add(step)
                                .ok_or_else(|| format!("a survey off the numbers: `{column}`"))?,
                        };
                        z = Some(here);
                        let chunk = IVec2::new(x, here);
                        // The same reach a position is held to, in chunks: an
                        // edited file is the one other door coordinates arrive
                        // through, and ground past where the world resolves is
                        // ground nobody ever looked at.
                        if !crate::in_the_world(chunk) {
                            return Err(format!("nobody ever surveyed {chunk}"));
                        }
                        surveyed.push(chunk);
                    }
                }
                // One line each, because a save writes one line each. Two
                // for a token is a file this build only half understands —
                // it cannot say which of them the world went on with — and
                // half-understood is what this format refuses.
                if surveys.insert(Token(hex(token)?), surveyed).is_some() {
                    return Err(format!("one player's survey told twice: `{line}`"));
                }
            }
            // What one player knows of other people's cairns, an island to a
            // field — see [`crate::Knowing`], and [`compose`] for the two keys.
            // Read strictly, on the survey's terms: the islands ascend, so a
            // file that reads at all names each of them once — and once across
            // *both* lines, which is the rule that matters, an island a player
            // has both only sighted and been up to being a file that cannot
            // say which. Two lines under one key are merged rather than
            // refused, unlike two surveys: they say the same thing between them
            // that one line would have said, and there is nothing to choose.
            "sighted" | "visited" => {
                let depth = match key {
                    "sighted" => crate::Knowing::Sighted,
                    _ => crate::Knowing::Visited,
                };
                let (token, islands) = value
                    .split_once(' ')
                    .ok_or_else(|| format!("a knowing of nothing: `{line}`"))?;
                let known = knowings.entry(Token(hex(token)?)).or_default();
                let mut previous: Option<(i32, i32)> = None;
                for field in islands.split(' ') {
                    let (x, z) = field
                        .split_once(':')
                        .ok_or_else(|| format!("`{field}` is not an island"))?;
                    let island = IVec2::new(whole(x)?, whole(z)?);
                    if previous.is_some_and(|before| (island.x, island.y) <= before) {
                        return Err(format!("a knowing out of order at `{field}`"));
                    }
                    previous = Some((island.x, island.y));
                    // The reach a claim's own identity is held to, and it has
                    // to be the same one: these name the same islands, so a
                    // knowing this refuses would be a world that will not open
                    // over a fact about somebody's chart.
                    if !crate::island_in_the_world(island) {
                        return Err(format!("nobody ever saw a cairn on {island}"));
                    }
                    if known.insert(island, depth).is_some() {
                        return Err(format!("one island known twice over: `{field}`"));
                    }
                }
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
            // Five fields and then the rest of the line, which is the name:
            // free text, so it may hold spaces and is split off last rather
            // than counted among the others.
            "claim" => {
                let mut fields = value.splitn(6, ' ');
                let (x, z, by, ax, az) = (
                    fields.next().ok_or("a claim of no island")?,
                    fields.next().ok_or("a claim of half an island")?,
                    fields.next().ok_or("a claim in nobody's name")?,
                    fields.next().ok_or("a cairn with no position")?,
                    fields.next().ok_or("a cairn with half a position")?,
                );
                let island = IVec2::new(whole(x)?, whole(z)?);
                // The reach everything else here is held to, asked of the chunk
                // the identity stands in — see [`crate::island_in_the_world`],
                // which is the same test the granting end applies. Not of the
                // metres the identity works out to: the lattice is a rounded
                // 255 steps to the chunk, so that arithmetic refuses the outer
                // chunks a survey may honestly reach, and a claim refused here
                // is a whole world that will not open.
                if !crate::island_in_the_world(island) {
                    return Err(format!("nobody ever sailed round {island}"));
                }
                let at = Vec2::new(finite(ax)?, finite(az)?);
                if !crate::reachable(at) {
                    return Err(format!("no cairn ever stood at {at}"));
                }
                // Held to the wire's own rule as well as the file's, so that
                // what a hand-edited file can put on a cairn is what a player
                // could have put there. A name that fails it is dropped and
                // the claim kept: the island is somebody's either way, and a
                // name can be written again.
                let name = fields
                    .next()
                    .map(filtered)
                    .as_deref()
                    .and_then(protocol::island_name)
                    .unwrap_or_default();
                // Once each, on the terms the players are: an island claimed
                // twice is a file that cannot say whose it is.
                if claims
                    .iter()
                    .any(|held: &ClaimRecord| held.island == island)
                {
                    return Err(format!("one island claimed twice: `{line}`"));
                }
                claims.push(ClaimRecord {
                    island,
                    by: Token(hex(by)?),
                    at,
                    name,
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

    // A survey against a token no `player` line named belongs to nobody, and
    // is dropped rather than refused: there is no position to hang it on, and
    // it is ink that can be earned again by going back.
    for (token, surveyed) in surveys {
        if let Some(player) = players.get_mut(&token) {
            player.surveyed = surveyed;
        }
    }
    // And what they knew, dropped on the same terms and for a nearer version
    // of the same reason: there is nobody to have known it.
    for (token, known) in knowings {
        if let Some(player) = players.get_mut(&token) {
            player.known = known;
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
        claims,
        beasts,
    })
}

/// A name as this file will carry it: control characters out, ends trimmed.
///
/// The wire's own rule ([`protocol::island_name`]) is the harder one and
/// refuses such a name outright, which is right for a name somebody is
/// offering. This is the format's rule, and it repairs rather than refuses
/// because what it guards is only that a line stays a line — a world must not
/// become unreadable over a stray byte in a name.
fn filtered(name: &str) -> String {
    name.chars()
        .filter(|letter| !letter.is_control())
        .collect::<String>()
        .trim()
        .to_string()
}

fn hex(value: &str) -> Result<u64, String> {
    u64::from_str_radix(value, 16).map_err(|_| format!("`{value}` is not sixteen hex digits"))
}

fn whole(value: &str) -> Result<i32, String> {
    value
        .parse::<i32>()
        .map_err(|_| format!("`{value}` is not a chunk coordinate"))
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
                        // Out of order and either side of the origin, so a
                        // save that sorted them wrong would still read back.
                        surveyed: vec![IVec2::new(2, -1), IVec2::new(-40, 300), IVec2::new(0, 0)],
                        // Both depths of knowing, and both out of order, for
                        // the same reason — and two islands to a depth, so a
                        // line that could only carry one would be caught.
                        known: HashMap::from([
                            (IVec2::new(2, -1), crate::Knowing::Visited),
                            (IVec2::new(-9, 4), crate::Knowing::Sighted),
                            (IVec2::new(-40, 300), crate::Knowing::Sighted),
                            (IVec2::new(0, 0), crate::Knowing::Visited),
                        ]),
                    },
                ),
                (
                    Token(0xFFFF_0000_0000_0002),
                    // Somebody who has been dealt papers and never looked at
                    // anything, which writes no survey line at all.
                    PlayerRecord {
                        position: Vec2::new(-0.125, 9000.0),
                        aboard: None,
                        surveyed: Vec::new(),
                        known: HashMap::new(),
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
            claims: vec![
                // One christened and one not, and the names either side of
                // the plainest word there is: spaces inside them, which is
                // what putting the name last on the line is for.
                ClaimRecord {
                    island: IVec2::new(-40, 300),
                    by: Token(7),
                    at: Vec2::new(12.5, -340.25),
                    name: "Ilha do Príncipe".to_string(),
                },
                ClaimRecord {
                    island: IVec2::new(2, -1),
                    by: Token(0xFFFF_0000_0000_0002),
                    at: Vec2::new(-0.125, 9000.0),
                    name: String::new(),
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

    /// Writes a world down, reads it back, and says the two are one world.
    ///
    /// Composing sorts — each player's survey, the boats, the claims, the
    /// beasts — so everything here is compared as content rather than as
    /// order.
    fn round_trips(record: &WorldRecord) {
        let read = parse(&compose(record))
            .unwrap_or_else(|why| panic!("what was composed will not parse: {why}"));
        assert_eq!(read.id, record.id);
        assert_eq!(read.seed, record.seed);
        assert_eq!(read.name, record.name);
        assert_eq!(read.opening, record.opening);
        assert_eq!(read.age, record.age);
        // Composing sorts each player's survey, as it does the boats and the
        // beasts, so the comparison is about content alone.
        let players_sorted = |players: HashMap<Token, PlayerRecord>| {
            let mut players: Vec<(Token, PlayerRecord)> = players.into_iter().collect();
            for (_, player) in &mut players {
                player.surveyed.sort_by_key(|chunk| (chunk.x, chunk.y));
            }
            players.sort_by_key(|(token, _)| token.0);
            players
        };
        assert_eq!(
            players_sorted(read.players),
            players_sorted(record.players.clone())
        );
        // Composing sorts the boats and the beasts, so compare content
        // rather than order.
        let boats_sorted = |mut boats: Vec<BoatRecord>| {
            boats.sort_by_key(|boat| boat.id.0);
            boats
        };
        assert_eq!(boats_sorted(read.boats), boats_sorted(record.boats.clone()));
        let claims_sorted = |mut claims: Vec<ClaimRecord>| {
            claims.sort_by_key(|claim| (claim.island.x, claim.island.y));
            claims
        };
        assert_eq!(
            claims_sorted(read.claims),
            claims_sorted(record.claims.clone()),
            "a claim came back as somebody else's island"
        );
        let sorted = |mut beasts: Vec<BeastRecord>| {
            beasts.sort_by_key(|beast| (word_of(beast.kind), beast.position.x.to_bits()));
            beasts
        };
        assert_eq!(sorted(read.beasts), sorted(record.beasts.clone()));
    }

    #[test]
    fn a_world_survives_the_round_trip() {
        round_trips(&a_record());
    }

    /// The furthest chunk the world resolves over, and so the furthest ground
    /// anybody may have surveyed — see [`crate::in_the_world`]. Worked out
    /// rather than written down, so that moving [`crate::MAX_RANGE`] moves the
    /// edge these tests are about.
    fn the_last_chunk() -> i32 {
        let brink = (crate::MAX_RANGE / protocol::ground::CHUNK_METRES) as i32;
        assert!(
            crate::in_the_world(IVec2::splat(brink))
                && !crate::in_the_world(IVec2::splat(brink + 1)),
            "the edge of the world is not where this test thinks it is"
        );
        brink
    }

    /// A world holding, in every field that has an edge, a value standing on
    /// it: the furthest a player or a hull may be, the furthest chunk anybody
    /// may have surveyed, the outermost island there can be an identity for,
    /// and beasts at and bound for the brink.
    fn a_record_at_the_edge() -> WorldRecord {
        let brink = the_last_chunk();
        let far = Vec2::splat(crate::MAX_RANGE);
        let steps = u8::MAX as i32;
        // The outermost identity the lattice carries inside the world: the last
        // mark of the last chunk. One step further is the boundary, and a
        // boundary belongs to the chunk beyond it — see
        // [`protocol::survey::chunk_of`].
        let last_ring = IVec2::splat(brink * steps + steps - 1);
        // A name at exactly the length the wire allows, with spaces in it and
        // letters that cost more than a byte: what a claim's line has to carry
        // through going last on it.
        let long = format!("Ilha {}!", "ô".repeat(45));
        assert_eq!(long.len(), protocol::NAME_BYTES);
        assert!(protocol::island_name(&long).is_some(), "not a name at all");

        WorldRecord {
            id: WorldId(u64::MAX),
            seed: u32::MAX,
            name: "Ilha do Príncipe".to_string(),
            // A phase runs from the stroke of midnight up to but not including
            // the next, so the edge to stand on is the stroke.
            opening: 0.0,
            age: f32::MAX,
            players: HashMap::from([
                (
                    Token(u64::MAX),
                    PlayerRecord {
                        position: far,
                        aboard: Some(BoatId(u64::MAX)),
                        // Both far corners, and two chunks of one column, so
                        // that the column shorthand is exercised out here too.
                        surveyed: vec![
                            IVec2::splat(-brink),
                            IVec2::splat(brink),
                            IVec2::new(brink, brink - 1),
                        ],
                        // Both outermost identities the lattice carries, one
                        // at each depth of knowing — the same two the claims
                        // below stand on, because a knowing names an island the
                        // very same way a claim does and so has the very same
                        // edge to fall off.
                        known: HashMap::from([
                            (last_ring, crate::Knowing::Visited),
                            (IVec2::splat(-brink * steps), crate::Knowing::Sighted),
                        ]),
                    },
                ),
                (
                    Token(0),
                    PlayerRecord {
                        position: -far,
                        aboard: None,
                        surveyed: Vec::new(),
                        known: HashMap::new(),
                    },
                ),
            ]),
            boats: vec![BoatRecord {
                id: BoatId(u64::MAX),
                kind: BoatKind::Sloop,
                position: -far,
                heading: f32::MAX,
            }],
            claims: vec![
                ClaimRecord {
                    island: last_ring,
                    by: Token(u64::MAX),
                    at: far,
                    name: long,
                },
                ClaimRecord {
                    island: IVec2::splat(-brink * steps),
                    by: Token(0),
                    at: -far,
                    name: String::new(),
                },
            ],
            beasts: vec![
                BeastRecord {
                    kind: BeastKind::Shark,
                    position: far,
                    goal: None,
                    left: u32::MAX,
                },
                BeastRecord {
                    kind: BeastKind::Whale,
                    position: -far,
                    goal: Some(far),
                    left: 0,
                },
            ],
        }
    }

    #[test]
    fn what_the_world_writes_it_can_read_back() {
        // The invariant the whole file rests on, said once and in one place:
        // **anything [`compose`] writes, [`parse`] takes back**.
        //
        // Four times now a writer has been able to emit something its own
        // reader refuses — a surveyed chunk past the edge, a beast past it, a
        // beast's goal past it, an island's identity past it — and none of them
        // cost the field they were about. They cost the world: the file will
        // not parse, [`load`] falls back to the copy beside it, the session is
        // quietly gone, and the next save takes the copy too. Each was found
        // one at a time, by somebody sailing to the brink.
        //
        // So this stands where the shape of the file is, rather than beside any
        // one of them. Add a field to a world file and it belongs in
        // [`a_record_at_the_edge`], standing on whatever edge it has.
        round_trips(&a_record_at_the_edge());
    }

    #[test]
    fn what_the_world_must_not_write_is_filtered_where_it_is_made() {
        // The other half of that contract, and the half that cannot be met
        // here. These compose into lines the reader refuses, and one refused
        // line is the whole world — so what keeps them out of the file is that
        // they never reach [`compose`]: `beasts::Flock::records` drops a beast
        // past the edge or making for past it, and `Shared::record` drops both
        // a claim whose island the lattice cannot carry back and a knowing
        // about one. This says what those filters are for, so that the next
        // field's writer knows it owes one.
        let brink = the_last_chunk();
        let past = crate::MAX_RANGE + 1.0;
        let beyond = (brink + 1) * u8::MAX as i32;
        for (record, what) in [
            (
                WorldRecord {
                    beasts: vec![BeastRecord {
                        kind: BeastKind::Shark,
                        position: Vec2::new(past, 0.0),
                        goal: None,
                        left: 1,
                    }],
                    ..a_record()
                },
                "a beast swimming past the end of the world",
            ),
            (
                WorldRecord {
                    beasts: vec![BeastRecord {
                        kind: BeastKind::Whale,
                        position: Vec2::ZERO,
                        goal: Some(Vec2::new(past, 0.0)),
                        left: 1,
                    }],
                    ..a_record()
                },
                "a beast making for past the end of the world",
            ),
            (
                WorldRecord {
                    claims: vec![ClaimRecord {
                        island: IVec2::splat(beyond),
                        by: Token(7),
                        at: Vec2::ZERO,
                        name: String::new(),
                    }],
                    ..a_record()
                },
                "an island nobody could have sailed round",
            ),
            (
                WorldRecord {
                    claims: vec![ClaimRecord {
                        island: IVec2::ZERO,
                        by: Token(7),
                        at: Vec2::new(past, 0.0),
                        name: String::new(),
                    }],
                    ..a_record()
                },
                "a cairn standing past the end of the world",
            ),
            (
                WorldRecord {
                    players: HashMap::from([(
                        Token(7),
                        PlayerRecord {
                            known: HashMap::from([(IVec2::splat(beyond), crate::Knowing::Sighted)]),
                            ..PlayerRecord::default()
                        },
                    )]),
                    ..a_record()
                },
                "a cairn seen on an island that cannot be one",
            ),
        ] {
            assert!(
                parse(&compose(&record)).is_err(),
                "{what} was written down as though it could be read back"
            );
        }
    }

    #[test]
    fn a_voyage_is_written_down_small() {
        // What a survey costs the file. A voyage is a swath — a band of
        // chunks either side of a way — so sorted into columns it is runs of
        // neighbours, and the line says so instead of naming five figures of
        // coordinate per chunk. The whole of it still reads back exactly:
        // this is a shorter way of saying the same thing, not a rounder one.
        let sailed: Vec<IVec2> = (0..400)
            .flat_map(|x| (0..5).map(move |z| IVec2::new(x, 6_000 + z)))
            .collect();
        let mut record = a_record();
        record.players.insert(
            Token(0x5A11),
            PlayerRecord {
                position: Vec2::ZERO,
                aboard: None,
                surveyed: sailed.clone(),
                known: HashMap::new(),
            },
        );

        let composed = compose(&record);
        let line = composed
            .lines()
            .find(|line| line.starts_with("surveyed 0000000000005a11"))
            .expect("the voyage was written down");
        assert!(
            line.len() < sailed.len() * 4,
            "{} chunks took {} bytes to write down",
            sailed.len(),
            line.len()
        );

        let mut read = parse(&composed).expect("parse what was composed").players[&Token(0x5A11)]
            .surveyed
            .clone();
        read.sort_by_key(|chunk| (chunk.x, chunk.y));
        assert_eq!(read, sailed, "the voyage came back as somewhere else");
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
            (
                "genovesa world 1\nid 1\nseed 7\nopening 0.35\nage 0\nsurveyed 1\n",
                "a survey of nowhere",
            ),
            (
                "genovesa world 1\nid 1\nseed 7\nopening 0.35\nage 0\nsurveyed 1 3 4\n",
                "surveyed chunks that name no column",
            ),
            (
                "genovesa world 1\nid 1\nseed 7\nopening 0.35\nage 0\nsurveyed 1 99999999:0\n",
                "a chunk past where the world resolves",
            ),
            (
                "genovesa world 1\nid 1\nseed 7\nopening 0.35\nage 0\nsurveyed 1 3:0,0\n",
                "a survey stepping nowhere, which is one chunk twice",
            ),
            (
                "genovesa world 1\nid 1\nseed 7\nopening 0.35\nage 0\nsurveyed 1 3:4,-1\n",
                "a survey stepping back the way it came",
            ),
            (
                "genovesa world 1\nid 1\nseed 7\nopening 0.35\nage 0\nsurveyed 1 5:0 3:0\n",
                "survey columns out of order",
            ),
            (
                "genovesa world 1\nid 1\nseed 7\nopening 0.35\nage 0\nsurveyed 1 3:0 3:9\n",
                "one column named twice",
            ),
            (
                "genovesa world 1\nid 1\nseed 7\nopening 0.35\nage 0\n\
                 player 1 0 0\nsurveyed 1 0:0\nsurveyed 1 4:4\n",
                "one player's survey told twice",
            ),
            (
                "genovesa world 1\nid 1\nseed 7\nopening 0.35\nage 0\n\
                 player 1 0 0\nplayer 1 8 8\n",
                "one player told twice",
            ),
            (
                "genovesa world 1\nid 1\nseed 7\nopening 0.35\nage 0\nclaim 1 2 7 3\n",
                "a cairn with half a position",
            ),
            (
                "genovesa world 1\nid 1\nseed 7\nopening 0.35\nage 0\nclaim 1 2 7 1e30 0\n",
                "a cairn past where the world resolves",
            ),
            (
                "genovesa world 1\nid 1\nseed 7\nopening 0.35\nage 0\nclaim 99999999 0 7 0 0\n",
                "an island past where the world resolves",
            ),
            (
                "genovesa world 1\nid 1\nseed 7\nopening 0.35\nage 0\n\
                 claim 1 2 7 0 0 Here\nclaim 1 2 9 4 4 There\n",
                "one island claimed twice",
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
    fn a_discarded_world_leaves_nothing_behind() {
        let dir = scratch();
        let path = dir.join("gone.world");
        {
            let keeper = Keeper::hold(&path).expect("hold");
            // Twice, so there is a backup to be thrown away as well.
            keeper.save(&a_record()).expect("save");
            keeper.save(&a_record()).expect("save");
        }
        assert_eq!(kept_worlds(&dir).len(), 1);

        discard(&path).expect("discard");
        assert!(kept_worlds(&dir).is_empty(), "the world is still listed");
        let left: Vec<PathBuf> = fs::read_dir(&dir)
            .expect("read")
            .flatten()
            .map(|entry| entry.path())
            .collect();
        assert!(left.is_empty(), "the discard left files behind: {left:?}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_hosted_world_refuses_to_be_discarded() {
        // Deleting the file under a running session would leave it saving a
        // history to a name nothing will read again.
        let dir = scratch();
        let path = dir.join("busy.world");
        let held = Keeper::hold(&path).expect("hold");
        held.save(&a_record()).expect("save");

        assert!(discard(&path).is_err(), "a hosted world was discarded");
        assert_eq!(kept_worlds(&dir).len(), 1, "and yet something went");

        // And once the host has let go, it goes.
        drop(held);
        discard(&path).expect("discard");
        assert!(kept_worlds(&dir).is_empty());
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
