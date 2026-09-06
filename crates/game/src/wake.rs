//! The wake: the white water a hull leaves around and behind it.
//!
//! A wake is foam, and the sea already knows how to paint foam — so nothing
//! here draws anything. This module keeps a short history of where the boat's
//! bow has been and hands it to the sea's own shader, which lays the same white
//! it lays surf and whitecaps in. A ribbon of geometry towed behind the hull
//! would have to be told about the waves under it and agree with them exactly;
//! painted by the surface itself, the wake is *on* the water by construction.
//!
//! What the shader gets is a [`Track`]: a polyline of points the bow has
//! lately laid — a shoulder's radius abaft the stem; [`lay_the_wake`] says
//! why — each carrying how long ago it was laid and how fast the hull was
//! moving when it was. Two things are painted off it: the **boil**, solid
//! white close about the hull, and the **arms**, the V a moving hull throws.
//!
//! Age takes both back, and it takes them back by *breaking them up* rather
//! than by fading them: the foam is thresholded against the same kind of
//! value noise the whitecaps' bar wanders on, so an old wake goes to patches
//! and then to nothing. A smooth fade would be the one gradient on a sea made
//! entirely of flat tones, and would read as the water going misty.
//!
//! Laying the wake along the hull's *track* rather than astern of its heading
//! is what makes manoeuvring come out right for nothing: the wake is where
//! the boat has been, so it curves behind a boat coming about and lies ahead
//! of the bow of one going astern, neither of which is worked out anywhere.
//!
//! A track per hull, whoever is moving it: a boat in tow makes the same
//! white as the ship pulling it, and another player's hull throws the same
//! V as ours. The track comes with the hull — every [`Vessel`] carries one —
//! and the way it is laid at is read off the water plane the hull is solved
//! on, which every hull has, rather than off sailing state, which only ours
//! does. Its length, so a hull carried sideways stirs the water it is
//! actually crossing. The one exception is a hull the wire moves: its way is
//! the wire's word rather than the spring steering it onto that word, since
//! the spring peaks well above [`STIRS`] bringing a hull a hand's breadth
//! home, and foam at a boat that did not sail is worse than a wake a telling
//! late. The shader takes the nearest [`WAKES`] of them, ranked for the
//! [`Eye`] the holes are cut for.

use std::collections::VecDeque;

use bevy::math::Vec3Swizzles;
use bevy::prelude::*;

use avian2d::prelude::LinearVelocity;

use crate::boat::{Eye, Rigged, Shoving, Telling, Vessel};
use crate::sea::{DepthWindow, SeaMaterial};
use crate::AppState;

/// How many points of track the shader is handed.
///
/// This is the one number here with a price on it: every sea fragment inside
/// the track's box walks the whole polyline, so the loop is paid per pixel of
/// water near the boat. The box is what keeps that from being paid over the
/// whole ocean — see [`Track::packed`].
///
/// The twin of the array length in `assets/shaders/sea.wgsl`, which
/// `the_shader_walks_the_whole_track` holds it to.
pub(crate) const TRAIL: usize = 34;

/// How many hulls' tracks the shader is handed at once — the twin of the
/// array length in the shader, which `the_shader_walks_the_whole_track`
/// holds it to.
///
/// Four is the ship with her boat in tow and another player's pair beside
/// them; past that the ones furthest from the [`Eye`] go unpainted. The room
/// is not free the way the holes' is — see [`Track::packed`] for what a slot
/// costs — so a hull with nothing to show takes none: see [`Track::shows`].
pub(crate) const WAKES: usize = 4;

/// Metres of travel between one point of the track and the next.
///
/// A wake is only as faithful as the line it is drawn along, and the shape
/// that tests it is a hard turn: the ship comes about inside a five-metre
/// circle, so a step of three metres cuts that arc into chords of about
/// forty degrees. The wake through a full-helm turn is visibly a polygon
/// rather than a curve — which is what everything else in this world is, so
/// it costs nothing to look at, and buying the curve back would mean
/// doubling the loop above for a shape seen for a second at a time.
const STEP: f32 = 3.0;

/// Seconds a wake lasts, from laid to gone.
///
/// With [`STEP`] and [`LAID`] this is also a length: the track holds so many
/// steps and no more, so a wake this old is only whole while the hull is
/// making under about [`FASTEST`]. That is the way round it should be — the
/// life is what ends a wake and the length has slack in hand — and
/// `the_track_is_long_enough_to_hold_a_whole_wake` keeps it there. A hull
/// faster than any here would find its wake ending in metres instead, which
/// is a shorter wake, not a broken one.
const LIFE: f32 = 8.0;

/// The fastest anything here sails, in metres a second: the ship's speed with
/// the whole of the wind's bonus on it, rounded up.
///
/// A bound rather than a rule — nothing is held to it — and it does two jobs.
/// It is what [`LIFE`] and the track's length are sized against, and it is
/// what a hull's way is held under before the shader ever sees it, because
/// every shape here scales with speed and a wild one does not merely look
/// wrong: the arms open at a fixed angle *per metre run*, so a hull recorded
/// at a few thousand metres a second throws a band tens of kilometres wide,
/// which arrives on screen as hairlines of foam ruled clean across the ocean
/// and a bounding box the whole sea is inside of.
///
/// That is not hypothetical. The way used to be measured off the hull's own
/// travel — the distance its stem moved in a frame, over the length of the
/// frame — which is exact right up until a frame is a few microseconds long,
/// or the hull swings its stem by pitching rather than by sailing. Both
/// happen. The way is the plane's now, which cannot answer nonsense, and
/// this is still here because one bad number is enough.
const FASTEST: f32 = 12.0;

/// How wide the water a hull turns over is where it leaves the stem, as a
/// multiple of the half-beam. Barely wider than the hull: enough that a hand's
/// breadth of white shows either side of the planking and the boat reads as
/// moving, and no more, because everything past that is a white pool the hull
/// sits in — which reads as a hole in the sea rather than as water being
/// pushed aside, and is the first thing that makes a wake look like too much.
const SHOULDER: f32 = 1.05;

/// How far the arms open away from the track, in metres across per metre run
/// — a tangent, so this is the wake's half-angle: about fifteen degrees,
/// inside the nineteen and a half a real displacement hull throws. Kept
/// under it because the honest angle at this length of track puts the arms
/// far enough out to fill the view at the default zoom, and a wake that
/// wide stops reading as belonging to the boat.
const SPREAD: f32 = 0.26;

/// How thick an arm is, in metres. What makes the V a pair of lines rather
/// than a filled wedge — which is what a wake looks like from above, the water
/// between the arms being merely disturbed rather than white. Thin: an arm is
/// a crest falling over, not a stretch of foam, and the width that reads as a
/// wake from the deck reads as a pair of painted stripes from anywhere nearer.
const ARM: f32 = 0.5;

/// The boil: how fast it widens, in metres of half-width per second of age,
/// and the seconds it takes to break up entirely.
///
/// It widens with *age* and not with distance run, unlike the arms, because
/// it is not a wave — it is the water the hull has just displaced closing back
/// over itself, and that happens at its own pace whatever speed the hull left
/// at. So a boat under way trails a boil a hull or two long, and a boat barely
/// moving wears a collar of one.
///
/// Barely widening, and not for long. What carries a wake is the arms, and
/// they leave the stem close together — so a boil that goes on opening behind
/// the hull runs into them and the three become one slab of white a few
/// beams across, which is the whole of what makes a wake look like too much
/// of itself.
///
/// A life of its own rather than a share of [`LIFE`], and worn away on the
/// same field as the arms rather than simply stopped: given a hard end the
/// boil finishes on a ruled line drawn across the wake, which is the one shape
/// water never makes.
const BOIL: (f32, f32) = (0.12, 1.5);

/// The cell of the field an ageing wake breaks up on, in metres — see the
/// module doc on why it breaks up rather than fades. About a boat's beam,
/// which puts the holes at the size of the patches of foam they are meant to
/// be leaving behind.
const BREAKUP: f32 = 3.0;

/// The least way that leaves any mark at all, in metres a second. Under it
/// the water closes over without white — a hull ghosting along at a knot
/// does not foam — and above it the wake is simply there. A hard threshold
/// rather than a ramp because a hull crosses it in a fraction of a second and
/// the wake at that speed is a couple of metres of collar: what the threshold
/// really does is take the foam off a boat lying still, and the way the wake
/// then retreats down its own old track as the hull glides to a stop is the
/// track's doing, not a fade's.
const STIRS: f32 = 0.6;

/// One point of the track a hull has left behind: where its bow was, how
/// long ago that was, and how fast it was going at the time.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Mark {
    at: Vec2,
    age: f32,
    way: f32,
}

/// Where a hull has been lately: the bow where it is this frame, and the
/// points laid behind it, newest first. Every [`Vessel`] carries one.
///
/// The head is kept apart from them because it is not a laid point — it is
/// carried, moved to wherever the bow now is every single frame, and only
/// the points behind it stand still and age. Without it the white water would
/// start up to [`STEP`] behind a moving hull, appearing and disappearing at
/// the bow as each new point went down; with it the foam is welded to the
/// boat, and the track behind is still only as dense as it needs to be.
#[derive(Component, Default)]
pub struct Track {
    head: Option<Mark>,
    laid: VecDeque<Mark>,
}

/// How many points may be laid behind the head — the head taking a slot of
/// the [`TRAIL`] the shader is given.
const LAID: usize = TRAIL - 1;

/// The box a track with nothing in it answers — the least corner past the
/// greatest, so no water is inside it. What every slot the shader is given
/// holds until a hull with a wake takes it; see [`Track::packed`].
pub(crate) const NOWHERE: Vec4 = Vec4::new(1.0, 1.0, -1.0, -1.0);

impl Track {
    /// Ages the track and brings its head to `at` — wherever the hull is
    /// laying its white this frame — `way` being how fast the hull is going.
    fn follow(&mut self, at: Vec2, way: f32, dt: f32) {
        let way = way.abs().min(FASTEST);

        // A hull that has covered more ground than it could possibly have
        // sailed has been *put* there rather than got there — a world
        // entered, a `goto` answered — and the
        // track behind it is a place it has never been. Kept, it would be
        // joined to where the boat now is by one straight segment across
        // however much ocean lies between.
        let travelled = self.head.map_or(0.0, |head| head.at.distance(at));
        if travelled > FASTEST * dt + 1.0 {
            self.laid.clear();
            // And where it came from is not somewhere it sailed from either,
            // so it is no use as the point behind this one.
            self.head = None;
        }

        for mark in &mut self.laid {
            mark.age += dt;
        }
        while self.laid.back().is_some_and(|mark| mark.age > LIFE) {
            self.laid.pop_back();
        }

        // A point goes down where the last one is a step astern — or where
        // there is no last one at all, which is a boat that has just been put
        // in the water, or one that has lain still long enough for its whole
        // wake to have gone.
        //
        // And only while the hull is stirring the water at all. A point laid
        // slower than that would paint nothing wherever it went, and leaving
        // it out is what lets a boat at anchor come to a complete stop:
        // otherwise the last dead point ages out, is laid again in the same
        // spot, ages out again, and a hull that has not moved in ten minutes
        // is still writing a new uniform every frame.
        let stepped = self
            .laid
            .front()
            .is_none_or(|newest| newest.at.distance(at) >= STEP);
        // What goes down is where the hull *was*, never where it is: the
        // head is the hull, and the head is already drawn, so laying the
        // point the hull is leaving keeps the track a step dense without
        // ever putting two live points on one spot.
        if let Some(previous) = self.head {
            if stepped && way >= STIRS {
                self.laid.push_front(previous);
                self.laid.truncate(LAID);
            }
        }
        self.head = Some(Mark { at, age: 0.0, way });
    }

    /// Whether the shader would paint anything from this track: a point laid
    /// and not yet worn away, or a head stirring the water now. The head is
    /// carried every frame whether the hull moves or not, so a hull lying
    /// still has one, and it is this and not the head that earns a slot of
    /// the [`WAKES`] — otherwise a crowded anchorage fills them with hulls
    /// painting nothing and the one boat sailing past goes without.
    fn shows(&self) -> bool {
        !self.laid.is_empty() || self.head.is_some_and(|head| head.way >= STIRS)
    }

    /// The track as the shader takes it, and the box it lies in.
    ///
    /// Slots past the end of the track repeat its last point rather than
    /// carrying a flag saying they are empty: a repeated point is a segment
    /// of no length, which the shader's own arithmetic already handles, and
    /// the oldest point is by then too worn to add anything wherever it is
    /// asked about. That is a branch not written in a loop run per pixel.
    ///
    /// The box is every point of track grown by the furthest the foam could
    /// possibly be from it, and it is the whole of why this is affordable:
    /// water outside it can reject the wake with two comparisons instead of
    /// walking [`TRAIL`] segments. A track that [`shows`] nothing answers
    /// [`NOWHERE`], which is a sea with no wake on it at all.
    ///
    /// [`shows`]: Track::shows
    fn packed(&self, stem_half: f32) -> ([Vec4; TRAIL], Vec4) {
        let Some(head) = self.head.filter(|_| self.shows()) else {
            return ([Vec4::ZERO; TRAIL], NOWHERE);
        };
        let last = self.laid.back().copied().unwrap_or(head);

        let mut points = [Vec4::ZERO; TRAIL];
        let mut least = Vec2::MAX;
        let mut most = Vec2::MIN;
        for (slot, point) in points.iter_mut().enumerate() {
            let mark = match slot {
                0 => head,
                _ => self.laid.get(slot - 1).copied().unwrap_or(last),
            };
            *point = Vec4::new(mark.at.x, mark.at.y, mark.age, mark.way);
            // Whichever of the two shapes reaches furthest here. An arm's own
            // thickness lies inside its half-width, so it adds nothing.
            let reach = stem_half + (SPREAD * mark.way).max(BOIL.0) * mark.age;
            least = least.min(mark.at - reach);
            most = most.max(mark.at + reach);
        }
        (points, Vec4::new(least.x, least.y, most.x, most.y))
    }
}

pub struct WakePlugin;

impl Plugin for WakePlugin {
    fn build(&self, app: &mut App) {
        // After every hull has been put where this frame leaves it, and
        // after the sailed one has had its tilt composed on top, or the
        // white water would be attached to where a boat was last frame —
        // which at ten metres a second is a hand's breadth, and free to
        // avoid. Both named, because the second only places the one hull.
        app.add_systems(
            Update,
            lay_the_wake
                .after(crate::boat::ride_the_plane)
                .after(crate::boat::float)
                .run_if(in_state(AppState::InWorld)),
        );
    }
}

/// Every hull afloat, as the wake reads one: where it is, what it is, how
/// fast the plane says it is going — or the wire, for a hull the wire moves
/// and this client is not shoving — and the track it is dragging.
type Hulls<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Transform,
        &'static Rigged,
        &'static LinearVelocity,
        Option<&'static Telling>,
        Has<Shoving>,
        &'static mut Track,
    ),
    With<Vessel>,
>;

/// Carries every hull's track along with it and writes the nearest
/// [`WAKES`] of them into the sea's material, which is the whole of how the
/// water hears about the wake. The module doc says whose way a track is laid
/// at; [`Eye`] says which tracks go in when there are more than fit.
///
/// The material is reached through the depth window because that is where the
/// handle already lives, and the write goes through the same
/// read-compare-write two-step the depth sweep and the weather use.
pub(crate) fn lay_the_wake(
    time: Res<Time>,
    mut hulls: Hulls,
    eye: Eye,
    window: Option<Res<DepthWindow>>,
    materials: Option<ResMut<Assets<SeaMaterial>>>,
) {
    let dt = time.delta_secs();
    for (_, transform, rigged, velocity, telling, shoving, mut track) in &mut hulls {
        let way = match telling {
            Some(told) if !shoving => told.way(),
            _ => velocity.0,
        };
        track.follow(bow_of(transform, *rigged), way.length(), dt);
    }

    let (Some(window), Some(mut materials)) = (window, materials) else {
        return;
    };
    let Some(focus) = eye.focus() else {
        return;
    };

    let mut showing: Vec<_> = hulls
        .iter()
        .filter(|(_, _, _, _, _, _, track)| track.shows())
        .map(|(hull, transform, rigged, _, _, _, track)| {
            (focus.rank(hull, transform), *rigged, track)
        })
        .collect();
    showing.sort_by_key(|(rank, _, _)| *rank);

    let mut wake = [[Vec4::ZERO; TRAIL]; WAKES];
    let mut bounds = [NOWHERE; WAKES];
    let mut hull = [Vec4::ZERO; WAKES];
    for (slot, (_, rigged, track)) in showing.iter().take(WAKES).enumerate() {
        let shoulder = shoulder_of(*rigged);
        (wake[slot], bounds[slot]) = track.packed(shoulder);
        hull[slot] = Vec4::new(shoulder, 0.0, 0.0, 0.0);
    }
    let wash = Vec4::new(SPREAD, ARM, LIFE, 0.0);
    let boil = Vec4::new(BOIL.0, BOIL.1, BREAKUP, STIRS);

    let stale = materials.get(window.material()).is_some_and(|material| {
        material.extension.wake != wake
            || material.extension.wake_bounds != bounds
            || material.extension.wake_hull != hull
            || material.extension.wash != wash
            || material.extension.boil != boil
    });
    if stale {
        if let Some(mut material) = materials.get_mut(window.material()) {
            material.extension.wake = wake;
            material.extension.wake_bounds = bounds;
            material.extension.wake_hull = hull;
            material.extension.wash = wash;
            material.extension.boil = boil;
        }
    }
}

/// Half the width of the water a hull turns over at its stem — its half-beam
/// with the [`SHOULDER`] on.
fn shoulder_of(rigged: Rigged) -> f32 {
    rigged.beam() * 0.5 * SHOULDER
}

/// Where a hull lays its white this frame, on the map.
///
/// A shoulder's radius abaft the stem, not on it. The foam's forward edge is
/// the cap the shader wraps round the head, and that cap's radius is this
/// same half-width — so laid on the stem, as it was first, the white led the
/// bow by half a beam of open water. Set back by exactly the radius, the
/// cap's edge falls on the stem itself and the wake opens from the bow point
/// instead of leading it.
fn bow_of(transform: &Transform, rigged: Rigged) -> Vec2 {
    transform
        .transform_point(rigged.stem() + Vec3::Z * shoulder_of(rigged))
        .xz()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame of the length the tests' clock runs at.
    const FRAME: f32 = 1.0 / 60.0;

    /// The ship's half-beam with its shoulder on, near enough — what the box
    /// arithmetic is measured against here. Nothing depends on it being the
    /// ship's exactly.
    const STEM_HALF: f32 = 1.8;

    /// Sails a track in a straight line at a speed, a frame at a time.
    fn sail(track: &mut Track, way: f32, seconds: f32) {
        let frames = (seconds / FRAME).round() as usize;
        let mut at = track.head.map_or(Vec2::ZERO, |head| head.at);
        for _ in 0..frames {
            at += Vec2::X * way * FRAME;
            track.follow(at, way, FRAME);
        }
    }

    #[test]
    fn the_head_of_the_track_is_the_bow_itself() {
        // What keeps the white water attached to the hull: whatever the boat
        // did this frame, the newest point of the track is where its bow now
        // is, to the metre and not to the step.
        let mut track = Track::default();
        for step in 0..200 {
            let bow = Vec2::new(step as f32 * 0.17, (step as f32 * 0.03).sin() * 4.0);
            track.follow(bow, 10.0, FRAME);
            let head = track.head.expect("a followed track has a head");
            assert_eq!(head.at, bow, "the track's head left the bow behind");
            assert_eq!(head.age, 0.0, "the head aged while it was still the head");
        }
    }

    #[test]
    fn the_track_is_laid_at_its_step() {
        // Points go down as the boat travels, not as time passes — so the
        // shape the shader walks is the shape the boat sailed, at the
        // resolution the step promises. A point goes down on the frame the
        // last one falls a step astern, so a gap is a step plus however far
        // that frame carried the hull and no further.
        let way = 6.0;
        let mut track = Track::default();
        sail(&mut track, way, LIFE * 0.5);
        assert!(track.laid.len() > 2, "a boat sailed and laid no track");
        let widest = STEP + way * FRAME;
        for pair in track.laid.iter().collect::<Vec<_>>().windows(2) {
            let gap = pair[0].at.distance(pair[1].at);
            assert!(
                gap <= widest + 1e-3,
                "the track steps {gap} m, past the {widest} m it promises"
            );
            assert!(
                pair[0].age < pair[1].age,
                "the track does not run newest first"
            );
        }
    }

    #[test]
    fn a_wild_way_cannot_reach_the_shader() {
        // Every shape in a wake scales with the speed that laid it, and the
        // arms scale with it *per metre run* — so one absurd reading does not
        // look a bit wrong, it rules hairlines of foam clean across the ocean
        // and hands back a box the whole sea is inside of. It got in once, off
        // a frame a few microseconds long; nothing may be recorded above the
        // bound now, whatever a hull claims.
        let mut track = Track::default();
        track.follow(Vec2::ZERO, 0.0, FRAME);
        track.follow(Vec2::new(0.2, 0.0), 17_226.0, FRAME);
        let (points, bounds) = track.packed(STEM_HALF);
        for point in points {
            assert!(point.w <= FASTEST, "the track recorded {} m/s", point.w);
        }
        let across = (bounds.z - bounds.x).max(bounds.w - bounds.y);
        assert!(across < 200.0, "one bad reading opened a {across} m box");
    }

    #[test]
    fn a_hull_put_somewhere_does_not_drag_its_wake_along() {
        // Entering a world, or being taken to a place by `goto`, moves a
        // hull further in a frame than any hull can sail. Keeping the
        // track across that would join the two places with one straight
        // segment, and paint a wake down however much ocean lies between.
        let mut track = Track::default();
        sail(&mut track, 8.0, 3.0);
        assert!(!track.laid.is_empty(), "nothing to lose");
        track.follow(Vec2::new(4_000.0, -2_500.0), 8.0, FRAME);
        assert!(
            track.laid.is_empty(),
            "a hull carried its old wake across the world with it"
        );
    }

    #[test]
    fn a_wake_is_forgotten_when_its_life_is_up() {
        // The far end of a wake has to end somewhere, and this is where: no
        // point of track outlives [`LIFE`], however long the boat sails on.
        let mut track = Track::default();
        sail(&mut track, 9.0, LIFE * 3.0);
        let oldest = track.laid.back().expect("a sailing boat has a track").age;
        assert!(oldest <= LIFE, "a {oldest} s wake outlived its {LIFE} s");
        assert!(
            oldest > LIFE - 1.0,
            "the track only reaches back {oldest} s of its {LIFE} s"
        );
    }

    #[test]
    fn the_track_is_long_enough_to_hold_a_whole_wake() {
        // The two limits on a wake are its life and the track's length, and
        // which one bites decides what the far end looks like. The life is
        // meant to be the one that does, at every speed a hull here can be
        // driven at, so the length has to hold a whole wake's worth of the
        // fastest of them with something to spare.
        let held = LAID as f32 * STEP;
        assert!(
            held / LIFE > FASTEST,
            "the track holds {held} m, only {} m/s of a {LIFE} s wake",
            held / LIFE
        );
    }

    #[test]
    fn a_hull_lying_still_leaves_nothing() {
        // A boat at anchor is the state the game is entered in and the one it
        // spends most of its time in, so it is the one that has to come out
        // clean: no way, no white, nothing left over from a wake that has had
        // its time — and, once all that is true, *nothing changing*, because a
        // track that keeps stirring is a uniform being written to the GPU
        // every frame for a hull that has not moved in ten minutes.
        let mut track = Track::default();
        sail(&mut track, 8.0, 2.0);
        sail(&mut track, 0.0, LIFE + 1.0);

        assert!(track.laid.is_empty(), "an old wake was kept");
        let (points, bounds) = track.packed(STEM_HALF);
        sail(&mut track, 0.0, 4.0);
        let (later, later_bounds) = track.packed(STEM_HALF);
        assert_eq!(points, later, "a boat at anchor keeps rewriting its wake");
        assert_eq!(bounds, later_bounds);
        for point in points {
            assert!(
                point.w < STIRS,
                "a boat at rest is still making {} m/s of foam",
                point.w
            );
        }
    }

    #[test]
    fn a_hull_with_nothing_to_show_yields_its_slot() {
        // The head is carried every frame whether the hull moves or not, so
        // "has a head" would admit every hull afloat and a crowded anchorage
        // would fill the slots with boats painting nothing. What earns a slot
        // is white the shader could paint: a head stirring the water, or a
        // point laid behind it and not yet worn away.
        let mut track = Track::default();
        sail(&mut track, 0.0, 1.0);
        assert!(!track.shows(), "a hull that has never moved shows a wake");
        assert_eq!(track.packed(STEM_HALF).1, NOWHERE);

        sail(&mut track, 8.0, 1.0);
        assert!(track.shows(), "a hull under way shows nothing");
        sail(&mut track, 0.0, 1.0);
        assert!(
            track.shows(),
            "a hull just stopped lost the wake still lying behind it"
        );
        sail(&mut track, 0.0, LIFE);
        assert!(
            !track.shows(),
            "a wake that has had its time keeps its slot"
        );
        assert_eq!(track.packed(STEM_HALF).1, NOWHERE);
    }

    #[test]
    fn the_box_holds_every_scrap_of_the_wake() {
        // The box is an optimisation that can only be wrong one way: water
        // outside it never asks about the wake at all, so any foam that would
        // have fallen out there is simply missing, and missing along a
        // straight edge. Check it against the widest the wake can reach —
        // both shapes, at the oldest age either lasts.
        let mut track = Track::default();
        sail(&mut track, 11.0, LIFE * 2.0);
        let (points, bounds) = track.packed(STEM_HALF);
        for point in points {
            let arms = STEM_HALF + SPREAD * point.w * point.z;
            let boil = STEM_HALF + BOIL.0 * point.z;
            let reach = arms.max(boil);
            let at = Vec2::new(point.x, point.y);
            assert!(
                at.x - reach >= bounds.x - 1e-3
                    && at.y - reach >= bounds.y - 1e-3
                    && at.x + reach <= bounds.z + 1e-3
                    && at.y + reach <= bounds.w + 1e-3,
                "wake reaching {reach} m from {at} falls outside {bounds}"
            );
        }
    }

    #[test]
    fn an_empty_track_is_a_box_nothing_is_inside() {
        // What a sea with no boat on it costs: two comparisons per fragment
        // of water, and no walk down a track of stale points.
        let (_, bounds) = Track::default().packed(STEM_HALF);
        assert!(
            bounds.x > bounds.z && bounds.y > bounds.w,
            "an empty track claimed the box {bounds}"
        );
    }

    #[test]
    fn the_wake_is_laid_at_the_bow_of_the_boat_the_game_launches() {
        // The wiring rather than the shape: in a real world the system finds
        // the hull that was launched, and the point it lays is a shoulder's
        // radius abaft that hull's stem — so the cap the shader wraps round
        // the head ends on the stem, and the white neither leads the bow nor
        // sits amidships, half a boat's length behind it.
        let mut app = crate::testing::world_app();
        app.update();

        let mut hulls = app
            .world_mut()
            .query_filtered::<(&Transform, &Rigged, &Track), With<Vessel>>();
        let (transform, rigged, track) = hulls
            .single(app.world())
            .expect("a world should have a boat in it");
        let bow = bow_of(transform, *rigged);
        let amidships = transform.translation.xz();

        let head = track
            .head
            .expect("a boat afloat should have the head of a track");
        assert_eq!(head.at, bow, "the wake is laid somewhere the bow is not");
        assert!(
            head.at.distance(amidships) > 1.0,
            "the wake is laid amidships"
        );
    }

    #[test]
    fn a_boat_in_tow_leaves_a_wake_of_its_own() {
        // A hull on the painter has no sailing state — the ship's is the only
        // one — and is going wherever the ship goes, as fast as the ship
        // goes. It is a hull through water all the same, and the water has
        // to say so: its own track, laid at its own bow, at the ship's way.
        use crate::boat::{with_the_ships_boat, Towed};
        use crate::testing::{hold, run_frames, set_wind};

        let mut app = crate::testing::world_app();
        let dinghy = with_the_ships_boat(&mut app);
        let ship = app.world().get::<Towed>(dinghy).expect("in tow").by();
        let ship_pose = *app.world().get::<Transform>(ship).unwrap();

        set_wind(&mut app, ship_pose.forward().xz() * 10.0);
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 60 * 6);

        let way_of = |app: &App, hull: Entity| {
            app.world()
                .get::<LinearVelocity>(hull)
                .expect("a hull is on the plane")
                .0
                .length()
        };
        assert!(way_of(&app, ship) > STIRS, "the ship never got under way");
        assert!(
            way_of(&app, dinghy) > STIRS,
            "the dinghy was left behind: {} m/s to the ship's {} m/s",
            way_of(&app, dinghy),
            way_of(&app, ship)
        );

        for (hull, name) in [(ship, "ship"), (dinghy, "dinghy")] {
            let track = app
                .world()
                .get::<Track>(hull)
                .expect("a hull keeps a track");
            let head = track
                .head
                .unwrap_or_else(|| panic!("the {name} has no head to its track"));
            let transform = app.world().get::<Transform>(hull).unwrap();
            let rigged = app.world().get::<Rigged>(hull).unwrap();
            assert_eq!(
                head.at,
                bow_of(transform, *rigged),
                "the {name}'s wake is laid somewhere its bow is not"
            );
            assert!(
                head.way >= STIRS,
                "the {name} is making {} m/s and stirring nothing",
                head.way
            );
            assert!(
                track.laid.len() > 3,
                "the {name} sailed and laid {} points of track",
                track.laid.len()
            );
        }
    }

    #[test]
    fn the_shader_walks_the_whole_track() {
        // The uniform is an array on both sides of the wire between Rust and
        // WGSL, and only one of them is compiled here. A shader reading fewer
        // points than are sent draws a wake that ends early for no reason
        // anything at runtime could explain.
        let shader = std::fs::read_to_string(format!(
            "{}/../../assets/shaders/sea.wgsl",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("the sea's shader under assets/shaders/");
        for declared in [
            format!("const TRAIL: i32 = {TRAIL};"),
            format!("const WAKES: i32 = {WAKES};"),
        ] {
            assert!(
                shader.contains(&declared),
                "the shader does not say `{declared}`"
            );
        }
    }
}
