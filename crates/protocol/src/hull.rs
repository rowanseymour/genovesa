//! The shape of a hull where it meets the ground, and the one arithmetic both
//! ends judge that meeting by.
//!
//! A hull is a keel line over a height field. Whether it may be somewhere is
//! not a question about the point it stands on but about that whole line: a
//! sloop is metres long, and a bed that leaves its middle in a fathom can have
//! its forefoot in rock. So the reading is taken at a spread of stations from
//! forefoot to heel and the worst one is the answer.
//!
//! **Both machines have to ask the same question.** A client steering a hull
//! holds it off the ground, and the sea moves every hull nobody is steering —
//! see `server::sea` — so the two take turns owning the same boat. If they
//! disagree about where it may be, a hull handed from one to the other is
//! either shoved off water it was sitting in quite happily or, the way it
//! actually went wrong, swung by the server into a cliff the client would
//! never have driven it into and left standing in the rock. That is why the
//! keel is written here with the rest of what both ends must agree about, for
//! [`crate::TENDER_ASTERN`]'s reason: the numbers are small, the disagreement
//! is not, and it shows up as a picture rather than as an error.
//!
//! What is *not* here is the rest of a boat. A hull carries a mast, a
//! displacement, a turning circle and a dozen other numbers that only the
//! machine drawing it and sailing it has any use for. Those stay there. What
//! crosses is the keel, because the keel is what the ground stops.
use glam::Vec2;

use crate::ground::CELL_METRES;
use crate::BoatKind;

/// How far a keel is let bite into the bed before the hull counts as stopped,
/// in metres.
///
/// A hull held off the bottom by exactly its draft would stop with its keel
/// drawn on the surface of the sand, which reads as a boat hovering. Letting
/// it settle a little into the bed is what makes a grounding look like one.
/// It is also slack in the reading's favour, which is the right direction to
/// miss in: the bed under a keel is linear between lattice corners, so a
/// probe can read a shade deep over a crest it straddles.
pub const KEEL_BITE: f32 = 0.2;

/// A hull's keel, as the ground sees it: how deep it runs, and the two
/// stations it runs between on the hull's own fore-and-aft axis.
///
/// Stations are metres abaft the hull's middle, so a forefoot is negative and
/// a heel positive — [`astern`]'s sign, which is the drawing convention both
/// ends already share.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Keel {
    /// How deep the hull draws, in metres.
    pub draft: f32,
    /// Where the keel begins forward. Short of the stem on a hull with any
    /// rake to it, which is what keeps a probe over timber rather than over
    /// the water a bow overhangs.
    pub forefoot: f32,
    /// Where it ends aft.
    pub heel: f32,
}

impl Keel {
    /// How long the keel is, forefoot to heel.
    pub fn run(&self) -> f32 {
        self.heel - self.forefoot
    }

    /// How little water the hull is held in: a bed standing higher than this
    /// far below the waterline stops it. See [`KEEL_BITE`].
    pub fn grounding_draft(&self) -> f32 {
        self.draft - KEEL_BITE
    }

    /// How many points along the keel are asked about the bottom, spread from
    /// forefoot to heel inclusive.
    ///
    /// Derived so the gap between them never exceeds [`CELL_METRES`]: no facet
    /// of the height field can then lie wholly between two probes, so ground
    /// rising across a facet is read on the way up rather than stepped over.
    /// Derived rather than picked, a count tuned to one cell size having
    /// quietly stopped holding once before when the mesh was refined.
    ///
    /// This is less than "nothing gets past". A crest one lattice line wide is
    /// not seen, the field being linear between its corners, so two probes
    /// either side read its flanks and the hull sails through a rock at the
    /// waterline. Coasts are safe by being coasts: the bottom shelves, so the
    /// ground under a keel is near enough monotone. What is exposed is the
    /// isolated skerry, paid off from the other end by giving skerries width.
    pub fn probes(&self) -> usize {
        (self.run() / CELL_METRES).ceil() as usize + 1
    }
}

/// The keel of each kind of hull — the wire's [`BoatKind`] resolved to the
/// shape the ground stops.
///
/// The numbers are the models', measured off the files the game draws, and
/// the client's own model tests hold them there. They are written once, here,
/// because a server with a copy of its own would swing a hull by arithmetic
/// the client it is drawn on does not share.
pub fn keel_of(kind: BoatKind) -> Keel {
    match kind {
        // Seven metres of hull; the forefoot stops short of the bow, which is
        // what gives the stem its rake, and the heel runs aft to the transom.
        BoatKind::Sloop => Keel {
            draft: 0.8,
            forefoot: -7.0 * 0.5 * 0.7,
            heel: 7.0 * 0.5,
        },
        // Rockered: deepest a little abaft amidships, rising to the forefoot
        // forward and carried aft to the transom's skeg. The probes read the
        // full draft along all of it, which errs a few centimetres shy at the
        // rockered ends — the right side to miss on.
        BoatKind::Rowboat => Keel {
            draft: 0.25,
            forefoot: -1.1,
            heel: 1.6,
        },
    }
}

/// The point `metres` straight astern of a hull lying at `at` and pointing
/// `heading`, and so the world point of any station on its keel.
///
/// The drawing convention both ends share, run backwards: a hull's forward is
/// its yaw applied to the client's own forward axis, so astern is the
/// negation, and a positive station is abaft the middle.
pub fn astern(at: Vec2, heading: f32, metres: f32) -> Vec2 {
    at + Vec2::new(heading.sin(), heading.cos()) * metres
}

/// How far into the bed the worst-placed point of a hull's keel is standing,
/// for a hull of `kind` lying at `at` and pointing `heading`.
///
/// Negative for as long as there is water enough under all of it, zero where
/// the hull is about to be stopped, and positive by however far it is in. Not
/// the keel's own penetration, which is this plus [`KEEL_BITE`]: the rule
/// wants one number that rises as the ground does, and nothing reads it but
/// its sign and its ordering against itself, both of which the offset leaves
/// alone.
///
/// `bed` gives the height of the bed at a point, or `None` where this machine
/// does not know — a chunk that has not arrived. An unknown probe says nothing
/// rather than objecting, so a hull over ground still on its way is not stopped
/// by ignorance; and a hull with nothing known under any of it answers
/// [`f32::NEG_INFINITY`], which is "no reason to stop it" and not "clear".
pub fn aground_by(
    kind: BoatKind,
    at: Vec2,
    heading: f32,
    bed: impl Fn(Vec2) -> Option<f32>,
) -> f32 {
    let keel = keel_of(kind);
    let probes = keel.probes();
    debug_assert!(probes > 1, "a keel with no run has no line to probe");
    (0..probes)
        .filter_map(|i| {
            let station = keel.forefoot + keel.run() * i as f32 / (probes - 1) as f32;
            Some(bed(astern(at, heading, station))? + keel.grounding_draft())
        })
        .fold(f32::NEG_INFINITY, f32::max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_2;

    /// Every kind's keel has to be a keel: run the right way along the hull,
    /// draw something, and leave the probes a line to be spread along.
    #[test]
    fn every_kind_has_a_keel_that_runs_forward_to_aft() {
        for kind in [BoatKind::Sloop, BoatKind::Rowboat] {
            let keel = keel_of(kind);
            assert!(
                keel.forefoot < 0.0 && keel.heel > 0.0,
                "{kind:?}'s keel does not straddle its middle"
            );
            assert!(keel.run() > 0.0, "{kind:?}'s keel runs backwards");
            assert!(keel.draft > 0.0, "{kind:?} draws nothing");
            assert!(
                keel.grounding_draft() > 0.0,
                "{kind:?} is bitten away to nothing by the keel bite"
            );
        }
    }

    /// The probes have to close the gaps a facet could hide in — the whole
    /// reason the count is derived rather than picked.
    #[test]
    fn the_probes_never_leave_a_facet_between_them() {
        for kind in [BoatKind::Sloop, BoatKind::Rowboat] {
            let keel = keel_of(kind);
            let gap = keel.run() / (keel.probes() - 1) as f32;
            assert!(
                gap <= CELL_METRES,
                "{kind:?} spreads its probes {gap} m apart over {CELL_METRES} m cells"
            );
        }
    }

    /// A station forward is forward and a station aft is aft, whichever way
    /// the hull is pointing. This is the sign the two machines would disagree
    /// about silently: a keel probed backwards sounds the water a bow is
    /// leaving instead of the rock it is running at.
    #[test]
    fn the_forefoot_is_probed_ahead_of_the_heel() {
        // Pointing north, which for this convention is a heading of zero.
        let north = astern(Vec2::ZERO, 0.0, -1.0);
        assert!(north.y < 0.0, "a forward station was not forward");
        assert!(
            astern(Vec2::ZERO, 0.0, 1.0).y > 0.0,
            "an aft station was not aft"
        );
        // And turned a quarter turn, the same station swings with the hull.
        let turned = astern(Vec2::ZERO, FRAC_PI_2, -1.0);
        assert!(
            turned.x < 0.0 && turned.y.abs() < 1e-6,
            "a forward station did not turn with the hull: {turned:?}"
        );
    }

    /// The reading is the *worst* point of the keel, not the point underfoot
    /// — the whole of what this is for. A rock under one end of a hull lying
    /// in deep water is a grounding.
    #[test]
    fn a_rock_under_one_end_grounds_a_hull_floating_at_its_middle() {
        let keel = keel_of(BoatKind::Sloop);
        let deep = -20.0;
        // A bed that is deep everywhere but right at the forefoot.
        let bow = astern(Vec2::ZERO, 0.0, keel.forefoot);
        let bed = |at: Vec2| Some(if at.distance(bow) < 0.5 { 0.0 } else { deep });

        assert!(
            aground_by(BoatKind::Sloop, Vec2::ZERO, 0.0, bed) >= 0.0,
            "a rock at the forefoot did not stop the hull"
        );
        // And the same bed with nothing on it does not.
        assert!(
            aground_by(BoatKind::Sloop, Vec2::ZERO, 0.0, |_| Some(deep)) < 0.0,
            "deep water all round stopped the hull"
        );
    }

    /// The reading a single point would give is not the reading, which is the
    /// bug this exists for: the server sounded the middle and swung a hull
    /// into a cliff its own bow was already in.
    #[test]
    fn sounding_the_middle_alone_misses_what_the_keel_finds() {
        let keel = keel_of(BoatKind::Sloop);
        let bow = astern(Vec2::ZERO, 0.0, keel.forefoot);
        let bed = |at: Vec2| Some(if at.distance(bow) < 0.5 { 0.0 } else { -20.0 });

        let underfoot = bed(Vec2::ZERO).expect("a bed") + keel.grounding_draft();
        assert!(underfoot < 0.0, "the middle was not in clear water");
        assert!(
            aground_by(BoatKind::Sloop, Vec2::ZERO, 0.0, bed) > underfoot,
            "the keel found no more than the middle did"
        );
    }

    /// Ground that has not arrived is not a reason to stop a hull — the
    /// client's own choice everywhere else it reads the bed.
    #[test]
    fn ground_that_has_not_arrived_says_nothing() {
        assert_eq!(
            aground_by(BoatKind::Sloop, Vec2::ZERO, 0.0, |_| None),
            f32::NEG_INFINITY,
            "an unsounded bed was read as something"
        );
        // And one known probe is enough to answer on, the rest being unknown
        // rather than clear.
        let keel = keel_of(BoatKind::Sloop);
        let bow = astern(Vec2::ZERO, 0.0, keel.forefoot);
        let only_the_bow = |at: Vec2| (at.distance(bow) < 0.5).then_some(0.0);
        assert!(
            aground_by(BoatKind::Sloop, Vec2::ZERO, 0.0, only_the_bow) >= 0.0,
            "the one probe that answered was thrown away"
        );
    }
}
