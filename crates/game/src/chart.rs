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
//! line where it meets the sea:
//!
//! | held per chunk | a 1.5 km island, sailed right around |
//! | --- | --- |
//! | the height grid it arrived as | about a megabyte |
//! | its coastline, simplified | a few kilobytes |
//!
//! Three things do that. Only *coastal* chunks hold anything at all — open
//! water arrives as no payload whatever, and an island's interior has no
//! sea-level crossing in it, so what is kept is a ring one chunk thick and not
//! an island's worth of area. What is kept of that ring is simplified to
//! [`TOLERANCE`], which the chart can afford: a bay owes the player its shape
//! and not its rocks. And a point is a [`Mark`], two bytes, because half a
//! metre is finer than any hand ever drew a shoreline.
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
//! A *claim* itself would cross the wire and be the server's to grant. The
//! server holds the same chunks, so it can check a claim by this same
//! arithmetic without trusting the client — and when that day comes, the
//! contour-and-ring arithmetic should move to `protocol`, where the things
//! both ends must agree on live, so the two cannot drift.
//!
//! # Ink, not paper
//!
//! An old chart's character is easy to get from a paper texture and a wash of
//! stains, and that is the one way it cannot be got here: this world is flat
//! tones with no texture and no gradient anywhere in it, and a mottled sheet
//! would be the only one of either. So the hand is in the line instead — a
//! coast weighted heavier than the graticule under it, ticked on its landward
//! side the way an engraved chart hatches its shores, and laid on one flat
//! tone of parchment. The furniture is lettered in the serif the menus are
//! set in; the islands are named in an italic of the Fell types (see
//! [`NAME_FONT`]), which is the nearest a flat sheet comes to an engraver's
//! hand.
//!
//! A coast that has not been closed is not closed on the sheet either. Half an
//! island is drawn as half an island, the line simply stopping where the survey
//! did — which is what a partly run coastline looked like on a real chart, and
//! saves inventing the other side.

use bevy::asset::RenderAssetUsages;
use bevy::camera::RenderTarget;
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::mesh::PrimitiveTopology;
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bevy::text::{Font, FontSize, FontSource};

use protocol::ground::{chunk_at, CHUNK_METRES, FACET_METRES, FACET_QUADS, FACET_VERTS};

use crate::bindings::{Action, KeyBindings};
use crate::camera::MapCamera;
use crate::compass::Rose;
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
    /// height field a second question.
    marks: Vec<Mark>,
    /// Whether the run closes on itself: an islet small enough to sit inside
    /// one chunk. An open run leaves the chunk by its edge, and is continued —
    /// or is not — by the neighbour's own survey.
    closed: bool,
}

impl Coast {
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
    coasts: HashMap<IVec2, Vec<Coast>>,
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
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Island {
    /// The middle of its bounding box, on the sheet (see [`on_the_sheet`]).
    pub centre: Vec2,
    /// How far it reaches: the longer side of that box, in metres.
    pub extent: f32,
}

impl Chart {
    /// Whether this chunk has been surveyed at all.
    pub fn surveyed(&self, chunk: IVec2) -> bool {
        self.coasts.contains_key(&chunk)
    }

    /// Counts what the chart holds — including, in [`ChartTally::islands`],
    /// the closed coastlines that are islands to claim.
    pub fn tally(&self) -> ChartTally {
        let surveyed = self.coasts.len();
        let coastal = self.coasts.values().filter(|runs| !runs.is_empty()).count();
        let mut islands = 0;
        let (complete, open) = self.coastlines(&mut |ring| {
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
        self.coastlines(&mut |ring| {
            let measured = measure(ring);
            if measured.is_island() {
                islands.push(Island {
                    centre: measured.centre,
                    extent: measured.extent,
                });
            }
        });
        islands
    }

    /// Follows every coastline the chart holds, handing each closed one to
    /// `close` — whole, in the order the shore is walked — and counting what
    /// it found: coastlines that close, and chains still hanging open.
    ///
    /// Whether a coastline closes is integer bookkeeping, not geometry: a
    /// [`Mark`] lands exactly on the chunk boundary, so a run's end and its
    /// continuation's start are the *same* point on the step lattice, and
    /// following a shore is a lookup. The walk is the survey's own two-pass
    /// one a scale up — chains first from every run nothing links into, then
    /// whatever is left, which can only be loops.
    fn coastlines(&self, close: &mut dyn FnMut(&mut dyn Iterator<Item = Vec2>)) -> (usize, usize) {
        let mut complete = 0;

        // Where each open run starts and ends, in whole steps of the mark
        // lattice — exact, so equality is equality.
        let mut starts: HashMap<IVec2, (IVec2, usize)> = HashMap::default();
        let mut ends: HashSet<IVec2> = HashSet::default();
        for (&chunk, runs) in &self.coasts {
            for (at, run) in runs.iter().enumerate() {
                if run.closed {
                    // A ring inside one chunk closed the moment it was drawn.
                    complete += 1;
                    close(&mut run.points(chunk));
                    continue;
                }
                starts.insert(run_steps(chunk, run.marks[0]), (chunk, at));
                ends.insert(run_steps(chunk, run.marks[run.marks.len() - 1]));
            }
        }

        // Follows a shore from one run for as long as the links hold, and
        // says which runs it passed through.
        let end_of = |key: (IVec2, usize)| {
            let run = &self.coasts[&key.0][key.1];
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
            close(
                &mut chain
                    .iter()
                    .flat_map(|&(chunk, at)| self.coasts[&chunk][at].points(chunk)),
            );
        }

        (complete, open)
    }

    /// Every run of coast within a rectangle of the world.
    ///
    /// The rectangle is what keeps drawing bounded. A chart of a long voyage
    /// holds far more coastline than a sheet can show, so the mesh is built
    /// from a window on it rather than from everything ever seen — and the cost
    /// of drawing follows how much paper there is rather than how far the
    /// player has sailed.
    fn within(&self, window: Rect) -> impl Iterator<Item = (IVec2, &Coast)> {
        let lower = chunk_at(window.min);
        let upper = chunk_at(window.max);
        (lower.y..=upper.y)
            .flat_map(move |z| (lower.x..=upper.x).map(move |x| IVec2::new(x, z)))
            .filter_map(|chunk| Some((chunk, self.coasts.get(&chunk)?)))
            .flat_map(|(chunk, runs)| runs.iter().map(move |run| (chunk, run)))
    }

    /// Records what one chunk's ground turned out to hold.
    fn record(&mut self, chunk: IVec2, runs: Vec<Coast>) {
        self.coasts.insert(chunk, runs);
    }
}

/// A mark as a point on the world-wide step lattice: 255 whole steps to a
/// chunk, so a run ending on a chunk's boundary and the run continuing it next
/// door land on the *same* integers — which is what lets [`Chart::tally`]
/// follow a shore across chunks by equality rather than by tolerance.
fn run_steps(chunk: IVec2, mark: Mark) -> IVec2 {
    chunk * u8::MAX as i32 + IVec2::new(mark.x as i32, mark.z as i32)
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
    /// Where the waterline cuts this edge, in chunk-local metres.
    ///
    /// Straight linear interpolation between the two corner heights, which is
    /// what the mesh does between the same two corners — so the line on the
    /// chart is the line the player could walk to.
    fn at(self, heights: &[f32]) -> Vec2 {
        let corner = |ix: usize, iz: usize| heights[iz * FACET_VERTS + ix];
        let near = corner(self.ix, self.iz);
        let far = match self.along {
            Axis::X => corner(self.ix + 1, self.iz),
            Axis::Z => corner(self.ix, self.iz + 1),
        };
        // The two corners straddle the waterline — one at or above it, the
        // other strictly below — so the difference is never zero and this never
        // divides by one.
        let t = near / (near - far);
        let along = match self.along {
            Axis::X => Vec2::new(t, 0.0),
            Axis::Z => Vec2::new(0.0, t),
        };
        (Vec2::new(self.ix as f32, self.iz as f32) + along) * FACET_METRES
    }
}

/// Whether a corner height is land. At the waterline counts as land, so that a
/// corner sitting exactly at zero belongs to one side rather than to neither
/// and the classification is total.
fn is_land(height: f32) -> bool {
    height >= 0.0
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
fn segments(cell: (usize, usize), heights: &[f32]) -> Vec<(Crossing, Crossing)> {
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

    let case = u8::from(is_land(tl))
        | u8::from(is_land(tr)) << 1
        | u8::from(is_land(br)) << 2
        | u8::from(is_land(bl)) << 3;
    // How a saddle is read: land through the middle, or sea through it.
    let middle_is_land = is_land((tl + tr + bl + br) / 4.0);

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

/// Every run of waterline across one chunk's height grid, ready to be kept.
///
/// The whole survey: contour, chain, simplify, quantise. A chunk with no
/// crossing in it — open water, or ground well inland — comes back empty, which
/// is the common answer and costs one walk of the grid and nothing else.
pub fn survey(heights: &[f32]) -> Vec<Coast> {
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
            for (from, to) in segments((ix, iz), heights) {
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
        let points: Vec<Vec2> = run.iter().map(|c| c.at(heights)).collect();
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
/// sheet and the graticule under it the faintest, so that the two read as
/// drawing and ruling rather than as two kinds of line. The reader's own mark
/// is the one thing in another colour, which is how the eye finds it.
const PAPER: Color = Color::srgb(0.85, 0.79, 0.64);
const INK: Color = Color::srgb(0.24, 0.17, 0.11);
const INK_DIM: Color = Color::srgb(0.46, 0.37, 0.26);
const INK_FAINT: Color = Color::srgba(0.40, 0.31, 0.21, 0.40);
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

/// The three inks, made once when the sheet is unrolled rather than on every
/// rebuild — a material per pan would be a new asset per pan.
#[derive(Resource)]
struct Inks {
    coast: Handle<ColorMaterial>,
    ruling: Handle<ColorMaterial>,
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
                    pan,
                    zoom,
                    hold_the_sheet,
                    engrave,
                    rule_the_scale,
                    mark_the_reader,
                )
                    .chain()
                    .run_if(in_state(Helm::Chart).and_then(resource_exists::<Chart>)),
            );
    }
}

/// A world gets a blank chart, and takes it with it when it goes: what has been
/// seen is a fact about *this* world, and carrying it into the next would draw
/// one seed's islands on another's water.
fn start_a_chart(mut commands: Commands, mut view: ResMut<ChartView>) {
    commands.insert_resource(Chart::default());
    *view = ChartView::default();
}

fn stow_the_chart(mut commands: Commands) {
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
    mut next: ResMut<NextState<Helm>>,
) {
    if !keys.just_pressed(bindings.key(Action::Chart)) {
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

    commands.insert_resource(Inks {
        coast: materials.add(ColorMaterial::from_color(INK)),
        ruling: materials.add(ColorMaterial::from_color(INK_FAINT)),
        mark: materials.add(ColorMaterial::from_color(MARK_INK)),
    });
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

    furniture(&mut commands, sheet);
}

/// Rolls the sheet up and gives the world its camera back.
fn roll_up(mut commands: Commands, mut world_camera: Query<&mut Camera, With<MapCamera>>) {
    for mut camera in &mut world_camera {
        camera.is_active = true;
    }
    commands.remove_resource::<Inks>();
    commands.remove_resource::<Lettering>();
    // The engraving goes with the sheet, so the next opening draws one for
    // wherever the reader has got to rather than trusting a window from before.
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
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
) {
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
    engraved: Option<Res<Engraved>>,
    sheet: Query<&Camera, With<ChartSheet>>,
) {
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
        if held && engraved.metres_per_pixel == view.metres_per_pixel && !chart.is_changed() {
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

    // The ruling first and underneath: it is the paper's own, and the coast is
    // drawn on top of it.
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
            .spawn(engraving("Chart graticule", mesh, ink, 0.0));
    }

    let mut shore = Strokes::default();
    for (chunk, coast) in chart.within(covered) {
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
    if let Some(mesh) = shore.mesh() {
        let mesh = engraver.meshes.add(mesh);
        let ink = engraver.inks.coast.clone();
        engraver
            .commands
            .spawn(engraving("Chart coastline", mesh, ink, 1.0));
    }

    // The names, over the ink. Every island the survey has closed carries
    // one, and until players can give their own, every one reads the same.
    // The walk is over the whole chart rather than the window — a chain can
    // cross any number of chunks, so a ring cannot be closed from a window's
    // worth — and only the lettering that lands on the paper is spawned.
    let on_paper = Rect::from_corners(on_the_sheet(covered.min), on_the_sheet(covered.max));
    for island in chart.islands() {
        if !on_paper.contains(island.centre) {
            continue;
        }
        engraver.commands.spawn((
            Name::new("Chart lettering"),
            Engraving,
            ChartSheet,
            Text2d::new("Unnamed island"),
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
        (!self.positions.is_empty()).then(|| {
            Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::default(),
            )
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
        })
    }
}

/// The landward ticks along a shore.
///
/// Laid at an even spacing measured *along the line* rather than one per point,
/// so the hatching reads as an even comb whatever the survey happened to leave:
/// a run simplified down to four points would otherwise wear four ticks spread
/// across a kilometre.
///
/// Which way they hang is the survey's business and not this function's — the
/// runs arrive with land on the left, so left is where the ticks go.
fn ticks(out: &mut Strokes, points: &[Vec2], closed: bool, spacing: f32, length: f32, width: f32) {
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
        // The sheet's y climbs, so the left hand of a heading is its
        // anticlockwise quarter turn — which is what `perp` gives.
        let inland = forward.perp();
        while walked < run {
            let foot = pair[0] + forward * walked;
            out.segment(foot, foot + inland * length, width);
            walked += spacing;
        }
        walked -= run;
    }
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
// The furniture
// ---------------------------------------------------------------------------

/// Diameter of the rose, and how far the instruments sit in from their corners.
const ROSE_SIZE: f32 = 84.0;
const FURNITURE_MARGIN: f32 = 16.0;

/// The instruments in the sheet's corners: the rose that says north is up, and
/// the bar that says how far a distance reaches across the paper.
///
/// Built as UI rather than drawn into the mesh because they are pinned to the
/// window and not to the world — the whole point of them being that they do not
/// move when the sheet does. They are given the sheet's own camera so they are
/// drawn in its pass, over the engraving; the world's own instruments are left
/// alone and simply painted over.
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
            // The rose. Flat, unlike the world's compass, which lies
            // foreshortened on the sea because a bearing read off it is meant
            // to be carried out into the picture. Nothing is foreshortened on a
            // chart and nothing on this one turns: it is here to say that north
            // is up and stays up, so it is drawn once and never touched again.
            sheet
                .spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        right: Val::Px(FURNITURE_MARGIN),
                        bottom: Val::Px(FURNITURE_MARGIN),
                        width: Val::Px(ROSE_SIZE),
                        height: Val::Px(ROSE_SIZE),
                        border: UiRect::all(Val::Px(1.0)),
                        border_radius: BorderRadius::MAX,
                        ..default()
                    },
                    BorderColor::all(INK_DIM),
                ))
                .with_children(|face| {
                    Rose {
                        ink: INK,
                        dim: INK_DIM,
                        cross: INK_FAINT,
                        letters: 16.0,
                        inset: 6.0,
                        arm: 22.0,
                    }
                    .draw(face);
                });

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

    #[test]
    fn open_water_and_open_country_hold_no_coast() {
        // The common answers, and the ones that have to cost nothing: a chunk
        // with no waterline crossing it holds no coast at all, whichever side
        // of the waterline it is on.
        assert!(survey(&all(-8.0)).is_empty());
        assert!(survey(&all(40.0)).is_empty());
    }

    #[test]
    fn a_shore_is_found_where_the_ground_meets_the_sea() {
        let runs = survey(&a_north_shore(50.0));
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
        let runs = survey(&a_north_shore(50.0));
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
        let runs = survey(&a_north_shore(50.0));
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
        let runs = survey(&a_north_shore(50.0));
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
        let runs = survey(&a_cone(IVec2::ZERO, middle, 30.0));
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
        let west = survey(&heights);
        let east = survey(&heights);

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
        assert_eq!(chart.within(near).count(), 1);
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
            .record(IVec2::ZERO, Vec::new());
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
}
