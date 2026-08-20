//! The chart: what the player has seen of the world, drawn in plan on paper.
//!
//! Pressing the chart key lays the sheet over the world. North is up and stays
//! up — that is the whole difference from the view it covers, which turns —
//! and the rose in the corner says so rather than pointing anywhere.
//!
//! A chart is a *drawing of a survey*, and nothing else. What a survey is —
//! what counts as looked at, how a coastline closes, what makes a closed one
//! an island — is [`protocol::survey`]'s, because a claim is the server's to
//! grant and both ends have to reach the same answer about a coast. This
//! module holds one [`Survey`], asks it questions, and puts ink on paper.
//!
//! **Nothing here surveys anything.** The survey belongs to the world: the
//! server says what has been seen and what is on it (see
//! [`protocol::ToClient::Surveyed`]), and this module records and draws that.
//! It could not honestly have a rule of its own — a claim is judged against a
//! coast the server has walked, and a client that decided for itself what it
//! had seen would have a chart and claims about two different worlds.
//!
//! # The one thing that accumulates
//!
//! Everything else this client holds is streamed and forgotten: chunks, meshes,
//! palms, the beasts in the water. The camera moves on and `terrain` drops what
//! it has left behind, because the server is the one holding the world. The
//! chart cannot do that — a coast is worth remembering exactly as long as the
//! player is in the world — so it is the one structure here that has to be
//! designed for a world with no edges. What arrives over the wire is told once
//! and never told again, so what is on the sheet is only ever what was put
//! there.
//!
//! What makes that affordable is throwing the ground away and keeping only the
//! two lines worth drawing — where it meets the sea, and where the water over
//! it reaches [`protocol::survey::SHOAL_DEPTH`]. Sailed right around, a 1.5 km
//! island is about a megabyte of height grid and a few kilobytes of those two
//! lines; the arithmetic is [`protocol::survey`]'s.
//!
//! What belongs to the *chart* is why the second line is kept at all: it is the
//! one piece of **depth** on a sheet that is otherwise all outline, drawn
//! stippled the way an engraved chart draws the limit of a bank. But it is ink
//! and nothing else — it rings nothing, names nothing and closes nothing, and
//! every question about islands is asked of the waterline alone.
//!
//! # Names
//!
//! An island this player has *claimed* can be named: clicking one on the sheet
//! puts a caret on its lettering and the keyboard becomes the pen — there is
//! no dialog, because a chart is written on, not filled in. The pen does not
//! open on anything else, an island nobody holds having nothing to write on
//! and somebody else's having nothing this player may write.
//!
//! Nothing is lettered here. A name rides the claim it is written on, so Enter
//! offers it to the world and the cairn comes back saying whatever it now says.
//! Both are settled against the ring's identity
//! ([`protocol::survey::Island::id`]), a fact about the coast and so the same
//! on every machine.
//!
//! # What the sheet knows, and how it came to
//!
//! Three things can be true of an island here, and they are earned three
//! different ways:
//!
//! | on the paper | what it took |
//! | --- | --- |
//! | a cairn, unlettered | having seen the stones from offshore |
//! | a cairn with a name beside it | having landed and read them |
//! | a coastline, closed and lettered | having sailed the whole way round |
//!
//! Only the third is this sheet's own seeing, and only the third earns the right
//! to claim. The first two arrive as [`Claimed`] and put no stroke of coastline
//! on the paper: a chart that let hearsay close a ring would be one a player
//! could claim an island off having been *told* about it.
//!
//! How near is near enough is the server's alone and is not written down twice.
//! What matters here is that a cairn with no name on it is not a puzzle — it is
//! either an island nobody has christened or one whose stones this player has
//! not been up to, and from a mile offshore those are the same thing.
//!
//! # Ink, not paper
//!
//! An old chart's character is easy to get from a paper texture and a wash of
//! stains, and that is the one way it cannot be got here: this world is flat
//! tones with no texture or gradient anywhere in it, and a mottled sheet would
//! be the only one of either. So the hand is in the line instead — a coast
//! weighted heavier than the graticule under it, ticked on its landward side
//! the way an engraved chart hatches its shores, stippled on its seaward side
//! where the water is shallow, on one flat tone of parchment. The furniture is
//! lettered in the menus' serif; the islands are named in an italic of the Fell
//! types (see [`NAME_FONT`]).
//!
//! Outside all of it the sheet has an edge — a double rule just inside the
//! window with the ruling's graduations between the two lines, and the
//! engraving covered over beyond it. A chart drawn to the window's own edge has
//! no edge at all, and reads as a viewport rather than as a sheet.
//!
//! Under all of it is the rhumb net: roses standing on the ruling's crossings,
//! each throwing the thirty-two points of the compass across the paper (see
//! [`rhumbs`]). It is the sheet's whole character and none of its content, so
//! it is ruled fainter than anything over it, and its rays are graded — the
//! principal winds ruled, the quarter winds dashed — so a dozen roses' worth
//! reads as a net rather than a haze. The roses are drawn in two flat tones,
//! each point of the star split down its own axis (see [`star`]), two tones
//! meeting on an edge being the only way to draw a lit thing on a sheet with
//! no gradients.
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

use protocol::survey::{Soundings, Survey, SurveyTally};

use crate::bindings::{Action, KeyBindings};
use crate::camera::MapCamera;
use crate::player::PlayerPlace;
use crate::{AppState, Helm};

// ---------------------------------------------------------------------------
// What has been surveyed
// ---------------------------------------------------------------------------

/// Everything the player has seen of the world's coasts, and what they have
/// called it.
///
/// The seeing is a [`Survey`], and belongs to `protocol` because a claim is
/// settled against one. The names are the sheet's own — no coast could tell
/// anybody what an island is called. They are held together because they are
/// the same kind of fact: what *this* player holds about *this* world, gone
/// with the world when they leave it.
#[derive(Resource, Default)]
pub struct Chart {
    survey: Survey,
    /// The claims this player has been told of, keyed by [`Island::id`] — see
    /// [`Claimed`]. Not the player's own notes: an island's name belongs to
    /// the claim it was written on, so everything here arrived over the wire
    /// and none of it outlives the visit.
    claims: HashMap<IVec2, Claimed>,
}

/// A claimed island, as this sheet knows it: where the cairn stands, what it
/// is called, and whether it is the player's own.
///
/// A cairn is the only reason a name is on the sheet at all. An island nobody
/// has claimed is drawn and unlettered, however many times its own discoverer
/// has sailed round it, because a name is a thing you plant rather than a
/// thing you think.
#[derive(Clone, Debug, PartialEq)]
pub struct Claimed {
    /// Where the cairn stands, in world metres — so the sheet can mark the
    /// spot rather than the island's middle. A cairn is on a headland
    /// somebody chose, and where they chose is worth drawing.
    pub at: Vec2,
    /// What it has been christened, as far as this player has any business
    /// knowing. Empty for a claim nobody has named — the cairn goes up when the
    /// island is taken and the name is written after — and empty just the same
    /// for one whose stones this player has not been near enough to read. The
    /// two are deliberately one thing on the wire and one thing here: from
    /// offshore they are the same sight.
    pub name: String,
    /// Whether the player holds this one.
    pub yours: bool,
}

/// An island the survey has closed, carried onto the sheet to be lettered.
///
/// Two things separate it from the [`protocol::survey::Island`] it is built
/// from, and both are the client's own. Its centre is where the name goes on
/// the *paper* rather than where the island stands in the world — see
/// [`on_the_sheet`] — and its name is what the player has written on it.
#[derive(Clone, Debug, PartialEq)]
pub struct Island {
    /// What names the island for as long as the chart lives — see
    /// [`protocol::survey::Island::id`].
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
        self.survey.surveyed(chunk)
    }

    /// How many chunks have been surveyed — [`SurveyTally::surveyed`] on its
    /// own, for the caller that wants only that and wants it every frame.
    /// [`Chart::tally`] walks every coastline to fill the rest of its fields.
    pub fn surveys(&self) -> usize {
        self.survey.chunks()
    }

    /// Counts what the chart holds — the survey's own tally, the sheet having
    /// nothing to add to it.
    pub fn tally(&self) -> SurveyTally {
        self.survey.tally()
    }

    /// The islands the chart has closed, each measured for its lettering.
    ///
    /// Asked afresh every time and deliberately not kept. The walk behind it is
    /// bounded by the whole voyage rather than by the window on the paper,
    /// which looks like exactly what a chart of a long sail should not do per
    /// frame — and it was measured: tens of microseconds for a well-sailed
    /// world, see [`protocol::survey::Survey::islands`], against a redraw that
    /// rebuilds every stroke on the sheet. A cached list would be a rule about
    /// when to throw it away, and that rule could be wrong.
    pub fn islands(&self) -> Vec<Island> {
        self.survey
            .islands()
            .into_iter()
            .map(|island| Island {
                id: island.id,
                // The survey measures in the world and the lettering goes on
                // the paper, so the centre makes the same crossing every other
                // point on the sheet does.
                centre: on_the_sheet(island.centre),
                // The reach needs no crossing: the longer side of a box is the
                // longer side of it whichever way up the box is drawn.
                extent: island.extent,
                name: self.name(island.id).map(str::to_string),
            })
            .collect()
    }

    /// Takes down what the world says about a claimed island — see
    /// [`crate::net::receive`], which is the only caller. A cairn is told
    /// again whenever its word changes, so this overwrites wholesale.
    pub(crate) fn claimed(&mut self, island: IVec2, at: Vec2, name: &str, yours: bool) {
        self.claims.insert(
            island,
            Claimed {
                at,
                name: name.trim().to_string(),
                yours,
            },
        );
    }

    /// The claim on an island, if this sheet has been told of one.
    pub fn claim(&self, island: IVec2) -> Option<&Claimed> {
        self.claims.get(&island)
    }

    /// Every cairn this sheet has been told of, to be drawn — see
    /// [`cairn_mark`].
    ///
    /// Not windowed the way the coast is: a cairn is one mark where a chunk of
    /// coast is a run of line, and a player has at most as many of these as
    /// there are claimed islands they have been near.
    pub(crate) fn cairns(&self) -> impl Iterator<Item = (IVec2, &Claimed)> {
        self.claims.iter().map(|(island, claim)| (*island, claim))
    }

    /// The island a world point stands on, if this sheet has closed one round
    /// it — the survey's own question, asked in world metres rather than on
    /// the paper. What the claim key asks before it asks the world; see
    /// [`crate::player::claim_the_island`].
    pub fn island_under(&self, at: Vec2) -> Option<IVec2> {
        self.survey.island_under(at).map(|island| island.id)
    }

    /// What an island is called, if it is claimed and named.
    pub fn name(&self, island: IVec2) -> Option<&str> {
        self.claims
            .get(&island)
            .map(|claimed| claimed.name.as_str())
            .filter(|name| !name.is_empty())
    }

    /// Everything surveyed within a rectangle of the world, chunk by chunk.
    ///
    /// The rectangle is what keeps drawing bounded. A chart of a long voyage
    /// holds far more coastline than a sheet can show, so the mesh is built
    /// from a window on it rather than from everything ever seen — and the cost
    /// of drawing follows how much paper there is rather than how far the
    /// player has sailed.
    fn within(&self, window: Rect) -> impl Iterator<Item = (IVec2, &Soundings)> {
        self.survey.within(window.min, window.max)
    }

    /// Records what the world says one chunk holds — see
    /// [`crate::net::receive`], which is the only caller and the only way ink
    /// reaches this sheet.
    pub(crate) fn record(&mut self, chunk: IVec2, found: Soundings) {
        self.survey.record(chunk, found);
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

/// The cairn symbol, in pixels of paper — for the reason the line weights are
/// in pixels: what a chart draws a beacon at is the engraver's business and not
/// the world's, so it holds its size however far the sheet is zoomed.
///
/// The silhouette of the thing itself — a pillar of stacked stone, tapering as
/// it rises — rather than the plain triangle a chart draws a beacon with: a
/// player who has walked up to one on a headland knows this without being told
/// what it means.
///
/// Drawn from the spot upward, so the *foot* of it is where the cairn stands.
/// Centred on its position it would put the stones half a pillar north of where
/// they are, which on a chart is a lie about a landmark.
///
/// The proportions are the standing thing's own, exaggerated as an engraver's
/// are: taller against its width than the stone is, because at this size a
/// true-proportioned pillar is a squat blob and it is the *taper* that has to
/// survive being three pixels wide. Courses are not drawn — they would close
/// into a smudge, where the mark's one job is being told from a rock, a dot and
/// a letter at a glance.
///
/// Drawn hollow, which took three tries to find. Filled, it is the only solid
/// shape on a sheet that is otherwise all line, so it reads as a marker dropped
/// on a map rather than as something engraved on it — at this size an ink blot
/// with a slightly wonky edge.
const CAIRN_PILLAR: Vec2 = Vec2::new(9.0, 13.0);
/// How wide the crown is against the foot, as a fraction — the taper, which is
/// the one thing the mark has to carry. The stone's own is about half, and half
/// on paper reads as a wedge; this is it pulled back to where it still says
/// *narrower at the top* without saying *tent*.
const CAIRN_TAPER: f32 = 0.62;
const CAIRN_WEIGHT: f32 = 1.4;

/// How far a cairn's own lettering sits off its mark, in pixels — clear of the
/// crown rather than under it, so a name is never read through the stone.
const CAIRN_NAME_GAP: f32 = 7.0;

/// The hand the islands are named in: an italic cut of the Fell types, the
/// nearest a flat sheet comes to the lettering on an engraved chart. Not the
/// menus' serif, which is the machine's own hand and does the furniture. Where
/// the file came from is in `assets/CREDITS.md`.
const NAME_FONT: &str = "fonts/IMFellEnglish-Italic.ttf";

/// How large the names are lettered, in pixels — pixels for the reason the
/// line weights are: lettering belongs to the engraver, not to the world, so
/// it holds its size on the paper however far the sheet is zoomed.
const NAME_SIZE: f32 = 17.0;

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
/// A whole number of squares rather than a spacing of its own, so the roses keep
/// step with the ruling however the zoom moves it — nudged off the crossings
/// (see [`rose_nudge`]) by a fraction of a square, so the two lattices are
/// still one lattice.
///
/// Six, which keeps two or three roses in a window at every zoom. A net thrown
/// from one rose is a sunburst and reads as decoration; it is rays from
/// *different* roses crossing that make a sheet look navigated, and many more
/// than three make the paper a cobweb.
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
/// A neatline is the edge of the drawing, and a chart drawn to the window's own
/// edge has none. The band is what the engraving would carry a scale in; here
/// it carries the ruling's own graduation, so the edge measures the same thing
/// the paper is ruled by.
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
            // Normally `UiPlugin`'s, initialised the same way for the tests.
            .init_resource::<UiScale>()
            .add_systems(OnEnter(AppState::InWorld), start_a_chart)
            .add_systems(OnExit(AppState::InWorld), stow_the_chart)
            .add_systems(OnExit(Helm::Chart), roll_up)
            .add_systems(Update, chart_key.run_if(in_state(AppState::InWorld)))
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

/// A world gets a blank sheet, and takes it with it when it goes: what has
/// been seen is a fact about *this* world, and carrying it into the next would
/// draw one seed's islands on another's water.
///
/// Blank even in a world this machine has been in before, and nothing is lost
/// by it: everything that was on the sheet is the world's to hand back, and
/// arrives over the wire moments later. A name kept on this side would be a
/// name only this player could read.
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
/// Only from the helm and only back to it: the pause menu, the screens under
/// it and the console each have the keyboard for their own reasons while they
/// are up, and a chart key typed into any of them means what that screen says
/// it means.
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
        Helm::Paused | Helm::Options | Helm::Display | Helm::Controls | Helm::Console => {}
    }
}

/// Whether the sheet still has to be laid out.
fn no_sheet_yet(sheets: Query<(), With<ChartSheet>>) -> bool {
    sheets.is_empty()
}

/// Lays the sheet over the world.
///
/// The sheet's camera draws after the world's and clears to parchment, which
/// covers the view and every instrument over it in one stroke, so nothing in
/// the world has to be told the chart is up. The world's camera is switched off
/// behind it, a scene drawn to be painted over being a scene drawn for nobody.
///
/// Run on the first frame the chart is up rather than as the state is entered,
/// because it needs the world's camera to already exist: a run started with
/// `--state chart` enters the state before `Startup` has spawned anything.
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
    // A name still being written is left unwritten: every way off the chart
    // with the pen down means the player's attention went elsewhere, and half a
    // name committed by a distraction is worse than the draft lost. The
    // engraving goes too, so the next opening draws one for wherever the reader
    // has got to.
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
/// `--state chart` does — would otherwise open on the world's origin and stay
/// there, [`unroll`]'s centring having had its one chance.
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

    // The cairns, over the coast they stand on. Every claim this sheet has been
    // told of, which is every one its player has come near enough to see — see
    // [`Claimed`], and the server, which is what decides that.
    let mut cairns = Strokes::default();
    for (_, claimed) in chart.cairns() {
        let spot = on_the_sheet(claimed.at);
        if on_paper.contains(spot) {
            cairn_mark(&mut cairns, spot, metres_per_pixel);
        }
    }
    if let Some(mesh) = cairns.mesh() {
        let mesh = engraver.meshes.add(mesh);
        let ink = engraver.inks.coast.clone();
        engraver
            .commands
            .spawn(engraving("Chart cairns", mesh, ink, 1.2));
    }

    // The names, over the ink — see [`lettering`], which is where the whole of
    // what goes on the paper is decided. Only what lands on this window's worth
    // of it is spawned.
    for (text, at) in lettering(&chart, naming.as_deref(), metres_per_pixel) {
        if on_paper.contains(at) {
            letter(&mut engraver, text, at, metres_per_pixel);
        }
    }

    engraver.commands.insert_resource(Engraved {
        covered,
        metres_per_pixel,
    });
}

/// Every name that goes on the paper and where on it, in the sheet's own
/// coordinates.
///
/// Two kinds of lettering, and which one a name gets is the whole of what this
/// decides. An island **this survey has closed** is lettered across its own
/// middle, the way a chart letters an island. An island that is only a cairn is
/// lettered against the mark instead, that being the only thing on the paper
/// the name is true of — writing it across a middle the sheet has not drawn
/// would be lettering a shape nobody has seen.
///
/// A closed island is skipped in the second pass, or its name would be on the
/// sheet twice. The island under the pen shows the draft with its caret, the
/// lettering *being* the text field.
///
/// The walk is over the whole chart rather than a window of it — a chain can
/// cross any number of chunks — and the caller drops whatever falls off the
/// paper.
fn lettering(chart: &Chart, naming: Option<&Naming>, metres_per_pixel: f32) -> Vec<(String, Vec2)> {
    let islands = chart.islands();
    let mut written: Vec<(String, Vec2)> = islands
        .iter()
        .map(|island| {
            let text = match naming {
                Some(naming) if naming.island == island.id => format!("{}|", naming.draft),
                _ => island
                    .name
                    .clone()
                    .unwrap_or_else(|| "Unnamed island".to_string()),
            };
            (text, island.centre)
        })
        .collect();
    let closed: HashSet<IVec2> = islands.iter().map(|island| island.id).collect();

    // Half a line on top of the gap, `Text2d` hanging its lettering off the
    // middle of the line where the gap is measured to the foot of it. Without
    // it the name sits half a line lower than the constant says and its
    // descenders come down over the stone.
    let above = (CAIRN_PILLAR.y + CAIRN_NAME_GAP + NAME_SIZE / 2.0) * metres_per_pixel;
    written.extend(
        chart
            .cairns()
            .filter(|(island, claimed)| !claimed.name.is_empty() && !closed.contains(island))
            .map(|(_, claimed)| {
                (
                    claimed.name.clone(),
                    on_the_sheet(claimed.at) + Vec2::new(0.0, above),
                )
            }),
    );
    written
}

/// One name written on the paper, in the sheet's own hand.
///
/// Its own entity per name rather than one text mesh, because that is what
/// `Text2d` is: the alternative is laying out glyphs here, which is a font
/// engine and not a chart.
fn letter(engraver: &mut Engraver, text: String, at: Vec2, metres_per_pixel: f32) {
    engraver.commands.spawn((
        Name::new("Chart lettering"),
        Engraving,
        ChartSheet,
        Text2d::new(text),
        TextFont {
            font: FontSource::Handle(engraver.lettering.0.clone()),
            font_size: FontSize::Px(NAME_SIZE),
            ..default()
        },
        TextColor(INK),
        // Between the coast and the reader's own mark, scaled like the line
        // weights so the name keeps its size on the paper.
        Transform::from_translation(at.extend(1.5)).with_scale(Vec3::splat(metres_per_pixel)),
        DespawnOnExit(Helm::Chart),
    ));
}

/// A cairn on the paper: a pillar of stone, tapering as it rises.
///
/// `at` is the spot itself and the mark is built up from it — see
/// [`CAIRN_PILLAR`] for why it stands on its position rather than being centred
/// on it. `scale` is metres to the pixel, the mark being measured in pixels of
/// paper and drawn in metres of world.
///
/// One closed run of four points. The mark is deliberately the simplest shape
/// on the sheet that is still unmistakably *made*: nothing else drawn here has
/// a straight edge that is not a coast, and nothing else is symmetrical about a
/// vertical.
fn cairn_mark(out: &mut Strokes, at: Vec2, scale: f32) {
    let paper = |pixels: f32| pixels * scale;
    let weight = paper(CAIRN_WEIGHT);
    let foot = paper(CAIRN_PILLAR.x) / 2.0;
    let crown = foot * CAIRN_TAPER;
    let up = Vec2::new(0.0, paper(CAIRN_PILLAR.y));

    out.run(
        &[
            at - Vec2::new(foot, 0.0),
            at + Vec2::new(foot, 0.0),
            at + up + Vec2::new(crown, 0.0),
            at + up - Vec2::new(crown, 0.0),
        ],
        true,
        weight,
    );
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
/// and the coast does not. Scaled by the zoom so it stays the same size on the
/// paper.
///
/// A `ParamSet` because both halves are transforms and Bevy will not let one
/// system hold `&Transform` and `&mut Transform` at once: read where the reader
/// is, then write where their mark goes.
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

/// The corners of the reader's mark — the notched arrowhead the player is
/// drawn as, here and at the middle of the compass card: the point, the two
/// tail corners and the notch between them, wound anticlockwise from the
/// point, in the mark's own units about the origin it turns on. One glyph
/// meaning *you* on every instrument, so neither sheet has to be learned
/// twice.
pub(crate) const READERS_MARK: [Vec2; 4] = [
    Vec2::new(0.0, 7.0),
    Vec2::new(-4.5, -5.0),
    Vec2::new(0.0, -2.0),
    Vec2::new(4.5, -5.0),
];

/// The reader's own mark: a plain arrowhead, in pixels of paper.
///
/// A drawn ship would be a picture of a ship eight pixels across. What the mark
/// has to say is *here*, and *this way*, and an arrowhead says both without
/// pretending to be anything else.
fn readers_mark() -> Mesh {
    let [point, left, notch, right] = READERS_MARK;
    let mut strokes = Strokes::default();
    strokes.triangle(point, left, notch);
    strokes.triangle(point, notch, right);
    strokes.mesh().expect("the mark is never empty")
}

// ---------------------------------------------------------------------------
// Naming
// ---------------------------------------------------------------------------

/// A name being written on the sheet: the island under the pen, and the
/// letters so far.
///
/// While this exists the keyboard is the pen's — the chart key and the driving
/// keys spell letters, and Escape puts the pen down — so [`chart_key`] and
/// [`pan`] stand down while it does. There is no dialog: the island's own
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
/// back, and Enter offers the draft to the world — see [`write_through`],
/// which is where it goes and what becomes of it.
///
/// Emptying the field and pressing Enter is *not* an erasure: the world refuses
/// a name it cannot carry rather than washing one off, so the cairn goes on
/// saying what it said. There is no way to unname an island, which is worth
/// saying plainly rather than leaving the empty field looking like one.
///
/// Asks what the keyboard *typed* rather than which positions were pressed, so
/// a name can hold whatever a layout can produce.
fn write_the_name(
    mut commands: Commands,
    online: Option<Res<crate::net::Online>>,
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
                write_through(&online, naming.island, &naming.draft);
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
                    if naming.draft.chars().count() < protocol::NAME_LETTERS {
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
    chart: Res<Chart>,
    online: Option<Res<crate::net::Online>>,
    view: Res<ChartView>,
    naming: Option<Res<Naming>>,
    mut pointer: Pointer,
) {
    let Some(at) = pointer.clicked() else {
        return;
    };

    let hit = hit_island(&chart.islands(), at, view.metres_per_pixel);
    if let Some(naming) = naming {
        write_through(&online, naming.island, &naming.draft);
        commands.remove_resource::<Naming>();
        // Clicking the island being written is only ever finishing it.
        if hit.as_ref().map(|island| island.id) == Some(naming.island) {
            return;
        }
    }
    // The pen only opens on an island this player holds: naming is what a
    // claim earns, so there is nothing to write on an island nobody has
    // claimed and nothing this player may write on somebody else's. The sheet
    // says so by not taking the pen up.
    if let Some(island) = hit.filter(|island| ours(&chart, island.id)) {
        commands.insert_resource(Naming {
            island: island.id,
            draft: island.name.unwrap_or_default(),
        });
    }
}

/// Whether this island is one the player holds — see [`click_to_name`].
fn ours(chart: &Chart, island: IVec2) -> bool {
    chart.claim(island).is_some_and(|claimed| claimed.yours)
}

/// Sends a christening to the world, which is the only place a name lives.
///
/// Nothing is written on the sheet here. The name goes up, the server judges
/// it, and the cairn is told back with whatever it now says — so the lettering
/// a player sees is always the lettering everybody else sees.
fn write_through(online: &Option<Res<crate::net::Online>>, island: IVec2, draft: &str) {
    if let Some(online) = online {
        online.connection.christen(island, draft);
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
/// stretch of bank is stippled the same way on every rebuild — a stipple that
/// reshuffled on a pan would read as the paper crawling.
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
/// `covered` arrives in world metres and the flip onto the sheet happens
/// *here* — the one drawing whose points do not each pass through
/// [`on_the_sheet`], being generated rather than surveyed. Ruled in world
/// coordinates directly it was mirrored about the equator: indistinguishable
/// near the origin, and gone from the top of the paper north of it.
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
/// That split is the whole trick. An engraved rose looks lit from one side,
/// which on real paper is done with hatching — which this sheet cannot have.
/// Two flat tones meeting on each point's axis gives the same reading with
/// nothing shaded: every point turns its dark half the same way round the card.
///
/// Two sinks rather than one because a tone is a material and a material is a
/// mesh, so the sheet costs two meshes however many roses stand on it.
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
/// A chart drawn from an exact lattice does not look like a chart: on a perfect
/// grid every rose's diagonal runs into its neighbour's and its east–west line
/// into the ruling, so thirty-two bearings from a dozen roses collapse into one
/// X repeated across the paper.
///
/// The nudge is a hash of the square rather than a random number, which is the
/// whole reason it can exist: the sheet is rebuilt every time the view leaves
/// its window, and a rose that stood somewhere else after a pan would be worse
/// than a lattice. This way a rose belongs to its patch of sea.
fn rose_nudge(square: IVec2) -> Vec2 {
    let hash =
        scramble((square.x as u32).wrapping_add((square.y as u32).wrapping_mul(0x85EB_CA6B)));
    let spread = |bits: u32| (bits as f32 / u16::MAX as f32 - 0.5) * 2.0 * RHUMB_NUDGE;
    Vec2::new(spread(hash >> 16), spread(hash & 0xFFFF))
}

/// The bearings one rose throws across the paper.
///
/// Thirty-two of them, and not all alike: the eight principal winds ruled at
/// full weight, the eight half winds lighter, the sixteen quarter winds dashed.
/// Thirty-two identical rays from a dozen roses is a grey haze, while a net
/// that gets fainter as it gets finer can be followed by eye from any rose to
/// any other.
///
/// Each ray is clipped to the paper being drawn and starts clear of the rose's
/// own star, so what gets built is bounded by the sheet rather than the world.
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
/// edge cut the ray, so neighbouring bearings break at the same distances out
/// and the dashes fall into rings. Counted the other way, sixteen rays of
/// unrelated dashes read as dirt on the sheet.
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
/// distances*. What asks for it is a screen made of the sheet without being a
/// chart of anywhere, where a pixel is a pixel and there is no world under the
/// paper. Lent out rather than copied because there is one sheet in this game,
/// and two would drift.
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
///
/// Spawned bare and sized in [`rule_the_scale`], every part of it. The
/// furniture stands on an engraving drawn by a camera that takes no notice of
/// [`UiScale`], so it is sized in the sheet's own pixels and the UI's scale is
/// divided back out of every measurement — most of all the bar's width, which
/// is read against the paper under it and would otherwise be off by exactly
/// the scale. Divided in one place rather than here as well, because a window
/// rescaled with the chart already open moves the scale under furniture that
/// has been laid: a bar of the right length in a frame of the wrong size is
/// worse than either, and nothing decides which of the two a run is looking at
/// if the answer depends on which system ran first.
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
            // The scale bar. Where it stands, how long and how deep it is
            // drawn and what its label is lettered at are all set the first
            // frame it is up — see [`rule_the_scale`] — its length because it
            // is the one piece of furniture that has to change with the zoom,
            // that being the whole of what it is for, and the rest because
            // they change with the UI's scale.
            sheet.spawn((
                ScaleBar,
                Node {
                    position_type: PositionType::Absolute,
                    flex_direction: FlexDirection::Column,
                    ..default()
                },
                children![
                    (ScaleRule, Node::default(), BorderColor::all(INK)),
                    (
                        ScaleLabel,
                        Text::new(""),
                        // The serif the menus and the compass resolve, for
                        // the reason they do: this is chart furniture, and
                        // the machine's serif is the hand charts are
                        // lettered in.
                        TextFont {
                            font: FontSource::Serif,
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
/// Flat, unlike the world's compass, which lies foreshortened on the sea because
/// a bearing read off it is carried out into the picture. Nothing on a chart is
/// foreshortened and nothing on this one turns: it says north is up and stays
/// up, so it is drawn once at the size it will always be.
///
/// Engraved rather than built from boxes: a kite point is a triangle and the UI
/// layer has only rectangles, so a star drawn there would be a spike rose.
/// Being a mesh it lives in the sheet's space rather than the window's, and
/// [`pin_the_rose`] does the job the UI layer would have done.
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
/// Furniture, pinned like the scale bar — but drawn in the sheet's space rather
/// than the window's (see [`corner_rose`]), so where the window's corner has
/// got to has to be worked out. The sheet's middle is the view's centre and a
/// pixel is [`ChartView::metres_per_pixel`] metres of paper; the scale on the
/// transform holds the rose at its drawn size through a zoom.
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
/// Both are one entity apiece with their mesh written over in place, this being
/// the one drawing on the sheet that changes every time the view moves at all:
/// the engraving is built for a window and panned inside it, but the edge is
/// *at* the window. Writing in place is what avoids leaking a mesh asset per
/// frame of a drag.
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
/// out past the neatline and the sheet would have only a line drawn across it.
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
    // what makes the sheet's *next* opening draw its edge, the meshes being
    // new and blank each time the chart is unrolled.
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

/// The corner the scale stands in, the bar of it, and its label.
#[derive(Component)]
struct ScaleBar;
#[derive(Component)]
struct ScaleRule;
#[derive(Component)]
struct ScaleLabel;

/// Sets the scale bar to a round distance, says which, and lays out the corner
/// it stands in.
///
/// The bar is drawn at whatever length that distance comes to rather than at a
/// fixed length labelled with an awkward number, because a scale bar is a thing
/// to lay a finger against: "two kilometres is this far" reads, and "this far
/// is 1.83 km" does not.
///
/// Every size here is in the sheet's own pixels with the UI's scale divided
/// back out of it — see [`furniture`] — and all of them are written together
/// on purpose. The bar has to follow the scale, being a measurement; so the
/// frame around it has to follow the scale too, or a window resized with the
/// chart open leaves a bar of the right length standing in a margin, above a
/// label and inside a border that are all of the size the UI was when the
/// sheet was unrolled.
fn rule_the_scale(
    view: Res<ChartView>,
    ui: Res<UiScale>,
    mut bars: Query<&mut Node, (With<ScaleBar>, Without<ScaleRule>)>,
    mut rules: Query<&mut Node, (With<ScaleRule>, Without<ScaleBar>)>,
    mut labels: Query<(&mut Text, &mut TextFont), With<ScaleLabel>>,
) {
    if !view.is_changed() && !ui.is_changed() {
        return;
    }
    let scale = ui.0;
    let metres = round_distance(SCALE_BAR_LEAST * view.metres_per_pixel);
    for mut node in &mut bars {
        node.left = Val::Px(FURNITURE_MARGIN / scale);
        node.bottom = Val::Px(FURNITURE_MARGIN / scale);
        node.row_gap = Val::Px(4.0 / scale);
    }
    for mut node in &mut rules {
        node.width = Val::Px(metres / view.metres_per_pixel / scale);
        node.height = Val::Px(5.0 / scale);
        node.border = UiRect::all(Val::Px(1.0 / scale));
    }
    for (mut text, mut font) in &mut labels {
        text.0 = distance_label(metres);
        font.font_size = FontSize::Px(13.0 / scale);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{run_frames, FRAME};
    use bevy::state::app::StatesPlugin;
    use bevy::time::{TimePlugin, TimeUpdateStrategy};
    use protocol::ground::{CHUNK_METRES, FACET_METRES, FACET_VERTS};
    use protocol::survey::{survey, SIGHT_RADIUS};
    // How near a point drawn from a survey can be asked to land: the survey's
    // own two roundings, and no promise finer than them.
    use protocol::survey::{MARK_STEP, TOLERANCE};

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
    fn the_sight_radius_stays_inside_what_the_client_holds() {
        // The reach the survey works to has to sit inside what streaming
        // brings in. Two things here read the survey against the ground in
        // hand — the compass rim and the haze — and were this the longer of
        // the two, both would be about ground the client had never been sent.
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
        let runs = survey(&a_cone(IVec2::ZERO, Vec2::splat(CHUNK_METRES / 2.0), 40.0));
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
    fn a_claim_letters_the_island_it_was_written_on() {
        // A name reaches the sheet by riding a cairn — see [`Chart::claimed`],
        // which is what the world's word lands in. Written against the ring's
        // id, read back off the island the walk finds.
        let middle = Vec2::new(CHUNK_METRES, CHUNK_METRES / 2.0);
        let mut chart = Chart::default();
        chart.record(IVec2::ZERO, survey(&a_cone(IVec2::ZERO, middle, 60.0)));
        chart.record(
            IVec2::new(1, 0),
            survey(&a_cone(IVec2::new(1, 0), middle, 60.0)),
        );

        let id = chart.islands()[0].id;
        // Unclaimed, it is drawn and unlettered however well it is known.
        assert_eq!(chart.islands()[0].name, None);

        chart.claimed(id, middle, "  Isla Genovesa  ", true);
        assert_eq!(chart.islands()[0].name.as_deref(), Some("Isla Genovesa"));
        assert_eq!(chart.name(id), Some("Isla Genovesa"));
        assert!(chart.claim(id).is_some_and(|claimed| claimed.yours));

        // A claim with nothing written on it yet is a cairn without a name:
        // the island is spoken for, and the paper says so by the mark rather
        // than by lettering.
        chart.claimed(id, middle, "   ", true);
        assert_eq!(chart.islands()[0].name, None);
        assert!(chart.claim(id).is_some(), "the cairn went with the name");
    }

    #[test]
    fn a_name_read_off_a_cairn_is_lettered_against_it() {
        // The second tier of knowing, on the paper. A player who has landed on
        // somebody's island and read the stones knows what it is called before
        // they have been round it — and there is no shape on this sheet to
        // write that across, so it goes beside the mark it was read off.
        let mut chart = Chart::default();
        let stones = Vec2::new(4_000.0, -2_500.0);
        chart.claimed(IVec2::new(9, -3), stones, "Ilha Verde", false);

        let written = lettering(&chart, None, 1.0);
        assert_eq!(written.len(), 1, "one cairn, one name");
        assert_eq!(written[0].0, "Ilha Verde");
        let (beside, spot) = (written[0].1, on_the_sheet(stones));
        assert_eq!(beside.x, spot.x, "the name wandered off its own mark");
        assert!(
            beside.y > spot.y,
            "the name was written through the cairn rather than above it"
        );

        // And a cairn with nothing legible on it letters nothing at all —
        // which is an island nobody has christened, or one this player has not
        // been up to. From here those are the same sight.
        chart.claimed(IVec2::new(9, -3), stones, "", false);
        assert!(lettering(&chart, None, 1.0).is_empty());
    }

    #[test]
    fn an_island_sailed_round_is_lettered_once_and_across_itself() {
        // The other half of that rule. Once a coast has been closed there is a
        // shape to write on, so the name goes where a chart puts an island's
        // name — and it must not also be written beside the cairn, or a player
        // who sailed round an island they had already visited would watch its
        // name come out twice.
        let middle = Vec2::new(CHUNK_METRES, CHUNK_METRES / 2.0);
        let mut chart = Chart::default();
        chart.record(IVec2::ZERO, survey(&a_cone(IVec2::ZERO, middle, 60.0)));
        chart.record(
            IVec2::new(1, 0),
            survey(&a_cone(IVec2::new(1, 0), middle, 60.0)),
        );
        let id = chart.islands()[0].id;
        chart.claimed(id, middle + Vec2::new(40.0, 0.0), "Ilha Verde", false);

        let written = lettering(&chart, None, 1.0);
        assert_eq!(
            written.len(),
            1,
            "an island both sailed round and visited was lettered twice"
        );
        assert_eq!(written[0].0, "Ilha Verde");
        assert_eq!(
            written[0].1,
            chart.islands()[0].centre,
            "the name was not written across the island"
        );
    }

    #[test]
    fn a_cairn_is_drawn_standing_on_its_own_spot() {
        // A landmark's symbol says where the landmark is, and a mark centred on
        // its position would put the stones half a heap north of where they
        // stand. So it is built upward from the spot, and its foot is the
        // cairn.
        //
        // "Its foot" to within half a stroke, the mark being drawn in outline
        // and a stroke sitting astride the line it follows — so the underside
        // of the base rule falls that far below the spot, as the seaward side
        // of a coastline falls outside the coast. That is the width of the pen
        // and not a claim about where anything is; what this is watching for is
        // a whole heap's worth of drift.
        let spot = Vec2::new(120.0, -45.0);
        let nib = CAIRN_WEIGHT / 2.0;
        let mut strokes = Strokes::default();
        cairn_mark(&mut strokes, spot, 1.0);

        let drawn: Vec<Vec2> = strokes
            .positions
            .iter()
            .map(|point| Vec2::new(point[0], point[1]))
            .collect();
        assert!(!drawn.is_empty(), "the mark drew nothing");
        assert!(
            drawn.iter().all(|point| point.y >= spot.y - nib),
            "the mark hangs below the spot it stands on"
        );
        assert!(
            drawn.iter().any(|point| point.y <= spot.y + nib),
            "the mark floats above the spot it stands on"
        );
        // And it holds its size on the paper: twice the metres to the pixel is
        // twice the mark in metres, which is the same mark on the sheet.
        let mut zoomed = Strokes::default();
        cairn_mark(&mut zoomed, spot, 2.0);
        let tallest = |strokes: &Strokes| {
            strokes
                .positions
                .iter()
                .map(|point| point[1] - spot.y)
                .fold(f32::MIN, f32::max)
        };
        assert!((tallest(&zoomed) - tallest(&strokes) * 2.0).abs() < TOLERANCE);
    }

    #[test]
    fn the_pen_only_opens_on_an_island_this_player_holds() {
        // The filter a hit goes through in [`click_to_name`], asked of the
        // three things an island can be. Naming is what a claim earns: there
        // is nothing to write on an island nobody has taken, and nothing this
        // player may write on somebody else's. The sheet says so by simply not
        // taking the pen up.
        let middle = Vec2::new(CHUNK_METRES, CHUNK_METRES / 2.0);
        let mut chart = Chart::default();
        chart.record(IVec2::ZERO, survey(&a_cone(IVec2::ZERO, middle, 60.0)));
        chart.record(
            IVec2::new(1, 0),
            survey(&a_cone(IVec2::new(1, 0), middle, 60.0)),
        );
        let id = chart.islands()[0].id;

        assert!(
            !ours(&chart, id),
            "the pen opened on an island nobody has claimed"
        );

        chart.claimed(id, middle, "Isla Ajena", false);
        assert!(!ours(&chart, id), "the pen opened on a stranger's island");

        chart.claimed(id, middle, "Isla Genovesa", true);
        assert!(ours(&chart, id), "the pen would not open on our own island");
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
    fn a_typed_name_is_taken_up_and_enter_puts_the_pen_down() {
        // What Enter does now is send: a name belongs to the claim it is
        // written on, so the world is asked and the sheet waits to be told —
        // see [`write_through`]. Nothing is lettered here, and with no session
        // to send down there is nowhere for the name to go, which is exactly
        // what this app is. What can still be watched is the pen: the letters
        // gather in the draft, and Enter puts it down.
        let mut app = keyed_app();
        press(&mut app, KeyCode::KeyM);
        assert_eq!(helm(&app), Helm::Chart);

        let island = IVec2::new(40, -17);
        app.insert_resource(Naming {
            island,
            draft: String::new(),
        });
        type_word(&mut app, "Skull Rock");
        assert_eq!(
            app.world().resource::<Naming>().draft,
            "Skull Rock",
            "the letters did not reach the draft"
        );

        type_key(&mut app, KeyCode::Enter, "\r");
        assert!(
            app.world().get_resource::<Naming>().is_none(),
            "the pen is still down"
        );
        assert_eq!(
            app.world().resource::<Chart>().name(island),
            None,
            "the sheet lettered a name the world had not confirmed"
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
        type_word(&mut app, &"a".repeat(protocol::NAME_LETTERS + 9));
        type_key(&mut app, KeyCode::Backspace, "\u{8}");

        // Read off the draft rather than the sheet: what is typed is this
        // side's business, and what is *written* is the world's — see
        // [`a_typed_name_is_taken_up_and_enter_puts_the_pen_down`].
        let drafted = &app.world().resource::<Naming>().draft;
        assert_eq!(drafted.chars().count(), protocol::NAME_LETTERS - 1);
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

    /// The bar as it stands on the sheet: how wide it is drawn, in the pixels
    /// a UI node is written in, and what its label says.
    fn scale_bar(app: &mut App) -> (f32, String) {
        let width = app
            .world_mut()
            .query_filtered::<&Node, With<ScaleRule>>()
            .single(app.world())
            .expect("an open chart has a scale bar")
            .width;
        let Val::Px(width) = width else {
            panic!("the bar is not drawn in pixels");
        };
        let said = app
            .world_mut()
            .query_filtered::<&Text, With<ScaleLabel>>()
            .single(app.world())
            .expect("the bar is labelled")
            .0
            .clone();
        (width, said)
    }

    /// What a label says, in metres.
    fn label_metres(said: &str) -> f32 {
        let (number, unit) = said.split_once(' ').expect("a distance and its unit");
        let number: f32 = number.parse().expect("a number");
        match unit {
            "km" => number * 1000.0,
            "m" => number,
            _ => panic!("{said} is not a distance"),
        }
    }

    #[test]
    fn the_scale_bar_is_as_long_as_it_says_at_any_ui_scale() {
        // The one real measurement on this sheet, and the one thing the UI's
        // scale can put wrong without looking wrong: the bar is a UI node and
        // the paper under it is not, so a bar drawn at its face value would
        // say "2 km" and be one kilometre long on a menu drawn at half size.
        // A scale bar that lies is worse than no scale bar.
        for scale in [0.5, 1.0, 2.0] {
            let mut app = keyed_app();
            press(&mut app, KeyCode::KeyM);
            app.world_mut().resource_mut::<UiScale>().0 = scale;
            let metres_per_pixel = 4.0;
            app.world_mut().resource_mut::<ChartView>().metres_per_pixel = metres_per_pixel;
            run_frames(&mut app, 2);

            let (width, said) = scale_bar(&mut app);
            // A UI pixel is `scale` of the sheet's own, and a sheet pixel is
            // `metres_per_pixel` metres of water.
            let reaches = width * scale * metres_per_pixel;
            assert!(
                (reaches - label_metres(&said)).abs() < 0.5,
                "a bar labelled {said} reaches {reaches} m at a UI scale of {scale}"
            );
        }
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
        for busy in [
            Helm::Paused,
            Helm::Options,
            Helm::Display,
            Helm::Controls,
            Helm::Console,
        ] {
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
    fn a_telling_of_the_survey_is_ink_on_the_sheet() {
        // The whole of what this module does about surveying now: the world
        // says what a chunk holds and the sheet has it — no rule of its own
        // about what has been seen, and nothing on the paper that was not
        // told. Whether the *right* chunks are told is the server's promise,
        // and its session tests are where that is asked.
        let mut chart = Chart::default();
        assert!(!chart.surveyed(IVec2::ZERO));

        let middle = Vec2::splat(CHUNK_METRES / 2.0);
        chart.record(IVec2::ZERO, survey(&a_cone(IVec2::ZERO, middle, 40.0)));
        // And a chunk the world called on and found blank, which is still
        // surveyed — it is the difference between water somebody has crossed
        // and water nobody has.
        chart.record(IVec2::new(1, 0), Soundings::default());

        assert!(chart.surveyed(IVec2::ZERO) && chart.surveyed(IVec2::new(1, 0)));
        assert_eq!(chart.surveys(), 2);
        let paper = Rect::from_corners(Vec2::ZERO, Vec2::splat(CHUNK_METRES));
        assert!(
            chart
                .within(paper)
                .any(|(_, found)| !found.coast.is_empty()),
            "an island was told and no coast came of it"
        );

        // Ground nobody has said anything about stays off the sheet.
        assert!(!chart.surveyed(IVec2::new(30, 30)));
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
