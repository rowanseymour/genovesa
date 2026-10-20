//! The shape of a hull where it meets the ground, and the one arithmetic both
//! ends judge that meeting by.
//!
//! A hull is a bottom over a height field. Whether it may be somewhere is not
//! a question about the point it stands on but about all of it: a sloop is
//! metres long, and a bed that leaves its middle in a fathom can have its
//! forefoot in rock. So the reading is taken at every place the two surfaces
//! could first meet, and the worst one is the answer.
//!
//! Both are made of flat pieces — the bed is facets between lattice corners
//! and the bottom is [`Keel::depth_at`]'s few planes — so where they first
//! meet is a corner of one over the other: the keel over the bed, read at
//! stations along it, or a lattice corner under the bottom, read at every
//! corner the footprint covers. The first catches a shelf rising under the
//! keel between corners, the second a rock, a crest or a cliff that the keel
//! line passes beside or between its stations. Where an edge of one crosses
//! an edge of the other is the third place, and is not read: it is never far
//! from one of the first two, and the slack is [`KEEL_BITE`]'s to cover.
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
use glam::{IVec2, Vec2};

use crate::ground::{corner_point, lattice_height, CELL_METRES};
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

/// A hull as the ground sees it: its waterline footprint, how deep it runs,
/// and the two stations its keel runs between on the hull's own fore-and-aft
/// axis.
///
/// Stations are metres abaft the hull's middle, so a forefoot is negative and
/// a heel positive — [`astern`]'s sign, which is the drawing convention both
/// ends already share.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Keel {
    /// Length overall, in metres, centred on the hull's middle.
    pub length: f32,
    /// Beam, in metres. With the length, the rectangle the hull covers —
    /// the same one it meets another hull with.
    pub beam: f32,
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

    /// How far below the waterline the hull's bottom is, `along` metres abaft
    /// its middle and `across` to either side — `None` off the footprint.
    ///
    /// A crude section, on purpose: the full draft along the keel, falling in
    /// straight lines to the waterline at the beam and at the ends beyond the
    /// keel. A real hull is fuller than a V, so this reads shallow of the
    /// planking everywhere but the keel, which is the side to miss on — and
    /// at the beam it is the waterline, so what stops a hull's side is ground
    /// standing out of the water there.
    pub fn depth_at(&self, along: f32, across: f32) -> Option<f32> {
        let (half_length, half_beam) = (self.length / 2.0, self.beam / 2.0);
        if along.abs() > half_length || across.abs() > half_beam {
            return None;
        }
        // Never divides by nothing: a heel at the transom leaves no `along`
        // abaft it, and a forefoot at the stem none ahead of it.
        let lengthwise = if along < self.forefoot {
            (along + half_length) / (self.forefoot + half_length)
        } else if along > self.heel {
            (half_length - along) / (half_length - self.heel)
        } else {
            1.0
        };
        Some(self.draft * lengthwise * (1.0 - across.abs() / half_beam))
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
    /// What this cannot see is a crest one lattice line wide, the field being
    /// linear between its corners, so two probes either side read its flanks.
    /// That is what the footprint's corners are read for — see [`aground_by`].
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
pub const fn keel_of(kind: BoatKind) -> Keel {
    match kind {
        // Seven metres of hull: at the client's default zoom the visible
        // ground is some tens of metres across, so this reads as a boat
        // rather than as a speck. The forefoot stops short of the bow, which
        // is what gives the stem its rake, and the heel runs aft to the
        // transom.
        BoatKind::Sloop => Keel {
            length: 7.0,
            beam: 2.4,
            draft: 0.8,
            forefoot: -7.0 * 0.5 * 0.7,
            heel: 7.0 * 0.5,
        },
        // Rockered: deepest a little abaft amidships, rising to the forefoot
        // forward and carried aft to the transom's skeg. The probes read the
        // full draft along all of it, which errs a few centimetres shy at the
        // rockered ends — the right side to miss on.
        BoatKind::Rowboat => Keel {
            length: 3.2,
            beam: 1.3,
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

/// How far into the bed the worst-placed point of a hull's bottom is
/// standing, for a hull of `kind` lying at `at` and pointing `heading` — read
/// at the keel's stations and at every lattice corner under the footprint;
/// the module doc says why those two.
///
/// Negative for as long as there is water enough under all of it, zero where
/// the hull is about to be stopped, and positive by however far it is in. Not
/// the keel's own penetration, which is this plus [`KEEL_BITE`]: the rule
/// wants one number that rises as the ground does, and nothing reads it but
/// its sign and its ordering against itself, both of which the offset leaves
/// alone.
///
/// `corner` gives the height of one lattice corner, or `None` where this
/// machine does not know — a chunk that has not arrived. The bed between
/// corners is [`lattice_height`]'s, the same on both machines because the
/// corners are the ones the wire carries. An unknown corner says nothing
/// rather than objecting, so a hull over ground still on its way is not stopped
/// by ignorance; and a hull with nothing known under any of it answers
/// [`f32::NEG_INFINITY`], which is "no reason to stop it" and not "clear".
pub fn aground_by(
    kind: BoatKind,
    at: Vec2,
    heading: f32,
    corner: impl Fn(IVec2) -> Option<f32>,
) -> f32 {
    let keel = keel_of(kind);
    let probes = keel.probes();
    debug_assert!(probes > 1, "a keel with no run has no line to probe");
    let along_the_keel = (0..probes).filter_map(|i| {
        let station = keel.forefoot + keel.run() * i as f32 / (probes - 1) as f32;
        Some(lattice_height(&corner, astern(at, heading, station))? + keel.grounding_draft())
    });

    let aft = astern(Vec2::ZERO, heading, 1.0);
    let reach = Vec2::splat(Vec2::new(keel.length, keel.beam).length() / 2.0);
    let low = ((at - reach) / CELL_METRES).floor().as_ivec2();
    let high = ((at + reach) / CELL_METRES).ceil().as_ivec2();
    let under_the_footprint = (low.y..=high.y)
        .flat_map(|z| (low.x..=high.x).map(move |x| IVec2::new(x, z)))
        .filter_map(|lattice| {
            let offset = corner_point(lattice) - at;
            let depth = keel.depth_at(offset.dot(aft), offset.perp_dot(aft))?;
            Some(corner(lattice)? + depth - KEEL_BITE)
        });

    along_the_keel
        .chain(under_the_footprint)
        .fold(f32::NEG_INFINITY, f32::max)
}

/// Whether a hull may come round from a heading it reads `was` at to one it
/// would read `would` at, both [`aground_by`] at the same place.
///
/// Refused only where the turn is what puts a hull that was clear into the
/// ground — swinging an end or a side into a cliff it was lying off. A hull
/// with nothing known under it was not clear, only unjudged, and ground
/// arriving under it is not its doing. A hull already aground turns whatever
/// the turn reads: a yaw sweeps the bottom over different ground, so nearly
/// every turn of a hull on a beach reads deeper, and holding it to "no
/// deeper" left one driven ashore unable to come round at all.
pub fn may_turn(was: f32, would: f32) -> bool {
    was > 0.0 || was == f32::NEG_INFINITY || would <= 0.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_2;

    /// A lattice standing at `deep` everywhere but the corners within
    /// `radius` of `rock`, which stand at the waterline.
    fn rock_at(rock: Vec2, radius: f32, deep: f32) -> impl Fn(IVec2) -> Option<f32> {
        move |corner| {
            Some(if corner_point(corner).distance(rock) < radius {
                0.0
            } else {
                deep
            })
        }
    }

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
        let bed = rock_at(bow, 0.6, deep);

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
        let bed = rock_at(bow, 0.6, -20.0);

        let underfoot = lattice_height(&bed, Vec2::ZERO).expect("a bed") + keel.grounding_draft();
        assert!(underfoot < 0.0, "the middle was not in clear water");
        assert!(
            aground_by(BoatKind::Sloop, Vec2::ZERO, 0.0, bed) > underfoot,
            "the keel found no more than the middle did"
        );
    }

    /// The section is the one described: the draft along the keel, the
    /// waterline at the beam and at a stem with rake to it, and nothing at
    /// all off the footprint.
    #[test]
    fn the_bottom_is_deepest_along_the_keel_and_meets_the_water_at_its_edges() {
        let keel = keel_of(BoatKind::Sloop);
        let (half_length, half_beam) = (keel.length / 2.0, keel.beam / 2.0);
        assert_eq!(keel.depth_at(0.0, 0.0), Some(keel.draft));
        assert_eq!(keel.depth_at(keel.forefoot, 0.0), Some(keel.draft));
        assert_eq!(keel.depth_at(0.0, half_beam), Some(0.0));
        assert_eq!(keel.depth_at(-half_length, 0.0), Some(0.0));
        assert_eq!(keel.depth_at(0.0, half_beam + 0.01), None);
        assert_eq!(keel.depth_at(half_length + 0.01, 0.0), None);
    }

    /// A lattice deep everywhere but the corners `rock` picks out, which
    /// stand at `height`.
    fn standing(rock: impl Fn(Vec2) -> bool, height: f32) -> impl Fn(IVec2) -> Option<f32> {
        move |corner| {
            Some(if rock(corner_point(corner)) {
                height
            } else {
                -20.0
            })
        }
    }

    /// The skerry the keel's stations step over: a crest along one lattice
    /// line, athwart the keel and between two stations, whose flanks are all
    /// the stations read. The footprint's corners stand on it.
    #[test]
    fn a_crest_between_the_stations_grounds_the_hull() {
        let (at, heading) = (Vec2::new(0.5, 0.3), 0.0);
        let crest = standing(|on| on.y == 0.0, 0.0);
        let keel = keel_of(BoatKind::Sloop);
        let probes = keel.probes();
        for i in 0..probes {
            let station = keel.forefoot + keel.run() * i as f32 / (probes - 1) as f32;
            let bed = lattice_height(&crest, astern(at, heading, station)).expect("a bed");
            assert!(
                bed + keel.grounding_draft() < 0.0,
                "station {station} reads the crest itself, so this tests nothing"
            );
        }
        assert!(
            aground_by(BoatKind::Sloop, at, heading, crest) >= 0.0,
            "the hull sailed over a crest at the waterline"
        );
    }

    /// Land standing a metre off the centreline is inside a hull with more
    /// than a metre either side of its keel, though the keel itself is in
    /// twenty metres of water.
    #[test]
    fn a_cliff_against_the_topsides_grounds_the_hull() {
        let cliff = standing(|on| on.x <= -1.0, 1.0);
        assert!(
            aground_by(BoatKind::Sloop, Vec2::ZERO, 0.0, cliff) >= 0.0,
            "the hull's side stood inside the cliff"
        );
    }

    /// And the same cliff with the hull lying alongside it, clear by a
    /// little: a footprint grown too wide would hold a boat off every quay.
    #[test]
    fn a_hull_lying_clear_alongside_a_cliff_is_not_stopped() {
        let keel = keel_of(BoatKind::Sloop);
        let cliff = standing(move |on| on.x <= -(keel.beam / 2.0).ceil(), 1.0);
        assert!(
            aground_by(BoatKind::Sloop, Vec2::ZERO, 0.0, cliff) < 0.0,
            "a hull clear of the cliff was stopped by it"
        );
    }

    /// A shelf the corners alone would miss: shoaler than the keel draws but
    /// deeper than the bottom stands half a metre off the keel, where the
    /// nearest corners are. The keel's own stations are what find it.
    #[test]
    fn a_shelf_between_the_corners_still_grounds_the_keel() {
        let keel = keel_of(BoatKind::Sloop);
        let shelf = -0.5;
        let beside = keel.depth_at(0.0, 0.5).expect("on the footprint");
        assert!(
            shelf + beside - KEEL_BITE < 0.0 && shelf + keel.grounding_draft() > 0.0,
            "the shelf is not between the keel and its corners"
        );
        assert!(
            aground_by(BoatKind::Sloop, Vec2::new(0.5, 0.0), 0.0, |_| Some(shelf)) >= 0.0,
            "a keel over a shelf shoaler than its draft was not stopped"
        );
    }

    /// A clear hull is refused only the turn that grounds it, and a hull
    /// aground is refused nothing.
    #[test]
    fn only_a_turn_that_grounds_a_clear_hull_is_refused() {
        assert!(may_turn(-0.3, -0.1), "a turn in clear water was refused");
        assert!(!may_turn(-0.3, 0.1), "a turn into the ground was allowed");
        assert!(may_turn(0.2, 0.5), "a hull aground was held from turning");
        assert!(may_turn(-0.3, 0.0), "touching is not aground");
        assert!(
            may_turn(f32::NEG_INFINITY, 0.5),
            "a hull with nothing known under it was held as though clear"
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
        let only_the_bow =
            |corner: IVec2| (corner_point(corner).distance(bow) < 1.5).then_some(0.0);
        assert!(
            aground_by(BoatKind::Sloop, Vec2::ZERO, 0.0, only_the_bow) >= 0.0,
            "the one probe that answered was thrown away"
        );
    }
}
