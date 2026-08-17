//! What a coast turns out to be, once somebody has been near enough to look.
//!
//! This is the arithmetic that turns a chunk's corner heights into lines: the
//! waterline, the edge of the shallows outside it, and — where a waterline has
//! been followed all the way round — an *island*, with an identity and a size.
//!
//! # Why it lives on the wire's own crate
//!
//! Because two machines have to reach the same answer about it, for a reason
//! stronger than tidiness. A player claims an island by having surveyed the
//! whole of its coast, and a claim is the server's to grant: the server holds
//! the same chunks the client does, so it can walk the same coast and settle
//! the claim without believing a word the client says about it. That only
//! works while both ends are running *this* code. Split it in two and a claim
//! becomes a negotiation between two nearly-identical implementations, which
//! is the kind of thing that works until the day it silently does not.
//!
//! So: no drawing here, and no notion of a sheet or a screen. A chart is a
//! client's rendering of a survey and lives in the client. What a survey *is*
//! lives here.
//!
//! # What counts as surveyed
//!
//! A chunk is surveyed once it comes within [`SIGHT_RADIUS`] of the player,
//! afloat or ashore, and the radius is deliberately short: where the haze
//! starts, not where it ends. A survey is a record of where somebody has
//! actually been, so coast goes onto it by being closed with, a band at a time
//! as they move. An island passed down one side is surveyed on that side and
//! blank on the other until somebody goes round — which is what makes
//! "surveyed the whole of it" a thing worth having done, and a claim worth
//! something.
//!
//! Where somebody has been is a *way*, not a string of places: a hull between
//! two position reports was somewhere, and coast it plainly ran past must not
//! fall down the gap. See [`in_sight_along`], which is the rule, and
//! [`in_sight`], which is the corner of it where nobody has moved.
//!
//! Deciding all this is the server's — it holds the chunks each player has
//! surveyed and tells them what is on them (see
//! [`crate::ToClient::Surveyed`]). A client draws what it is told. It could
//! not honestly do otherwise: a claim is settled against a coast the server
//! has walked, and a client with a rule of its own about what it had seen
//! would be a client whose chart and whose claims were about two different
//! worlds.
//!
//! Not a test against any camera: a survey that filled in and stopped filling
//! in as a view was spun would record where a player had *looked* rather than
//! where they had been, and the server — which has no camera at all — could
//! not agree with it. Nor is there any test for what a headland hides.
//!
//! # Closing a coastline, and what an island is
//!
//! Because coast is surveyed by going there, "has the whole of this shore been
//! run" is a question with an answer. Strokes meet *exactly* at chunk
//! boundaries — see [`Mark`] — so a circumnavigated coastline is a chain of
//! runs that links back to its own start, found by integer bookkeeping rather
//! than by comparing points that are nearly equal.
//!
//! A closed coastline that rings **land** and reaches at least
//! [`LEAST_ISLAND`] across is an island: the claimable, nameable unit. The
//! land-on-the-left convention the contour is built with makes a ring around
//! land and a ring around a lagoon run opposite ways, so the signed area tells
//! the two apart for free.
//!
//! An island is defined here, on the coastline, rather than by asking the
//! generator what it planned. The generator plans an island to grow ground
//! from, but what it grows may meet the sea in more pieces than one: a planned
//! island can surface as a main shore and a scatter of skerries, or as two
//! hills with a drowned middle. Each piece big enough is its own island, each
//! rock awash is surveyed without being anybody's island, and none of it asks
//! the plan — which keeps generation what it ought to be, a machine that
//! produces chunks and owes nothing downstream an explanation.

use std::collections::{HashMap, HashSet};

use glam::{IVec2, Vec2};

use crate::ground::{chunk_at, CHUNK_METRES, FACET_METRES, FACET_QUADS, FACET_VERTS};

// ---------------------------------------------------------------------------
// What the survey is agreed to be
// ---------------------------------------------------------------------------

/// How near the player a chunk has to come to be surveyed, in metres.
///
/// The distance at which the haze starts taking the ground over: coast nearer
/// than this is coast somebody has actually closed with, and coast beyond it
/// is already dissolving into the air. A survey records where the player has
/// *been*, so this is deliberately short — surveying the far side of an island
/// means sailing round it, and there is a reason it must (see the module docs
/// on closing a coastline). It was most of a client's streaming radius once,
/// and that surveyed whole islands from the anchorage off one corner of them.
///
/// It sits far inside a client's streaming radius, and that is what makes the
/// survey safe rather than merely tidy: a chunk within sight is one the client
/// is certain to be holding the ground of, so ink and ground arrive together
/// and neither ever waits on the other. A client's haze is drawn to this same
/// number on purpose — the edge of what is worth recording and the edge of
/// what can be made out are one distance, and it is this one, because this is
/// the one both ends must agree on.
pub const SIGHT_RADIUS: f32 = 320.0;

/// [`in_sight_along`] measures a chunk by its four corners, which is only the
/// whole answer while no chunk can hide inside the radius — see the note there
/// on why a way running clean through a square still comes back with a corner's
/// distance. A radius shorter than a chunk's diagonal would quietly stop
/// surveying the ground a fast hull ran straight over, which is a hole nothing
/// would report, so the invariant is pinned rather than left to be remembered.
const _: () = assert!(SIGHT_RADIUS * SIGHT_RADIUS >= 2.0 * CHUNK_METRES * CHUNK_METRES);

/// How far a surveyed line may stray from the contour it was taken from, in
/// metres.
///
/// The contour comes off the grid with a point every crossing, which is far
/// more than a coast's shape needs; this is what most of them are thrown away
/// against. Three metres is under a facet, so nothing a facet could actually
/// say is lost.
pub const TOLERANCE: f32 = 3.0;

/// The depth whose edge is worth recording alongside the waterline, in metres.
///
/// Three metres because it is deeper than any boat here draws and shallower
/// than the shelf most coasts stand on: a shore whose ground plunges carries
/// the line right against its own outline, and a shore with a bank off it
/// carries the line out where the bank ends, which is the difference worth
/// seeing.
pub const SHOAL_DEPTH: f32 = -3.0;

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
/// the rest of it is the neighbour's.
pub const LEAST_ISLET: f32 = 6.0;

/// The smallest closed coastline that counts as an *island*, as the longer side
/// of what it rings, in metres.
///
/// The claimable, nameable unit — see the module docs. Below this a ring is a
/// rock or a skerry: surveyed, drawn, honestly part of the coast, but not a
/// place anybody would put a name to. A hundred metres is a little under the
/// smallest island the generator sets out to make, so everything *meant* as an
/// island qualifies and the accidents of a coast mostly do not.
///
/// Not to be confused with [`LEAST_ISLET`], which is about ink — what is too
/// small to draw at all — where this is about standing: what is too small to
/// claim. The gap between the two is exactly the skerries.
pub const LEAST_ISLAND: f32 = 100.0;

// ---------------------------------------------------------------------------
// The pieces of a survey
// ---------------------------------------------------------------------------

/// One point of a surveyed coastline, in chunk-local steps.
///
/// Two bytes, and the whole reason an infinite world's survey fits: a step is
/// [`CHUNK_METRES`] over 255, about half a metre, which is a quarter of a facet
/// and far finer than [`TOLERANCE`] has already thrown away.
///
/// A step also lands the ends of a stroke *exactly* on the chunk boundary — 0
/// and 255 are the boundary itself — so a coast leaving one chunk and the one
/// continuing it next door meet with nothing between them. The two neighbours
/// agree because they were sent the same corner heights along the edge they
/// share, and both quantise the crossing the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark {
    x: u8,
    z: u8,
}

/// Metres one step of a [`Mark`] stands for.
pub const MARK_STEP: f32 = CHUNK_METRES / u8::MAX as f32;

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
    pub fn local(self) -> Vec2 {
        Vec2::new(self.x as f32, self.z as f32) * MARK_STEP
    }

    /// The mark as the two bytes it is, for whoever is writing it down — and
    /// [`Mark::unpack`] to take it back. The pair is `[x, z]`.
    pub fn pack(self) -> [u8; 2] {
        [self.x, self.z]
    }

    pub fn unpack([x, z]: [u8; 2]) -> Self {
        Self { x, z }
    }
}

/// One run of surveyed coastline inside a single chunk.
///
/// Held per chunk rather than as whole islands because that is the unit the
/// survey works in and the unit a player discovers the world in. An island half
/// seen is a set of chunks half of which have been looked at, and joining their
/// strokes into one island's outline would mean deciding what to do about the
/// joins that are not there yet.
#[derive(Clone, Debug, PartialEq)]
pub struct Coast {
    /// The points of the run, land always on the left hand of the direction of
    /// travel — which is what lets a shore's ticks be drawn without asking the
    /// height field a second question, and what makes the runs chain.
    pub marks: Vec<Mark>,
    /// Whether the run closes on itself: an islet small enough to sit inside
    /// one chunk. An open run leaves the chunk by its edge, and is continued —
    /// or is not — by the neighbour's own survey.
    pub closed: bool,
}

impl Coast {
    /// A run as somebody read it back off a file — the survey's own runs are
    /// built by [`survey`], and this is only for reloading what that once made.
    pub fn new(marks: Vec<Mark>, closed: bool) -> Self {
        Self { marks, closed }
    }

    /// The run in world metres, given the chunk it belongs to.
    pub fn points(&self, chunk: IVec2) -> impl Iterator<Item = Vec2> + '_ {
        let base = chunk.as_vec2() * CHUNK_METRES;
        self.marks.iter().map(move |mark| base + mark.local())
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
    /// Where the ground meets the sea. The line a survey is *about*: what
    /// islands are measured from, what closes.
    pub coast: Vec<Coast>,
    /// Where the water reaches [`SHOAL_DEPTH`]. Kept to be drawn and nothing
    /// else — it rings nothing, names nothing and closes nothing, and every
    /// question asked here about islands is asked of the waterline alone.
    pub shoal: Vec<Coast>,
}

/// Both lines of one chunk's ground, ready to be kept.
pub fn survey(heights: &[f32]) -> Soundings {
    Soundings {
        coast: contour(heights, 0.0),
        shoal: contour(heights, SHOAL_DEPTH),
    }
}

/// Whether any part of a chunk is within sight of a point — a way that goes
/// nowhere; see [`in_sight_along`], which is the rule this is a corner of.
pub fn in_sight(chunk: IVec2, at: Vec2) -> bool {
    in_sight_along(chunk, at, at)
}

/// Whether any part of a chunk comes within sight of anywhere on the way from
/// `from` to `to`.
///
/// The way rather than only the end of it, because where a player *is* arrives
/// a few times a second and a hull between two of those reports was somewhere:
/// a coast a fast boat plainly ran past must not fall down the gap between two
/// positions. A survey is a record of where somebody has been, and being
/// somewhere for a fifteenth of a second is having been there.
///
/// The distance is between the square and the whole segment, exactly, rather
/// than sampled along it — samples spaced anything at all leave a scallop of
/// coast between them uncovered, which is the same hole one report further
/// apart. Both shapes are convex, so the nearest pair of points has a corner
/// of one of them in it: the least of the two ends against the square and the
/// square's four corners against the segment is the whole answer.
///
/// "Whole answer" for a *predicate*, and only that. A way that runs clean
/// through the square, entering by one edge and leaving by another, is nought
/// metres from it and comes back here as the distance to whichever corner is
/// nearest — never more than the square's diagonal out. So this is a true
/// distance where the two are apart and an overestimate where they overlap,
/// and it answers correctly only because [`SIGHT_RADIUS`] is longer than that
/// diagonal, which is pinned where the radius is set. Do not read the number
/// out of it; read the answer.
pub fn in_sight_along(chunk: IVec2, from: Vec2, to: Vec2) -> bool {
    let corner = chunk.as_vec2() * CHUNK_METRES;
    let (least, most) = (corner, corner + CHUNK_METRES);
    let to_square = |at: Vec2| at.clamp(least, most).distance_squared(at);

    let along = to - from;
    let reach = along.length_squared();
    let to_way = |at: Vec2| {
        // A way that goes nowhere is its own nearest point, which is what the
        // zero guard is for — and what makes [`in_sight`] this same function.
        let t = if reach > 0.0 {
            ((at - from).dot(along) / reach).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (from + along * t).distance_squared(at)
    };

    let nearest = to_square(from)
        .min(to_square(to))
        .min(to_way(least))
        .min(to_way(Vec2::new(most.x, least.y)))
        .min(to_way(most))
        .min(to_way(Vec2::new(least.x, most.y)));
    nearest <= SIGHT_RADIUS * SIGHT_RADIUS
}

// ---------------------------------------------------------------------------
// Soundings, written down
// ---------------------------------------------------------------------------

/// The most bytes one chunk's soundings can take on the wire.
///
/// Derived rather than picked, because a frame's ceiling is worked out from it
/// — see [`crate::ToClient::Surveyed`] — and what it has to promise is that
/// one chunk's ink always fits one message, however torn its coast.
///
/// A level's contour runs along the edges of the facet grid, and every
/// crossing belongs to exactly one run, so a level's marks are at most the
/// edges there are: [`FACET_QUADS`] × [`FACET_VERTS`] of them each way. A run
/// that is kept holds at least two marks, so the runs are at most half that
/// again, and the worst case is where both bounds are tight at once. Two
/// levels of it, and the two counts on the front.
///
/// It comes to a great deal more than any real coast: a chunk of ordinary
/// shore is a few dozen bytes, and this is what a chunk would cost whose
/// ground crossed the waterline at every facet of it. That is the point — a
/// ceiling that only holds for plausible ground is not a ceiling.
pub const SOUNDINGS_BYTES: usize = {
    let crossings = FACET_QUADS * FACET_VERTS * 2;
    2 * (crossings * 2 + crossings / 2 * 3) + 4
};

impl Soundings {
    /// How many bytes these soundings take on the wire — what a server fills
    /// a message to a budget by, so that the budget and the bytes that
    /// actually go are one arithmetic rather than two.
    pub fn bytes(&self) -> usize {
        4 + self
            .coast
            .iter()
            .chain(&self.shoal)
            .map(|run| 3 + run.marks.len() * 2)
            .sum::<usize>()
    }

    /// Appends these soundings: how many runs of each line, then the
    /// waterline's runs and the shoal's, each as its closing flag, its count
    /// of marks and the marks themselves.
    ///
    /// The counts are `u16` and cannot overflow one: the grid bounds both of
    /// them far below that — see [`SOUNDINGS_BYTES`], which is the same
    /// bound spent in bytes.
    pub(crate) fn put(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&(self.coast.len() as u16).to_le_bytes());
        out.extend_from_slice(&(self.shoal.len() as u16).to_le_bytes());
        for run in self.coast.iter().chain(&self.shoal) {
            out.push(u8::from(run.closed));
            out.extend_from_slice(&(run.marks.len() as u16).to_le_bytes());
            for mark in &run.marks {
                out.extend_from_slice(&mark.pack());
            }
        }
    }

    /// Reads one chunk's soundings off the front of `bytes`, and hands back
    /// what is left — a message carries a batch of them, so the reader has to
    /// be told where each one ended.
    ///
    /// `None` for anything that is not soundings: a count that runs off the
    /// end, or a flag byte that is neither shape of run.
    pub(crate) fn take(bytes: &[u8]) -> Option<(Self, &[u8])> {
        fn count(bytes: &[u8]) -> Option<(usize, &[u8])> {
            let (head, rest) = bytes.split_at_checked(2)?;
            Some((u16::from_le_bytes([head[0], head[1]]) as usize, rest))
        }
        // Nothing is reserved ahead of being read: the counts are a reader's
        // to believe only as far as the bytes behind them go, and a frame
        // claiming thousands of runs it has not got must cost nothing.
        fn runs(bytes: &[u8], how_many: usize) -> Option<(Vec<Coast>, &[u8])> {
            let mut rest = bytes;
            let mut runs = Vec::new();
            for _ in 0..how_many {
                let (&flag, after) = rest.split_first()?;
                let closed = match flag {
                    0 => false,
                    1 => true,
                    _ => return None,
                };
                let (marks, after) = count(after)?;
                // A run of fewer than two marks is not a line, and it is the
                // same bar [`contour`] builds behind: everything downstream
                // reads a run's first mark and its last without asking, so a
                // run of none would take [`Survey::coastlines`] straight off
                // the end of it. The contour cannot make one, which means a
                // frame carrying one is a server saying something this code
                // has no meaning for — refused here rather than carried into
                // the drawing, where it would be a client crashed by whatever
                // it was talking to.
                if marks < 2 {
                    return None;
                }
                let (packed, after) = after.split_at_checked(marks * 2)?;
                runs.push(Coast::new(
                    packed
                        .chunks_exact(2)
                        .map(|pair| Mark::unpack([pair[0], pair[1]]))
                        .collect(),
                    closed,
                ));
                rest = after;
            }
            Some((runs, rest))
        }

        let (coasts, rest) = count(bytes)?;
        let (shoals, rest) = count(rest)?;
        let (coast, rest) = runs(rest, coasts)?;
        let (shoal, rest) = runs(rest, shoals)?;
        Some((Self { coast, shoal }, rest))
    }
}

// ---------------------------------------------------------------------------
// A survey, and the islands in it
// ---------------------------------------------------------------------------

/// Everything one player has been near enough to look at.
///
/// An entry means *surveyed*, and most entries are empty: open water and the
/// inland parts of an island have no coastline on them, and recording that they
/// were looked at is what stops them being surveyed again. It is also, in the
/// end, the record of where that player has been.
///
/// The arithmetic that matters is the size: a player who calls at a thousand
/// islands is holding a few megabytes, which is a fraction of a single island's
/// ground. There is no eviction here and none is wanted — an infinite world is
/// affordable because of what is never stored, not because of what is thrown
/// away later.
#[derive(Clone, Debug, Default)]
pub struct Survey {
    soundings: HashMap<IVec2, Soundings>,
}

/// An island a survey has closed: the claimable, nameable unit.
///
/// What names it is the ring's least point on the step lattice, which is the
/// same points however the walk went round — see [`Survey::coastlines`]. No
/// name is carried here: what an island is called is something a player knows
/// about it, not something the coast says.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Island {
    /// What names the island for as long as the world lives.
    pub id: IVec2,
    /// The middle of its bounding box, in world metres.
    pub centre: Vec2,
    /// How far it reaches: the longer side of that box, in metres.
    pub extent: f32,
}

/// What a survey holds, counted — the whole of it in one walk of the coasts.
///
/// The first two are the record of where somebody has been. The last three are
/// what going and looking earns: how many coastlines have been followed all the
/// way round, how many are still being worked on, and how many of the closed
/// ones are islands anybody could claim.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SurveyTally {
    /// Chunks surveyed at all.
    pub surveyed: usize,
    /// Chunks whose survey found coast, and so hold any ink.
    pub coastal: usize,
    /// Coastlines that close: an islet ringed inside one chunk, or a chain of
    /// runs across chunks that links back to its own start.
    pub complete: usize,
    /// Chains that do not close yet — coasts with their ends still hanging,
    /// waiting for somebody to go and look.
    pub open: usize,
    /// The closed coastlines that are *islands*: rings with the land inside and
    /// at least [`LEAST_ISLAND`] across. The claimable count — the gap between
    /// it and [`SurveyTally::complete`] is the skerries and the lagoons.
    pub islands: usize,
}

impl Survey {
    /// Whether this chunk has been surveyed at all.
    pub fn surveyed(&self, chunk: IVec2) -> bool {
        self.soundings.contains_key(&chunk)
    }

    /// How many chunks have been surveyed.
    pub fn chunks(&self) -> usize {
        self.soundings.len()
    }

    /// How many surveyed chunks found any coast, and so hold any ink.
    pub fn coastal(&self) -> usize {
        self.soundings
            .values()
            .filter(|found| !found.coast.is_empty())
            .count()
    }

    /// What one chunk's survey found, if it has been surveyed.
    pub fn get(&self, chunk: IVec2) -> Option<&Soundings> {
        self.soundings.get(&chunk)
    }

    /// Records what one chunk's ground turned out to hold.
    pub fn record(&mut self, chunk: IVec2, found: Soundings) {
        self.soundings.insert(chunk, found);
    }

    /// Everything surveyed within a rectangle of the world, chunk by chunk.
    ///
    /// The rectangle is what keeps drawing bounded: a survey of a long voyage
    /// holds far more coastline than a sheet can show, so a client builds its
    /// mesh from a window on it rather than from everything ever seen.
    pub fn within(&self, least: Vec2, most: Vec2) -> impl Iterator<Item = (IVec2, &Soundings)> {
        let lower = chunk_at(least);
        let upper = chunk_at(most);
        (lower.y..=upper.y)
            .flat_map(move |z| (lower.x..=upper.x).map(move |x| IVec2::new(x, z)))
            .filter_map(|chunk| Some((chunk, self.soundings.get(&chunk)?)))
    }

    /// Follows every coastline the survey holds, handing each closed one to
    /// `close` — whole, in the order the shore is walked, under the identity
    /// that names it — and counting what it found: coastlines that close, and
    /// chains still hanging open.
    ///
    /// Whether a coastline closes is integer bookkeeping, not geometry: a
    /// [`Mark`] lands exactly on the chunk boundary, so a run's end and its
    /// continuation's start are the *same* point on the step lattice, and
    /// following a shore is a lookup. The walk is the contour's own two-pass
    /// one a scale up — chains first from every run nothing links into, then
    /// whatever is left, which can only be loops.
    ///
    /// The identity handed with each ring is its least point on that same
    /// lattice. It is integers, so no tolerance is involved; chunks are
    /// surveyed once and never again, so a closed ring is the same points for
    /// as long as the survey lives; and the *least* point in particular does
    /// not care where the walk happened to start, which a hash map decides.
    pub fn coastlines(
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

        // Follows a shore from one run for as long as the links hold, and says
        // which runs it passed through.
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

        // Whatever is left links into itself: a coastline somebody has been
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

    /// The islands the survey has closed, each measured.
    ///
    /// Sorted by identity, so that two machines walking the same survey hand
    /// back the same list in the same order however their hash maps felt about
    /// it. Nothing here depends on that order — an island's identity is its
    /// least point, not its place in a list — but a claim is settled by
    /// comparing these, and a stable order makes that comparison something a
    /// test can pin.
    pub fn islands(&self) -> Vec<Island> {
        let mut islands = Vec::new();
        self.coastlines(&mut |id, ring| {
            let measured = measure(ring);
            if measured.is_island() {
                islands.push(Island {
                    id,
                    centre: measured.centre,
                    extent: measured.extent,
                });
            }
        });
        islands.sort_by_key(|island| (island.id.x, island.id.y));
        islands
    }

    /// The island of this identity, if the survey has closed it — which is how
    /// a claim is settled: the claimant names an island, and the answer is
    /// whether the coast they are standing on says there is one.
    pub fn island(&self, id: IVec2) -> Option<Island> {
        self.islands().into_iter().find(|island| island.id == id)
    }

    /// Everything the survey holds, counted in a single walk of the coasts.
    ///
    /// One walk rather than a count and a list, because measuring a ring is
    /// what decides whether it is an island, and the walk is already handing
    /// every ring over to be measured.
    pub fn tally(&self) -> SurveyTally {
        let mut islands = 0;
        let (complete, open) = self.coastlines(&mut |_, ring| {
            if measure(ring).is_island() {
                islands += 1;
            }
        });
        SurveyTally {
            surveyed: self.chunks(),
            coastal: self.coastal(),
            complete,
            open,
            islands,
        }
    }
}

/// A mark as a point on the world-wide step lattice: 255 whole steps to a
/// chunk, so a run ending on a chunk's boundary and the run continuing it next
/// door land on the *same* integers — which is what lets
/// [`Survey::coastlines`] follow a shore across chunks by equality rather than
/// by tolerance.
///
/// Saturating, because the chunk comes off the wire and nothing on the way in
/// holds it to the reach the world resolves over: a server naming a chunk out
/// near [`i32::MAX`] would otherwise overflow this, which is a panic in a
/// client for a message it merely received. Saturating puts such a chunk
/// somewhere absurd where it links to nothing, which is the right amount of
/// attention to pay it.
fn run_steps(chunk: IVec2, mark: Mark) -> IVec2 {
    chunk
        .saturating_mul(IVec2::splat(u8::MAX as i32))
        .saturating_add(IVec2::new(mark.x as i32, mark.z as i32))
}

/// The least of a ring's points on the step lattice, west before south — the
/// ring's identity, whatever order its points arrive in.
fn ring_id(marks: impl Iterator<Item = IVec2>) -> IVec2 {
    marks
        .min_by_key(|point| (point.x, point.y))
        .expect("a ring has points")
}

/// A closed ring, measured.
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

/// A closed ring measured: its reach, its signed area, and where the middle of
/// it falls.
///
/// The sign is what tells an island from a lagoon, and it is the contour's
/// land-on-the-left convention paying out a second time: a ring walked with the
/// land always on the left runs one way around land and the other around
/// enclosed water, so land inside is exactly a positive shoelace sum. No second
/// look at any height field, and no flag stored — the direction of travel *is*
/// the answer.
///
/// The sum is taken with the z axis negated, because world z runs south while a
/// shoelace sum assumes the second axis runs north; without the flip every sign
/// here would be the wrong way round.
///
/// Consecutive duplicate points — the joins, where one run's end is the next
/// run's identical start — contribute nothing to any measure, so a chain can be
/// fed through whole without trimming them.
fn measure(ring: &mut dyn Iterator<Item = Vec2>) -> Ring {
    let upright = |at: Vec2| Vec2::new(at.x, -at.y);
    let mut least = Vec2::splat(f32::INFINITY);
    let mut most = Vec2::splat(f32::NEG_INFINITY);
    let mut area = 0.0;
    let mut first = None;
    let mut previous: Option<Vec2> = None;
    for point in ring.map(upright) {
        least = least.min(point);
        most = most.max(point);
        if let Some(previous) = previous {
            area += previous.perp_dot(point);
        }
        first.get_or_insert(point);
        previous = Some(point);
    }
    // The ring is closed, so the walk back from the last point to the first is
    // part of it whether or not the points spell it out.
    if let (Some(first), Some(last)) = (first, previous) {
        area += last.perp_dot(first);
    }
    // The centre goes back the way it came: everything outside this function
    // works in world metres.
    let centre = (least + most) / 2.0;
    Ring {
        extent: (most - least).max_element(),
        area: area / 2.0,
        centre: upright(centre),
    }
}

// ---------------------------------------------------------------------------
// Marching squares
// ---------------------------------------------------------------------------

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
/// convention the rest of this module leans on. It is what hangs a shore's
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

#[cfg(test)]
mod tests {
    use super::*;

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

    /// The waterline alone, for the tests that are about the contour walk
    /// rather than about what a chunk is worth keeping.
    fn waterline(heights: &[f32]) -> Vec<Coast> {
        contour(heights, 0.0)
    }

    /// What a survey has closed, counted: coastlines followed all the way
    /// round, chains still hanging open, and the closed ones big enough to be
    /// islands. The three numbers most of these tests are about.
    fn counted(survey: &Survey) -> (usize, usize, usize) {
        let (complete, open) = survey.coastlines(&mut |_, _| {});
        (complete, open, survey.islands().len())
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
        // The convention everything else here leans on. This shore has land to
        // the north — falling z — so a walk with land on its left hand runs
        // east, in the direction of rising x.
        let runs = waterline(&a_north_shore(50.0));
        let marks = &runs[0].marks;
        assert!(
            marks[marks.len() - 1].x > marks[0].x,
            "the run goes west, so land is on its right"
        );
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
    fn a_chunk_sailed_past_between_two_reports_is_still_seen() {
        // The hole a fast hull would otherwise leave. Two positions a
        // kilometre apart — neither of them anywhere near this chunk — with
        // the chunk sitting square on the line between them.
        let chunk = IVec2::new(4, 0);
        let (from, to) = (Vec2::new(0.0, 64.0), Vec2::new(1_024.0, 64.0));
        assert!(!in_sight(chunk, from) && !in_sight(chunk, to));
        assert!(
            in_sight_along(chunk, from, to),
            "a chunk was sailed through"
        );

        // And the way is a way, not a corridor without end: ground off to one
        // side of it stays unsurveyed.
        let aside = IVec2::new(4, 6);
        assert!(!in_sight_along(aside, from, to));
    }

    #[test]
    fn soundings_survive_being_written_down() {
        // The wire's own copy of a chunk's ink, there and back — and the
        // count of bytes a server fills a message by agreeing exactly with
        // the bytes that go into it.
        let middle = Vec2::splat(CHUNK_METRES / 2.0);
        for found in [
            survey(&a_cone(IVec2::ZERO, middle, 30.0)),
            survey(&a_north_shore(50.0)),
            // Surveyed and blank, which is most of an ocean.
            Soundings::default(),
        ] {
            let mut written = Vec::new();
            found.put(&mut written);
            assert_eq!(written.len(), found.bytes(), "the count is not the bytes");
            assert!(found.bytes() <= SOUNDINGS_BYTES);

            // Read back off the front of a longer run of bytes, since a
            // message carries a batch of these one after another.
            written.extend([0xAB, 0xCD]);
            let (read, rest) = Soundings::take(&written).expect("soundings");
            assert_eq!(read, found);
            assert_eq!(rest, [0xAB, 0xCD], "the reader lost its place");
        }
    }

    #[test]
    fn soundings_that_are_not_soundings_are_refused() {
        // A count with nothing behind it, and a run of a shape that is
        // neither open nor closed.
        assert!(Soundings::take(&[1, 0, 0, 0]).is_none());
        assert!(Soundings::take(&[1, 0, 0, 0, 9, 1, 0, 5, 5]).is_none());
        assert!(Soundings::take(&[0, 0]).is_none());
        // And a run with no line in it: the contour cannot make one, and
        // everything downstream reads a run's ends without asking, so one
        // arriving off the wire is a message to refuse rather than a shape
        // to carry into the drawing.
        assert!(
            Soundings::take(&[1, 0, 0, 0, 0, 0, 0]).is_none(),
            "a run of no marks"
        );
        assert!(
            Soundings::take(&[1, 0, 0, 0, 0, 1, 0, 5, 5]).is_none(),
            "a run of one mark"
        );
        // The shoal's runs are read by the same rule as the waterline's.
        assert!(
            Soundings::take(&[0, 0, 1, 0, 0, 1, 0, 5, 5]).is_none(),
            "a shoal run of one mark"
        );
        // And nothing at all is a chunk surveyed and found blank.
        assert_eq!(
            Soundings::take(&[0, 0, 0, 0]).expect("blank soundings").0,
            Soundings::default()
        );
    }

    #[test]
    fn a_chunk_from_the_end_of_the_integers_is_read_without_falling_over() {
        // Chunk coordinates arrive off the wire and nothing on the way in
        // holds them to the reach the world resolves over. A survey naming a
        // chunk out at the end of the integers has to come back as a chart
        // nobody can steer by — not as a client that overflowed reading what
        // it was sent.
        let ring = Coast::new(
            vec![
                Mark::unpack([0, 0]),
                Mark::unpack([200, 0]),
                Mark::unpack([200, 200]),
            ],
            true,
        );
        let mut kept = Survey::default();
        kept.record(
            IVec2::splat(i32::MAX),
            Soundings {
                coast: vec![ring],
                shoal: Vec::new(),
            },
        );
        assert_eq!(kept.tally().complete, 1, "the ring was not even walked");
    }

    #[test]
    fn a_mark_is_half_a_metre_and_lands_on_the_boundary() {
        // The two properties two bytes have to have: fine enough that nothing
        // drawn from them notices, and exact at a chunk's edges so that two
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
    fn a_surveyed_chunk_with_nothing_on_it_is_still_surveyed() {
        // Which is what stops open water being surveyed again on every frame
        // somebody sits beside it.
        let mut kept = Survey::default();
        assert!(!kept.surveyed(IVec2::ZERO));
        kept.record(IVec2::ZERO, survey(&all(-8.0)));
        assert!(kept.surveyed(IVec2::ZERO));

        assert_eq!((kept.chunks(), kept.coastal()), (1, 0));
    }

    #[test]
    fn a_coastline_closes_only_when_the_player_has_been_all_the_way_round() {
        // An island astride two chunks: a cone centred on their shared
        // boundary, so each chunk's survey holds half its shore as one open
        // run. With one half surveyed the coastline hangs open — which is what
        // stops half-explored islands reading as done — and the other half
        // closes it: the two runs link end-to-start, exactly, across the
        // boundary.
        let middle = Vec2::new(CHUNK_METRES, CHUNK_METRES / 2.0);
        let mut kept = Survey::default();
        kept.record(IVec2::ZERO, survey(&a_cone(IVec2::ZERO, middle, 60.0)));

        assert_eq!(
            counted(&kept),
            (0, 1, 0),
            "half an island read as a closed coastline"
        );

        kept.record(
            IVec2::new(1, 0),
            survey(&a_cone(IVec2::new(1, 0), middle, 60.0)),
        );
        assert_eq!(
            counted(&kept),
            (1, 0, 1),
            "the whole shore does not close into an island"
        );
    }

    #[test]
    fn a_skerry_closes_as_a_coastline_without_being_an_island() {
        // Many an island is a little archipelago — a main shore with skerries
        // off it. Each ring closes on its own, and the threshold is what says
        // which of them are islands to claim: the main shore is one, and the
        // rock off it is surveyed without being anybody's island.
        let mut kept = Survey::default();
        kept.record(
            IVec2::ZERO,
            survey(&a_cone(IVec2::ZERO, Vec2::splat(64.0), 55.0)),
        );
        kept.record(
            IVec2::new(5, 5),
            survey(&a_cone(
                IVec2::new(5, 5),
                Vec2::splat(5.0 * CHUNK_METRES + 64.0),
                12.0,
            )),
        );

        let (complete, open, islands) = counted(&kept);
        assert_eq!((complete, open), (2, 0));
        assert_eq!(islands, 1, "the skerry counts as an island");
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
        let mut kept = Survey::default();
        kept.record(IVec2::ZERO, survey(&heights));

        let (complete, open, islands) = counted(&kept);
        assert_eq!((complete, open), (1, 0));
        assert_eq!(islands, 0, "a lagoon was claimed as an island");
    }

    #[test]
    fn an_islet_ringed_in_one_chunk_is_already_closed() {
        // Big enough to close, and — at sixty metres across — too small to be
        // an island anybody names.
        let mut kept = Survey::default();
        kept.record(
            IVec2::ZERO,
            survey(&a_cone(IVec2::ZERO, Vec2::splat(CHUNK_METRES / 2.0), 30.0)),
        );

        assert_eq!(counted(&kept), (1, 0, 0));
    }

    #[test]
    fn an_islands_identity_survives_more_of_the_world_arriving() {
        // The id has to keep naming the same ring while the survey around it
        // grows, or whatever was written against it — a name, a claim — would
        // fall off its island when the player sailed on. More survey arriving
        // elsewhere must not move it.
        let middle = Vec2::new(CHUNK_METRES, CHUNK_METRES / 2.0);
        let mut kept = Survey::default();
        kept.record(IVec2::ZERO, survey(&a_cone(IVec2::ZERO, middle, 60.0)));
        kept.record(
            IVec2::new(1, 0),
            survey(&a_cone(IVec2::new(1, 0), middle, 60.0)),
        );
        let id = kept.islands()[0].id;

        kept.record(IVec2::new(4, 4), survey(&a_north_shore(50.0)));
        kept.record(IVec2::new(-3, 2), survey(&all(-8.0)));
        assert_eq!(kept.islands()[0].id, id);
    }
}
