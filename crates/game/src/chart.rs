//! The chart: what the player has seen of the world, drawn in plan on paper.
//!
//! Pressing the chart key lays the sheet over the world. North is up and stays
//! up — that is the whole difference from the view it covers, which turns —
//! and the rose in the corner says so rather than pointing anywhere.
//!
//! **Nothing here is generated, and nothing new crosses the wire.** A
//! coastline is the sea-level contour of the height grid a chunk already
//! arrived with (see [`protocol::ground::ChunkPayload`]), so the chart is drawn
//! out of what the client was told and nothing else. That is deliberate: having
//! *seen* a stretch of coast is a fact about one player's client, and a second
//! client written against the protocol alone would keep a chart of its own the
//! same way, without a byte being added to the session for it.
//!
//! # The one thing that accumulates
//!
//! Everything else this client holds is streamed and forgotten: chunks, meshes,
//! palms, the beasts in the water. The camera moves on and `terrain` drops what
//! it has left behind, because the server is the one holding the world. The
//! chart cannot do that — a coast is worth remembering exactly as long as the
//! player is in the world — so it is the one structure here that has to be
//! designed for a world with no edges.
//!
//! What makes that affordable is throwing the ground away and keeping only the
//! two lines worth drawing — where it meets the sea, and where the water over
//! it reaches [`SHOAL_DEPTH`]:
//!
//! | held per chunk | a 1.5 km island, sailed right around |
//! | --- | --- |
//! | the height grid it arrived as | about a megabyte |
//! | its coastline and its shoal line, simplified | a few kilobytes |
//!
//! Three things do that. Only chunks with a crossing on them hold anything at
//! all — open water arrives as no payload whatever, and an island's interior
//! has neither line on it, so what is kept is a band a chunk or two thick round
//! each island and not an island's worth of area. What is kept of it is
//! simplified to [`TOLERANCE`], which the chart can afford: a bay owes the
//! player its shape and not its rocks. And a point is a [`Mark`], two bytes,
//! because half a metre is finer than any hand ever drew a shoreline.
//!
//! The second line roughly doubles what a coastal chunk holds, and it is worth
//! it: it is the only *depth* on a sheet that is otherwise all outline, and it
//! is the difference between a shore that plunges and a shore with a bank off
//! it. It is drawn and nothing more — it rings nothing, names nothing and
//! closes nothing, and every question the rest of this module asks about
//! islands is asked of the waterline alone.
//!
//! The arithmetic that matters is the last column: a player who calls at a
//! thousand islands is holding a few megabytes, which is a fraction of a single
//! island's ground. There is no eviction here and none is wanted — an infinite
//! world is affordable because of what is never stored, not because of what is
//! thrown away later.
//!
//! # What counts as charted
//!
//! A chunk is surveyed once it comes within [`SIGHT_RADIUS`] of the *player* —
//! afloat or ashore, the survey following whatever carries them — and the
//! radius is short: where the haze starts, not where it ends. The chart is a
//! record of where the player has been, so coast goes on it by being closed
//! with, a band at a time as they move, and an island passed down one side is
//! charted on that side and blank on the other until somebody goes round.
//!
//! Not a test against the camera's frustum, which is wrong twice over: the
//! chart would fill in and stop filling in as the view was spun, and ground
//! the player sailed past with the camera pointed the other way is ground the
//! player sailed past. Nor is there any test for what a headland hides — a bay
//! left empty because a hill stood in front of it would read as the chart
//! being broken rather than as the survey being honest.
//!
//! # Closing a coastline, and what an island is
//!
//! Because coast is charted by going there, "has the player been all the way
//! round this shore" is a question with an answer: strokes meet *exactly* at
//! chunk boundaries — see [`Mark`] — so a circumnavigated coastline is a
//! chain of runs that links back to its own start, found by integer
//! bookkeeping. [`Chart::tally`] counts the coastlines that close and the
//! chains still hanging open.
//!
//! And a closed coastline is what an *island* is, for the game: a ring that
//! rings **land** — the survey's land-on-the-left convention makes a ring
//! around land and a ring around a lagoon run opposite ways, so the signed
//! area tells them apart for free — and that reaches at least
//! [`LEAST_ISLAND`] across. That is the unit a player will one day claim and
//! name, and it is deliberately defined here, on the coastline, rather than
//! by asking the generator. The generator plans an island to grow ground
//! from, but what it grows may meet the sea in more pieces than one: a
//! planned island can surface as a main shore and a scatter of skerries, or
//! as two hills with a drowned middle. Each piece big enough is its own
//! island in the game, each rock awash is charted without being anybody's
//! island, and no part of any of it asks the plan — which keeps generation
//! what it ought to be, a machine that produces chunks and what stands on
//! them and owes nothing downstream an explanation.
//!
//! An island can already be *named*: clicking one on the sheet puts a caret
//! on its lettering and the keyboard becomes the pen — there is no dialog,
//! because a chart is written on, not filled in. The name is written against
//! the ring's identity on the step lattice (see [`Chart::coastlines`]) and is
//! the chart's own to keep: it lives and dies with the sheet, exactly as what
//! the sheet has seen does.
//!
//! A *claim* itself would cross the wire and be the server's to grant. The
//! server holds the same chunks, so it can check a claim by this same
//! arithmetic without trusting the client — and when that day comes, the
//! contour-and-ring arithmetic should move to `protocol`, where the things
//! both ends must agree on live, so the two cannot drift — and a name worth
//! showing to anybody else would ride with the claim.
//!
//! # Ink, not paper
//!
//! An old chart's character is easy to get from a paper texture and a wash of
//! stains, and that is the one way it cannot be got here: this world is flat
//! tones with no texture and no gradient anywhere in it, and a mottled sheet
//! would be the only one of either. So the hand is in the line instead — a
//! coast weighted heavier than the graticule under it, ticked on its landward
//! side the way an engraved chart hatches its shores, stippled on its seaward
//! side where the water is shallow, and laid on one flat
//! tone of parchment. The furniture is lettered in the serif the menus are
//! set in; the islands are named in an italic of the Fell types (see
//! [`NAME_FONT`]), which is the nearest a flat sheet comes to an engraver's
//! hand.
//!
//! Outside all of it the sheet has an edge — a double rule just inside the
//! window with the ruling's own graduations laid between the two lines, and the
//! engraving covered over beyond it. A chart drawn to the window's own edge has
//! no edge at all: the paper runs out wherever the player last dragged their
//! mouse, which reads as a viewport rather than as a sheet.
//!
//! Under all of it is the rhumb net: roses standing on the ruling's own
//! crossings, each throwing the thirty-two points of the compass across the
//! paper (see [`rhumbs`]). It is the sheet's whole character and none of its
//! content, so it is ruled fainter than anything that runs over it, and its
//! rays are graded — the principal winds ruled, the quarter winds dashed — so
//! that a dozen roses' worth of rays reads as a net rather than as a haze. The
//! roses themselves are drawn in two flat tones, each point of the star split
//! down its own axis (see [`star`]): a hatched engraving is what that is
//! imitating, and two tones meeting on an edge is the only way to draw a lit
//! thing on a sheet with no gradients on it.
//!
//! A coast that has not been closed is not closed on the sheet either. Half an
//! island is drawn as half an island, the line simply stopping where the survey
//! did — which is what a partly run coastline looked like on a real chart, and
//! saves inventing the other side.

use bevy::asset::RenderAssetUsages;
use bevy::camera::RenderTarget;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::input::ButtonState;
use bevy::mesh::PrimitiveTopology;
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bevy::text::{Font, FontSize, FontSource};
use bevy::window::PrimaryWindow;

use protocol::ground::{chunk_at, CHUNK_METRES, FACET_METRES, FACET_QUADS, FACET_VERTS};

use crate::bindings::{Action, KeyBindings};
use crate::camera::MapCamera;
use crate::player::PlayerPlace;
use crate::terrain::Ground;
use crate::{AppState, Helm};

// ---------------------------------------------------------------------------
// Surveying
// ---------------------------------------------------------------------------

/// How near the player a chunk has to come to be surveyed, in metres.
///
/// The distance at which the haze starts taking the ground over: coast nearer
/// than this is coast the player has actually closed with, and coast beyond it
/// is already dissolving into the air. The chart records where the player has
/// *been*, so this is deliberately short — charting the far side of an island
/// means sailing round it, and there is a reason it must (see the module docs
/// on closing a coastline). It was most of the streaming radius once, and that
/// charted whole islands from the anchorage off one corner of them.
///
/// Far inside `terrain`'s streaming radius, which is what makes the survey
/// safe rather than merely tidy: a chunk within sight is one the client is
/// certain to be holding the ground of, so it never has to ask for anything
/// or wait.
pub const SIGHT_RADIUS: f32 = crate::HAZE_START;

/// Chunks surveyed in any one frame.
///
/// A survey is cheap — marching squares across four thousand cells — but they
/// arrive in clumps rather than spread out, an island's whole rectangle landing
/// within a frame or two of itself, and a hundred at once would be a hitch
/// exactly when the player has reached somewhere worth looking at. The rest are
/// picked up over the following frames, and nothing is lost by waiting: a chunk
/// in sight stays in sight, and stays in hand a good deal longer than that.
const SURVEYS_PER_FRAME: usize = 8;

/// How far a simplified coastline may stray from the contour it was taken from,
/// in metres.
///
/// Three metres is a metre and a half of facet. It takes a chunk's shore from a
/// couple of hundred segments down to a handful, which is most of the reason a
/// thousand islands fit in memory at once.
const TOLERANCE: f32 = 3.0;

/// How deep the water has to get before it stops being a shoal, in metres
/// below the waterline.
///
/// The chart draws this line stippled, the way an engraved chart draws the
/// limit of a bank — so it is not decoration but the one piece of *depth* on a
/// sheet that is otherwise all outline, and it says where a hull would find
/// the bottom. Three metres because it is deeper than any boat here draws and
/// shallower than the shelf most coasts stand on: a shore whose ground plunges
/// carries the line right against its own outline, and a shore with a bank off
/// it carries the line out where the bank ends, which is the difference the
/// player wants to see.
const SHOAL_DEPTH: f32 = -3.0;

/// The smallest closed shore worth keeping, as the longer side of what it
/// encloses, in metres.
///
/// A height field crossing the waterline leaves a scatter of one- and two-cell
/// rings around any coast — rocks awash, and the odd hummock of sand a facet
/// wide. Every one is honest ground, and drawn they read as dirt on the paper
/// rather than as anything anybody could steer by. Six metres is three facets,
/// which keeps a real skerry and loses the speckle.
///
/// Open runs are not filtered: a short one is a coast leaving the chunk, and
/// the rest of it is the neighbour's to draw.
const LEAST_ISLET: f32 = 6.0;

/// The smallest closed coastline that counts as an *island*, as the longer
/// side of what it rings, in metres.
///
/// The claimable, nameable unit — see the module docs. Below this a ring is a
/// rock or a skerry: charted, drawn, honestly part of the coast, but not a
/// place anybody would put a name to. A hundred metres is a little under the
/// smallest island the generator sets out to make, so everything *meant* as
/// an island qualifies and the accidents of a coast mostly do not.
///
/// Not to be confused with [`LEAST_ISLET`], which is about ink — what is too
/// small to draw at all — where this is about standing: what is too small to
/// claim. The gap between the two is exactly the skerries.
pub const LEAST_ISLAND: f32 = 100.0;

/// One point of a surveyed coastline, in chunk-local steps.
///
/// Two bytes, and the whole reason an infinite world's chart fits: a step is
/// [`CHUNK_METRES`] over 255, about half a metre, which is a quarter of a facet
/// and far finer than [`TOLERANCE`] has already thrown away.
///
/// A step also lands the ends of a stroke *exactly* on the chunk boundary — 0
/// and 255 are the boundary itself — so a coast leaving one chunk and the one
/// continuing it in the next meet on the sheet with nothing between them. The
/// two neighbours agree because they were sent the same corner heights along
/// the edge they share, and both quantise the crossing the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark {
    x: u8,
    z: u8,
}

/// Metres one step of a [`Mark`] stands for.
const MARK_STEP: f32 = CHUNK_METRES / u8::MAX as f32;

impl Mark {
    /// The nearest mark to a point in chunk-local metres.
    fn of(local: Vec2) -> Self {
        let step = |v: f32| (v / MARK_STEP).round().clamp(0.0, u8::MAX as f32) as u8;
        Self {
            x: step(local.x),
            z: step(local.y),
        }
    }

    /// Where this mark stands, in chunk-local metres.
    fn local(self) -> Vec2 {
        Vec2::new(self.x as f32, self.z as f32) * MARK_STEP
    }

    /// The mark as the two bytes it is, for the logbook to write down —
    /// and [`Mark::unpack`] to take back. The pair is `[x, z]`.
    pub(crate) fn pack(self) -> [u8; 2] {
        [self.x, self.z]
    }

    pub(crate) fn unpack([x, z]: [u8; 2]) -> Self {
        Self { x, z }
    }
}

/// One run of surveyed coastline inside a single chunk.
///
/// Held per chunk rather than as whole islands because that is the unit the
/// survey works in and the unit the player discovers the world in. An island
/// half seen is a set of chunks half of which have been looked at, and joining
/// their strokes into one island's outline would mean deciding what to do about
/// the joins that are not there yet.
#[derive(Clone, Debug, PartialEq)]
pub struct Coast {
    /// The points of the run, land always on the left hand of the direction of
    /// travel — which is what lets the shore ticks be drawn without asking the
    /// height field a second question. Readable by the logbook, which writes
    /// the survey down between visits.
    pub(crate) marks: Vec<Mark>,
    /// Whether the run closes on itself: an islet small enough to sit inside
    /// one chunk. An open run leaves the chunk by its edge, and is continued —
    /// or is not — by the neighbour's own survey.
    pub(crate) closed: bool,
}

impl Coast {
    /// A run as the logbook read it back — the survey's own runs are built
    /// by [`survey`], and this is only for reloading what that once made.
    pub(crate) fn new(marks: Vec<Mark>, closed: bool) -> Self {
        Self { marks, closed }
    }

    /// The run in world metres, given the chunk it belongs to.
    fn points(&self, chunk: IVec2) -> impl Iterator<Item = Vec2> + '_ {
        let base = chunk.as_vec2() * CHUNK_METRES;
        self.marks.iter().map(move |mark| base + mark.local())
    }
}

/// Everything the player has seen of the world's coasts.
///
/// An entry means *surveyed*, and most entries are empty: open water and the
/// inland parts of an island have no coastline on them, and recording that they
/// were looked at is what stops them being surveyed again on every frame the
/// player sits near them. It is also, in the end, the record of where the
/// player has been.
#[derive(Resource, Default)]
pub struct Chart {
    soundings: HashMap<IVec2, Soundings>,
    /// What the player has christened their islands, keyed by [`Island::id`].
    /// Part of the chart rather than a resource of its own because it is the
    /// same kind of fact as the coasts: what *this* player holds about *this*
    /// world, gone with the world when they leave it.
    names: HashMap<IVec2, String>,
}

/// What the chart holds, counted.
///
/// The first two are for the debug readout. The last two are the fact the
/// module docs promise under *Closing a coastline*: how many coastlines the
/// player has been all the way round, and how many they are still working on —
/// which is what an island becoming claimable will one day be read off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChartTally {
    /// Chunks surveyed at all — the record of where the player has been.
    pub surveyed: usize,
    /// Chunks whose survey found coast, and so hold any ink.
    pub coastal: usize,
    /// Coastlines that close: an islet ringed inside one chunk, or a chain of
    /// runs across chunks that links back to its own start. Every one is a
    /// shore the player has followed all the way round.
    pub complete: usize,
    /// Chains that do not close yet — coasts with their ends still hanging,
    /// waiting for the player to go and look.
    pub open: usize,
    /// The closed coastlines that are *islands*: rings with the land inside
    /// and at least [`LEAST_ISLAND`] across. The claimable count, and the
    /// only number here gameplay will ever read — the gap between it and
    /// [`ChartTally::complete`] is the skerries and the lagoons.
    pub islands: usize,
}

/// An island the chart has closed — the nameable unit, see the module docs —
/// measured for the lettering that goes on it.
#[derive(Clone, Debug, PartialEq)]
pub struct Island {
    /// What names the island for as long as the chart lives: the least point
    /// of its ring on the step lattice, which is the same points however the
    /// walk went round — see [`Chart::coastlines`].
    pub id: IVec2,
    /// The middle of its bounding box, on the sheet (see [`on_the_sheet`]).
    pub centre: Vec2,
    /// How far it reaches: the longer side of that box, in metres.
    pub extent: f32,
    /// What the player has christened it, if they have.
    pub name: Option<String>,
}

impl Chart {
    /// Whether this chunk has been surveyed at all.
    pub fn surveyed(&self, chunk: IVec2) -> bool {
        self.soundings.contains_key(&chunk)
    }

    /// Counts what the chart holds — including, in [`ChartTally::islands`],
    /// the closed coastlines that are islands to claim.
    pub fn tally(&self) -> ChartTally {
        let surveyed = self.soundings.len();
        let coastal = self
            .soundings
            .values()
            .filter(|found| !found.coast.is_empty())
            .count();
        let mut islands = 0;
        let (complete, open) = self.coastlines(&mut |_, ring| {
            if measure(ring).is_island() {
                islands += 1;
            }
        });
        ChartTally {
            surveyed,
            coastal,
            complete,
            open,
            islands,
        }
    }

    /// The islands the chart has closed, each measured for its lettering.
    pub fn islands(&self) -> Vec<Island> {
        let mut islands = Vec::new();
        self.coastlines(&mut |id, ring| {
            let measured = measure(ring);
            if measured.is_island() {
                islands.push(Island {
                    id,
                    centre: measured.centre,
                    extent: measured.extent,
                    name: self.names.get(&id).cloned(),
                });
            }
        });
        islands
    }

    /// Writes a name against an island — [`Island::id`] says which — or,
    /// given only whitespace, washes it off again.
    pub fn christen(&mut self, island: IVec2, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            self.names.remove(&island);
        } else {
            self.names.insert(island, name.to_string());
        }
    }

    /// What an island is called, if the player has called it anything.
    pub fn name(&self, island: IVec2) -> Option<&str> {
        self.names.get(&island).map(String::as_str)
    }

    /// Follows every coastline the chart holds, handing each closed one to
    /// `close` — whole, in the order the shore is walked, under the identity
    /// that names it — and counting what it found: coastlines that close,
    /// and chains still hanging open.
    ///
    /// Whether a coastline closes is integer bookkeeping, not geometry: a
    /// [`Mark`] lands exactly on the chunk boundary, so a run's end and its
    /// continuation's start are the *same* point on the step lattice, and
    /// following a shore is a lookup. The walk is the survey's own two-pass
    /// one a scale up — chains first from every run nothing links into, then
    /// whatever is left, which can only be loops.
    ///
    /// The identity handed with each ring is its least point on that same
    /// lattice. It is integers, so no tolerance is involved; chunks are
    /// surveyed once and never again, so a closed ring is the same points for
    /// as long as the chart lives; and the *least* point in particular does
    /// not care where the walk happened to start, which a hash map decides.
    fn coastlines(
        &self,
        close: &mut dyn FnMut(IVec2, &mut dyn Iterator<Item = Vec2>),
    ) -> (usize, usize) {
        let mut complete = 0;

        // Where each open run starts and ends, in whole steps of the mark
        // lattice — exact, so equality is equality.
        let mut starts: HashMap<IVec2, (IVec2, usize)> = HashMap::default();
        let mut ends: HashSet<IVec2> = HashSet::default();
        for (&chunk, found) in &self.soundings {
            for (at, run) in found.coast.iter().enumerate() {
                if run.closed {
                    // A ring inside one chunk closed the moment it was drawn.
                    complete += 1;
                    let id = ring_id(run.marks.iter().map(|&mark| run_steps(chunk, mark)));
                    close(id, &mut run.points(chunk));
                    continue;
                }
                starts.insert(run_steps(chunk, run.marks[0]), (chunk, at));
                ends.insert(run_steps(chunk, run.marks[run.marks.len() - 1]));
            }
        }

        // Follows a shore from one run for as long as the links hold, and
        // says which runs it passed through.
        let end_of = |key: (IVec2, usize)| {
            let run = &self.soundings[&key.0].coast[key.1];
            run_steps(key.0, run.marks[run.marks.len() - 1])
        };
        let mut walked: HashSet<IVec2> = HashSet::default();
        let walk = |from: (IVec2, usize), walked: &mut HashSet<IVec2>| {
            let mut chain = vec![from];
            let mut here = from;
            loop {
                let end = end_of(here);
                match starts.get(&end) {
                    Some(&next) if walked.insert(end) => {
                        chain.push(next);
                        here = next;
                    }
                    _ => break,
                }
            }
            chain
        };

        // The open chains first, from every run whose start nothing ends at —
        // a coast the survey has left hanging.
        let mut open = 0;
        for (&start, &key) in &starts {
            if ends.contains(&start) || !walked.insert(start) {
                continue;
            }
            open += 1;
            walk(key, &mut walked);
        }

        // Whatever is left links into itself: a coastline the player has been
        // all the way round, measured whole — every run of the chain, in the
        // order the shore is walked.
        for (&start, &key) in &starts {
            if !walked.insert(start) {
                continue;
            }
            let chain = walk(key, &mut walked);
            complete += 1;
            let id = ring_id(chain.iter().flat_map(|&(chunk, at)| {
                self.soundings[&chunk].coast[at]
                    .marks
                    .iter()
                    .map(move |&mark| run_steps(chunk, mark))
            }));
            close(
                id,
                &mut chain
                    .iter()
                    .flat_map(|&(chunk, at)| self.soundings[&chunk].coast[at].points(chunk)),
            );
        }

        (complete, open)
    }

    /// Everything surveyed within a rectangle of the world, chunk by chunk.
    ///
    /// The rectangle is what keeps drawing bounded. A chart of a long voyage
    /// holds far more coastline than a sheet can show, so the mesh is built
    /// from a window on it rather than from everything ever seen — and the cost
    /// of drawing follows how much paper there is rather than how far the
    /// player has sailed.
    fn within(&self, window: Rect) -> impl Iterator<Item = (IVec2, &Soundings)> {
        let lower = chunk_at(window.min);
        let upper = chunk_at(window.max);
        (lower.y..=upper.y)
            .flat_map(move |z| (lower.x..=upper.x).map(move |x| IVec2::new(x, z)))
            .filter_map(|chunk| Some((chunk, self.soundings.get(&chunk)?)))
    }

    /// Records what one chunk's ground turned out to hold.
    fn record(&mut self, chunk: IVec2, found: Soundings) {
        self.soundings.insert(chunk, found);
    }

    /// Everything surveyed, and every name written on the sheet, as plain
    /// entries for the logbook to write down.
    pub(crate) fn entries(&self) -> (Vec<(IVec2, Soundings)>, Vec<(IVec2, String)>) {
        (
            self.soundings
                .iter()
                .map(|(chunk, found)| (*chunk, found.clone()))
                .collect(),
            self.names
                .iter()
                .map(|(island, name)| (*island, name.clone()))
                .collect(),
        )
    }

    /// A chart rebuilt from a logbook's entries — the survey, and the
    /// christenings, taken up from wherever the last visit left off.
    pub(crate) fn from_entries(
        soundings: impl IntoIterator<Item = (IVec2, Soundings)>,
        names: impl IntoIterator<Item = (IVec2, String)>,
    ) -> Self {
        Self {
            soundings: soundings.into_iter().collect(),
            names: names.into_iter().collect(),
        }
    }
}

/// A mark as a point on the world-wide step lattice: 255 whole steps to a
/// chunk, so a run ending on a chunk's boundary and the run continuing it next
/// door land on the *same* integers — which is what lets [`Chart::tally`]
/// follow a shore across chunks by equality rather than by tolerance.
fn run_steps(chunk: IVec2, mark: Mark) -> IVec2 {
    chunk * u8::MAX as i32 + IVec2::new(mark.x as i32, mark.z as i32)
}

/// The least of a ring's points on the step lattice, west before south — the
/// ring's identity, whatever order its points arrive in.
fn ring_id(marks: impl Iterator<Item = IVec2>) -> IVec2 {
    marks
        .min_by_key(|point| (point.x, point.y))
        .expect("a ring has points")
}

/// A closed ring, measured on the sheet.
struct Ring {
    /// How far it reaches: the longer side of its bounding box, in metres.
    extent: f32,
    /// Its signed area. Positive is land inside — see [`measure`].
    area: f32,
    /// The middle of its bounding box.
    centre: Vec2,
}

impl Ring {
    /// Whether this coastline is an island: land inside, and enough of it. A
    /// lagoon fails the sign however big it is, a skerry the reach.
    fn is_island(&self) -> bool {
        self.area > 0.0 && self.extent >= LEAST_ISLAND
    }
}

/// A closed ring measured: its reach, its signed area on the sheet, and where
/// the middle of it falls.
///
/// The sign is what tells an island from a lagoon, and it is the survey's
/// land-on-the-left convention paying out a second time: a ring walked with
/// the land always on the left runs anticlockwise around land and clockwise
/// around enclosed water, so land inside is exactly a positive shoelace sum
/// on the sheet. No second look at any height field, and no flag stored — the
/// direction of travel *is* the answer.
///
/// Consecutive duplicate points — the joins, where one run's end is the next
/// run's identical start — contribute nothing to any measure, so a chain can
/// be fed through whole without trimming them.
fn measure(ring: &mut dyn Iterator<Item = Vec2>) -> Ring {
    let mut least = Vec2::splat(f32::INFINITY);
    let mut most = Vec2::splat(f32::NEG_INFINITY);
    let mut area = 0.0;
    let mut first = None;
    let mut previous: Option<Vec2> = None;
    for point in ring.map(on_the_sheet) {
        least = least.min(point);
        most = most.max(point);
        if let Some(previous) = previous {
            area += previous.perp_dot(point);
        }
        first.get_or_insert(point);
        previous = Some(point);
    }
    // The ring is closed, so the walk back from the last point to the first
    // is part of it whether or not the points spell it out.
    if let (Some(first), Some(last)) = (first, previous) {
        area += last.perp_dot(first);
    }
    Ring {
        extent: (most - least).max_element(),
        area: area / 2.0,
        centre: (least + most) / 2.0,
    }
}

/// Which grid edge a contour crosses — named by the edge rather than by where
/// on it the crossing lands.
///
/// Naming the *edge* is what makes the strokes joinable without comparing
/// floats: two neighbouring cells sharing an edge work the crossing out from
/// the same two corner heights, so they agree exactly, and chaining is integer
/// bookkeeping instead of a hunt for points that are nearly equal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Crossing {
    iz: usize,
    ix: usize,
    /// Whether the edge runs along the grid's x axis or its z axis.
    along: Axis,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Axis {
    X,
    Z,
}

impl Crossing {
    /// Where the given level cuts this edge, in chunk-local metres.
    ///
    /// Straight linear interpolation between the two corner heights, which is
    /// what the mesh does between the same two corners — so the line on the
    /// chart is the line the player could walk to.
    fn at(self, heights: &[f32], level: f32) -> Vec2 {
        let corner = |ix: usize, iz: usize| heights[iz * FACET_VERTS + ix];
        let near = corner(self.ix, self.iz);
        let far = match self.along {
            Axis::X => corner(self.ix + 1, self.iz),
            Axis::Z => corner(self.ix, self.iz + 1),
        };
        // The two corners straddle the level — one at or above it, the other
        // strictly below — so the difference is never zero and this never
        // divides by one.
        let t = (near - level) / (near - far);
        let along = match self.along {
            Axis::X => Vec2::new(t, 0.0),
            Axis::Z => Vec2::new(0.0, t),
        };
        (Vec2::new(self.ix as f32, self.iz as f32) + along) * FACET_METRES
    }
}

/// Whether a corner height is on the shallow side of a level. Sitting exactly
/// on it counts as shallow, so that a corner at the level belongs to one side
/// rather than to neither and the classification is total.
fn is_above(height: f32, level: f32) -> bool {
    height >= level
}

/// The directed contour segments crossing one cell of the facet grid.
///
/// Every segment is emitted with the **land on its left**, which is the one
/// convention the rest of this module leans on. It is what hangs the shore
/// ticks on the correct side without a second look at the height field, and it
/// is what makes the runs chain unambiguously: each crossing ends exactly one
/// segment and starts exactly one other, so following the contour is a lookup
/// rather than a search.
///
/// The two ambiguous cases are the saddles — opposite corners alike, the other
/// two alike and different — where the cell can be read as two capes or as one
/// isthmus. They are settled on the cell's own average, which is the nearest
/// thing to asking the height field what is actually in the middle of it.
fn segments(cell: (usize, usize), heights: &[f32], level: f32) -> Vec<(Crossing, Crossing)> {
    let (ix, iz) = cell;
    let corner = |cx: usize, cz: usize| heights[cz * FACET_VERTS + cx];
    let (tl, tr) = (corner(ix, iz), corner(ix + 1, iz));
    let (bl, br) = (corner(ix, iz + 1), corner(ix + 1, iz + 1));

    let top = Crossing {
        ix,
        iz,
        along: Axis::X,
    };
    let bottom = Crossing {
        ix,
        iz: iz + 1,
        along: Axis::X,
    };
    let left = Crossing {
        ix,
        iz,
        along: Axis::Z,
    };
    let right = Crossing {
        ix: ix + 1,
        iz,
        along: Axis::Z,
    };

    let case = u8::from(is_above(tl, level))
        | u8::from(is_above(tr, level)) << 1
        | u8::from(is_above(br, level)) << 2
        | u8::from(is_above(bl, level)) << 3;
    // How a saddle is read: land through the middle, or sea through it.
    let middle_is_land = is_above((tl + tr + bl + br) / 4.0, level);

    match case {
        0 | 15 => Vec::new(),
        1 => vec![(left, top)],
        2 => vec![(top, right)],
        3 => vec![(left, right)],
        4 => vec![(right, bottom)],
        6 => vec![(top, bottom)],
        7 => vec![(left, bottom)],
        8 => vec![(bottom, left)],
        9 => vec![(bottom, top)],
        11 => vec![(bottom, right)],
        12 => vec![(right, left)],
        13 => vec![(right, top)],
        14 => vec![(top, left)],
        // Land at top-left and bottom-right. Joined through the middle, the two
        // sea corners are the pockets to be drawn around; parted, the two land
        // corners are.
        5 if middle_is_land => vec![(right, top), (left, bottom)],
        5 => vec![(left, top), (right, bottom)],
        // And the same the other way up.
        10 if middle_is_land => vec![(top, left), (bottom, right)],
        _ => vec![(top, right), (bottom, left)],
    }
}

/// What one walk of a chunk's height grid is worth keeping: the waterline, and
/// the edge of the shallows outside it.
///
/// The two are held together because they are found together and go stale
/// together — a chunk is surveyed once, and either both its lines are known or
/// neither is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Soundings {
    /// Where the ground meets the sea. The line the chart is *about*: what the
    /// islands are measured from, what the ticks hang off, what closes.
    /// Readable by the logbook, which writes the survey down between visits.
    pub(crate) coast: Vec<Coast>,
    /// Where the water reaches [`SHOAL_DEPTH`]. Drawn and nothing else — it
    /// rings nothing, names nothing and closes nothing.
    pub(crate) shoal: Vec<Coast>,
}

/// Both lines of one chunk's ground, ready to be kept.
pub fn survey(heights: &[f32]) -> Soundings {
    Soundings {
        coast: contour(heights, 0.0),
        shoal: contour(heights, SHOAL_DEPTH),
    }
}

/// Every run of one level across one chunk's height grid.
///
/// The whole survey: contour, chain, simplify, quantise. A chunk with no
/// crossing in it — open water, or ground well inland — comes back empty, which
/// is the common answer and costs one walk of the grid and nothing else.
fn contour(heights: &[f32], level: f32) -> Vec<Coast> {
    // Where each crossing leads, and what led to it. Both are functions rather
    // than fan-outs: an interior edge is shared by exactly two cells, and the
    // land-on-the-left convention makes it the end of one cell's segment and
    // the start of the other's.
    let mut next: HashMap<Crossing, Crossing> = HashMap::default();
    let mut previous: HashSet<Crossing> = HashSet::default();
    // Kept in grid order so that two runs of the same shape come out in the
    // same order every time, whatever a hash map felt like doing.
    let mut starts: Vec<Crossing> = Vec::new();
    for iz in 0..FACET_QUADS {
        for ix in 0..FACET_QUADS {
            for (from, to) in segments((ix, iz), heights, level) {
                next.insert(from, to);
                previous.insert(to);
                starts.push(from);
            }
        }
    }

    let mut walked: HashSet<Crossing> = HashSet::default();
    let mut open: Vec<Vec<Crossing>> = Vec::new();
    let mut closed: Vec<Vec<Crossing>> = Vec::new();

    // The open runs first, from every crossing that nothing leads to — which is
    // a crossing on the chunk's own border, where the coast leaves the ground
    // this chunk covers.
    for start in starts.iter().copied() {
        if previous.contains(&start) || !walked.insert(start) {
            continue;
        }
        let mut run = vec![start];
        let mut here = start;
        while let Some(&on) = next.get(&here) {
            if !walked.insert(on) {
                break;
            }
            run.push(on);
            here = on;
        }
        open.push(run);
    }

    // Then whatever is left, which can only be loops closing inside the chunk:
    // an islet, or a lagoon.
    for start in starts.iter().copied() {
        if !walked.insert(start) {
            continue;
        }
        let mut run = vec![start];
        let mut here = start;
        while let Some(&on) = next.get(&here) {
            if !walked.insert(on) {
                break;
            }
            run.push(on);
            here = on;
        }
        closed.push(run);
    }

    let build = |run: Vec<Crossing>, closed: bool| {
        let points: Vec<Vec2> = run.iter().map(|c| c.at(heights, level)).collect();
        if closed && spread(&points) < LEAST_ISLET {
            return None;
        }
        let points = simplify(&points, closed, TOLERANCE);
        (points.len() >= 2).then(|| Coast {
            marks: points.into_iter().map(Mark::of).collect(),
            closed,
        })
    };

    open.into_iter()
        .filter_map(|run| build(run, false))
        .chain(closed.into_iter().filter_map(|run| build(run, true)))
        .collect()
}

/// Ramer–Douglas–Peucker: the fewest points that stay within `tolerance` of the
/// line they were taken from.
///
/// A closed ring is cut at the point furthest from its first and simplified as
/// two open chains, because the algorithm needs two ends and a ring has none.
/// Cutting at the furthest point rather than anywhere is what keeps the cut off
/// a straight — a cut in the middle of one would pin a point that is not a
/// corner and let the real corners move around it.
fn simplify(points: &[Vec2], closed: bool, tolerance: f32) -> Vec<Vec2> {
    if points.len() < 3 {
        return points.to_vec();
    }
    if !closed {
        return thin(points, tolerance);
    }

    let opposite = points
        .iter()
        .enumerate()
        .max_by(|a, b| {
            a.1.distance_squared(points[0])
                .total_cmp(&b.1.distance_squared(points[0]))
        })
        .map(|(at, _)| at)
        .expect("a ring has points");

    let mut ring = thin(&points[..=opposite], tolerance);
    // The other half runs from the cut back round to the start. Its first point
    // is the cut, which `ring` already ends on, and its last is the start,
    // which closing the ring supplies — so only what lies between them is new.
    let mut back: Vec<Vec2> = points[opposite..].to_vec();
    back.push(points[0]);
    let back = thin(&back, tolerance);
    ring.extend(back[1..back.len() - 1].iter().copied());
    ring
}

/// The open-chain half of [`simplify`], iterative rather than recursive so a
/// pathological coastline cannot run the stack out.
fn thin(points: &[Vec2], tolerance: f32) -> Vec<Vec2> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;

    let mut spans = vec![(0usize, points.len() - 1)];
    while let Some((first, last)) = spans.pop() {
        if last <= first + 1 {
            continue;
        }
        let (mut worst, mut furthest) = (first, 0.0f32);
        for (at, point) in points.iter().enumerate().take(last).skip(first + 1) {
            let off = off_the_line(*point, points[first], points[last]);
            if off > furthest {
                (worst, furthest) = (at, off);
            }
        }
        if furthest <= tolerance {
            continue;
        }
        keep[worst] = true;
        spans.push((first, worst));
        spans.push((worst, last));
    }

    points
        .iter()
        .zip(keep)
        .filter_map(|(point, keep)| keep.then_some(*point))
        .collect()
}

/// The longer side of what a run of points covers, in metres.
fn spread(points: &[Vec2]) -> f32 {
    let (least, most) = points.iter().fold(
        (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY)),
        |(least, most), point| (least.min(*point), most.max(*point)),
    );
    (most - least).max_element()
}

/// How far a point lies off the segment from `from` to `to`.
fn off_the_line(point: Vec2, from: Vec2, to: Vec2) -> f32 {
    let span = to - from;
    let length = span.length();
    if length < f32::EPSILON {
        return point.distance(from);
    }
    (span.x * (from.y - point.y) - (from.x - point.x) * span.y).abs() / length
}

/// Whether any part of a chunk is within sight of a point.
fn in_sight(chunk: IVec2, at: Vec2) -> bool {
    let corner = chunk.as_vec2() * CHUNK_METRES;
    at.clamp(corner, corner + CHUNK_METRES).distance_squared(at) <= SIGHT_RADIUS * SIGHT_RADIUS
}

/// Surveys the chunks that have come within sight of the player.
///
/// Runs whatever the player is doing, the chart included — a boat left drifting
/// keeps finding coast, and a sheet open over it should say so.
fn take_soundings(
    mut chart: ResMut<Chart>,
    ground: Res<Ground>,
    place: PlayerPlace,
    cameras: Query<&MapCamera>,
) {
    // The player when there is one, and otherwise wherever the view is centred
    // — which is where the player is about to be put down.
    let Some(at) = place.on_the_map().or_else(|| {
        cameras
            .single()
            .ok()
            .map(|camera| Vec2::new(camera.focus.x, camera.focus.z))
    }) else {
        return;
    };

    let reach = (SIGHT_RADIUS / CHUNK_METRES).ceil() as i32;
    let centre = chunk_at(at);
    let mut surveyed = 0;
    for dz in -reach..=reach {
        for dx in -reach..=reach {
            if surveyed >= SURVEYS_PER_FRAME {
                return;
            }
            let chunk = centre + IVec2::new(dx, dz);
            if chart.surveyed(chunk) || !in_sight(chunk, at) {
                continue;
            }
            // Ground that has not arrived is passed over rather than recorded,
            // and comes round again on a later frame — it is still in sight,
            // and will be for a good while yet.
            let Some(heights) = ground.heights(chunk) else {
                continue;
            };
            chart.record(chunk, survey(&heights));
            surveyed += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// The sheet
// ---------------------------------------------------------------------------

/// The parchment, and the inks laid on it.
///
/// Flat tones like everything else in this world — see the module docs on why
/// there is no paper here to be stained. The coast is the darkest thing on the
/// sheet, the graticule under it fainter and the rhumb net under that fainter
/// again, so the three read as drawing, ruling and net rather than as three
/// kinds of line. The reader's own mark is the one thing in another colour,
/// which is how the eye finds it.
pub(crate) const PAPER: Color = Color::srgb(0.85, 0.79, 0.64);
pub(crate) const INK: Color = Color::srgb(0.24, 0.17, 0.11);
pub(crate) const INK_DIM: Color = Color::srgb(0.46, 0.37, 0.26);
pub(crate) const INK_FAINT: Color = Color::srgba(0.40, 0.31, 0.21, 0.40);
pub(crate) const INK_GHOST: Color = Color::srgba(0.40, 0.31, 0.21, 0.22);
const MARK_INK: Color = Color::srgb(0.55, 0.16, 0.12);

/// How wide the sheet's lines are drawn, in pixels — pixels rather than metres
/// because a chart's line weight belongs to the engraver and not to the world,
/// so it has to stay put however far the sheet is zoomed.
const COAST_WEIGHT: f32 = 1.8;
const TICK_WEIGHT: f32 = 1.0;
const GRATICULE_WEIGHT: f32 = 1.0;

/// The landward ticks: how far apart along the shore, and how far they reach
/// inland — both in pixels, for the reason the weights are.
const TICK_SPACING: f32 = 9.0;
const TICK_LENGTH: f32 = 4.5;

/// The stipple along the edge of the shallows: how far apart the dots fall
/// along the line, how big each is, and how far off the line they may scatter
/// — all in pixels, for the reason the weights are.
///
/// Scattered rather than laid exactly on the line, because a shoal has no
/// edge: the line is where the ground happens to pass three metres down, and a
/// ruled dotted line would claim a precision the sounding does not have. A
/// ragged band of dots says *shallow about here*, which is the truth, and it is
/// also what an engraver's stipple looks like.
const SHOAL_SPACING: f32 = 5.5;
const SHOAL_DOT: f32 = 1.5;
const SHOAL_SCATTER: f32 = 3.0;

/// The hand the islands are named in: an italic cut of the Fell types, the
/// letterforms of the seventeenth-century press — the nearest a flat sheet
/// comes to the lettering on an engraved chart. Not the menus' serif, which
/// is the machine's own hand and does the furniture; a name written *on* the
/// paper should look written on the paper. Where the file came from, and
/// under what terms, is in `assets/CREDITS.md`.
const NAME_FONT: &str = "fonts/IMFellEnglish-Italic.ttf";

/// How large the names are lettered, in pixels — pixels for the reason the
/// line weights are: lettering belongs to the engraver, not to the world, so
/// it holds its size on the paper however far the sheet is zoomed.
const NAME_SIZE: f32 = 17.0;

/// The most letters a name may run to. A chart has room for a real name and
/// not for a sentence, and the cap is what keeps one island's lettering from
/// being laid across its neighbour's.
const NAME_LENGTH: usize = 24;

/// How far the mouse may wander between press and release and still mean a
/// click rather than a small drag, in pixels. The same hand does both — the
/// sheet is panned with this button — so the two have to be told apart, and
/// they are told apart the way every map application does it.
const CLICK_SLOP: f32 = 5.0;

/// Half-extents of the click target under an island's lettering, in pixels —
/// so the name is still clickable when the sheet is zoomed out far enough
/// that the island itself has become a speck.
const NAME_REACH: Vec2 = Vec2::new(60.0, 14.0);

/// Metres of world to a pixel of sheet, at either end of the zoom and where it
/// opens.
///
/// The default puts about five kilometres across a window: an island the sheet
/// can be read at, with room for the water round it and whatever is next along.
/// The near end is finer than the survey was ever taken at, so there is nothing
/// to be had by going closer; the far end puts eighty kilometres on the sheet,
/// which is a voyage.
const MIN_METRES_PER_PIXEL: f32 = 1.0;
const MAX_METRES_PER_PIXEL: f32 = 64.0;
const DEFAULT_METRES_PER_PIXEL: f32 = 4.0;
/// What one notch of the wheel does to the zoom, geometrically — so a notch
/// moves the sheet by the same proportion however far out it already is, as the
/// world's own camera does.
const ZOOM_STEP: f32 = 1.2;
/// Scroll pixels that count as one notch, for the trackpads that report them.
const PIXELS_PER_NOTCH: f32 = 50.0;

/// How fast the keys slide the sheet, in pixels of paper a second — so panning
/// covers the same amount of *paper* at every zoom, and winding out to look at
/// a voyage does not make the keys crawl.
const PAN_SPEED: f32 = 700.0;

/// The round distances a graticule and a scale bar may be drawn at. A chart is
/// read off round numbers, so the spacing is chosen from this ladder rather
/// than computed to whatever the zoom happens to make convenient.
const ROUND_DISTANCES: [f32; 8] = [
    100.0, 250.0, 500.0, 1000.0, 2000.0, 5000.0, 10_000.0, 25_000.0,
];
/// The fewest pixels apart the graticule may be ruled, and the shortest a scale
/// bar may be drawn — between them, what picks a rung of the ladder.
const GRATICULE_GAP: f32 = 110.0;
const SCALE_BAR_LEAST: f32 = 110.0;

/// How many graticule squares apart the roses that throw the rhumbs stand.
///
/// A whole number of squares rather than a spacing of its own, so the roses
/// keep step with the ruling however the zoom moves it — they are nudged off
/// the crossings themselves (see [`rose_nudge`]), but by a fraction of a
/// square, so the two lattices are still one lattice.
///
/// Six, which is what keeps two or three roses in a window at every zoom. The
/// count matters more than it sounds: a net thrown from one rose is a sunburst
/// and reads as decoration, and it is rays from *different* roses crossing each
/// other that make a sheet look navigated. Many more than three and the paper
/// is a cobweb with a coast somewhere under it.
const RHUMB_SQUARES: f32 = 6.0;

/// How many bearings each rose throws — the thirty-two points of the compass,
/// as a chart of this hand would carry.
const RHUMB_BEARINGS: usize = 32;

/// How heavily a rhumb is ruled, in pixels, and the mark and gap of a dashed
/// one — both for the reason the other weights are in pixels.
const RHUMB_WEIGHT: f32 = 1.0;
const RHUMB_DASH: (f32, f32) = (13.0, 10.0);

/// How far the longest points of a rose on the paper reach, in pixels. Small:
/// it is where the net comes from and not a thing to be read off, and every one
/// of them is saying what the corner rose already said.
const PAPER_ROSE: f32 = 22.0;

/// The rose in the corner, in pixels: how far the longest points of its star
/// reach, how wide the graduated band outside them runs, how far beyond that
/// the letters sit and how large they are set.
///
/// Bigger than the plain lettered circle it replaces, because a sixteen-point
/// star has detail in it that a circle did not: at the old diameter the half
/// winds closed up into a blur.
const ROSE_REACH: f32 = 40.0;
const ROSE_BAND: f32 = 7.0;
const ROSE_LETTER_GAP: f32 = 12.0;
const ROSE_LETTERS: f32 = 14.0;
/// How much room the whole rose takes from its middle, letters and all.
pub(crate) const ROSE_EXTENT: f32 = ROSE_REACH + ROSE_BAND + ROSE_LETTER_GAP + ROSE_LETTERS / 2.0;

/// How heavily a rose's own circles and ticks are ruled, in pixels.
const ROSE_WEIGHT: f32 = 1.0;

/// How many segments a drawn circle is bent from. Enough that the largest
/// circle on the sheet — the corner rose's band — reads as round.
const CIRCLE_FACETS: usize = 64;

/// The sheet's edge, in pixels: how far in from the window the outer rule
/// runs, how wide the graduated band inside it is, and how heavily both rules
/// are ruled.
///
/// A neatline is the edge of the drawing, and a chart drawn to the window's
/// own edge has none — the paper simply runs out wherever the player last
/// dragged their mouse, which reads as a viewport rather than as a sheet. The
/// band is what the engraving would carry a scale in; here it carries the
/// ruling's own graduation, so the edge measures the same thing the paper is
/// ruled by.
const NEATLINE_INSET: f32 = 10.0;
const NEATLINE_BAND: f32 = 7.0;
const NEATLINE_WEIGHT: f32 = 1.2;

/// How many graduations of the band go to one square of the ruling. Ten, so a
/// square is five inked cells and five of paper — fine enough to read as a
/// scale and coarse enough that the cells never close up into a grey rule.
pub(crate) const NEATLINE_CELLS: f32 = 10.0;

/// How much bigger than the window the drawn sheet is built, each way.
///
/// The mesh is built for a window and the camera pans freely inside it, which
/// is what makes dragging the chart cost nothing: only leaving the window
/// rebuilds. The same hysteresis the ground is streamed with, for the same
/// reason — a sheet rebuilt on every frame of a drag would be a mesh uploaded
/// on every frame of a drag.
const SHEET_MARGIN: f32 = 0.5;

/// Where the sheet is being read: what point of the world lies under the middle
/// of the window, and how much world a pixel of it covers.
///
/// A resource rather than state on the camera because it outlives the sheet.
/// Closing the chart and opening it again comes back to the same place at the
/// same zoom, which is what anyone who has just been looking at something
/// expects of putting it down and picking it up.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct ChartView {
    /// The world point in the middle of the window, in metres.
    pub centre: Vec2,
    pub metres_per_pixel: f32,
    /// False until the chart has been opened once, so that the first opening
    /// centres on the player rather than on wherever the world's origin is.
    opened: bool,
}

impl Default for ChartView {
    fn default() -> Self {
        Self {
            centre: Vec2::ZERO,
            metres_per_pixel: DEFAULT_METRES_PER_PIXEL,
            opened: false,
        }
    }
}

/// Where a world point falls on the sheet.
///
/// North is up, and this is the whole of what that means: the sheet's x is the
/// world's x, and its y is the world's *negative* z, because
/// [`protocol::ground::NORTH`] is negative z and a screen's y climbs. Nothing
/// in this module rotates anything, which is the point — a chart that turned
/// with the view would be a compass rose with extra steps.
pub fn on_the_sheet(at: Vec2) -> Vec2 {
    Vec2::new(at.x, -at.y)
}

/// The window of world the sheet shows, in metres.
fn window(view: &ChartView, size: Vec2) -> Rect {
    let half = size * view.metres_per_pixel / 2.0;
    Rect::from_corners(view.centre - half, view.centre + half)
}

/// Marks everything that belongs to the sheet — its camera, its engraving and
/// its furniture — so that nothing of it is left when the chart is rolled up.
#[derive(Component)]
struct ChartSheet;

/// Marks the drawn coastline and the ruling under it, so a rebuild can replace
/// them.
#[derive(Component)]
struct Engraving;

/// Marks the reader's own position on the sheet, which moves every frame while
/// the coast does not.
#[derive(Component)]
struct OwnMark;

/// Marks the rose in the corner, which is pinned to the window while the paper
/// slides under it.
#[derive(Component)]
struct CornerRose;

/// The inks, made once when the sheet is unrolled rather than on every rebuild
/// — a material per pan would be a new asset per pan.
#[derive(Resource)]
struct Inks {
    /// Not an ink at all: the parchment, for the one drawing that covers
    ///rather than marks — see [`sheet_edge`].
    paper: Handle<ColorMaterial>,
    coast: Handle<ColorMaterial>,
    ruling: Handle<ColorMaterial>,
    /// The lit half of a rose's star, and the circles round it.
    dim: Handle<ColorMaterial>,
    /// The rhumb net, under everything.
    rhumb: Handle<ColorMaterial>,
    mark: Handle<ColorMaterial>,
}

/// The lettering hand, asked for once when the sheet is unrolled — the asset
/// server hands back the same font every time, but the handle belongs with
/// the inks: it is part of what the sheet is drawn with.
#[derive(Resource)]
struct Lettering(Handle<Font>);

/// What the drawn sheet was built for, so that a change can be noticed.
///
/// A rebuild is wanted when the zoom has changed — the line weights are in
/// pixels, so what is in the mesh is metres *of this zoom* — or when the view
/// has wandered off the built window, or when the survey has turned up more
/// coast.
#[derive(Resource)]
struct Engraved {
    covered: Rect,
    metres_per_pixel: f32,
}

pub struct ChartPlugin;

impl Plugin for ChartPlugin {
    fn build(&self, app: &mut App) {
        // The bindings are shared with the settings screen and the camera,
        // whichever is built first winning — see `MapCameraPlugin`.
        app.init_resource::<ChartView>()
            .init_resource::<KeyBindings>()
            .add_systems(OnEnter(AppState::InWorld), start_a_chart)
            .add_systems(OnExit(AppState::InWorld), stow_the_chart)
            .add_systems(OnExit(Helm::Chart), roll_up)
            .add_systems(
                Update,
                (
                    take_soundings
                        .run_if(resource_exists::<Chart>.and_then(resource_exists::<Ground>)),
                    chart_key,
                )
                    .run_if(in_state(AppState::InWorld)),
            )
            .add_systems(
                Update,
                (
                    unroll.run_if(no_sheet_yet),
                    find_the_reader,
                    escape_key,
                    write_the_name,
                    click_to_name,
                    pan,
                    zoom,
                    hold_the_sheet,
                    engrave,
                    rule_the_scale,
                    rule_the_edge,
                    pin_the_rose,
                    mark_the_reader,
                )
                    .chain()
                    .run_if(in_state(Helm::Chart).and_then(resource_exists::<Chart>)),
            );
    }
}

/// A world gets a blank chart — or, in a world this machine remembers, the
/// chart the last visit left off with — and takes it with it when it goes:
/// what has been seen is a fact about *this* world, and carrying it into the
/// next would draw one seed's islands on another's water. The logbook is
/// keyed by the world's own id, which is what makes reloading it safe where
/// carrying it over would not be.
fn start_a_chart(
    mut commands: Commands,
    mut view: ResMut<ChartView>,
    logbook: Option<Res<crate::logbook::Logbook>>,
) {
    commands.insert_resource(logbook.map_or_else(Chart::default, |logbook| logbook.charted()));
    *view = ChartView::default();
}

/// Pub within the crate so the logbook's closing write can order itself
/// before this: a chart stowed first would be a chart with nothing left to
/// write down.
pub(crate) fn stow_the_chart(mut commands: Commands) {
    commands.remove_resource::<Chart>();
    commands.remove_resource::<Engraved>();
}

/// Opens the chart, and closes it again.
///
/// Only from the helm and only back to it: the pause menu, the controls screen
/// and the console each have the keyboard for their own reasons while they are
/// up, and a chart key typed into any of them means what that screen says it
/// means.
fn chart_key(
    keys: Res<ButtonInput<KeyCode>>,
    bindings: Res<KeyBindings>,
    helm: Res<State<Helm>>,
    naming: Option<Res<Naming>>,
    mut next: ResMut<NextState<Helm>>,
) {
    // While a name is being written the chart key is a letter of it.
    if naming.is_some() || !keys.just_pressed(bindings.key(Action::Chart)) {
        return;
    }
    match helm.get() {
        Helm::Sailing => next.set(Helm::Chart),
        Helm::Chart => next.set(Helm::Sailing),
        Helm::Paused | Helm::Controls | Helm::Console => {}
    }
}

/// Whether the sheet still has to be laid out.
fn no_sheet_yet(sheets: Query<(), With<ChartSheet>>) -> bool {
    sheets.is_empty()
}

/// Lays the sheet over the world.
///
/// The sheet's camera draws after the world's and clears to parchment, which
/// covers the view and every instrument standing over it in one stroke — so
/// nothing in the world has to be told the chart is up. The world's camera is
/// switched off behind it: there is nothing of it left to see, and a scene
/// drawn to be painted over is a scene drawn for nobody.
///
/// Run on the first frame the chart is up rather than as the state is entered,
/// because it needs the world's camera to already exist and there is one moment
/// when it does not: a run started with `--state chart` enters the state before
/// `Startup` has spawned anything. Entering would have found no camera to take
/// a target from, no camera to switch off, and drawn the chart into a window
/// that a capturing run does not have.
fn unroll(
    mut commands: Commands,
    mut materials: ResMut<Assets<ColorMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    assets: Res<AssetServer>,
    mut view: ResMut<ChartView>,
    place: PlayerPlace,
    mut world_camera: Query<(&mut Camera, &RenderTarget), With<MapCamera>>,
) {
    centre_on(&mut view, place.on_the_map());

    // Wherever the world is being drawn, the chart is drawn over it — which is
    // usually the window, and is an off-screen image in a run that is taking
    // pictures. Copied rather than defaulted so that `--state chart --shot`
    // photographs a chart rather than an empty sheet.
    let mut target = RenderTarget::default();
    for (mut camera, world_target) in &mut world_camera {
        camera.is_active = false;
        target = world_target.clone();
    }

    let inks = Inks {
        paper: materials.add(ColorMaterial::from_color(PAPER)),
        coast: materials.add(ColorMaterial::from_color(INK)),
        ruling: materials.add(ColorMaterial::from_color(INK_FAINT)),
        dim: materials.add(ColorMaterial::from_color(INK_DIM)),
        rhumb: materials.add(ColorMaterial::from_color(INK_GHOST)),
        mark: materials.add(ColorMaterial::from_color(MARK_INK)),
    };
    commands.insert_resource(Lettering(assets.load(NAME_FONT)));

    let sheet = commands
        .spawn((
            Name::new("Chart"),
            ChartSheet,
            Camera2d,
            Camera {
                order: 1,
                clear_color: ClearColorConfig::Custom(PAPER),
                ..default()
            },
            target,
            Projection::Orthographic(OrthographicProjection {
                scale: view.metres_per_pixel,
                ..OrthographicProjection::default_2d()
            }),
            Transform::from_translation(on_the_sheet(view.centre).extend(0.0)),
            DespawnOnExit(Helm::Chart),
        ))
        .id();

    commands.spawn((
        Name::new("Chart mark"),
        OwnMark,
        ChartSheet,
        Transform::default(),
        Visibility::Hidden,
        DespawnOnExit(Helm::Chart),
    ));

    sheet_edge(&mut commands, &mut meshes, &inks);
    corner_rose(&mut commands, &mut meshes, &inks);
    commands.insert_resource(inks);
    furniture(&mut commands, sheet);
}

/// Rolls the sheet up and gives the world its camera back.
fn roll_up(mut commands: Commands, mut world_camera: Query<&mut Camera, With<MapCamera>>) {
    for mut camera in &mut world_camera {
        camera.is_active = true;
    }
    commands.remove_resource::<Inks>();
    commands.remove_resource::<Lettering>();
    // A name still being written is left unwritten: the ways off the chart
    // with the pen down all mean the player's attention went elsewhere, and
    // half a name committed by a distraction would be worse than the draft
    // lost. The engraving goes with the sheet too, so the next opening draws
    // one for wherever the reader has got to rather than trusting a window
    // from before.
    commands.remove_resource::<Naming>();
    commands.remove_resource::<Engraved>();
}

/// Where a sheet that has never been read opens: on the reader.
///
/// Every opening after the first comes back to where the sheet was left, which
/// is what anyone who has just been looking at something expects of putting it
/// down and picking it up. The first has nowhere of its own to be.
fn centre_on(view: &mut ChartView, reader: Option<Vec2>) {
    if view.opened {
        return;
    }
    if let Some(at) = reader {
        view.centre = at;
        view.opened = true;
    }
}

/// The same, a frame or more later.
///
/// A chart opened before there is anybody to centre it on — which is what
/// `--state chart` does, the world's boat being launched on the frame after the
/// state is entered — would otherwise open on the world's origin and stay
/// there, the centring in [`unroll`] having already had its one chance.
fn find_the_reader(mut view: ResMut<ChartView>, place: PlayerPlace) {
    if view.opened {
        return;
    }
    let mut wanted = *view;
    centre_on(&mut wanted, place.on_the_map());
    view.set_if_neq(wanted);
}

/// Slides the sheet under the reader.
///
/// Dragging with the mouse is the obvious hand, and the keys that drive the
/// boat are the other: ahead and astern run the sheet north and south, the helm
/// runs it east and west. Those are already whatever the player chose them to
/// be, and the arrows shadow them permanently — see [`KeyBindings::driving`] —
/// so a chart is pannable whatever anyone has done to the bindings.
fn pan(
    mut view: ResMut<ChartView>,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    bindings: Res<KeyBindings>,
    naming: Option<Res<Naming>>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
) {
    // While a name is being written the keys spell it — see [`Naming`] — and
    // the mouse is on its way to a click, which must not shove the sheet.
    if naming.is_some() {
        return;
    }
    let (ahead, helm) = bindings.driving(&keys);
    // The helm's positive is to port, which on a sheet with north up is west.
    let keyed = Vec2::new(-helm, ahead) * PAN_SPEED * view.metres_per_pixel * time.delta_secs();
    // A drag moves the paper with the hand, so the view goes the other way.
    let dragged = if buttons.pressed(MouseButton::Left) {
        Vec2::new(-motion.delta.x, motion.delta.y) * view.metres_per_pixel
    } else {
        Vec2::ZERO
    };

    let step = keyed + dragged;
    if step == Vec2::ZERO {
        return;
    }
    // The sheet's y climbs where the world's z falls, so a step north across
    // the paper is a step of negative z in the world.
    view.centre += Vec2::new(step.x, -step.y);
}

fn zoom(mut view: ResMut<ChartView>, scroll: Res<AccumulatedMouseScroll>) {
    let notches = match scroll.unit {
        MouseScrollUnit::Line => scroll.delta.y,
        MouseScrollUnit::Pixel => scroll.delta.y / PIXELS_PER_NOTCH,
    };
    if notches == 0.0 {
        return;
    }
    view.metres_per_pixel = (view.metres_per_pixel / ZOOM_STEP.powf(notches))
        .clamp(MIN_METRES_PER_PIXEL, MAX_METRES_PER_PIXEL);
}

/// Puts the sheet's camera where the view says.
///
/// One place writes the camera, and everything that moves the sheet writes the
/// view instead — so panning, zooming and opening on the reader cannot each
/// have their own opinion about which of the two is the truth.
fn hold_the_sheet(
    view: Res<ChartView>,
    mut sheet: Query<(&mut Transform, &mut Projection), With<ChartSheet>>,
) {
    if !view.is_changed() {
        return;
    }
    for (mut transform, mut projection) in &mut sheet {
        transform.translation = on_the_sheet(view.centre).extend(transform.translation.z);
        if let Projection::Orthographic(ortho) = &mut *projection {
            ortho.scale = view.metres_per_pixel;
        }
    }
}

/// What it takes to put a layer of drawing on the sheet: somewhere to spawn it,
/// somewhere to put the mesh, the inks to draw it in, and what is already
/// drawn and about to be replaced.
#[derive(bevy::ecs::system::SystemParam)]
struct Engraver<'w, 's> {
    commands: Commands<'w, 's>,
    meshes: ResMut<'w, Assets<Mesh>>,
    inks: Res<'w, Inks>,
    lettering: Res<'w, Lettering>,
    drawn: Query<'w, 's, Entity, With<Engraving>>,
}

/// Draws the sheet, when there is a reason to draw it again.
fn engrave(
    mut engraver: Engraver,
    chart: Res<Chart>,
    view: Res<ChartView>,
    naming: Option<Res<Naming>>,
    mut was_naming: Local<bool>,
    engraved: Option<Res<Engraved>>,
    sheet: Query<&Camera, With<ChartSheet>>,
) {
    // The pen moving is a reason to redraw: each letter typed, the pen going
    // down, and — since a cancelled draft leaves the chart itself untouched —
    // the pen coming up again, which nothing but this remembers.
    let pen_moved = naming.as_ref().is_some_and(|naming| naming.is_changed())
        || naming.is_some() != *was_naming;
    *was_naming = naming.is_some();
    // The sheet's own viewport rather than the window's size, because they are
    // not always the same thing: a run taking pictures draws to an off-screen
    // image and has no window at all. `None` on the frame the camera is spawned
    // and never again — nothing is engraved that frame, and the next one has
    // the same reason to draw as this one did.
    let Some(size) = sheet
        .single()
        .ok()
        .and_then(|camera| camera.logical_viewport_size())
    else {
        return;
    };
    let showing = window(&view, size);
    if let Some(engraved) = &engraved {
        let held = engraved.covered.contains(showing.min) && engraved.covered.contains(showing.max);
        if held
            && engraved.metres_per_pixel == view.metres_per_pixel
            && !chart.is_changed()
            && !pen_moved
        {
            return;
        }
    }

    let stale: Vec<Entity> = engraver.drawn.iter().collect();
    for entity in stale {
        engraver.commands.entity(entity).despawn();
    }

    let margin = showing.size() * SHEET_MARGIN;
    let covered = Rect::from_corners(showing.min - margin, showing.max + margin);
    let metres_per_pixel = view.metres_per_pixel;
    let paper = |pixels: f32| pixels * metres_per_pixel;
    let on_paper = Rect::from_corners(on_the_sheet(covered.min), on_the_sheet(covered.max));

    // The net first and furthest under: the rhumbs are the first thing on the
    // paper and the last thing to be read, so everything else is drawn over
    // them.
    let spacing = rhumb_spacing(metres_per_pixel);
    let mut net = Strokes::default();
    let mut inked = Strokes::default();
    let mut dimmed = Strokes::default();
    for centre in rhumb_roses(on_paper, spacing) {
        rhumbs(
            &mut net,
            on_paper,
            centre,
            paper(PAPER_ROSE),
            paper(RHUMB_WEIGHT),
            (paper(RHUMB_DASH.0), paper(RHUMB_DASH.1)),
        );
        star(&mut inked, &mut dimmed, centre, paper(PAPER_ROSE));
        circle(
            &mut dimmed,
            centre,
            paper(PAPER_ROSE),
            paper(ROSE_WEIGHT * 0.8),
        );
    }
    if let Some(mesh) = net.mesh() {
        let mesh = engraver.meshes.add(mesh);
        let ink = engraver.inks.rhumb.clone();
        engraver
            .commands
            .spawn(engraving("Chart rhumbs", mesh, ink, 0.0));
    }

    // The ruling over the net and under the coast: it is the paper's own, and
    // the coast is drawn on top of it.
    let mut ruling = Strokes::default();
    graticule(
        &mut ruling,
        covered,
        metres_per_pixel,
        paper(GRATICULE_WEIGHT),
    );
    if let Some(mesh) = ruling.mesh() {
        let mesh = engraver.meshes.add(mesh);
        let ink = engraver.inks.ruling.clone();
        engraver
            .commands
            .spawn(engraving("Chart graticule", mesh, ink, 0.5));
    }

    // The roses themselves, over their own net and the ruling both.
    for (strokes, ink, name) in [
        (inked, engraver.inks.coast.clone(), "Chart roses"),
        (dimmed, engraver.inks.dim.clone(), "Chart roses, lit"),
    ] {
        if let Some(mesh) = strokes.mesh() {
            let mesh = engraver.meshes.add(mesh);
            engraver.commands.spawn(engraving(name, mesh, ink, 0.75));
        }
    }

    // The shallows under the shore, so a coast is never drawn through its own
    // stipple.
    let mut shallows = Strokes::default();
    let mut shore = Strokes::default();
    for (chunk, found) in chart.within(covered) {
        for (nth, bank) in found.shoal.iter().enumerate() {
            let points: Vec<Vec2> = bank.points(chunk).map(on_the_sheet).collect();
            stipple(
                &mut shallows,
                &points,
                bank.closed,
                scramble((chunk.x as u32) ^ (chunk.y as u32).rotate_left(16) ^ nth as u32),
                (paper(SHOAL_SPACING), paper(SHOAL_DOT), paper(SHOAL_SCATTER)),
            );
        }
        for coast in &found.coast {
            let points: Vec<Vec2> = coast.points(chunk).map(on_the_sheet).collect();
            shore.run(&points, coast.closed, paper(COAST_WEIGHT));
            ticks(
                &mut shore,
                &points,
                coast.closed,
                paper(TICK_SPACING),
                paper(TICK_LENGTH),
                paper(TICK_WEIGHT),
            );
        }
    }
    if let Some(mesh) = shallows.mesh() {
        let mesh = engraver.meshes.add(mesh);
        let ink = engraver.inks.dim.clone();
        engraver
            .commands
            .spawn(engraving("Chart shallows", mesh, ink, 0.9));
    }
    if let Some(mesh) = shore.mesh() {
        let mesh = engraver.meshes.add(mesh);
        let ink = engraver.inks.coast.clone();
        engraver
            .commands
            .spawn(engraving("Chart coastline", mesh, ink, 1.0));
    }

    // The names, over the ink. Every island the survey has closed carries
    // one — the player's own if they have written one, and a placeholder
    // until they do. The island under the pen shows the draft instead, caret
    // and all: the lettering is the text field, because a chart is written
    // on, not filled in. The walk is over the whole chart rather than the
    // window — a chain can cross any number of chunks, so a ring cannot be
    // closed from a window's worth — and only the lettering that lands on
    // the paper is spawned.
    for island in chart.islands() {
        if !on_paper.contains(island.centre) {
            continue;
        }
        let lettered = match &naming {
            Some(naming) if naming.island == island.id => format!("{}|", naming.draft),
            _ => island
                .name
                .clone()
                .unwrap_or_else(|| "Unnamed island".to_string()),
        };
        engraver.commands.spawn((
            Name::new("Chart lettering"),
            Engraving,
            ChartSheet,
            Text2d::new(lettered),
            TextFont {
                font: FontSource::Handle(engraver.lettering.0.clone()),
                font_size: FontSize::Px(NAME_SIZE),
                ..default()
            },
            TextColor(INK),
            // Between the coast and the reader's own mark, scaled like the
            // line weights so the name keeps its size on the paper.
            Transform::from_translation(island.centre.extend(1.5))
                .with_scale(Vec3::splat(metres_per_pixel)),
            DespawnOnExit(Helm::Chart),
        ));
    }

    engraver.commands.insert_resource(Engraved {
        covered,
        metres_per_pixel,
    });
}

/// One layer of the drawn sheet.
fn engraving(
    name: &'static str,
    mesh: Handle<Mesh>,
    ink: Handle<ColorMaterial>,
    z: f32,
) -> impl Bundle {
    (
        Name::new(name),
        Engraving,
        ChartSheet,
        Mesh2d(mesh),
        MeshMaterial2d(ink),
        Transform::from_xyz(0.0, 0.0, z),
        DespawnOnExit(Helm::Chart),
    )
}

/// Keeps the reader's own mark where they actually are, pointing the way they
/// are actually heading.
///
/// Its own entity rather than part of the engraving, because it moves every
/// frame — a boat left drifting while the chart is open really is drifting —
/// and the coast does not. Scaled by the zoom so that it stays the same size on
/// the paper however much world the sheet is showing.
/// A set rather than two parameters because both halves are transforms, and
/// Bevy will not let one system hold `&Transform` and `&mut Transform` at once
/// — the same wall `PlayerPlace` and `PlayerSweep` are two separate types for.
/// Here the two halves are one job, so they are one system holding them in
/// turn: read where the reader is, then write where their mark goes.
#[allow(clippy::type_complexity)]
fn mark_the_reader(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    inks: Res<Inks>,
    view: Res<ChartView>,
    mut marking: ParamSet<(
        PlayerPlace,
        Query<(Entity, &mut Transform, &mut Visibility, Has<Mesh2d>), With<OwnMark>>,
    )>,
) {
    let place = marking.p0();
    let (where_they_are, which_way) = (place.on_the_map(), place.heading());

    let mut marks = marking.p1();
    let Ok((entity, mut transform, mut visibility, drawn)) = marks.single_mut() else {
        return;
    };
    let (Some(at), Some(heading)) = (where_they_are, which_way) else {
        *visibility = Visibility::Hidden;
        return;
    };

    if !drawn {
        commands.entity(entity).insert((
            Mesh2d(meshes.add(readers_mark())),
            MeshMaterial2d(inks.mark.clone()),
        ));
    }

    *visibility = Visibility::Visible;
    transform.translation = on_the_sheet(at).extend(2.0);
    transform.scale = Vec3::splat(view.metres_per_pixel);
    transform.rotation = Quat::from_rotation_z(bearing_on_the_sheet(heading));
}

/// The turn that lays a mark drawn pointing up the sheet along a world heading.
///
/// Up the sheet is north, so this is the heading's bearing — and a bearing runs
/// clockwise while a rotation runs the other way, which is the whole of why the
/// x term is negated rather than the y.
fn bearing_on_the_sheet(heading: Vec2) -> f32 {
    let along = on_the_sheet(heading);
    f32::atan2(-along.x, along.y)
}

/// The reader's own mark: a plain arrowhead, in pixels of paper.
///
/// A drawn ship would be a picture of a ship eight pixels across. What the mark
/// has to say is *here*, and *this way*, and an arrowhead says both without
/// pretending to be anything else.
fn readers_mark() -> Mesh {
    let points = [
        Vec2::new(0.0, 7.0),
        Vec2::new(-4.5, -5.0),
        Vec2::new(0.0, -2.0),
        Vec2::new(4.5, -5.0),
    ];
    let mut strokes = Strokes::default();
    strokes.triangle(points[0], points[1], points[2]);
    strokes.triangle(points[0], points[2], points[3]);
    strokes.mesh().expect("the mark is never empty")
}

// ---------------------------------------------------------------------------
// Naming
// ---------------------------------------------------------------------------

/// A name being written on the sheet: the island under the pen, and the
/// letters so far.
///
/// While this exists the keyboard is the pen's: the chart key spells a
/// letter, the driving keys spell letters, and Escape means put the pen down
/// — so [`chart_key`] and [`pan`] stand down while it does, and
/// [`escape_key`] hears Escape first. There is no dialog and no state for
/// one, because a chart is written on, not filled in: the island's own
/// lettering shows the draft, with a caret on the end.
#[derive(Resource)]
pub struct Naming {
    island: IVec2,
    draft: String,
}

/// Escape on the chart: with a name half-written it puts the pen down and
/// leaves the name as it was, and otherwise it closes the sheet — one step
/// back, the same as from the pause menu. The chart hears its own Escape the
/// way the console does, and for the same reason: while a name is being
/// written the key means something the helm cannot know about.
fn escape_key(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    naming: Option<Res<Naming>>,
    mut next: ResMut<NextState<Helm>>,
) {
    if !keys.just_pressed(KeyCode::Escape) {
        return;
    }
    if naming.is_some() {
        commands.remove_resource::<Naming>();
    } else {
        next.set(Helm::Sailing);
    }
}

/// The pen at work: letters typed go into the draft, Backspace takes one
/// back, and Enter writes the name against the island — where writing an
/// emptied draft washes the name off, which is how a mistake is undone.
///
/// Asks what the keyboard *typed* rather than which positions were pressed,
/// the same way the console and the menus' two fields do, so a name can hold
/// whatever a layout can produce.
fn write_the_name(
    mut commands: Commands,
    mut chart: ResMut<Chart>,
    naming: Option<ResMut<Naming>>,
    mut presses: MessageReader<KeyboardInput>,
) {
    let Some(mut naming) = naming else {
        // Drained even with no pen down, so that picking an island up does
        // not deliver everything typed since the chart was opened.
        presses.clear();
        return;
    };

    for press in presses.read() {
        // A held key repeats, which is what a text field wants: holding
        // backspace should clear the name rather than one letter of it.
        if press.state != ButtonState::Pressed {
            continue;
        }
        match press.key_code {
            KeyCode::Enter | KeyCode::NumpadEnter => {
                chart.christen(naming.island, &naming.draft);
                commands.remove_resource::<Naming>();
                return;
            }
            KeyCode::Backspace => {
                naming.draft.pop();
            }
            _ => {
                // The space named rather than read off the key, as the
                // console found before this did: it arrives as [`Key::Space`]
                // and not as a character.
                let typed = match &press.logical_key {
                    Key::Character(typed) => typed.as_str(),
                    Key::Space => " ",
                    _ => continue,
                };
                for letter in typed.chars().filter(|c| !c.is_control()) {
                    if naming.draft.chars().count() < NAME_LENGTH {
                        naming.draft.push(letter);
                    }
                }
            }
        }
    }
}

/// The hand on the sheet, watched for a click: the button, the motion that
/// tells a click from a drag, and what it takes to say where on the sheet the
/// click landed. One parameter rather than five for the same reason
/// [`Engraver`] is one — they are one job.
#[derive(bevy::ecs::system::SystemParam)]
struct Pointer<'w, 's> {
    buttons: Res<'w, ButtonInput<MouseButton>>,
    motion: Res<'w, AccumulatedMouseMotion>,
    dragged: Local<'s, f32>,
    windows: Query<'w, 's, &'static Window, With<PrimaryWindow>>,
    sheet: Query<'w, 's, (&'static Camera, &'static GlobalTransform), With<ChartSheet>>,
}

impl Pointer<'_, '_> {
    /// Where a click landed on the sheet, if this frame ended one.
    ///
    /// A click is a release that never became a drag, which is the only way
    /// to share the button with panning: the hand that drags the sheet and
    /// the hand that taps an island are the same hand on the same button.
    fn clicked(&mut self) -> Option<Vec2> {
        if self.buttons.just_pressed(MouseButton::Left) {
            *self.dragged = 0.0;
        } else if self.buttons.pressed(MouseButton::Left) {
            *self.dragged += self.motion.delta.length();
        }
        if !self.buttons.just_released(MouseButton::Left) || *self.dragged > CLICK_SLOP {
            return None;
        }
        // The cursor through the sheet's own camera, so the point comes back
        // in the sheet's coordinates at whatever pan and zoom the sheet is
        // at. A run taking pictures has no window and no cursor, and takes
        // no clicks.
        let cursor = self
            .windows
            .iter()
            .next()
            .and_then(Window::cursor_position)?;
        let (camera, placed) = self.sheet.single().ok()?;
        camera.viewport_to_world_2d(placed, cursor).ok()
    }
}

/// Picks the pen up and puts it down: a click on an island starts writing its
/// name, and a click anywhere with a name in hand writes that name as it
/// stands — clicking away is finishing, not abandoning, because a half-typed
/// name lost to a stray click would be the sheet eating somebody's work.
fn click_to_name(
    mut commands: Commands,
    mut chart: ResMut<Chart>,
    view: Res<ChartView>,
    naming: Option<Res<Naming>>,
    mut pointer: Pointer,
) {
    let Some(at) = pointer.clicked() else {
        return;
    };

    let hit = hit_island(&chart.islands(), at, view.metres_per_pixel);
    if let Some(naming) = naming {
        chart.christen(naming.island, &naming.draft);
        commands.remove_resource::<Naming>();
        // Clicking the island being written is only ever finishing it.
        if hit.as_ref().map(|island| island.id) == Some(naming.island) {
            return;
        }
    }
    if let Some(island) = hit {
        commands.insert_resource(Naming {
            island: island.id,
            draft: island.name.unwrap_or_default(),
        });
    }
}

/// The island a click on the sheet lands on, if any: within the island's own
/// bounds, or within the stretch of paper its lettering sits on — and where
/// those crowd, whichever island's middle lies nearest.
fn hit_island(islands: &[Island], at: Vec2, metres_per_pixel: f32) -> Option<Island> {
    islands
        .iter()
        .filter(|island| {
            let half = Vec2::splat(island.extent / 2.0).max(NAME_REACH * metres_per_pixel);
            let off = (at - island.centre).abs();
            off.x <= half.x && off.y <= half.y
        })
        .min_by(|a, b| {
            a.centre
                .distance_squared(at)
                .total_cmp(&b.centre.distance_squared(at))
        })
        .cloned()
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

/// Lines being turned into a mesh. Everything on this sheet is a stroke of some
/// width, so there is one accumulator and one triangle list for the lot.
///
/// Strokes rather than a line list because a line list is one pixel wide
/// whatever anybody wants, and the weight of a line is half of what makes a
/// chart look drawn rather than plotted.
#[derive(Default)]
struct Strokes {
    positions: Vec<[f32; 3]>,
}

impl Strokes {
    fn triangle(&mut self, a: Vec2, b: Vec2, c: Vec2) {
        for point in [a, b, c] {
            self.positions.push([point.x, point.y, 0.0]);
        }
    }

    /// A filled rectangle — the one thing here that is not a stroke, and only
    /// ever used for a solid block: the neatline's graduations, and the paper
    /// laid over the engraving outside it.
    fn quad(&mut self, rect: Rect) {
        let (nw, se) = (
            Vec2::new(rect.min.x, rect.max.y),
            Vec2::new(rect.max.x, rect.min.y),
        );
        self.triangle(rect.min, se, rect.max);
        self.triangle(rect.min, rect.max, nw);
    }

    /// One dot of a stipple.
    fn dot(&mut self, at: Vec2, size: f32) {
        self.quad(Rect::from_center_size(at, Vec2::splat(size)));
    }

    /// One straight stroke of the given width.
    fn segment(&mut self, from: Vec2, to: Vec2, width: f32) {
        let along = to - from;
        if along.length_squared() < f32::EPSILON {
            return;
        }
        let off = along.normalize().perp() * width / 2.0;
        self.triangle(from - off, from + off, to + off);
        self.triangle(from - off, to + off, to - off);
    }

    /// A whole run of them, with the corners filled in.
    ///
    /// The little square at each joint is what stops a sharp turn opening a
    /// notch on the outside of the bend — a coastline after simplification
    /// turns sharply quite often, that being the point of simplifying it.
    fn run(&mut self, points: &[Vec2], closed: bool, width: f32) {
        for pair in points.windows(2) {
            self.segment(pair[0], pair[1], width);
        }
        if closed {
            if let (Some(last), Some(first)) = (points.last(), points.first()) {
                self.segment(*last, *first, width);
            }
        }
        let half = width / 2.0;
        for joint in points {
            let (nw, ne) = (*joint + Vec2::new(-half, half), *joint + Vec2::splat(half));
            let (sw, se) = (*joint - Vec2::splat(half), *joint + Vec2::new(half, -half));
            self.triangle(sw, se, ne);
            self.triangle(sw, ne, nw);
        }
    }

    /// The mesh, or `None` where nothing was drawn.
    fn mesh(self) -> Option<Mesh> {
        let drawn = !self.positions.is_empty();
        drawn.then(move || self.drawn())
    }

    /// The mesh whether anything was drawn or not — for the layers that are
    /// always on the sheet and have their mesh written over in place rather
    /// than being respawned.
    fn drawn(self) -> Mesh {
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
    }
}

/// Walks a line at an even pace, handing back each footfall and the direction
/// of travel there.
///
/// Measured *along the line* rather than one step per point, which is what
/// keeps the shore's ticks an even comb and the shoal's stipple an even scatter
/// whatever the survey happened to leave: a run simplified down to four points
/// would otherwise wear four ticks spread across a kilometre.
fn paced(points: &[Vec2], closed: bool, spacing: f32, mut footfall: impl FnMut(Vec2, Vec2)) {
    if points.len() < 2 || spacing <= 0.0 {
        return;
    }
    let along: Vec<Vec2> = if closed {
        points
            .iter()
            .copied()
            .chain(points.first().copied())
            .collect()
    } else {
        points.to_vec()
    };

    let mut walked = spacing / 2.0;
    for pair in along.windows(2) {
        let step = pair[1] - pair[0];
        let run = step.length();
        if run < f32::EPSILON {
            continue;
        }
        let forward = step / run;
        while walked < run {
            footfall(pair[0] + forward * walked, forward);
            walked += spacing;
        }
        walked -= run;
    }
}

/// The landward ticks along a shore.
///
/// Which way they hang is the survey's business and not this function's — the
/// runs arrive with land on the left, so left is where the ticks go. The
/// sheet's y climbs, so the left hand of a heading is its anticlockwise quarter
/// turn, which is what `perp` gives.
fn ticks(out: &mut Strokes, points: &[Vec2], closed: bool, spacing: f32, length: f32, width: f32) {
    paced(points, closed, spacing, |foot, forward| {
        out.segment(foot, foot + forward.perp() * length, width);
    });
}

/// The stipple along the edge of the shallows.
///
/// Dots rather than a line, scattered across the line rather than laid on it —
/// see [`SHOAL_SCATTER`] for why a shoal is not entitled to a ruled edge. The
/// scatter is a hash of the dot's number and the run it belongs to, so a
/// stretch of bank is stippled the same way every time the sheet is rebuilt;
/// a stipple that reshuffled on a pan would read as the paper crawling.
fn stipple(
    out: &mut Strokes,
    points: &[Vec2],
    closed: bool,
    seed: u32,
    (spacing, size, scatter): (f32, f32, f32),
) {
    let mut nth = 0u32;
    paced(points, closed, spacing, |foot, forward| {
        nth += 1;
        let off = scramble(seed.wrapping_add(nth.wrapping_mul(0x85EB_CA6B)));
        let off = (off >> 16) as f32 / u16::MAX as f32 - 0.5;
        out.dot(foot + forward.perp() * scatter * off, size);
    });
}

/// A small integer hash, for the handful of places here that want a number
/// that is the same every time but looks like it isn't: the roses' nudge off
/// their lattice, and the scatter of a stipple.
fn scramble(of: u32) -> u32 {
    let mut hash = of.wrapping_mul(0x9E37_79B9);
    hash ^= hash >> 15;
    hash = hash.wrapping_mul(0x2545_F491);
    hash ^ (hash >> 13)
}

/// The ruled grid under everything, at whichever round spacing falls far enough
/// apart to be read.
///
/// `covered` arrives in world metres, like everything the engraving is asked
/// for, and the flip onto the sheet happens *here* — the one drawing on the
/// chart whose points do not each pass through [`on_the_sheet`], because it is
/// generated rather than surveyed. It used to be ruled in world coordinates
/// directly, which mirrored it about the equator: indistinguishable near the
/// origin, where every test shot happened to be taken, and gone from the top
/// of the paper anywhere north of it.
fn graticule(out: &mut Strokes, covered: Rect, metres_per_pixel: f32, width: f32) {
    let sheet = Rect::from_corners(on_the_sheet(covered.min), on_the_sheet(covered.max));
    let spacing = round_distance(GRATICULE_GAP * metres_per_pixel);
    let first = |v: f32| (v / spacing).ceil() * spacing;

    let mut x = first(sheet.min.x);
    while x <= sheet.max.x {
        out.segment(Vec2::new(x, sheet.min.y), Vec2::new(x, sheet.max.y), width);
        x += spacing;
    }
    let mut y = first(sheet.min.y);
    while y <= sheet.max.y {
        out.segment(Vec2::new(sheet.min.x, y), Vec2::new(sheet.max.x, y), width);
        y += spacing;
    }
}

// ---------------------------------------------------------------------------
// Roses, and the net they throw
// ---------------------------------------------------------------------------

/// A point of the compass as a direction on the sheet: `turns` clockwise from
/// north, north being up.
fn wind(turns: f32) -> Vec2 {
    let angle = turns * std::f32::consts::TAU;
    Vec2::new(angle.sin(), angle.cos())
}

/// The orders of point a rose's star is built from: where the first of them
/// lies and how far apart they stand, both in turns clockwise from north, then
/// how far the point reaches as a fraction of the rose's radius and how wide it
/// is at the hub.
///
/// Three orders, sixteen points: four cardinals reaching the rim, four
/// intercardinals well short of it, and eight half winds shorter again. The
/// falling-off is what lets the eye count the rose without reading it — the
/// longest four are north, east, south and west and no arithmetic is needed to
/// see which.
const POINT_ORDERS: [(f32, f32, f32, f32); 3] = [
    (0.0, 0.25, 1.0, 0.115),
    (0.125, 0.25, 0.70, 0.085),
    (0.0625, 0.125, 0.48, 0.060),
];

/// The star of a rose: sixteen kite points, each split down its own axis so one
/// half goes to `dark` and the other to `light`.
///
/// That split is the whole trick, and it is why a rose is drawn rather than
/// ruled. An engraved rose looks lit from one side, and on real paper that is
/// done with hatching — which this sheet cannot have, being flat tones with no
/// texture anywhere in it. Two flat tones meeting on each point's axis gives
/// the same reading with nothing shaded: every point turns its dark half the
/// same way round the card, so the star reads as a solid thing catching light
/// rather than as sixteen flat triangles.
///
/// Two sinks rather than one because a tone is a material and a material is a
/// mesh — so the caller draws every rose's dark halves into one and every
/// rose's light halves into the other, and the sheet costs two meshes however
/// many roses stand on it.
fn star(dark: &mut Strokes, light: &mut Strokes, centre: Vec2, radius: f32) {
    for (first, step, reach, width) in POINT_ORDERS {
        let mut turns = first;
        while turns < 1.0 {
            let along = wind(turns);
            let apex = centre + along * reach * radius;
            let across = along.perp() * width * radius;
            dark.triangle(centre, apex, centre - across);
            light.triangle(centre, apex, centre + across);
            turns += step;
        }
    }
}

/// A circle, bent from [`CIRCLE_FACETS`] straight strokes.
fn circle(out: &mut Strokes, centre: Vec2, radius: f32, width: f32) {
    let points: Vec<Vec2> = (0..CIRCLE_FACETS)
        .map(|facet| centre + wind(facet as f32 / CIRCLE_FACETS as f32) * radius)
        .collect();
    out.run(&points, true, width);
}

/// The graduated band round a rose: two circles with the thirty-two winds
/// ticked between them, the eight principal ones ticked the whole way across.
///
/// It is what makes the difference between a star drawn on paper and an
/// instrument: a rose without a rim is a decoration, and a rim a bearing could
/// in principle be counted round is a rose.
fn band(out: &mut Strokes, centre: Vec2, inner: f32, outer: f32, width: f32) {
    circle(out, centre, inner, width);
    circle(out, centre, outer, width);
    for point in 0..RHUMB_BEARINGS {
        let along = wind(point as f32 / RHUMB_BEARINGS as f32);
        let principal = point % 4 == 0;
        let from = if principal {
            inner
        } else {
            inner + (outer - inner) * 0.45
        };
        out.segment(
            centre + along * from,
            centre + along * outer,
            width * if principal { 1.3 } else { 0.8 },
        );
    }
}

/// How far apart the roses that throw the rhumbs stand, in metres of paper: a
/// whole number of graticule squares, so a rose lands on a crossing of the
/// ruling — see [`RHUMB_SQUARES`].
fn rhumb_spacing(metres_per_pixel: f32) -> f32 {
    round_distance(GRATICULE_GAP * metres_per_pixel) * RHUMB_SQUARES
}

/// How far off its square's crossing a rose may be nudged, as a fraction of the
/// square — see [`rose_nudge`].
const RHUMB_NUDGE: f32 = 0.28;

/// Where those roses stand on the stretch of paper being drawn.
fn rhumb_roses(on_paper: Rect, spacing: f32) -> Vec<Vec2> {
    let square = |v: f32| (v / spacing).ceil() as i32;
    let mut roses = Vec::new();
    let mut ix = square(on_paper.min.x);
    while (ix as f32) * spacing <= on_paper.max.x {
        let mut iy = square(on_paper.min.y);
        while (iy as f32) * spacing <= on_paper.max.y {
            let square = IVec2::new(ix, iy);
            roses.push((square.as_vec2() + rose_nudge(square)) * spacing);
            iy += 1;
        }
        ix += 1;
    }
    roses
}

/// How far a rose stands off its square's crossing, in squares.
///
/// A chart drawn from an exact lattice does not look like a chart. On a perfect
/// grid every rose's diagonal runs straight into its neighbour's and its
/// east–west line into the ruling, so thirty-two bearings from a dozen roses
/// collapse into one X repeated across the paper — regular in a way no net
/// thrown by hand ever was, and the same everywhere the player goes.
///
/// The nudge is a hash of the square and not a random number, which is the
/// whole reason it can exist: the sheet is rebuilt every time the view leaves
/// its window, and a rose that stood somewhere else after a pan would be far
/// worse than a lattice. This way a rose belongs to its patch of sea, and a
/// stretch of water can be recognised by the net over it.
fn rose_nudge(square: IVec2) -> Vec2 {
    let hash =
        scramble((square.x as u32).wrapping_add((square.y as u32).wrapping_mul(0x85EB_CA6B)));
    let spread = |bits: u32| (bits as f32 / u16::MAX as f32 - 0.5) * 2.0 * RHUMB_NUDGE;
    Vec2::new(spread(hash >> 16), spread(hash & 0xFFFF))
}

/// The bearings one rose throws across the paper.
///
/// Thirty-two of them, and not all alike: the eight principal winds are ruled
/// at full weight, the eight half winds lighter, and the sixteen quarter winds
/// dashed. That grain is the difference between a net and a wash — thirty-two
/// identical rays from a dozen roses is a grey haze over the sheet, while a net
/// that gets fainter as it gets finer can be followed by eye from any rose to
/// any other.
///
/// Each ray is clipped to the paper being drawn and starts clear of the rose's
/// own star, so what gets built is bounded by the sheet rather than by the
/// world — the net is infinite in the same sense the chart is, which is to say
/// it is drawn as far as there is paper and no further.
fn rhumbs(out: &mut Strokes, on_paper: Rect, centre: Vec2, hub: f32, width: f32, dash: (f32, f32)) {
    for point in 0..RHUMB_BEARINGS {
        let along = wind(point as f32 / RHUMB_BEARINGS as f32);
        let Some((near, far)) = ray_across(on_paper, centre, along) else {
            continue;
        };
        let near = near.max(hub);
        if near >= far {
            continue;
        }
        match point % 4 {
            0 => out.segment(centre + along * near, centre + along * far, width),
            2 => out.segment(centre + along * near, centre + along * far, width * 0.7),
            _ => dashes(out, centre, along, (near, far), dash, width * 0.7),
        }
    }
}

/// Where a ray leaving `from` along `along` enters and leaves a rectangle, as
/// distances from `from` — `None` where it never gets there at all.
///
/// The near end is never behind the start: a rose outside the paper still
/// throws its rays across it, but it does not throw them backwards.
fn ray_across(rect: Rect, from: Vec2, along: Vec2) -> Option<(f32, f32)> {
    let mut near = 0.0f32;
    let mut far = f32::INFINITY;
    for axis in 0..2 {
        let (start, step) = (from[axis], along[axis]);
        let (low, high) = (rect.min[axis], rect.max[axis]);
        if step.abs() < f32::EPSILON {
            if start < low || start > high {
                return None;
            }
        } else {
            let (a, b) = ((low - start) / step, (high - start) / step);
            near = near.max(a.min(b));
            far = far.min(a.max(b));
        }
    }
    (far > near).then_some((near, far))
}

/// A dashed stretch of a ray.
///
/// The dashes are counted from the rose rather than from wherever the paper's
/// edge happened to cut the ray, so that neighbouring bearings break at the
/// same distances out and the net's dashes fall into rings. Counted the other
/// way they would land anywhere, and sixteen rays of unrelated dashes read as
/// dirt on the sheet.
fn dashes(
    out: &mut Strokes,
    from: Vec2,
    along: Vec2,
    (near, far): (f32, f32),
    (mark, gap): (f32, f32),
    width: f32,
) {
    let step = mark + gap;
    if step <= 0.0 {
        return;
    }
    let mut at = (near / step).floor() * step;
    while at < far {
        let (start, end) = (at.max(near), (at + mark).min(far));
        if end > start {
            out.segment(from + along * start, from + along * end, width);
        }
        at += step;
    }
}

/// The smallest round distance at least this many metres. The ladder runs out
/// at its top rather than inventing a rung, because past that the whole sheet
/// is one square of the grid anyway.
fn round_distance(least: f32) -> f32 {
    ROUND_DISTANCES
        .into_iter()
        .find(|rung| *rung >= least)
        .unwrap_or(ROUND_DISTANCES[ROUND_DISTANCES.len() - 1])
}

/// How a distance is written on the sheet: metres up to a kilometre and
/// kilometres past it, because that is how it would be said out loud.
fn distance_label(metres: f32) -> String {
    if metres >= 1000.0 {
        format!("{} km", metres / 1000.0)
    } else {
        format!("{metres} m")
    }
}

// ---------------------------------------------------------------------------
// The sheet, lent out
// ---------------------------------------------------------------------------

/// The three drawings that make paper *this* paper, built for a stretch of it
/// measured in its own pixels — see [`engraved_paper`].
pub(crate) struct EngravedPaper {
    /// The ruling, in [`INK_FAINT`].
    pub ruling: Mesh,
    /// The rhumb net thrown across it, in [`INK_GHOST`].
    pub net: Mesh,
    /// The dark halves of the roses the net comes from, in [`INK`], and their
    /// light halves, in [`INK_DIM`] — two meshes because two tones, which is
    /// what draws a lit thing on a sheet with no gradients (see [`star`]).
    pub roses: Mesh,
    pub lit: Mesh,
}

/// The sheet's character with nothing charted on it: ruling, net and roses,
/// over a stretch of paper `square` pixels to the square.
///
/// The chart never asks for this — its own ruling is spaced in *round
/// distances*, because a chart is read off round numbers, and it always has a
/// zoom to work that out from. What asks for it is a screen that is made of
/// the sheet without being a chart of anywhere, where a pixel is a pixel and
/// there is no world under the paper at all.
///
/// Lending the drawing out rather than letting the other screen copy it is
/// the whole point: there is one sheet in this game, and two would drift.
pub(crate) fn engraved_paper(on_paper: Rect, square: f32) -> EngravedPaper {
    let mut ruling = Strokes::default();
    let mut net = Strokes::default();
    let mut roses = Strokes::default();
    let mut lit = Strokes::default();

    let first = |v: f32| (v / square).ceil() * square;
    let mut x = first(on_paper.min.x);
    while x <= on_paper.max.x {
        let (top, bottom) = (Vec2::new(x, on_paper.min.y), Vec2::new(x, on_paper.max.y));
        ruling.segment(top, bottom, GRATICULE_WEIGHT);
        x += square;
    }
    let mut y = first(on_paper.min.y);
    while y <= on_paper.max.y {
        let (west, east) = (Vec2::new(on_paper.min.x, y), Vec2::new(on_paper.max.x, y));
        ruling.segment(west, east, GRATICULE_WEIGHT);
        y += square;
    }

    for centre in rhumb_roses(on_paper, square * RHUMB_SQUARES) {
        rhumbs(
            &mut net,
            on_paper,
            centre,
            PAPER_ROSE,
            RHUMB_WEIGHT,
            RHUMB_DASH,
        );
        star(&mut roses, &mut lit, centre, PAPER_ROSE);
    }

    EngravedPaper {
        ruling: ruling.drawn(),
        net: net.drawn(),
        roses: roses.drawn(),
        lit: lit.drawn(),
    }
}

/// A rose on its own, at the size the chart pins in its corner: the star's
/// dark halves in [`INK`], and its light halves and graduated band in
/// [`INK_DIM`].
pub(crate) fn drawn_rose() -> (Mesh, Mesh) {
    let mut inked = Strokes::default();
    let mut dimmed = Strokes::default();
    star(&mut inked, &mut dimmed, Vec2::ZERO, ROSE_REACH);
    band(
        &mut dimmed,
        Vec2::ZERO,
        ROSE_REACH,
        ROSE_REACH + ROSE_BAND,
        ROSE_WEIGHT,
    );
    (inked.drawn(), dimmed.drawn())
}

/// The sheet's edge ruled round a window, as the rules themselves in [`INK`]
/// and the paper that covers whatever ran out past them — see [`neatline`].
pub(crate) fn ruled_edge(window: Rect, cell: f32) -> (Mesh, Mesh) {
    let mut edge = Strokes::default();
    let mut mask = Strokes::default();
    neatline(&mut edge, &mut mask, window, cell, 1.0);
    (edge.drawn(), mask.drawn())
}

/// How far in from the window anything standing on the paper has to sit to be
/// clear of that edge.
pub(crate) const PAPER_MARGIN: f32 = FURNITURE_MARGIN;

/// The four letters that make a rose mean north rather than "this way", and
/// how far out from the middle of one they are set — the chart letters its
/// own corner rose this way, and so does anything that borrows it.
pub(crate) const ROSE_LETTERING: [(&str, f32); 4] =
    [("N", 0.0), ("E", 0.25), ("S", 0.5), ("W", 0.75)];
pub(crate) const ROSE_LETTER_OUT: f32 = ROSE_REACH + ROSE_BAND + ROSE_LETTER_GAP;
pub(crate) const ROSE_LETTER_SIZE: f32 = ROSE_LETTERS;

/// A point of the compass as a direction on the sheet, for anything laying
/// something out around a rose.
pub(crate) fn on_the_card(turns: f32) -> Vec2 {
    wind(turns)
}

// ---------------------------------------------------------------------------
// The furniture
// ---------------------------------------------------------------------------

/// How far the instruments sit in from the window — measured clear of the
/// sheet's edge rather than from the window itself, so a rose and a scale bar
/// stand *on the paper* rather than on the neatline drawn round it.
const FURNITURE_MARGIN: f32 = NEATLINE_INSET + NEATLINE_BAND + 14.0;

/// The bar in the sheet's corner that says how far a distance reaches across
/// the paper.
///
/// Built as UI rather than drawn into the mesh because it is pinned to the
/// window and not to the world — the whole point of it being that it does not
/// move when the sheet does. It is given the sheet's own camera so it is drawn
/// in that pass, over the engraving; the world's own instruments are left alone
/// and simply painted over. The rose in the other corner is pinned the same way
/// but drawn otherwise — see [`corner_rose`].
fn furniture(commands: &mut Commands, sheet: Entity) {
    commands
        .spawn((
            Name::new("Chart furniture"),
            ChartSheet,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            UiTargetCamera(sheet),
            DespawnOnExit(Helm::Chart),
        ))
        .with_children(|sheet| {
            // The scale bar. Its length is set as the sheet is drawn — see
            // [`rule_the_scale`] — because it is the one piece of furniture that
            // has to change with the zoom, that being the whole of what it is
            // for.
            sheet.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(FURNITURE_MARGIN),
                    bottom: Val::Px(FURNITURE_MARGIN),
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(4.0),
                    ..default()
                },
                children![
                    (
                        ScaleRule,
                        Node {
                            width: Val::Px(SCALE_BAR_LEAST),
                            height: Val::Px(5.0),
                            border: UiRect::all(Val::Px(1.0)),
                            ..default()
                        },
                        BorderColor::all(INK),
                    ),
                    (
                        ScaleLabel,
                        Text::new(""),
                        // The serif the menus and the compass resolve, for
                        // the reason they do: this is chart furniture, and
                        // the machine's serif is the hand charts are
                        // lettered in.
                        TextFont {
                            font: FontSource::Serif,
                            font_size: FontSize::Px(13.0),
                            ..default()
                        },
                        TextColor(INK),
                    ),
                ],
            ));
        });
}

/// The rose in the corner: a sixteen-point star in its graduated band, lettered
/// at the four cardinals.
///
/// Flat, unlike the world's compass, which lies foreshortened on the sea
/// because a bearing read off it is meant to be carried out into the picture.
/// Nothing is foreshortened on a chart and nothing on this one turns: it is
/// here to say that north is up and stays up, so it is drawn once, at the size
/// it will always be, and only its transform is touched again.
///
/// Engraved rather than built from boxes the way the world's compass is. That
/// is not a preference: a kite point is a triangle and the UI layer has only
/// rectangles, so a star drawn there would have to be a spike rose, which is a
/// different and poorer thing. Being a mesh means it lives in the sheet's own
/// space rather than the window's, and [`pin_the_rose`] does the job the UI
/// layer would have done — which is the whole cost of the change, and it is one
/// short system.
fn corner_rose(commands: &mut Commands, meshes: &mut Assets<Mesh>, inks: &Inks) {
    let (inked, dimmed) = drawn_rose();

    let rose = commands
        .spawn((
            Name::new("Chart rose"),
            CornerRose,
            ChartSheet,
            Transform::default(),
            Visibility::Visible,
            DespawnOnExit(Helm::Chart),
        ))
        .id();

    for (mesh, ink) in [(inked, &inks.coast), (dimmed, &inks.dim)] {
        commands.spawn((
            Mesh2d(meshes.add(mesh)),
            MeshMaterial2d(ink.clone()),
            Transform::default(),
            ChildOf(rose),
        ));
    }

    // The letters, outside the band because the star fills the disc. N in the
    // reading ink and the other three dimmed, as on the world's compass and for
    // the same reason: they are what make N mean north rather than "this way".
    for (letter, turns) in ROSE_LETTERING {
        commands.spawn((
            Text2d::new(letter),
            // The serif the menus and the scale bar resolve, for the reason
            // they do: this is chart furniture, and the machine's serif is the
            // hand charts are lettered in. Not the islands' italic, which is a
            // hand writing *on* the paper.
            TextFont {
                font: FontSource::Serif,
                font_size: FontSize::Px(ROSE_LETTER_SIZE),
                ..default()
            },
            TextColor(if turns == 0.0 { INK } else { INK_DIM }),
            Transform::from_translation((on_the_card(turns) * ROSE_LETTER_OUT).extend(0.1)),
            ChildOf(rose),
        ));
    }
}

/// Keeps the rose in the corner of the window.
///
/// It is furniture, pinned like the scale bar — but it is drawn in the sheet's
/// space rather than the window's (see [`corner_rose`]), so where the window's
/// corner has got to has to be worked out rather than declared. The sheet's
/// middle is the view's centre and a pixel is [`ChartView::metres_per_pixel`]
/// metres of paper, which is the whole of the arithmetic; the same scale on the
/// transform is what holds the rose at its drawn size through a zoom, exactly
/// as the reader's own mark is held.
fn pin_the_rose(
    view: Res<ChartView>,
    sheet: Query<&Camera, With<ChartSheet>>,
    mut roses: Query<&mut Transform, With<CornerRose>>,
) {
    let Some(size) = sheet
        .single()
        .ok()
        .and_then(|camera| camera.logical_viewport_size())
    else {
        return;
    };
    let at = rose_corner(&view, size);
    for mut transform in &mut roses {
        // Over the coast and the lettering both: it is furniture standing on
        // the sheet, and a shore drawn through it would read as an error.
        transform.translation = at.extend(3.0);
        transform.scale = Vec3::splat(view.metres_per_pixel);
    }
}

/// Where the rose's middle falls on the paper, for a view and a window: the
/// sheet's own middle, plus the corner in pixels of paper.
fn rose_corner(view: &ChartView, size: Vec2) -> Vec2 {
    let inset = ROSE_EXTENT + FURNITURE_MARGIN;
    let corner = Vec2::new(size.x / 2.0 - inset, inset - size.y / 2.0);
    on_the_sheet(view.centre) + corner * view.metres_per_pixel
}

/// The two rules of the sheet's edge and the graduations between them, and the
/// paper that hides the engraving outside them.
#[derive(Component)]
struct SheetEdge;
#[derive(Component)]
struct SheetMask;

/// The sheet's edge: a double rule just inside the window with the ruling's own
/// graduations laid between the two lines, and the engraving outside it covered
/// over.
///
/// Both are one entity apiece with their mesh written over in place, because
/// this is the one drawing on the sheet that changes every time the view moves
/// at all. The engraving can be built for a window and panned about inside it —
/// that is what makes dragging free — but the edge is *at* the window, so it
/// has to be re-ruled whenever the paper slides under it. It is a hundred-odd
/// triangles; respawning an entity and leaking a mesh asset per frame of a
/// drag is what writing in place avoids.
fn sheet_edge(commands: &mut Commands, meshes: &mut Assets<Mesh>, inks: &Inks) {
    let layer = |name: &'static str, ink: Handle<ColorMaterial>, z: f32, mesh: Handle<Mesh>| {
        (
            Name::new(name),
            ChartSheet,
            Mesh2d(mesh),
            MeshMaterial2d(ink),
            Transform::from_xyz(0.0, 0.0, z),
            DespawnOnExit(Helm::Chart),
        )
    };
    let mut blank = || meshes.add(Strokes::default().drawn());
    let (mask, rules) = (blank(), blank());
    commands.spawn((
        SheetMask,
        layer("Chart edge, masked", inks.paper.clone(), 2.4, mask),
    ));
    commands.spawn((
        SheetEdge,
        layer("Chart edge", inks.coast.clone(), 2.5, rules),
    ));
}

/// Draws the edge for the window the sheet is showing.
///
/// `window` arrives already on the paper. The mask goes on first and covers
/// everything outside the inner rule — the engraving is built half a window
/// wider than the view (see [`SHEET_MARGIN`]), so without it a coast would run
/// out past the neatline and the sheet would have no edge at all, only a line
/// drawn across it.
fn neatline(edge: &mut Strokes, mask: &mut Strokes, window: Rect, cell: f32, on_paper: f32) {
    let outer = window.inflate(-NEATLINE_INSET * on_paper);
    let inner = outer.inflate(-NEATLINE_BAND * on_paper);
    if inner.is_empty() {
        return;
    }

    // Four strips: the top and bottom run the window's whole width and take the
    // corners with them, and the sides fill in between.
    let (low, high) = (window.min, window.max);
    for strip in [
        Rect::new(low.x, inner.max.y, high.x, high.y),
        Rect::new(low.x, low.y, high.x, inner.min.y),
        Rect::new(low.x, inner.min.y, inner.min.x, inner.max.y),
        Rect::new(inner.max.x, inner.min.y, high.x, inner.max.y),
    ] {
        mask.quad(strip);
    }

    let weight = NEATLINE_WEIGHT * on_paper;
    for rect in [outer, inner] {
        let corners = [
            rect.min,
            Vec2::new(rect.max.x, rect.min.y),
            rect.max,
            Vec2::new(rect.min.x, rect.max.y),
        ];
        edge.run(&corners, true, weight);
    }

    // The graduations, counted off the paper's own lattice rather than off the
    // window, so they hold still as the sheet is dragged under them and the
    // cells of the top edge stand over the cells of the bottom.
    let inked = |at: f32| ((at / cell).floor() as i64).rem_euclid(2) == 0;
    let first = |at: f32| (at / cell).floor() * cell;

    let mut x = first(outer.min.x);
    while x < outer.max.x {
        if inked(x + cell / 2.0) {
            let (from, to) = (x.max(outer.min.x), (x + cell).min(outer.max.x));
            edge.quad(Rect::new(from, outer.min.y, to, inner.min.y));
            edge.quad(Rect::new(from, inner.max.y, to, outer.max.y));
        }
        x += cell;
    }
    // The sides run between the inner rule's corners, leaving the corners
    // themselves to the top and bottom — which is what stops a corner cell
    // being inked twice over and reading heavier than the rest.
    let mut y = first(inner.min.y);
    while y < inner.max.y {
        if inked(y + cell / 2.0) {
            let (from, to) = (y.max(inner.min.y), (y + cell).min(inner.max.y));
            edge.quad(Rect::new(outer.min.x, from, inner.min.x, to));
            edge.quad(Rect::new(inner.max.x, from, outer.max.x, to));
        }
        y += cell;
    }
}

/// Re-rules the sheet's edge for wherever the window has got to.
fn rule_the_edge(
    view: Res<ChartView>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut ruled: Local<Option<Vec2>>,
    sheet: Query<&Camera, With<ChartSheet>>,
    edges: Query<&Mesh2d, With<SheetEdge>>,
    masks: Query<&Mesh2d, With<SheetMask>>,
) {
    let Some(size) = sheet
        .single()
        .ok()
        .and_then(|camera| camera.logical_viewport_size())
    else {
        return;
    };
    // Nothing to write into yet, or nothing left: forgetting what was ruled is
    // what makes the sheet's *next* opening draw its edge. The meshes are new
    // and blank each time the chart is unrolled, and the view need not have
    // changed since it was last put down.
    let (Ok(rules), Ok(cover)) = (edges.single(), masks.single()) else {
        *ruled = None;
        return;
    };
    // The window is what this is drawn for, so a resize is as much a reason to
    // re-rule as a pan is.
    if !view.is_changed() && *ruled == Some(size) {
        return;
    }
    *ruled = Some(size);

    let showing = window(&view, size);
    let on_paper = Rect::from_corners(on_the_sheet(showing.min), on_the_sheet(showing.max));
    let cell = round_distance(GRATICULE_GAP * view.metres_per_pixel) / NEATLINE_CELLS;
    let (mut edge, mut mask) = (Strokes::default(), Strokes::default());
    neatline(&mut edge, &mut mask, on_paper, cell, view.metres_per_pixel);

    for (drawn, strokes) in [(rules, edge), (cover, mask)] {
        if let Some(mut slot) = meshes.get_mut(&drawn.0) {
            *slot = strokes.drawn();
        }
    }
}

/// The bar of the scale, and its label.
#[derive(Component)]
struct ScaleRule;
#[derive(Component)]
struct ScaleLabel;

/// Sets the scale bar to a round distance, and says which.
///
/// The bar is drawn at whatever length that distance comes to rather than at a
/// fixed length labelled with an awkward number, because a scale bar is a thing
/// to lay a finger against: "two kilometres is this far" reads, and "this far
/// is 1.83 km" does not.
fn rule_the_scale(
    view: Res<ChartView>,
    mut rules: Query<&mut Node, With<ScaleRule>>,
    mut labels: Query<&mut Text, With<ScaleLabel>>,
) {
    if !view.is_changed() {
        return;
    }
    let metres = round_distance(SCALE_BAR_LEAST * view.metres_per_pixel);
    for mut node in &mut rules {
        node.width = Val::Px(metres / view.metres_per_pixel);
    }
    for mut text in &mut labels {
        text.0 = distance_label(metres);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{run_frames, test_ground, world_app, FRAME};
    use bevy::state::app::StatesPlugin;
    use bevy::time::{TimePlugin, TimeUpdateStrategy};

    /// A chunk's height grid tilted so that everything north of a line is land
    /// and everything south of it is sea — the simplest coast there is, and one
    /// whose waterline sits at a height anyone can work out.
    ///
    /// The grid's z runs south as it increases, as the world's does, so a
    /// height falling with `iz` puts the land at the top of the grid.
    fn a_north_shore(shore: f32) -> Vec<f32> {
        (0..FACET_VERTS * FACET_VERTS)
            .map(|i| shore - (i / FACET_VERTS) as f32 * FACET_METRES)
            .collect()
    }

    /// A grid that is all one thing.
    fn all(height: f32) -> Vec<f32> {
        vec![height; FACET_VERTS * FACET_VERTS]
    }

    /// The waterline alone, for the tests that are about the contour walk
    /// rather than about what a chunk is worth keeping.
    fn waterline(heights: &[f32]) -> Vec<Coast> {
        contour(heights, 0.0)
    }

    #[test]
    fn open_water_and_open_country_hold_no_coast() {
        // The common answers, and the ones that have to cost nothing: a chunk
        // with no waterline crossing it holds no coast at all, whichever side
        // of the waterline it is on.
        assert!(waterline(&all(-8.0)).is_empty());
        assert!(waterline(&all(40.0)).is_empty());
    }

    #[test]
    fn a_shore_is_found_where_the_ground_meets_the_sea() {
        let runs = waterline(&a_north_shore(50.0));
        assert_eq!(runs.len(), 1, "one shore, one run");
        assert!(!runs[0].closed, "a shore crossing the chunk does not close");

        for point in runs[0].points(IVec2::ZERO) {
            assert!(
                (point.y - 50.0).abs() < TOLERANCE + MARK_STEP,
                "{point:?} is not on the waterline 50 m down the grid"
            );
        }
    }

    #[test]
    fn a_straight_shore_survives_as_a_handful_of_points() {
        // The storage argument in one assertion: a chunk's waterline is
        // sixty-four cells of contour, and what is kept of a straight one is
        // its two ends.
        let runs = waterline(&a_north_shore(50.0));
        assert!(
            runs[0].marks.len() <= 4,
            "a straight shore kept {} points",
            runs[0].marks.len()
        );
    }

    #[test]
    fn a_run_leaves_the_chunk_by_its_own_edges() {
        // What lets two chunks' strokes meet: an open run starts and ends
        // exactly on the chunk's boundary, so the neighbour's own run begins
        // where this one stopped.
        let runs = waterline(&a_north_shore(50.0));
        let marks = &runs[0].marks;
        for end in [marks[0], marks[marks.len() - 1]] {
            assert!(
                end.x == 0 || end.x == u8::MAX,
                "a run ended at {end:?}, inside the chunk"
            );
        }
    }

    #[test]
    fn land_lies_on_the_left_of_a_run() {
        // The convention the shore ticks hang off. This shore has land to the
        // north — falling z — so a walk with land on its left hand runs east,
        // in the direction of rising x.
        let runs = waterline(&a_north_shore(50.0));
        let marks = &runs[0].marks;
        assert!(
            marks[marks.len() - 1].x > marks[0].x,
            "the run goes west, so land is on its right"
        );
    }

    /// A cone standing out of the water: land above the waterline within
    /// `radius` metres of `middle`, given in the world metres of `chunk`, and
    /// sea everywhere else — the simplest island there is, put down wherever a
    /// test wants one, spanning however many chunks the radius reaches.
    fn a_cone(chunk: IVec2, middle: Vec2, radius: f32) -> Vec<f32> {
        let base = chunk.as_vec2() * CHUNK_METRES;
        (0..FACET_VERTS * FACET_VERTS)
            .map(|i| {
                let local =
                    Vec2::new((i % FACET_VERTS) as f32, (i / FACET_VERTS) as f32) * FACET_METRES;
                radius - (base + local).distance(middle)
            })
            .collect()
    }

    #[test]
    fn an_islet_inside_one_chunk_closes_on_itself() {
        // A cone in the middle of the chunk: its waterline is a ring that
        // never reaches an edge, which is the one case a run has to close.
        let middle = Vec2::splat(CHUNK_METRES / 2.0);
        let runs = waterline(&a_cone(IVec2::ZERO, middle, 30.0));
        assert_eq!(runs.len(), 1);
        assert!(runs[0].closed, "an islet's shore has to close");
        for point in runs[0].points(IVec2::ZERO) {
            assert!(
                (point.distance(middle) - 30.0).abs() < TOLERANCE + MARK_STEP,
                "{point:?} is not on the islet's waterline"
            );
        }
    }

    #[test]
    fn two_chunks_of_one_shore_meet_along_their_boundary() {
        // The reason a mark is quantised *onto* the boundary rather than near
        // it. The same shore surveyed in two chunks side by side has to leave
        // the first exactly where it enters the second.
        let heights = a_north_shore(50.0);
        let west = waterline(&heights);
        let east = waterline(&heights);

        let leaving = west[0].points(IVec2::ZERO).last().expect("an end");
        let arriving = east[0].points(IVec2::new(1, 0)).next().expect("a start");
        assert!(
            (leaving - arriving).length() < 1.0e-3,
            "{leaving:?} and {arriving:?} do not meet"
        );
    }

    #[test]
    fn simplifying_keeps_the_ends_and_the_corner() {
        // A dog-leg: the corner is what the line is, and the points strung
        // along each straight are what it is not.
        let mut points: Vec<Vec2> = (0..=10).map(|x| Vec2::new(x as f32, 0.0)).collect();
        points.extend((1..=10).map(|y| Vec2::new(10.0, y as f32)));

        assert_eq!(
            simplify(&points, false, 0.5),
            [Vec2::ZERO, Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0)]
        );
    }

    #[test]
    fn nothing_strays_further_than_the_tolerance_allows() {
        // What the tolerance is a promise about: a quarter circle thinned, and
        // every point of the original still within reach of what was kept.
        let points: Vec<Vec2> = (0..=90)
            .map(|degrees| {
                let angle = (degrees as f32).to_radians();
                Vec2::new(angle.cos(), angle.sin()) * 100.0
            })
            .collect();
        let thinned = simplify(&points, false, 1.0);
        assert!(thinned.len() < points.len(), "nothing was thinned out");

        for point in &points {
            let nearest = thinned
                .windows(2)
                .map(|pair| off_the_line(*point, pair[0], pair[1]))
                .fold(f32::INFINITY, f32::min);
            assert!(nearest <= 1.0 + 1.0e-3, "{point:?} strayed {nearest} m");
        }
    }

    #[test]
    fn a_ring_thins_without_coming_apart() {
        // The closed case, which has no ends for the algorithm to hold on to.
        // A circle of a hundred points is a circle of rather fewer, and it is
        // still a circle: nothing doubled back, and nothing left the rim.
        let points: Vec<Vec2> = (0..100)
            .map(|step| {
                let angle = step as f32 / 100.0 * std::f32::consts::TAU;
                Vec2::new(angle.cos(), angle.sin()) * 50.0
            })
            .collect();

        let ring = simplify(&points, true, 1.0);
        assert!(ring.len() < points.len() && ring.len() >= 8);
        for point in &ring {
            assert!(
                (point.length() - 50.0).abs() < 1.0e-3,
                "{point:?} left the rim"
            );
        }
        // And no point was kept twice, which is what a mishandled cut does.
        for (at, point) in ring.iter().enumerate() {
            assert!(
                !ring[at + 1..].contains(point),
                "{point:?} was kept twice over"
            );
        }
    }

    #[test]
    fn a_chunk_is_in_sight_by_its_nearest_corner() {
        // The same reach the ground is streamed by, measured the same way: a
        // chunk counts as seen when any of it comes inside the radius, not when
        // its corner does.
        assert!(in_sight(IVec2::ZERO, Vec2::new(64.0, 64.0)));
        assert!(in_sight(
            IVec2::ZERO,
            Vec2::new(CHUNK_METRES + SIGHT_RADIUS - 1.0, 64.0)
        ));
        assert!(!in_sight(
            IVec2::ZERO,
            Vec2::new(CHUNK_METRES + SIGHT_RADIUS + 1.0, 64.0)
        ));
    }

    #[test]
    fn the_sight_radius_stays_inside_what_the_client_holds() {
        // The survey never asks for ground: it reads what streaming has already
        // brought in. If this stopped being true, chunks would come into sight
        // that the client had never been sent, and the chart would have holes
        // in it that nothing would ever come back to fill.
        const { assert!(SIGHT_RADIUS < crate::terrain::STREAM_RADIUS) };
    }

    #[test]
    fn north_is_up_on_the_sheet() {
        // The whole of what "north is up" means. North is negative z in the
        // world and a screen's y climbs, so a point to the north draws above
        // one at the origin, and a point to the east draws to its right.
        let north = on_the_sheet(Vec2::new(0.0, -100.0));
        let east = on_the_sheet(Vec2::new(100.0, 0.0));
        assert!(north.y > 0.0 && north.x == 0.0);
        assert!(east.x > 0.0 && east.y == 0.0);
    }

    #[test]
    fn a_mark_heading_north_is_not_turned_at_all() {
        // And the quarter turns either side of it, which is where a sign error
        // hides: a bearing runs clockwise and a rotation does not.
        let quarter = std::f32::consts::FRAC_PI_2;
        assert!(bearing_on_the_sheet(Vec2::new(0.0, -1.0)).abs() < 1.0e-5);
        assert!((bearing_on_the_sheet(Vec2::new(1.0, 0.0)) + quarter).abs() < 1.0e-5);
        assert!((bearing_on_the_sheet(Vec2::new(-1.0, 0.0)) - quarter).abs() < 1.0e-5);
    }

    #[test]
    fn the_ruling_is_at_a_distance_worth_reading() {
        // A chart is read off round numbers, and the rung chosen has to be far
        // enough apart on the paper to be told apart.
        for metres_per_pixel in [1.0, 2.0, 8.0, 30.0, 64.0] {
            let spacing = round_distance(GRATICULE_GAP * metres_per_pixel);
            assert!(ROUND_DISTANCES.contains(&spacing), "{spacing} is not round");
            assert!(
                spacing / metres_per_pixel >= GRATICULE_GAP
                    || spacing == ROUND_DISTANCES[ROUND_DISTANCES.len() - 1],
                "ruled every {} px at {metres_per_pixel} m/px",
                spacing / metres_per_pixel
            );
        }
    }

    #[test]
    fn the_ruling_is_drawn_on_the_sheet_and_not_on_the_world() {
        // A window well north of the origin: world z deeply negative, sheet y
        // as deeply positive. Ruled in world coordinates the grid lands
        // mirrored about the equator — right at the origin, where every early
        // test shot happened to be taken, and off the top of the paper
        // anywhere north of it.
        let covered = Rect::from_corners(Vec2::new(0.0, -1000.0), Vec2::new(400.0, -800.0));
        let mut strokes = Strokes::default();
        graticule(&mut strokes, covered, 1.0, 1.0);

        assert!(!strokes.positions.is_empty(), "nothing was ruled at all");
        for point in &strokes.positions {
            assert!(
                (799.0..=1001.0).contains(&point[1]),
                "a rule reaches sheet y {}, off the covered paper",
                point[1]
            );
        }
    }

    #[test]
    fn distances_are_written_the_way_they_are_said() {
        assert_eq!(distance_label(500.0), "500 m");
        assert_eq!(distance_label(1000.0), "1 km");
        assert_eq!(distance_label(2000.0), "2 km");
    }

    #[test]
    fn only_the_coast_in_the_window_is_drawn() {
        // What keeps the sheet's cost bounded by how much paper there is rather
        // than by how far the player has sailed.
        let mut chart = Chart::default();
        let runs = survey(&a_north_shore(50.0));
        chart.record(IVec2::ZERO, runs.clone());
        chart.record(IVec2::new(60, 0), runs);

        let near = Rect::from_corners(Vec2::new(-100.0, -100.0), Vec2::new(200.0, 200.0));
        let drawn: usize = chart
            .within(near)
            .map(|(_, found)| found.coast.len() + found.shoal.len())
            .sum();
        assert_eq!(chart.within(near).count(), 1, "one chunk of paper");
        assert!(drawn > 0 && drawn < 4, "{drawn} runs from one chunk");
    }

    #[test]
    fn a_surveyed_chunk_with_nothing_on_it_is_still_surveyed() {
        // Which is what stops open water being surveyed again on every frame
        // the player sits beside it.
        let mut chart = Chart::default();
        assert!(!chart.surveyed(IVec2::ZERO));
        chart.record(IVec2::ZERO, survey(&all(-8.0)));
        assert!(chart.surveyed(IVec2::ZERO));

        let tally = chart.tally();
        assert_eq!((tally.surveyed, tally.coastal), (1, 0));
    }

    #[test]
    fn a_coastline_closes_only_when_the_player_has_been_all_the_way_round() {
        // An island astride two chunks: a cone centred on their shared
        // boundary, so each chunk's survey holds half its shore as one open
        // run. With one half charted the coastline hangs open — which is what
        // stops half-explored islands reading as done — and the other half
        // closes it: the two runs link end-to-start, exactly, across the
        // boundary.
        let middle = Vec2::new(CHUNK_METRES, CHUNK_METRES / 2.0);
        let mut chart = Chart::default();
        chart.record(IVec2::ZERO, survey(&a_cone(IVec2::ZERO, middle, 60.0)));

        let tally = chart.tally();
        assert_eq!(
            (tally.complete, tally.open, tally.islands),
            (0, 1, 0),
            "half an island read as a closed coastline"
        );

        chart.record(
            IVec2::new(1, 0),
            survey(&a_cone(IVec2::new(1, 0), middle, 60.0)),
        );
        let tally = chart.tally();
        assert_eq!(
            (tally.complete, tally.open, tally.islands),
            (1, 0, 1),
            "the whole shore does not close into an island"
        );
    }

    #[test]
    fn a_skerry_closes_as_a_coastline_without_being_an_island() {
        // Many an island is a little archipelago — a main shore with skerries
        // off it. Each ring closes on its own, and the threshold is what says
        // which of them are islands to claim: the main shore is one, and the
        // rock off it is charted without being anybody's island.
        let mut chart = Chart::default();
        chart.record(
            IVec2::ZERO,
            survey(&a_cone(IVec2::ZERO, Vec2::splat(64.0), 55.0)),
        );
        chart.record(
            IVec2::new(5, 5),
            survey(&a_cone(
                IVec2::new(5, 5),
                Vec2::splat(5.0 * CHUNK_METRES + 64.0),
                12.0,
            )),
        );

        let tally = chart.tally();
        assert_eq!((tally.complete, tally.open), (2, 0));
        assert_eq!(tally.islands, 1, "the skerry counts as an island");
    }

    #[test]
    fn a_lagoon_rings_water_and_is_never_an_island() {
        // The mirror image of an islet: land everywhere except a basin of
        // water in the middle, whose waterline is a closed ring the same size
        // an island's would be. The land-on-the-left convention runs the two
        // opposite ways round, which is the whole test of whether the sign
        // does its job — a lagoon must fail however big it is.
        let middle = Vec2::splat(CHUNK_METRES / 2.0);
        let heights: Vec<f32> = (0..FACET_VERTS * FACET_VERTS)
            .map(|i| {
                let at =
                    Vec2::new((i % FACET_VERTS) as f32, (i / FACET_VERTS) as f32) * FACET_METRES;
                at.distance(middle) - 55.0
            })
            .collect();
        let mut chart = Chart::default();
        chart.record(IVec2::ZERO, survey(&heights));

        let tally = chart.tally();
        assert_eq!((tally.complete, tally.open), (1, 0));
        assert_eq!(tally.islands, 0, "a lagoon was claimed as an island");
    }

    #[test]
    fn an_island_is_lettered_in_the_middle_of_itself() {
        // Where the name goes: an island astride two chunks, and its measured
        // centre landing where the cone was put down — on the sheet, so the
        // world's z is flipped.
        let middle = Vec2::new(CHUNK_METRES, CHUNK_METRES / 2.0);
        let mut chart = Chart::default();
        chart.record(IVec2::ZERO, survey(&a_cone(IVec2::ZERO, middle, 60.0)));
        chart.record(
            IVec2::new(1, 0),
            survey(&a_cone(IVec2::new(1, 0), middle, 60.0)),
        );

        let islands = chart.islands();
        assert_eq!(islands.len(), 1);
        assert!(
            (islands[0].centre - on_the_sheet(middle)).length() < TOLERANCE + MARK_STEP,
            "the name sits at {:?}, off the island at {:?}",
            islands[0].centre,
            on_the_sheet(middle)
        );
        assert!(
            (islands[0].extent - 120.0).abs() < 2.0 * (TOLERANCE + MARK_STEP),
            "a 120 m island measured {} m",
            islands[0].extent
        );
    }

    #[test]
    fn half_an_island_carries_no_name_yet() {
        // The other half of the promise: nothing is lettered until the player
        // has been all the way round. Half a shore is an open chain, and an
        // open chain is not an island however big it is.
        let middle = Vec2::new(CHUNK_METRES, CHUNK_METRES / 2.0);
        let mut chart = Chart::default();
        chart.record(IVec2::ZERO, survey(&a_cone(IVec2::ZERO, middle, 60.0)));

        assert!(chart.islands().is_empty());
    }

    #[test]
    fn an_islet_ringed_in_one_chunk_is_already_closed() {
        // Big enough to close, and — at sixty metres across — too small to be
        // an island anybody names.
        let mut chart = Chart::default();
        chart.record(
            IVec2::ZERO,
            survey(&a_cone(IVec2::ZERO, Vec2::splat(CHUNK_METRES / 2.0), 30.0)),
        );

        let tally = chart.tally();
        assert_eq!((tally.complete, tally.open, tally.islands), (1, 0, 0));
    }

    #[test]
    fn a_mark_is_half_a_metre_and_lands_on_the_boundary() {
        // The two properties two bytes have to have: fine enough that nothing
        // on a chart notices, and exact at a chunk's edges so that two
        // neighbours' strokes meet.
        const { assert!(MARK_STEP < 0.51) };
        assert_eq!(Mark::of(Vec2::ZERO).local(), Vec2::ZERO);
        assert_eq!(
            Mark::of(Vec2::splat(CHUNK_METRES)).local(),
            Vec2::splat(CHUNK_METRES)
        );

        // And nothing in between is out by more than half a step.
        for tenth in 0..=1280 {
            let metres = tenth as f32 / 10.0;
            let back = Mark::of(Vec2::new(metres, 0.0)).local().x;
            assert!((back - metres).abs() <= MARK_STEP / 2.0 + 1.0e-4);
        }
    }

    #[test]
    fn ticks_hang_at_an_even_comb_along_the_shore() {
        // Spaced along the line rather than one per point, so a shore thinned
        // down to two points still wears a full comb.
        let mut strokes = Strokes::default();
        ticks(
            &mut strokes,
            &[Vec2::ZERO, Vec2::new(100.0, 0.0)],
            false,
            10.0,
            5.0,
            1.0,
        );
        // Two triangles a tick, ten ticks along a hundred metres.
        assert_eq!(strokes.positions.len(), 10 * 2 * 3);
    }

    /// A headless app with the chart's own systems and nothing else — enough
    /// to press its key at and watch the state go.
    fn keyed_app() -> App {
        let mut app = App::new();
        // `AssetPlugin` because the sheet is drawn out of meshes and inks, and
        // `TaskPoolPlugin` because that is where the asset server finds a
        // thread. Nothing here loads anything from disk.
        app.add_plugins((
            TaskPoolPlugin::default(),
            AssetPlugin::default(),
            TimePlugin,
            StatesPlugin,
            ChartPlugin,
        ))
        .insert_resource(TimeUpdateStrategy::ManualDuration(FRAME))
        .init_state::<AppState>()
        .add_sub_state::<Helm>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>()
        .init_resource::<AccumulatedMouseMotion>()
        .init_resource::<AccumulatedMouseScroll>()
        .add_message::<KeyboardInput>()
        .init_asset::<Mesh>()
        .init_asset::<ColorMaterial>()
        // The lettering hand: unrolling the sheet asks the asset server for
        // it, and a server without the asset type registered panics. No
        // loader is registered, so nothing is read from disk.
        .init_asset::<Font>();
        app.update();
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::InWorld);
        app.update();
        app
    }

    fn helm(app: &App) -> Helm {
        *app.world().resource::<State<Helm>>().get()
    }

    /// Presses a key for one frame, and lets the state it asked for arrive.
    ///
    /// Released first because a key that is already down cannot be pressed:
    /// [`run_frames`] clears the just-pressed flag between frames the way the
    /// real input plugin does, but leaves the key held — so a second press
    /// without a release is a key that never went up.
    fn press(app: &mut App, key: KeyCode) {
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.release(key);
        keys.press(key);
        run_frames(app, 2);
    }

    /// Types one keypress into the name being written, the way a real
    /// keyboard delivers it — the same shape the console's tests fake.
    fn type_key(app: &mut App, key: KeyCode, typed: &str) {
        app.world_mut().write_message(KeyboardInput {
            key_code: key,
            logical_key: match typed {
                " " => Key::Space,
                "\r" => Key::Enter,
                "\u{8}" => Key::Backspace,
                typed => Key::Character(typed.into()),
            },
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
        app.update();
    }

    fn type_word(app: &mut App, text: &str) {
        for character in text.chars() {
            let key = if character == ' ' {
                KeyCode::Space
            } else {
                KeyCode::KeyA
            };
            type_key(app, key, &character.to_string());
        }
    }

    #[test]
    fn christening_an_island_letters_it_by_name() {
        // The names ride the islands the walk finds: written against the id,
        // read back off the island — and only whitespace washes them off.
        let middle = Vec2::new(CHUNK_METRES, CHUNK_METRES / 2.0);
        let mut chart = Chart::default();
        chart.record(IVec2::ZERO, survey(&a_cone(IVec2::ZERO, middle, 60.0)));
        chart.record(
            IVec2::new(1, 0),
            survey(&a_cone(IVec2::new(1, 0), middle, 60.0)),
        );

        let id = chart.islands()[0].id;
        chart.christen(id, "  Isla Genovesa  ");
        assert_eq!(chart.islands()[0].name.as_deref(), Some("Isla Genovesa"));
        assert_eq!(chart.name(id), Some("Isla Genovesa"));

        chart.christen(id, "   ");
        assert_eq!(chart.islands()[0].name, None);
    }

    #[test]
    fn an_islands_identity_survives_more_of_the_world_arriving() {
        // The id has to keep naming the same ring while the chart around it
        // grows, or a name would fall off its island when the player sailed
        // on. More survey arriving elsewhere must not move it.
        let middle = Vec2::new(CHUNK_METRES, CHUNK_METRES / 2.0);
        let mut chart = Chart::default();
        chart.record(IVec2::ZERO, survey(&a_cone(IVec2::ZERO, middle, 60.0)));
        chart.record(
            IVec2::new(1, 0),
            survey(&a_cone(IVec2::new(1, 0), middle, 60.0)),
        );
        let id = chart.islands()[0].id;

        chart.record(IVec2::new(4, 4), survey(&a_north_shore(50.0)));
        chart.record(IVec2::new(-3, 2), survey(&all(-8.0)));
        assert_eq!(chart.islands()[0].id, id);
    }

    #[test]
    fn a_click_lands_on_the_island_or_its_lettering() {
        let island = Island {
            id: IVec2::ZERO,
            centre: Vec2::ZERO,
            extent: 200.0,
            name: None,
        };
        let islands = vec![island];

        // On the island itself, and just off it.
        assert!(hit_island(&islands, Vec2::new(80.0, 40.0), 1.0).is_some());
        assert!(hit_island(&islands, Vec2::new(300.0, 0.0), 1.0).is_none());

        // Zoomed far out the island is a speck — extent under a pixel of
        // reach — but the stretch of lettering still takes the click.
        assert!(hit_island(&islands, Vec2::new(1500.0, 0.0), 32.0).is_some());
        assert!(hit_island(&islands, Vec2::new(0.0, 1500.0), 32.0).is_none());
    }

    #[test]
    fn a_typed_name_is_written_by_enter() {
        let mut app = keyed_app();
        press(&mut app, KeyCode::KeyM);
        assert_eq!(helm(&app), Helm::Chart);

        let island = IVec2::new(40, -17);
        app.insert_resource(Naming {
            island,
            draft: String::new(),
        });
        type_word(&mut app, "Skull Rock");
        type_key(&mut app, KeyCode::Enter, "\r");

        let world = app.world();
        assert_eq!(world.resource::<Chart>().name(island), Some("Skull Rock"));
        assert!(
            world.get_resource::<Naming>().is_none(),
            "the pen is still down"
        );
    }

    #[test]
    fn backspace_takes_a_letter_back_and_a_name_stops_at_its_cap() {
        let mut app = keyed_app();
        press(&mut app, KeyCode::KeyM);

        let island = IVec2::ZERO;
        app.insert_resource(Naming {
            island,
            draft: String::new(),
        });
        // Far past the cap, so the surplus has something to be dropped from.
        type_word(&mut app, &"a".repeat(NAME_LENGTH + 9));
        type_key(&mut app, KeyCode::Backspace, "\u{8}");
        type_key(&mut app, KeyCode::Enter, "\r");

        let written = app
            .world()
            .resource::<Chart>()
            .name(island)
            .expect("a name was written");
        assert_eq!(written.chars().count(), NAME_LENGTH - 1);
    }

    #[test]
    fn escape_puts_the_pen_down_without_writing() {
        let mut app = keyed_app();
        press(&mut app, KeyCode::KeyM);

        let island = IVec2::ZERO;
        app.insert_resource(Naming {
            island,
            draft: "Half a nam".to_string(),
        });
        press(&mut app, KeyCode::Escape);

        // The draft is gone, nothing was written, and the chart is still up:
        // Escape spoke to the pen, not the sheet.
        assert_eq!(helm(&app), Helm::Chart);
        let world = app.world();
        assert!(world.get_resource::<Naming>().is_none());
        assert_eq!(world.resource::<Chart>().name(island), None);
    }

    #[test]
    fn escape_with_no_pen_down_closes_the_chart() {
        let mut app = keyed_app();
        press(&mut app, KeyCode::KeyM);
        assert_eq!(helm(&app), Helm::Chart);

        press(&mut app, KeyCode::Escape);
        assert_eq!(helm(&app), Helm::Sailing);
    }

    #[test]
    fn the_chart_key_spells_a_letter_while_a_name_is_written() {
        let mut app = keyed_app();
        press(&mut app, KeyCode::KeyM);

        app.insert_resource(Naming {
            island: IVec2::ZERO,
            draft: String::new(),
        });
        press(&mut app, KeyCode::KeyM);
        assert_eq!(helm(&app), Helm::Chart, "the chart key closed the sheet");
    }

    #[test]
    fn the_chart_key_lays_the_sheet_down_and_picks_it_up() {
        let mut app = keyed_app();
        assert_eq!(helm(&app), Helm::Sailing);

        press(&mut app, KeyCode::KeyM);
        assert_eq!(helm(&app), Helm::Chart);

        // The same key again, because a chart is a thing you put down the way
        // you picked it up.
        press(&mut app, KeyCode::KeyM);
        assert_eq!(helm(&app), Helm::Sailing);
    }

    #[test]
    fn the_chart_key_is_the_players_to_move() {
        let mut app = keyed_app();
        app.world_mut()
            .resource_mut::<KeyBindings>()
            .bind(Action::Chart, KeyCode::KeyZ, None);

        press(&mut app, KeyCode::KeyM);
        assert_eq!(helm(&app), Helm::Sailing, "the old key still opened it");

        press(&mut app, KeyCode::KeyZ);
        assert_eq!(helm(&app), Helm::Chart);
    }

    #[test]
    fn the_chart_does_not_open_over_a_menu_or_a_console() {
        // Every one of these has the keyboard for its own reasons while it is
        // up, and a chart key typed into one of them means what that screen
        // says it means — a letter in the console, a key being bound on the
        // controls screen.
        for busy in [Helm::Paused, Helm::Controls, Helm::Console] {
            let mut app = keyed_app();
            app.world_mut().resource_mut::<NextState<Helm>>().set(busy);
            app.update();

            press(&mut app, KeyCode::KeyM);
            assert_eq!(helm(&app), busy, "the chart opened over {busy:?}");
        }
    }

    #[test]
    fn a_world_gets_a_blank_chart_and_takes_it_away_again() {
        // What has been seen is a fact about *this* world. Carrying it into
        // the next would draw one seed's islands on another's water.
        let mut app = keyed_app();
        app.world_mut()
            .resource_mut::<Chart>()
            .record(IVec2::ZERO, Soundings::default());
        assert!(app.world().resource::<Chart>().surveyed(IVec2::ZERO));

        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::MainMenu);
        app.update();
        assert!(app.world().get_resource::<Chart>().is_none());

        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::InWorld);
        app.update();
        assert!(!app.world().resource::<Chart>().surveyed(IVec2::ZERO));
    }

    #[test]
    fn the_survey_follows_the_player_and_stops_where_they_stop_seeing() {
        // The whole behaviour, in a world already delivered: the coast near
        // where the player is put down goes on the chart, and ground they have
        // never been near stays off it — which is what makes half an island
        // half an island.
        let mut app = world_app();
        app.add_plugins(ChartPlugin);
        app.insert_resource(Chart::default());
        app.insert_resource(test_ground());
        run_frames(&mut app, 60);

        let chart = app.world().resource::<Chart>();
        // The test island's waterline runs at about 200 m from the origin,
        // which is where the player is put down.
        let shore = chunk_at(Vec2::new(crate::testing::TEST_ISLAND_REACH, 0.0));
        assert!(chart.surveyed(shore), "the shore was never surveyed");

        let coast = Rect::from_corners(Vec2::splat(-400.0), Vec2::splat(400.0));
        assert!(
            chart.within(coast).count() > 0,
            "an island was surveyed and no coast came of it"
        );

        // And a chunk well past the horizon, which the player has never been
        // anywhere near.
        assert!(!chart.surveyed(chunk_at(Vec2::splat(4000.0))));
    }

    #[test]
    fn a_stroke_has_the_width_it_was_given() {
        let mut strokes = Strokes::default();
        strokes.segment(Vec2::ZERO, Vec2::new(10.0, 0.0), 2.0);
        let across: Vec<f32> = strokes.positions.iter().map(|point| point[1]).collect();
        assert_eq!(across.iter().copied().fold(f32::MIN, f32::max), 1.0);
        assert_eq!(across.iter().copied().fold(f32::MAX, f32::min), -1.0);
    }

    /// How many straight strokes a run of triangles came from.
    fn segments(strokes: &Strokes) -> usize {
        strokes.positions.len() / 6
    }

    #[test]
    fn the_roses_keep_step_with_the_ruling() {
        // The two lattices are one lattice, at every rung of the zoom: a rose
        // stands a whole number of graticule squares from its neighbour, so
        // the net does not drift against the ruling as the sheet is zoomed.
        for zoom in [MIN_METRES_PER_PIXEL, 4.0, 17.0, MAX_METRES_PER_PIXEL] {
            let square = round_distance(GRATICULE_GAP * zoom);
            let squares = rhumb_spacing(zoom) / square;
            assert_eq!(squares, RHUMB_SQUARES, "at {zoom} m to the pixel");
        }
    }

    #[test]
    fn a_rose_stands_off_its_crossing_but_never_far() {
        // Nudged, or the net is a repeating X — but by a fraction of a square,
        // so a rose still belongs to the crossing it came from.
        let spacing = 1000.0;
        let paper = Rect::from_corners(Vec2::ZERO, Vec2::splat(6000.0));
        let roses = rhumb_roses(paper, spacing);
        assert!(roses.len() > 20, "a lattice of {} roses", roses.len());

        let mut nudged = 0;
        for rose in &roses {
            let off = *rose / spacing - (*rose / spacing).round();
            assert!(
                off.abs().max_element() <= RHUMB_NUDGE + 1e-4,
                "{rose:?} strayed {off:?} squares off its crossing"
            );
            if off.abs().max_element() > 0.05 {
                nudged += 1;
            }
        }
        assert!(
            nudged > roses.len() / 2,
            "only {nudged} of {} roses moved at all",
            roses.len()
        );
    }

    #[test]
    fn a_rose_stands_in_the_same_place_however_the_sheet_is_cut() {
        // The sheet is rebuilt whenever the view leaves the window it was
        // drawn for, and the rebuild covers a different stretch of paper. A
        // rose that moved between two such cuts would swim about the sea as
        // the player panned, which is the one thing the nudge must not do.
        let spacing = 1000.0;
        let first = rhumb_roses(Rect::from_corners(Vec2::ZERO, Vec2::splat(6000.0)), spacing);
        let second = rhumb_roses(
            Rect::from_corners(Vec2::splat(2500.0), Vec2::splat(9000.0)),
            spacing,
        );

        let shared: Vec<&Vec2> = first
            .iter()
            .filter(|rose| second.iter().any(|other| other.abs_diff_eq(**rose, 1e-3)))
            .collect();
        let overlap = Rect::from_corners(Vec2::splat(3000.0), Vec2::splat(5500.0));
        let expected = first.iter().filter(|rose| overlap.contains(**rose)).count();
        assert!(
            shared.len() >= expected && expected > 0,
            "{} roses agreed where {expected} were drawn twice",
            shared.len()
        );
    }

    #[test]
    fn a_ray_is_clipped_to_the_paper_and_never_runs_backwards() {
        let paper = Rect::from_corners(Vec2::ZERO, Vec2::splat(10.0));
        let east = Vec2::new(1.0, 0.0);

        // From a rose on the paper: nothing behind it, and it stops at the edge.
        assert_eq!(ray_across(paper, Vec2::splat(5.0), east), Some((0.0, 5.0)));
        // From one off the paper: it starts where it arrives.
        assert_eq!(
            ray_across(paper, Vec2::new(-5.0, 5.0), east),
            Some((5.0, 15.0))
        );
        // And a bearing that never gets there draws nothing at all, whether it
        // runs parallel to the sheet or away from it.
        assert_eq!(ray_across(paper, Vec2::new(-5.0, 20.0), east), None);
        assert_eq!(
            ray_across(paper, Vec2::splat(20.0), Vec2::splat(0.5f32.sqrt())),
            None
        );
    }

    #[test]
    fn the_dashes_of_a_ray_are_counted_from_its_rose() {
        // Whole dashes on the beat from the rose, so that neighbouring
        // bearings break together — see `dashes`.
        let mut whole = Strokes::default();
        dashes(
            &mut whole,
            Vec2::ZERO,
            Vec2::new(1.0, 0.0),
            (0.0, 100.0),
            (10.0, 10.0),
            1.0,
        );
        assert_eq!(segments(&whole), 5);
        let along: Vec<f32> = whole.positions.iter().map(|point| point[0]).collect();
        assert_eq!(along.iter().copied().fold(f32::MIN, f32::max), 90.0);

        // A ray whose near end lands mid-gap keeps the same beat rather than
        // starting a new one there.
        let mut cut = Strokes::default();
        dashes(
            &mut cut,
            Vec2::ZERO,
            Vec2::new(1.0, 0.0),
            (15.0, 100.0),
            (10.0, 10.0),
            1.0,
        );
        let start = cut
            .positions
            .iter()
            .map(|point| point[0])
            .fold(f32::MAX, f32::min);
        assert_eq!(start, 20.0, "the beat restarted at the cut");
    }

    #[test]
    fn a_star_is_drawn_in_two_halves() {
        // Sixteen points, each split down its axis — one half to each tone,
        // which is what stands in for an engraver's hatching. Neither sink may
        // be short: a point with only one half drawn is a point missing.
        let (mut dark, mut light) = (Strokes::default(), Strokes::default());
        star(&mut dark, &mut light, Vec2::ZERO, 40.0);
        assert_eq!(dark.positions.len(), 16 * 3);
        assert_eq!(light.positions.len(), 16 * 3);

        // The four longest reach the rim, and nothing overruns it — the band
        // is ruled at exactly that radius.
        let reach = dark
            .positions
            .iter()
            .map(|point| Vec2::new(point[0], point[1]).length())
            .fold(f32::MIN, f32::max);
        assert!((reach - 40.0).abs() < 1e-3, "the star reached {reach}");
    }

    #[test]
    fn a_survey_keeps_the_waterline_and_the_shoal_outside_it() {
        // A cone standing out of the water: the waterline rings it, and the
        // shoal line rings that — wider, because the ground goes on shelving
        // down after it has left the air.
        let middle = Vec2::splat(CHUNK_METRES / 2.0);
        let found = survey(&a_cone(IVec2::ZERO, middle, 30.0));
        assert_eq!(found.coast.len(), 1, "one shore");
        assert_eq!(found.shoal.len(), 1, "one bank");

        let reach = |runs: &[Coast]| {
            runs[0]
                .points(IVec2::ZERO)
                .map(|point| point.distance(middle))
                .fold(f32::MIN, f32::max)
        };
        assert!(
            reach(&found.shoal) > reach(&found.coast) + 1.0,
            "the shoal line at {} did not stand outside the shore at {}",
            reach(&found.shoal),
            reach(&found.coast)
        );

        // Ground that never gets near the waterline has neither line on it,
        // which is what keeps the second one affordable — see the module docs.
        assert_eq!(survey(&all(-80.0)), Soundings::default());
        assert_eq!(survey(&all(40.0)), Soundings::default());
    }

    #[test]
    fn the_shoal_line_is_drawn_and_nothing_else() {
        // It rings nothing and closes nothing: an island is still the
        // waterline's business, and a bank around one adds no coastline to
        // the tally and no second island to name.
        let middle = Vec2::new(CHUNK_METRES, CHUNK_METRES / 2.0);
        let mut chart = Chart::default();
        chart.record(IVec2::ZERO, survey(&a_cone(IVec2::ZERO, middle, 60.0)));
        chart.record(
            IVec2::new(1, 0),
            survey(&a_cone(IVec2::new(1, 0), middle, 60.0)),
        );

        assert_eq!(chart.islands().len(), 1);
        assert_eq!(chart.tally().complete, 1);
        assert!(
            chart
                .within(Rect::from_corners(Vec2::ZERO, Vec2::splat(CHUNK_METRES)))
                .any(|(_, found)| !found.shoal.is_empty()),
            "the bank was surveyed but never kept"
        );
    }

    #[test]
    fn a_stipple_scatters_the_same_way_every_time() {
        // The engraving is rebuilt whenever the view leaves the window it was
        // drawn for. A stipple that reshuffled on a pan would read as the
        // paper crawling, so the scatter is a hash and not a random number.
        let line: Vec<Vec2> = (0..40).map(|at| Vec2::new(at as f32 * 4.0, 0.0)).collect();
        let stippled = |seed| {
            let mut out = Strokes::default();
            stipple(&mut out, &line, false, seed, (5.0, 1.5, 3.0));
            out.positions
        };
        assert_eq!(stippled(7), stippled(7));
        assert_ne!(stippled(7), stippled(8), "every run stippled alike");

        // And it stays a band along the line rather than wandering off it.
        let dots = stippled(7);
        assert!(!dots.is_empty());
        for dot in &dots {
            assert!(
                dot[1].abs() <= 3.0 / 2.0 + 1.5,
                "a dot strayed {} from the line",
                dot[1]
            );
        }
    }

    #[test]
    fn the_edge_is_ruled_inside_the_window_and_covers_what_is_outside() {
        let window = Rect::from_corners(Vec2::new(-600.0, -400.0), Vec2::new(600.0, 400.0));
        let (mut edge, mut mask) = (Strokes::default(), Strokes::default());
        neatline(&mut edge, &mut mask, window, 25.0, 1.0);

        // Nothing is drawn outside the window — the edge is furniture at the
        // window, not another thing hanging off the paper.
        for drawn in edge.positions.iter().chain(mask.positions.iter()) {
            let at = Vec2::new(drawn[0], drawn[1]);
            assert!(
                window.inflate(1.0).contains(at),
                "{at:?} was drawn outside the window"
            );
        }

        // And every scrap of window outside the inner rule is covered, all the
        // way round. The engraving is built half a window wider than the view,
        // so anywhere the mask misses is somewhere a coast can run out past the
        // neatline — the sides are the easy ones to leave out.
        let inner = window.inflate(-(NEATLINE_INSET + NEATLINE_BAND));
        for edge in [0.5, 12.0, 16.5] {
            for at in [
                Vec2::new(window.min.x + edge, 0.0),
                Vec2::new(window.max.x - edge, 0.0),
                Vec2::new(0.0, window.min.y + edge),
                Vec2::new(0.0, window.max.y - edge),
                window.min + edge,
                window.max - edge,
            ] {
                assert!(!inner.contains(at), "{at:?} is inside the drawing");
                assert!(masked(&mask, at), "{at:?} was left uncovered");
            }
        }
        // What is inside the rule is emphatically not covered.
        assert!(!masked(&mask, Vec2::ZERO));
    }

    /// Whether a point falls under any quad of a mask.
    fn masked(mask: &Strokes, at: Vec2) -> bool {
        mask.positions.chunks(6).any(|quad| {
            let corner = |pick: fn(Vec2, Vec2) -> Vec2, start| {
                quad.iter()
                    .map(|point| Vec2::new(point[0], point[1]))
                    .fold(start, pick)
            };
            Rect::from_corners(
                corner(Vec2::min, Vec2::splat(f32::INFINITY)),
                corner(Vec2::max, Vec2::splat(f32::NEG_INFINITY)),
            )
            .contains(at)
        })
    }

    #[test]
    fn the_graduations_hold_still_as_the_sheet_is_dragged() {
        // They are counted off the paper's own lattice, so dragging the sheet
        // by a whole cell draws the very same ladder one cell along — which is
        // what makes the edge read as a rule laid on the paper rather than as
        // a pattern painted on the glass.
        let cell = 25.0;
        let window = Rect::from_corners(Vec2::new(-600.0, -400.0), Vec2::new(600.0, 400.0));
        let ruled = |shift: f32| {
            let moved = Rect::from_corners(
                window.min + Vec2::new(shift, 0.0),
                window.max + Vec2::new(shift, 0.0),
            );
            let (mut edge, mut mask) = (Strokes::default(), Strokes::default());
            neatline(&mut edge, &mut mask, moved, cell, 1.0);
            edge.positions
        };
        let here = ruled(0.0);
        let along = ruled(cell * 2.0);
        assert_eq!(here.len(), along.len());
        for (a, b) in here.iter().zip(&along) {
            assert!(
                (b[0] - a[0] - cell * 2.0).abs() < 1e-3 && (b[1] - a[1]).abs() < 1e-3,
                "the ladder drew differently two cells along"
            );
        }
    }

    #[test]
    fn the_rose_keeps_its_corner_through_a_zoom() {
        // Furniture: it sits the same number of *pixels* in from the same
        // corner however much world the sheet is showing.
        let size = Vec2::new(1280.0, 720.0);
        for zoom in [MIN_METRES_PER_PIXEL, DEFAULT_METRES_PER_PIXEL, 40.0] {
            let view = ChartView {
                centre: Vec2::new(-345.0, -445.0),
                metres_per_pixel: zoom,
                opened: true,
            };
            let off = (rose_corner(&view, size) - on_the_sheet(view.centre)) / zoom;
            assert!(
                (off.x - (size.x / 2.0 - ROSE_EXTENT - FURNITURE_MARGIN)).abs() < 1e-3
                    && (off.y + (size.y / 2.0 - ROSE_EXTENT - FURNITURE_MARGIN)).abs() < 1e-3,
                "at {zoom} m to the pixel the rose sat {off:?} pixels off the middle"
            );
            // The bottom right of the sheet, which is where it is drawn.
            assert!(off.x > 0.0 && off.y < 0.0);
        }
    }
}
