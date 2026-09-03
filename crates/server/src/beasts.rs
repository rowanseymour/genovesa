//! The beasts: the creatures the server *means*.
//!
//! The client's own wildlife — eagles, seabird lines — is scenery it raises
//! for itself and the server has never heard of it. What earns a creature a
//! place here is mattering, and there are two ways to matter: the shark's is
//! consequence, since it will one day go for a swimmer and one client seeing
//! that while another did not would make it a private hallucination; the
//! whale's and the pod's is company, they being rare pointable events two
//! players anchored side by side must be able to point at together. Birds are
//! texture nobody compares notes on.
//!
//! So a beast is run the way a player is: the server holds where it is and
//! tells everyone on a beat — see [`ToClient::Beast`], one message that is
//! both introduction and movement, so joining mid-life needs no catching up.
//!
//! Beasts are world state with session manners. Raised where the players are
//! and let go when the last of them sails away, the *population* being a
//! performance around whoever is present rather than something a seed means
//! the way it means palms. But the animals alive when a kept world is written
//! are in its file, because nothing with consequence may be escapable by
//! relogging — a shark that had somebody cornered when they quit must still
//! have them cornered when they reload.
//!
//! # What passes for a mind
//!
//! Every beast is born in deep water with **one place it is going**, and swims
//! there; the two kinds are two answers to what the place is. For a whale or a
//! pod it is open water a kilometre off, laid so the course runs near whoever
//! it was raised for, and arriving is the *end* — the journey is the life, so
//! nothing is ever stationed near a player or can drift into being so. For a
//! shark it is the shallows off a coast, and arriving is the *start* of
//! snooping its strip of water.
//!
//! Nothing is plotted beyond that. Where it is going is a *want*, steered
//! towards each beat through the water actually found; deciding in advance
//! where an animal would be was tried and made one that could not be
//! surprised, which is exactly what a boat is here to be.
//!
//! Two rules finish the shape. **Born deep, and never watched appearing** —
//! deep water is the water players are not standing in, so it is somewhere a
//! beast can be raised at any moment. **Gone under, and never watched
//! vanishing** — an animal on its way out submerges and is let go only once it
//! is down over deep floor, and one nobody has been near for a while is told
//! to leave rather than dropped where it stands, "nobody is near it" being a
//! statement about a radius and not about whether anyone is looking.
//!
//! # What one makes of another
//!
//! What an animal does about anything else in the water is one entry in
//! [`REGARDS`], a row per kind and a column per thing there is to meet, and
//! the entry carries its own reach — how far off a whale starts caring is a
//! fact about a whale *and a hull*. A single "how wary is this kind" number
//! came first and could not say that dolphins drive a shark off while a shark
//! gives a pod room, which is the interesting half of these animals.
//!
//! A stance is about *steering*, never about the life: it slots into [`swim`]
//! between wanting to be somewhere and holding to water it can be in, so
//! nothing here can make an animal live longer, die sooner, or arrive
//! somewhere it was not going. Mostly it comes out as a berth given
//! ([`skirt`]). The exception is the pod, which will come and *ride a bow*
//! ([`ride`]) — the one stance that takes over where an animal is going rather
//! than bending how it gets there, two swings under the same clamp cancelling.
//! It is bounded, the alternative being a pod that has become scenery attached
//! to a boat.
//!
//! The one thing the server cannot yet see is whether a player is *in the
//! water*: a client reports a position, not whether its player is afoot,
//! aboard or wading, so the table's two player columns are read as one and a
//! pod will happily ride the bow of a swimmer. That split is blocked on the
//! wire rather than on this file.
// Nearly everything worth pointing at from these docs — the table, the
// stances, the swimming — is private to this file and staying that way.
#![allow(rustdoc::private_intra_doc_links)]

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, TAU};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use glam::Vec2;
use protocol::{BeastId, BeastKind, PlayerId, ToClient};

use crate::{broadcast_all, Held, Shared};

/// How often the beasts are minded, which is also how often everyone is told
/// where they are. Slower than the players' own ten-a-second trickle: a
/// cruising animal is the steadiest mover in the world, and a client eases
/// and extrapolates between tellings — see [`ToClient::Beast`] — so four
/// beats a second read as one unbroken glide. It is also the only clock this
/// file has, and wants no other: every span here is counted in beats.
const BEAST_TICK: Duration = Duration::from_millis(250);

/// How far ahead a beast sounds the bottom before swimming there, in metres —
/// a few seconds of cruising, so a turn starts while there is still water to
/// turn in rather than at the sand.
const SOUNDING: f32 = 8.0;

/// How many spots are sounded, per beast wanted, before giving up until the
/// next beat. A coast offers shallows in abundance and open ocean offers
/// deep water without limit; this bounds what a player anchored somewhere
/// that offers neither costs.
const RAISE_ATTEMPTS: u32 = 12;

/// Seafloor at or below this counts as deep water, in metres: where beasts
/// are born and where they go to be forgotten. Open ocean floor lies at minus
/// [`OCEAN_DEPTH`] at its shallowest, comfortably under it, so the answer to
/// "where is there deep water" is never far from anywhere.
///
/// [`OCEAN_DEPTH`]: protocol::ground::OCEAN_DEPTH
const DEEP: f32 = -7.0;

/// Water anything here can swim through, whatever it lives in — a shark's
/// crossing (see [`Habitat::crossing`]) and what a leaving animal holds to,
/// which is anything wet enough not to be a beach.
const SWIMMABLE: (f32, f32) = (f32::NEG_INFINITY, -1.2);

/// Rings sounded outward from a shark's home when looking for the deep water
/// it is to be born in, in metres. Wider than the summons' rings below
/// because this is looking for open floor rather than for a particular band,
/// and open floor is either close by or the coast is a long shelf.
const BIRTH_RINGS: [f32; 4] = [70.0, 140.0, 260.0, 420.0];

/// And the rings a summons sounds — see [`conjure`], which wants the nearest
/// water of a kind rather than the deepest.
const SUMMONS_RINGS: [f32; 5] = [40.0, 80.0, 160.0, 320.0, 480.0];

/// How much faster than its cruise a beast travels when the swimming is not
/// the animal being itself, as a factor — see [`Beast::pace`], which is where
/// that distinction is drawn and what it costs.
const TRANSIT: f32 = 1.6;

/// How long a leaving beast must have been down, in beats, before it may be
/// let go: long enough for the dive to have been drawn, so nothing is ever
/// seen to vanish.
const SLIPPING: u32 = 12;

/// How long a beast goes on being minded with nobody inside its kind's
/// `waters`, in beats — fifteen seconds, which is a good hundred metres of
/// sailing away from an edge already hundreds of metres out. At the end of it
/// the animal is told to leave rather than dropped; see the forgetting in
/// [`Flock::beat`], both for that and for why this is a span of time rather
/// than a second, wider radius.
const FORGOTTEN_AFTER: u32 = 60;

/// And how long the leaving is given before the beast is let go wherever it
/// has got to, in beats. A beast that cannot find deep water is in a lagoon
/// or a lake, where it is also submerged and out of anyone's way; four
/// minutes of trying is patience enough. It is the outside of every ending
/// there is, the forgetting included, which is what stops an animal nobody is
/// near from swimming on forever for want of anyone to notice it has gone.
const LEAVING_PATIENCE: u32 = 960;

/// How often a dwelling beast redraws the lazy arc it is swimming, in beats
/// — see [`Habitat::meander`].
const ARC: u32 = 40;

/// How near the end of a journey counts as having got there, in metres; how
/// many courses are tried when laying one; and how far abeam of the player
/// the course may be aimed, in metres — see [`journey`].
const ARRIVED: f32 = 60.0;
const AIM_TRIES: u32 = 8;
const ABEAM: f32 = 220.0;

/// How much of the reach a boat is noticed at a beast actually insists on
/// clearing it by — see [`skirt`], where the gap between the two is what buys
/// the turn room to be gradual.
const BERTH: f32 = 0.5;

/// The whale's breathing, in beats: how long a bout at the surface runs, how
/// long it spends down between bouts, and how long it stays down after a hull
/// has driven it there. The last is much the longest — being put down by a
/// boat is not a breath cycle, and a whale that popped back up beside the
/// hull that spooked it would read as not having minded at all.
const SURFACED_BOUT: (u32, u32) = (40, 90);
const SOUNDED_BOUT: (u32, u32) = (70, 150);
const SPOOKED: u32 = 140;

/// Where a pod rides a bow, in metres: how far ahead of the boat it holds
/// station, and how far out to the side. Ahead, because the whole of what is
/// being ridden is the water a hull is pushing in front of itself, and a pod
/// astern of one is following a boat rather than riding it. To the side,
/// because a pod is drawn as several animals arranged around the one point the
/// wire carries — see [`ToClient::Beast`] — so a station dead on the bow puts
/// half of them under the keel.
const BOW: (f32, f32) = (10.0, 4.0);

/// How fast a player has to be going to have a bow worth riding, in metres a
/// second. A boat at anchor makes no water, and a pod mobbing one that is
/// simply sitting there does not read as playful — it reads as broken, and as
/// the animals minding a player, which is the one thing this module keeps
/// saying they do not do.
const UNDERWAY: f32 = 1.5;

/// How long a pod stays on a bow, in beats, drawn per ride: half a minute to a
/// minute and a half.
///
/// Bounded, and this is the bound that matters most in the file. A stance that
/// closes the range can hold an animal somewhere until the player gets bored,
/// which is a pod that has quietly stopped crossing anywhere and become
/// scenery attached to a boat. Riding happens in the middle of a crossing and
/// then stops happening, so the crossing finishes.
const RIDE: (u32, u32) = (120, 360);

/// And how long before it will take an interest in a bow again, in beats. Long
/// enough to have got somewhere: at a pod's pace it is the better part of four
/// hundred metres, so a pod that peels off is a pod a player watches leave
/// rather than one that thinks better of it a few seconds later. It is also
/// what stops the bound above from being one ride in name and a permanent ride
/// in practice.
const ALOOF: u32 = 720;

/// How much of the gap to its station a riding pod makes up a second, as a
/// factor — see [`Riding::pace`]. Proportional rather than a sprint-then-settle
/// pair of speeds, because the fall to nothing at the end is what makes coming
/// alongside look like arriving rather than like stopping.
const CLOSING: f32 = 0.5;

/// Where and how one kind of beast lives. Every number that differs between
/// a shark, a pod and a whale is here, so the differences can be read in one
/// place — and so the life around them is written once.
struct Habitat {
    kind: BeastKind,
    /// The seafloor this kind calls home, in metres of terrain height: the
    /// water a beast is raised to be *in* and holds to once it is there. The
    /// shark's band is the sunlit strip where a beach shelves away, which is
    /// where the players are; dolphins ask only for depth, whales for real
    /// depth.
    band: (f32, f32),
    /// And the water it will *cross* to get there, which is a different
    /// question and only differs for the shark: a shark is born in the deep
    /// and its home is the shallows, so it must be free to swim water it
    /// could not live in. A traveller's is its own band, because a whale
    /// routes round a reef rather than over it — the journey is the animal,
    /// and taking a short cut through a shark's water would be some other one.
    crossing: (f32, f32),
    /// Cruising pace, metres per second. A shark ambles. The other two are
    /// tuning as much as character: the client pitches a porpoising body by
    /// its leap *against this speed*, so a slower whale is a steeper,
    /// spy-hopping whale.
    cruise: f32,
    /// How many of this kind are kept in the waters around each player.
    /// Sharks come in twos — the second is what makes the first read as
    /// *sharks live here* rather than as a one-off — while a pod or a whale
    /// is an event, and two events at once read as an aquarium.
    about: usize,
    /// How far around a player counts as their waters, in metres: this kind is
    /// counted and raised inside this reach, and a beast with nobody inside it
    /// is one nobody can see — which is what [`FORGOTTEN_AFTER`] eventually
    /// does something about. Wider for the animals meant to be met seldom.
    /// Nothing here models an animal no player could ever meet, which is the
    /// whole reason a session can afford to run these at all.
    ///
    /// For a traveller it has a second, binding job: the far end of a journey
    /// has to fall inside it, or the animal is dropped mid-crossing rather
    /// than finishing it. See [`Habitat::journey`].
    waters: f32,
    /// The ring a newcomer is raised on, in metres from the player it is
    /// raised for: outside the view at any ordinary zoom, inside `waters`. It
    /// is where a traveller is *born* and where a shark's home is looked for
    /// — see [`raise`], which is the one place that difference lives.
    ring: (f32, f32),
    /// How far this kind's one journey runs, in metres — `None` for a kind
    /// that is not passing through.
    ///
    /// This is most of what a whale or a pod *is*, and the whole of how long
    /// one lasts: it is born, it swims this far, it goes down. The only thing
    /// about it that is about the player is that the course is laid to pass
    /// near whoever it was raised for, an animal crossing out of sight being
    /// one nobody meets. The shark journeys nowhere — see
    /// [`Habitat::meander`].
    ///
    /// How far it may run is not free: a journey is only a life if the animal
    /// gets to the end of it, so the far end has to lie inside the `waters`
    /// that keep it minded. Born `r` out on the `ring` and aimed at a point
    /// `a` abeam of the player, an animal that swims `out` metres finishes at
    /// `√(r²(out/L − 1)² + (out·a/L)²)` from where the player was, with
    /// `L = √(r² + a²)` — at worst 774 m for a pod and 1062 m for a whale,
    /// both comfortably inside their waters. Journeys that did not fit were
    /// the first version of this, and two crossings in five ended in the
    /// animal being forgotten mid-passage instead of arriving.
    journey: Option<(f32, f32)>,
    /// How long one of these lives, in beats, drawn per beast from this span.
    ///
    /// The shark's is its life outright: long enough to be met, sailed past
    /// and left behind, short enough that a player who anchors for an
    /// afternoon watches the sea change its cast rather than keeping the one
    /// it opened with. A traveller's is only the backstop the [`Beast::life`]
    /// field describes, and so is set at a comfortable multiple of what its
    /// journey ought to take.
    life: (u32, u32),
    /// How hard this kind's dwelling arcs, in radians a beat: the bias it
    /// holds for [`ARC`] beats at a stretch before drawing another. This is
    /// the difference between an animal *searching* its water and one
    /// crossing it — the shark's is what makes it read as snooping the
    /// shallows, and the travelling kinds have next to none.
    meander: f32,
}

const HABITATS: [Habitat; 3] = [
    Habitat {
        kind: BeastKind::Shark,
        // The top of the band is three wade-depths of water: a shark holds
        // a little off where a player can stand, close enough to circle a
        // swimmer the moment there is such a thing as swimming.
        band: (-6.0, -1.5),
        // The one kind whose home is not the water it is born in, so the one
        // kind that has to be free to cross anything wet on its way there.
        crossing: SWIMMABLE,
        cruise: 1.4,
        about: 2,
        waters: 450.0,
        ring: (350.0, 450.0),
        journey: None,
        life: (1_400, 2_400),
        meander: 0.05,
    },
    Habitat {
        kind: BeastKind::Dolphins,
        band: (f32::NEG_INFINITY, -4.0),
        crossing: (f32::NEG_INFINITY, -4.0),
        cruise: 4.5,
        about: 1,
        waters: 900.0,
        ring: (400.0, 650.0),
        // Comfortably past the ring it is born on, so the course runs by the
        // player it was raised for and out the other side, and short enough of
        // the waters above that the far end is still inside them. At a pod's
        // pace the shortest of these crossings is three minutes and the
        // longest four.
        journey: Some((800.0, 1_100.0)),
        life: (1_400, 2_000),
        meander: 0.008,
    },
    Habitat {
        kind: BeastKind::Whale,
        band: (f32::NEG_INFINITY, -7.0),
        crossing: (f32::NEG_INFINITY, -7.0),
        cruise: 2.2,
        about: 1,
        waters: 1_300.0,
        ring: (500.0, 900.0),
        journey: Some((1_000.0, 1_500.0)),
        // Twice what the longest crossing takes a whale, which is the slowest
        // traveller here: fifteen hundred metres at 2.2 m/s is the better part
        // of three thousand beats, and a backstop shorter than the journey is
        // not a backstop but a second, quieter way for a whale to end — one in
        // ten of them going down mid-passage for no reason a player could see.
        life: (5_400, 7_200),
        meander: 0.004,
    },
];

/// A player as a beat finds them: where they are, and the way they are making.
///
/// The wire says only the first half — `ToServer::Move` carries a position and
/// nothing else — so the second is worked out here, from where the same player
/// was a beat ago. That is enough: a beat is four position reports, so somebody
/// under way has moved a metre or two by the time anything asks.
///
/// What it is *for* is [`Regard::Play`], which needs to know where a bow is,
/// and a position alone cannot say — a boat has a front only if it is going
/// somewhere.
struct Wake {
    position: Vec2,
    /// The course made good over the last beat, as a unit vector. Whatever
    /// bearing a standing player last had, which is nobody's business while
    /// they stand: `speed` is what says whether this means anything.
    course: Vec2,
    /// And how fast, in metres a second. Zero for a player a beat has not seen
    /// move, and for one it is seeing for the first time.
    speed: f32,
}

impl Wake {
    /// Whether they are moving enough to have a bow — see [`UNDERWAY`].
    fn making_way(&self) -> bool {
        self.speed >= UNDERWAY
    }

    /// Where a pod rides this bow, on one side or the other — see [`BOW`].
    /// Ahead by the boat's own course, which is the only reason the course is
    /// worked out at all.
    fn bow(&self, side: f32) -> Vec2 {
        self.position + self.course * BOW.0 + self.course.perp() * side * BOW.1
    }
}

/// What one kind of beast makes of one thing it has met.
///
/// The reach lives in the stance rather than on the kind: a whale gives a hull
/// forty-five metres and another whale nothing at all, so "how far off does
/// this animal start caring" has one answer per pairing and not per kind.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Regard {
    /// Nothing at all: as far as this animal is concerned that is water, and
    /// the course it was already swimming is the course it swims.
    Ignore,
    /// Pass it wide, noticing it this many metres off — [`skirt`], which holds
    /// the course and bends it round rather than breaking off.
    ///
    /// Not shyness, and the difference decides the steering: these animals are
    /// going somewhere and a boat is in the way, so what they do about one is
    /// *avoid contact* — hold the course and pass it wide — rather than break
    /// off and flee, which is what turning straight away from a hull read as
    /// when it was tried. The pass is aimed to clear [`BERTH`] of the reach.
    ///
    /// In no cell at the moment, unlike `Harry`, this one is written and
    /// working: every `Play` beast falls back on it the moment there is no bow
    /// to ride.
    #[allow(dead_code)]
    Berth(f32),
    /// Go under while it is inside this many metres, and pass wide of it as
    /// well — the whale's answer, and [`breathe`]'s business.
    ///
    /// Both halves, deliberately: a stance that only sounded would be a whale
    /// going down and then swimming its line straight at the boat on top of
    /// it. Near enough to be worth avoiding is near enough to go down for, so
    /// it is one reach and not two.
    Sound(f32),
    /// Close on it and stay with it while it is inside this many metres: a pod
    /// running a shark out of its water, and one day a shark working a swimmer.
    ///
    /// Not written, because nothing in [`REGARDS`] asks for it yet — see there
    /// for what standing it up needs.
    #[allow(dead_code)]
    Harry(f32),
    /// Come *to* it while it is inside this many metres and keep it company:
    /// dolphins on a bow wave, and the one entry in this table that closes the
    /// range for the pleasure of it.
    ///
    /// The only stance that takes over where an animal is going rather than
    /// bending it — see [`ride`] — and so the only one with a clock on it:
    /// everything else is a swing applied to a course the animal keeps, while
    /// this one *is* the course while it lasts. When it is not riding, a
    /// `Play` beast passes a hull the way a `Berth` one does.
    Play(f32),
}

/// Something a beast can meet: a column of [`REGARDS`].
///
/// The three kinds come first and in [`HABITATS`] order, so the top-left block
/// of the table is what the beasts make of each other and a kind's row lines up
/// with its own column.
///
/// `Afloat` and `Swimming` are one player in two states, and the wire cannot
/// yet tell them apart. They are kept apart here regardless, because what a
/// shark makes of a swimmer is most of why beasts are on the server at all.
/// Until a client says how its player is travelling, every player is read as
/// [`A_PLAYER`].
#[derive(Clone, Copy, PartialEq, Debug)]
enum Met {
    Shark,
    Dolphins,
    Whale,
    /// A player on a hull.
    Afloat,
    /// A player in the water.
    Swimming,
}

impl Met {
    /// Everything there is to meet, in column order: what sets the table's
    /// width, and what the totality test walks.
    const ALL: [Met; 5] = [
        Met::Shark,
        Met::Dolphins,
        Met::Whale,
        Met::Afloat,
        Met::Swimming,
    ];

    /// Which column of [`REGARDS`] holds what a beast makes of this.
    fn column(self) -> usize {
        match self {
            Met::Shark => 0,
            Met::Dolphins => 1,
            Met::Whale => 2,
            Met::Afloat => 3,
            Met::Swimming => 4,
        }
    }
}

/// What a player is taken to be, until the wire can say which they are — see
/// [`Met`]. The one place that pretence lives, so the day a client reports how
/// its player is travelling is a change here rather than to the table.
const A_PLAYER: Met = Met::Afloat;

/// What each kind of beast makes of each kind of thing it can meet: a row per
/// kind in [`HABITATS`] order, a column per [`Met`]. The row is who is
/// deciding; the column is what they have come across.
///
/// It is asymmetric on purpose and has to stay so: dolphins run a shark out of
/// their water while a shark gives a pod a wide berth, which is two answers to
/// one pairing that no single number held *between* two kinds could say.
///
/// The reaches are in metres, and the pod's is deliberately only a few boat
/// lengths. Widening it when `Play` arrived was tried — fifty metres reads
/// better as the range you notice a bow wave from, but the same number is
/// what the give-way half promises to clear by [`BERTH`], and against a
/// course laid at the hull a pod could only make fourteen of the twenty-five.
/// A reach that keeps one of its two promises is worse than a close one that
/// keeps both. The shark's is `Ignore` pointedly: that a shark gives you no
/// room is the first thing a player learns about one.
///
/// The nine cells of the top-left block — what one beast makes of another —
/// are all `Ignore`, and filling them in is not a matter of writing stances
/// in. There is no still picture of what is *in* the water the way
/// [`Flock::beat`] takes one of where the players are, and `Harry` and `Play`
/// both close the range, so two animals with stances on each other make a
/// chase that drags both across the map. `Play` is written and `Harry` is not
/// because a ride closes on a *player*, who has no stance and cannot close
/// back — so [`RIDE`] only has to stop a pod forgetting its crossing.
#[rustfmt::skip]
const REGARDS: [[Regard; Met::ALL.len()]; HABITATS.len()] = {
    use Regard::{Ignore, Play, Sound};
    [
        //              shark    pod      whale    afloat       swimming
        /* shark */   [ Ignore,  Ignore,  Ignore,  Ignore,      Ignore      ],
        /* pod   */   [ Ignore,  Ignore,  Ignore,  Play(25.0),  Play(25.0)  ],
        /* whale */   [ Ignore,  Ignore,  Ignore,  Sound(45.0), Sound(45.0) ],
    ]
};

impl Regard {
    /// What a beast of a kind makes of something it has met.
    fn of(kind: BeastKind, met: Met) -> Self {
        REGARDS[Self::row(kind)][met.column()]
    }

    /// Which row of [`REGARDS`] holds a kind's opinions. A match rather than a
    /// scan of [`HABITATS`], so that a fourth kind of beast is a compile error
    /// here rather than a row somebody has to notice is missing — and the
    /// totality test is what holds the two orders together.
    fn row(kind: BeastKind) -> usize {
        match kind {
            BeastKind::Shark => 0,
            BeastKind::Dolphins => 1,
            BeastKind::Whale => 2,
        }
    }

    /// How far off this stance starts passing wide, in metres — `None` where it
    /// does no such thing. `Sound` answers here as well as `Berth`; the reason
    /// is written where `Sound` is.
    fn berth(self) -> Option<f32> {
        match self {
            Self::Berth(reach) | Self::Sound(reach) | Self::Play(reach) => Some(reach),
            Self::Ignore => None,
            // Closing the range is the opposite of a berth, and this one is not
            // written. Nothing in the table asks for it, and a test says so,
            // which is what keeps this arm from being a stance that quietly
            // does nothing.
            Self::Harry(_) => None,
        }
    }

    /// How far off this stance goes to *keep company* with something, in metres
    /// — `None` for every stance that does not, which is all of them but the
    /// pod's. See [`ride`].
    fn playing(self) -> Option<f32> {
        match self {
            Self::Play(reach) => Some(reach),
            Self::Ignore | Self::Berth(_) | Self::Sound(_) | Self::Harry(_) => None,
        }
    }

    /// How far off this stance answers by going under, in metres — `None` for
    /// every stance that does not, which is every stance but the whale's.
    fn sounding(self) -> Option<f32> {
        match self {
            Self::Sound(reach) => Some(reach),
            Self::Ignore | Self::Berth(_) | Self::Harry(_) | Self::Play(_) => None,
        }
    }
}

/// Which part of its life a beast is in — see the module's opening, where
/// what each means is set out.
///
/// For a whale or a pod there are only two of these: it is bound somewhere,
/// and then it is leaving. Dwelling is the shark's, and is what reaching the
/// place it was going gets it instead of an ending.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Doing {
    Bound,
    Dwelling,
    Leaving,
}

/// A bow a pod has taken up station on — see [`ride`], which is the only thing
/// that makes one and the only thing that reads one.
struct Riding {
    /// Which side of the bow it took, as a sign, and holds for the whole ride.
    /// Chosen once because a pod that reconsiders every beat is not a pod
    /// changing its mind, it is a pod flickering from one side to the other.
    side: f32,
    /// Beats left on this bow, drawn from [`RIDE`] when it came alongside.
    left: u32,
    /// How fast to swim this beat, in metres a second: the boat's own speed,
    /// plus as much again as it takes to make up whatever gap is left to the
    /// station — see [`CLOSING`].
    ///
    /// The boat's speed *alone* was the first version and could not work: a pod
    /// that matches a boat the instant it decides to ride holds whatever gap it
    /// happened to notice the boat at, forever. The closing term falls to
    /// nothing exactly as it arrives, so the two are one number rather than a
    /// sprint and a mode change.
    pace: f32,
}

/// One beast, as the server minds it.
///
/// It carries its [`Habitat`] rather than its kind, the habitat holding the
/// kind: every beat asks each beast where it lives several times over, and
/// looking that up by kind was a scan of the table per beast per question.
struct Beast {
    habitat: &'static Habitat,
    position: Vec2,
    /// Which way it is swimming, as a unit vector. Pace is the habitat's
    /// business and its part of life's, so bearing is the whole of the state
    /// a cruise needs.
    heading: Vec2,
    doing: Doing,
    /// The one place it is going, while it is still going anywhere. A want
    /// and not a route — nothing about how it gets there is decided here, and
    /// the water it meets on the way outranks it every beat. A leaving beast
    /// keeps none: deeper is a thing the floor under the animal answers on its
    /// own, and a dwelling shark is already where it was going.
    goal: Option<Vec2>,
    /// Beats since it was raised, and the beats it has to live.
    ///
    /// For a shark that is its life. For a traveller it is only a backstop —
    /// the journey is what ends a whale, and this ends one that has spent an
    /// implausible while failing to make it, which is a whale in a bay it
    /// cannot find the mouth of.
    age: u32,
    life: u32,
    /// Beats since it last changed what it is doing — how long a leaving has
    /// been going on, which is the whole of what anything asks it.
    since: u32,
    /// Whether its back is out of the water, which is the one thing beyond
    /// place and bearing a client is told. Anything on its way out is under;
    /// so is a whale that has sounded, which is [`breathe`]'s business; every
    /// other animal, going about its business, is up.
    surfaced: bool,
    /// Beats until the whale changes its mind about that — see [`breathe`].
    bout: u32,
    /// Beats since anybody was last within its kind's `waters`. Running past
    /// [`FORGOTTEN_AFTER`] is the sea getting on without it — which it does by
    /// sending the animal down, not by taking it away. It goes on counting
    /// after that and nothing reads it.
    unminded: u32,
    /// The lazy arc it is currently swimming, in radians a beat, redrawn
    /// every [`ARC`] beats of dwelling from its habitat's `meander`.
    turning: f32,
    /// The bow it is riding, if it is riding one — see [`ride`]. The one piece
    /// of state here belonging to a stance rather than to a life, and
    /// deliberately kept out of [`Doing`]: a riding pod is still bound where it
    /// was bound and still ends where it was going to end.
    riding: Option<Riding>,
    /// Beats before it will look at another bow — see [`ALOOF`]. Counts down
    /// whether or not there is anything to ride, so a pod that peels off in
    /// open water is as done with riding as one that peels off beside a boat.
    aloof: u32,
}

impl Beast {
    /// A newcomer, born at `at` and bound for `toward` — see [`raise`], which
    /// is where both of those are decided and what they mean to each kind.
    ///
    /// It arrives already swimming, on a bearing drawn with its spot when it
    /// has nowhere in particular to be: a beast is never doing nothing.
    fn born(habitat: &'static Habitat, at: Vec2, toward: Option<Vec2>, entropy: u32) -> Self {
        let heading = toward
            .map(|goal| (goal - at).normalize_or(Vec2::X))
            .unwrap_or_else(|| Vec2::from_angle(unit(entropy, 0xF1_5B) * TAU));
        Self {
            habitat,
            position: at,
            heading,
            doing: Doing::Bound,
            goal: toward,
            age: 0,
            life: span(entropy, 0x11FE, habitat.life),
            since: 0,
            // A traveller is itself from the first beat, its journey being its
            // life. The shark comes in under the water, its swim in from the
            // deep being a commute nobody is meant to watch.
            surfaced: habitat.journey.is_some(),
            bout: span(entropy, 0xB0_07, SURFACED_BOUT),
            unminded: 0,
            turning: 0.0,
            riding: None,
            aloof: 0,
        }
    }

    /// One the console summoned, which is a different animal in exactly one
    /// way: it is wanted *seen*, right there, rather than met on its way
    /// somewhere. It is simply already where it was going — see [`conjure`].
    fn summoned(habitat: &'static Habitat, at: Vec2, entropy: u32) -> Self {
        let mut beast = Self::born(habitat, at, None, entropy);
        beast.settle(Doing::Dwelling, entropy);
        beast
    }

    /// Moves on to another part of life, which is also where the surfacing is
    /// decided: an animal on its way out is under the water, and one starting
    /// anything else is up.
    fn settle(&mut self, doing: Doing, entropy: u32) {
        self.doing = doing;
        self.since = 0;
        self.surfaced = doing != Doing::Leaving;
        if self.surfaced {
            self.bout = span(entropy, 0xB0_07, SURFACED_BOUT);
        }
        // Whatever it was making for, it is not making for it any more: a
        // shark that has arrived is standing in it, and nothing else here has
        // anywhere further to be.
        self.goal = None;
        // And whatever it was riding, it has other business now: leaving is
        // done under the water and away from boats.
        self.riding = None;
    }

    /// The water this beast is holding to right now: what it lives in once it
    /// is living somewhere, and what it will *cross* while it is on its way —
    /// which for a shark is any water at all, the deep between its birth and
    /// its coast included, and for a traveller is its own water, since a whale
    /// routes round a reef rather than over it.
    fn band(&self) -> (f32, f32) {
        match self.doing {
            Doing::Dwelling => self.habitat.band,
            Doing::Bound => self.habitat.crossing,
            Doing::Leaving => SWIMMABLE,
        }
    }

    /// Metres per second. The distinction is not which part of life this is
    /// but whether the swimming is the animal *being itself*, at the pace its
    /// kind was drawn at, or merely getting somewhere, which is nobody's
    /// spectacle and better over with.
    ///
    /// A pod on a bow is the exception and matches the boat — see
    /// [`Riding::pace`] — held to what the animal could actually swim. A boat
    /// that outruns a pod leaves it behind, which [`ride`]'s range check then
    /// reads as the ride being over.
    fn pace(&self) -> f32 {
        if let Some(riding) = &self.riding {
            return riding.pace.min(self.habitat.cruise * TRANSIT);
        }
        match (self.doing, self.habitat.journey) {
            // A traveller's journey *is* its life, and a dwelling shark is
            // living in its water.
            (Doing::Bound, Some(_)) | (Doing::Dwelling, _) => self.habitat.cruise,
            // A shark's commute in from the deep, and anything's exit: both
            // happen under the water with nobody watching.
            (Doing::Bound, None) | (Doing::Leaving, _) => self.habitat.cruise * TRANSIT,
        }
    }
}

fn habitat_of(kind: BeastKind) -> &'static Habitat {
    HABITATS
        .iter()
        .find(|habitat| habitat.kind == kind)
        .expect("every kind of beast has a habitat")
}

/// Finds water a summoned beast could be raised in near a point — the
/// console's `spawn`, which is what [`Shared::summoned`] carries the answers
/// of. Unlike [`raise`], which sends its newcomers in from deep water so they
/// are discovered rather than watched appearing, a summons wants the animal
/// *seen*: this sounds outward from close by, so the reply can say how far
/// away the animal is.
///
/// The kind's own water is preferred and any water will do, which is the
/// console being a console: a whale summoned into a lagoon is a whale to look
/// at, and [`swim`]'s band correction works it back out within a few beats.
/// Only somewhere with no water at all is refused.
pub(crate) fn conjure(shared: &Shared, kind: BeastKind, near: Vec2, entropy: u32) -> Option<Vec2> {
    let habitat = habitat_of(kind);
    sound_out(shared, near, entropy, &SUMMONS_RINGS, habitat.band)
        .or_else(|| sound_out(shared, near, entropy, &SUMMONS_RINGS, SWIMMABLE))
}

/// Water of some band near a point: eight bearings on each of a set of rings,
/// widening, the first that lands in the band winning. Both the bearing it
/// starts at and how far out each ring actually falls come from the bits, so
/// a hundred calls in one breath scatter over the water around a point rather
/// than beading it onto circles. `None` where every sounding found the wrong
/// water or ground nobody has been near enough to generate.
fn sound_out(
    shared: &Shared,
    near: Vec2,
    entropy: u32,
    rings: &[f32],
    band: (f32, f32),
) -> Option<Vec2> {
    for (ring, out) in rings.iter().enumerate() {
        let ring = ring as u32;
        let entered = unit(entropy, ring) * TAU;
        let out = out * (0.55 + 0.9 * unit(entropy, ring ^ 0x0D15));
        for step in 0..8 {
            let bearing = entered + step as f32 * (TAU / 8.0);
            let spot = near + Vec2::from_angle(bearing) * out;
            if floor_in(shared, spot, band) {
                return Some(spot);
            }
        }
    }
    None
}

/// Minds the beasts on a thread of its own: raises them where the players
/// are, swims them, lets them go, and tells the whole roster where they stand
/// on every beat.
///
/// The flock is owned by this thread outright rather than shared — nothing
/// else needs to read it, [`ToClient::Beast`] being an upsert that catches a
/// newly joined client up on the next beat. That is what keeps this file free
/// of locks of its own.
///
/// The thread ends with the session and is deliberately not joined, on the
/// same terms the sky thread lives on.
pub(crate) fn mind_the_beasts(shared: &Arc<Shared>) {
    let shared = shared.clone();
    thread::spawn(move || {
        // Stirred into every roll the beasts make, so two sessions on one seed
        // do not raise their animals on the same bearings. Decorative: nothing
        // here needs to agree with another machine, the tellings being the
        // agreement.
        let mut flock = Flock::new(shared.world.seed());

        // The beasts the world's file remembered, taken back up before the
        // first beat. Read rather than drained: the ledger keeps saying what
        // it said until the first beat rewrites it, so a save landing in the
        // beat between reopening and here still writes the beasts down.
        {
            let remembered = shared.beasts.held().clone();
            for record in remembered {
                flock.adopt(&record);
            }
        }

        loop {
            if shared.stopping.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            thread::sleep(BEAST_TICK);

            // Gathered before anything is decided, and the lock dropped
            // before any terrain is sounded: the beat works from one still
            // picture of where everyone is, and never holds the session up
            // while it thinks.
            let players: Vec<(PlayerId, Vec2)> = {
                let players = shared.players.held();
                players
                    .iter()
                    .map(|(id, player)| (*id, player.position))
                    .collect()
            };
            let summoned: Vec<(BeastKind, Vec2)> = shared.summoned.held().drain(..).collect();

            let news = flock.beat(&shared, &players, &summoned);

            // The ledger a save reads, rewritten while no other lock is
            // held: the flock stays this thread's own, and what everyone
            // else sees is a summary from at most a beat ago.
            *shared.beasts.held() = flock.records();

            let players = shared.players.held();
            for word in news {
                broadcast_all(&players, word);
            }
        }
    });
}

/// Every beast there is, and the bits the next decision about one comes out of.
/// Split from the thread above so a beat can be called a thousand times in a
/// test rather than only while a real quarter-second goes by.
struct Flock {
    beasts: HashMap<BeastId, Beast>,
    next_id: u32,
    entropy: u32,
    /// Where everyone was a beat ago, which is the whole of how a course is
    /// arrived at — see [`Wake`]. Kept here rather than on the roster, being
    /// nobody else's business.
    ///
    /// Trimmed to whoever is still connected on every beat, so a player who
    /// leaves and comes back is a stranger again rather than somebody who has
    /// apparently just travelled a very long way in a quarter of a second.
    wakes: HashMap<PlayerId, Vec2>,
}

impl Flock {
    fn new(seed: u32) -> Self {
        Self {
            beasts: HashMap::new(),
            next_id: 0,
            entropy: scramble(seed),
            wakes: HashMap::new(),
        }
    }

    /// One beat of minding every beast, and what the roster is to be told of
    /// it. Everything the flock does happens here, in an order that is itself
    /// a decision: swim, then let go, then raise — so a beast that has just
    /// left is not told about once more, and one raised this beat is told
    /// about by the loop at the bottom rather than trusted to a later one.
    fn beat(
        &mut self,
        shared: &Shared,
        afloat: &[(PlayerId, Vec2)],
        summoned: &[(BeastKind, Vec2)],
    ) -> Vec<ToClient> {
        // A world with nobody in it keeps no beasts: there is nothing for
        // them to matter to, and nobody to tell. Quietly, with no parting
        // words — a departure emptied the roster, so there is no one left to
        // hear a `BeastGone`.
        if afloat.is_empty() {
            self.beasts.clear();
            self.wakes.clear();
            return Vec::new();
        }

        // Everyone, with the way they are making worked out from where they
        // were when this last ran. A newcomer, having no last time, stands
        // still until the next beat has an opinion — a quarter second of a pod
        // not knowing there is a bow there, and nobody's loss.
        let players: Vec<Wake> = afloat
            .iter()
            .map(|(id, position)| {
                let went = self.wakes.get(id).map(|was| *position - *was);
                let (course, speed) = match went {
                    Some(went) if went.length() > f32::EPSILON => {
                        (went.normalize(), went.length() / BEAST_TICK.as_secs_f32())
                    }
                    _ => (Vec2::X, 0.0),
                };
                Wake {
                    position: *position,
                    course,
                    speed,
                }
            })
            .collect();
        self.wakes = afloat.iter().copied().collect();
        let players = &players[..];

        let mut news: Vec<ToClient> = Vec::new();

        for (kind, spot) in summoned {
            let entropy = self.roll();
            let beast = Beast::summoned(habitat_of(*kind), *spot, entropy);
            self.keep(beast);
        }

        // In id order rather than the map's own, which differs from run to
        // run. Nothing about a beast has to agree with another machine, but
        // the entropy below is chained from beast to beast, so the map's order
        // would decide which animal got which bits and a test that failed
        // under one process order could not be re-run under it.
        let mut minding: Vec<BeastId> = self.beasts.keys().copied().collect();
        minding.sort_unstable_by_key(|id| id.0);

        let mut entropy = self.entropy;
        let mut done: Vec<BeastId> = Vec::new();
        for id in minding {
            let beast = self.beasts.get_mut(&id).expect("a beast just listed");
            entropy = scramble(entropy);
            swim(beast, shared, players, entropy);
            entropy = scramble(entropy);
            if !mind(beast, shared, players, entropy) {
                done.push(id);
                continue;
            }

            // The other way of being over, and it is not an ending so much as
            // the sea getting on without one: nobody has been near this animal
            // for long enough that swimming it is arithmetic about something
            // no player could see.
            //
            // Counted in beats rather than measured in metres so that it has
            // some patience in it: a player tacking along a coast leaves and
            // re-enters an animal's waters constantly, and a rule with no
            // hysteresis would spend that whole time letting the same shark go
            // and raising it again.
            //
            // What it does *not* do is drop the animal where it stands. It
            // goes down first, and [`mind`] hands it over once it is under and
            // deep or once [`LEAVING_PATIENCE`] is out. Dropping it in place
            // was the first version, and it cost a surfaced pod blinking out
            // in plain sight.
            let watched = players
                .iter()
                .any(|player| player.position.distance(beast.position) <= beast.habitat.waters);
            beast.unminded = if watched { 0 } else { beast.unminded + 1 };
            if beast.unminded >= FORGOTTEN_AFTER && beast.doing != Doing::Leaving {
                entropy = scramble(entropy);
                beast.settle(Doing::Leaving, entropy);
            }
        }
        self.entropy = entropy;

        // A beast has finished — its journey, or its life, or the leaving
        // either of those ends in. Not a death, and the wire has one word for
        // all of it.
        for id in &done {
            self.beasts.remove(id);
            news.push(ToClient::BeastGone { id: *id });
        }

        for player in players {
            let player = player.position;
            for habitat in &HABITATS {
                // A beast on its way out is not counted: its replacement is
                // owed now, so the waters turn over instead of thinning for
                // as long as a departure takes.
                let about = self
                    .beasts
                    .values()
                    .filter(|beast| beast.habitat.kind == habitat.kind)
                    .filter(|beast| beast.doing != Doing::Leaving)
                    .filter(|beast| beast.position.distance(player) <= habitat.waters)
                    .count();
                for _ in about..habitat.about {
                    let entropy = self.roll();
                    if let Some(beast) = raise(habitat, shared, player, entropy) {
                        self.keep(beast);
                    }
                }
            }
        }

        // In id order, for the same reason the minding above is in it. A beat
        // tells the whole flock at once, so this order is the order a client
        // reads the sea in — and a test that asks what the next whale is is
        // asking the map's hashing under one that runs from the map's own.
        // Nothing a client *draws* turns on it, a telling being an upsert.
        let mut telling: Vec<BeastId> = self.beasts.keys().copied().collect();
        telling.sort_unstable_by_key(|id| id.0);
        for id in telling {
            let beast = &self.beasts[&id];
            news.push(ToClient::Beast {
                id,
                kind: beast.habitat.kind,
                position: beast.position,
                velocity: beast.heading * beast.pace(),
                surfaced: beast.surfaced,
            });
        }

        news
    }

    /// Takes a newcomer onto the roster under the next id there is.
    fn keep(&mut self, beast: Beast) -> BeastId {
        let id = BeastId(self.next_id);
        self.beasts.insert(id, beast);
        self.next_id += 1;
        id
    }

    /// Takes back up a beast the world's file remembered — the reverse of
    /// [`Flock::records`]. One still bound somewhere is rebuilt as a raise
    /// builds one, still making for its goal; one living where it stood as a
    /// summons builds one, already arrived. Everything re-derived each beat
    /// starts over, which nothing can see.
    fn adopt(&mut self, record: &crate::keeper::BeastRecord) {
        let habitat = habitat_of(record.kind);
        let entropy = self.roll();
        let mut beast = match record.goal {
            Some(goal) => Beast::born(habitat, record.position, Some(goal), entropy),
            None => Beast::summoned(habitat, record.position, entropy),
        };
        // The life it had left is the life it gets: one life, however many
        // sessions it spans. A beast restored with nothing left simply
        // leaves on the first beat, which is the honest answer.
        beast.life = record.left;
        self.keep(beast);
    }

    /// The flock as a file writes it down — the reverse of [`Flock::adopt`].
    /// A leaving beast is left out: an exit is finished under the water
    /// where nobody can watch it, so a world reopened without one reads
    /// exactly as a world it had already left.
    ///
    /// So is one that has got out past the world's own edge, or that is making
    /// for somewhere past it. A file may only say what it can be read back
    /// saying, and a world that wrote one down would be a world that would not
    /// open again — far worse than a shark nobody sees leave. The guarantee
    /// rather than the fix: [`raise`] no longer makes one, but a beast that
    /// swam out under its own steam would be just as unreadable.
    fn records(&self) -> Vec<crate::keeper::BeastRecord> {
        self.beasts
            .values()
            .filter(|beast| beast.doing != Doing::Leaving)
            .filter(|beast| {
                crate::reachable(beast.position) && beast.goal.is_none_or(crate::reachable)
            })
            .map(|beast| crate::keeper::BeastRecord {
                kind: beast.habitat.kind,
                position: beast.position,
                goal: beast.goal,
                left: beast.life.saturating_sub(beast.age),
            })
            .collect()
    }

    /// Stirs the flock's bits on and hands back the new ones.
    fn roll(&mut self) -> u32 {
        self.entropy = scramble(self.entropy);
        self.entropy
    }
}

/// One beat of being somewhere: keep swimming, hold to the water you are in
/// for now, make for wherever this part of your life is making for, and — for
/// the kinds that yield — bear away from boats.
///
/// Each beat it sounds the bottom a few seconds ahead. Water it can be in
/// means bearing on with only the wander. Water it cannot means trying
/// bearings further and further off the current one, alternating sides so it
/// has no favourite; a beast boxed in on every sounding simply holds course
/// this beat and tries again on the next, a beat being a fraction of a metre.
///
/// The order of the three swings matters and is not the order they read in.
/// The band correction comes last on purpose: a whale gives a boat a berth,
/// but never onto ground that is not a whale's. And wanting to be somewhere
/// never overrides either, which is what stops an animal swimming into a
/// headland because the far end of its journey lies beyond it.
///
/// One stance does not fit that shape, which is why the want below is worked
/// out as a *place to want* rather than applied where it is decided: a pod
/// riding a bow ([`ride`]) replaces where it is going instead of bending how
/// it gets there. It has to — a swing added to the goal want would be a swing
/// fighting it, both clamped alike, and the animal would make about half of
/// whichever it wanted more.
fn swim(beast: &mut Beast, shared: &Shared, players: &[Wake], entropy: u32) {
    let habitat = beast.habitat;

    // The wander: a few degrees a beat, either way, always applied — it is
    // what keeps a long straight reach from reading as a bearing being held.
    // The arc on top of it is the dwelling animal's own, and held for long
    // enough to be a shape rather than more noise.
    let wander = (unit(entropy, 0x5EA5) - 0.5) * 0.12;
    let arc = if beast.doing == Doing::Dwelling {
        beast.turning
    } else {
        0.0
    };
    beast.heading = turned(beast.heading, wander + arc);

    // The stance: what this kind makes of what it has met, which for now is a
    // player and nothing else — see [`REGARDS`].
    let regard = Regard::of(habitat.kind, A_PLAYER);
    let station = ride(beast, players, regard, entropy);

    // Where it wants to go, if it wants anywhere: the bow it has taken up on,
    // or whatever it is making for, or — for one on its way out — whichever
    // sounding found the deepest floor, which is a thing the water answers
    // rather than anything this file knows about the map.
    let want = station.or(match beast.doing {
        Doing::Leaving => Some(deeper(shared, beast)),
        Doing::Bound | Doing::Dwelling => beast
            .goal
            .map(|goal| (goal - beast.position).normalize_or(beast.heading)),
    });
    if let Some(want) = want {
        let off = beast.heading.angle_to(want);
        beast.heading = turned(beast.heading, off.clamp(-0.25, 0.25));
    }

    // And the berth, where a regard that bends a course goes: after the want,
    // so what an animal has come across outranks where it was going, and
    // before the ground below, so nothing dodges onto a beach. Bounded to the
    // same turn per beat as everything else, so an avoidance reads as deciding
    // rather than as deflection, and skipped while riding — an animal that
    // came to a bow on purpose has nothing to say to a rule about keeping off
    // hulls.
    if let Some(reach) = regard.berth().filter(|_| beast.riding.is_none()) {
        if let Some(boat) = nearest_player(players, beast.position, reach) {
            let round = skirt(beast, boat.position, reach);
            let off = beast.heading.angle_to(round);
            beast.heading = turned(beast.heading, off.clamp(-0.25, 0.25));
        }
    }

    let band = beast.band();
    if !floor_in(shared, beast.position + beast.heading * SOUNDING, band) {
        for step in 1..=8 {
            // An eighth of a turn at a time, +1, -1, +2, -2, … out to a half
            // turn either way, so a beast working its way out of a bay has no
            // favourite hand. The halving is integer division on purpose:
            // floating it steps the sides half an eighth apart, which never
            // tries straight back the way it came.
            let side = if step % 2 == 1 { 1.0 } else { -1.0 };
            let off = side * ((step + 1) / 2) as f32 * FRAC_PI_4;
            let tried = turned(beast.heading, off);
            if floor_in(shared, beast.position + tried * SOUNDING, band) {
                beast.heading = tried;
                break;
            }
        }
    }

    beast.position += beast.heading * beast.pace() * BEAST_TICK.as_secs_f32();
}

/// One beat of *being* a beast rather than of swimming: age it, move it on
/// through its life where it has come to the end of a part, and let a whale
/// breathe. `false` where the beast is finished and the sea is to be emptier
/// by one.
fn mind(beast: &mut Beast, shared: &Shared, players: &[Wake], entropy: u32) -> bool {
    beast.age += 1;
    beast.since += 1;

    match beast.doing {
        // Getting there means two different things, and which one a kind
        // means is the whole difference between them. A traveller is done
        // when it reaches the point it was going to. A shark is *started*
        // when it reaches its water — the band and not the goal, because the
        // goal was only ever the nearest example of that water and the animal
        // may well have crossed better on the way in.
        Doing::Bound => {
            let done = match beast.habitat.journey {
                // A bound beast always has somewhere to be. One that somehow
                // had none, read as "arrived", would end silently where it
                // stood; read as "not yet" it falls to the backstop below,
                // which is the failure that can be seen.
                Some(_) => beast
                    .goal
                    .is_some_and(|goal| beast.position.distance(goal) < ARRIVED),
                None => floor_in(shared, beast.position, beast.habitat.band),
            };
            if done {
                let next = match beast.habitat.journey {
                    Some(_) => Doing::Leaving,
                    None => Doing::Dwelling,
                };
                beast.settle(next, entropy);
            } else if beast.age >= beast.life {
                // Whatever it was making for, it has spent a life failing to
                // get there — a whale in a bay it cannot find the mouth of.
                // It goes the way everything goes.
                beast.settle(Doing::Leaving, entropy);
            } else {
                breathe(beast, players, entropy);
            }
        }
        // The shark's own, and nothing else's: it lives here now, and what it
        // does with that is snoop — the arc [`swim`] walks it round — until
        // its life runs out.
        Doing::Dwelling => {
            if beast.age >= beast.life {
                beast.settle(Doing::Leaving, entropy);
            } else {
                if beast.age.is_multiple_of(ARC) {
                    beast.turning = (unit(entropy, 0xA5C) - 0.5) * 2.0 * beast.habitat.meander;
                }
                breathe(beast, players, entropy);
            }
        }
        // Gone once it is down and deep, or once it has been trying long
        // enough that where it is has stopped being the point.
        Doing::Leaving => {
            let deep = floor_in(shared, beast.position, (f32::NEG_INFINITY, DEEP));
            let slipped = beast.since >= SLIPPING && deep;
            return !(slipped || beast.since >= LEAVING_PATIENCE);
        }
    }
    true
}

/// The whale's breathing, which is the one thing about how these animals
/// carry themselves that a client cannot invent for itself — see
/// [`ToClient::Beast`]'s `surfaced`. Bouts of a long back at the surface and
/// longer stretches down, and the one answer a whale has to a boat that is
/// not simply steering round it: go under, and stay under while it is there.
///
/// Nothing else here has anything to say, and it is [`REGARDS`] that says so
/// rather than a kind named here. A shark is a fin whenever it is alive and a
/// pod arcs the whole way across; both of those are drawing, and this is
/// deliberately the only thing that is not.
fn breathe(beast: &mut Beast, players: &[Wake], entropy: u32) {
    let Some(reach) = Regard::of(beast.habitat.kind, A_PLAYER).sounding() else {
        return;
    };

    // The reach it sounds at is the reach it is already bending its course
    // round — one number, for the reason written on [`Regard::Sound`] — so a
    // boat closing on a whale gets one long back and then nothing.
    let boat = nearest_player(players, beast.position, reach).is_some();
    beast.bout = beast.bout.saturating_sub(1);

    if beast.surfaced {
        if boat {
            // Cut short, whatever was left of the bout: the dive is the whole
            // of what a whale has to say about a boat.
            beast.surfaced = false;
            beast.bout = SPOOKED;
        } else if beast.bout == 0 {
            beast.surfaced = false;
            beast.bout = span(entropy, 0x50, SOUNDED_BOUT);
        }
    } else if beast.bout == 0 {
        if boat {
            // It will not come up under a hull: down again for as long as
            // being driven down costs, and asked afresh at the end of that.
            beast.bout = SPOOKED;
        } else {
            beast.surfaced = true;
            beast.bout = span(entropy, 0x51, SURFACED_BOUT);
        }
    }
}

/// Where a traveller born at `from` is going: a journey's length off, along a
/// course laid to pass close by `player` rather than at them or away from
/// them. `None` where no try landed in water this kind would cross, which
/// leaves it to the next beat's attempt at raising one.
///
/// Aiming the course at the watcher is the one thing about these animals that
/// is *for* a player, and it is staging rather than behaviour: an animal
/// crossing the map is only met if the map it crosses is the one being looked
/// at. The offset abeam makes it a passing rather than a collision course.
fn journey(
    shared: &Shared,
    habitat: &Habitat,
    from: Vec2,
    player: Vec2,
    length: (f32, f32),
    entropy: u32,
) -> Option<Vec2> {
    (0..AIM_TRIES).find_map(|attempt| {
        let salt = attempt * 2;
        let along = (player - from).normalize_or(Vec2::X);
        let abeam = along.perp() * ABEAM * (unit(entropy, salt) - 0.5) * 2.0;
        let out = length.0 + unit(entropy, salt + 1) * (length.1 - length.0);
        let end = from + (player + abeam - from).normalize_or(along) * out;
        floor_in(shared, end, habitat.crossing).then_some(end)
    })
}

/// One beat of [`Regard::Play`]: the bearing towards the bow a pod is riding,
/// or `None` where it is not riding one — which is most beats of most pods.
///
/// This is the one stance that *closes* on a player, and the whole of what it
/// has to get right is stopping. A pod that comes to a bow and stays reads
/// beautifully for about a minute and then reads as an animal that has
/// forgotten what it was doing, because it has. So a ride is drawn a length
/// when it starts ([`RIDE`]), and afterwards the pod wants nothing to do with
/// bows for a good while ([`ALOOF`]).
///
/// It ends on any of four things, and the last three matter as much as the
/// clock: the beats run out, the player stops making way (see [`UNDERWAY`]),
/// the player outruns the pod and leaves it outside the reach it noticed them
/// at, or the animal stops being one with anything to play.
///
/// What it does *not* touch is where the pod was going: the goal stands
/// untouched, [`swim`] simply wants the bow more while there is one to want,
/// and the crossing picks up from wherever the pod has got to. That is why
/// this returns a bearing instead of setting one.
fn ride(beast: &mut Beast, players: &[Wake], regard: Regard, entropy: u32) -> Option<Vec2> {
    beast.aloof = beast.aloof.saturating_sub(1);

    // Not a kind that plays, or not in a part of life that has any business
    // playing: an animal on its way out is under the water and making for the
    // deep, and coming up alongside a hull would undo how things leave.
    let reach = regard.playing().filter(|_| beast.doing != Doing::Leaving);
    let Some(reach) = reach else {
        beast.riding = None;
        return None;
    };

    // The bow it is on, or the nearest one worth taking up: either way it has
    // to still be there, still be moving, and still be in reach.
    let boat = nearest_player(players, beast.position, reach).filter(|wake| wake.making_way());
    let Some(boat) = boat else {
        beast.riding = None;
        return None;
    };

    let riding = match beast.riding.take() {
        // The side is kept from when it came alongside; the pace is worked out
        // afresh below, because matching a boat means matching the boat it is
        // now rather than the boat it was a minute ago.
        Some(riding) if riding.left > 0 => Riding {
            left: riding.left - 1,
            ..riding
        },
        // A ride that has run its length, and the pod goes back to its
        // crossing — the beats of not wanting another bow start here, not when
        // it next happens past one.
        Some(_) => {
            beast.aloof = ALOOF;
            return None;
        }
        // A fresh one, if it is willing. The side is whichever the pod is
        // already on, so that coming to the bow is a matter of easing across
        // rather than of crossing the boat's track to get to the far side.
        None => {
            if beast.aloof > 0 {
                return None;
            }
            let side = if boat.course.perp_dot(beast.position - boat.position) >= 0.0 {
                1.0
            } else {
                -1.0
            };
            Riding {
                side,
                left: span(entropy, 0x80_D5, RIDE),
                pace: 0.0,
            }
        }
    };

    let station = boat.bow(riding.side);
    let gap = station.distance(beast.position);
    beast.riding = Some(Riding {
        pace: boat.speed + gap * CLOSING,
        ..riding
    });
    Some((station - beast.position).normalize_or(beast.heading))
}

/// The bearing that carries a beast *past* a boat rather than away from it:
/// the tangent of a circle of `berth` metres round the hull, taken on
/// whichever side the animal is already leaning.
///
/// This is the whole difference between an animal that is going somewhere and
/// one that minds you. Turning away from the hull, which is what this was
/// once, makes every encounter about the player: the animal breaks off, runs,
/// and comes back when you go, so a boat is a hole in the sea that pushes. A
/// tangent holds the course and bends it round, so what a player sees is a pod
/// that was always going to pass them and made room to do it.
///
/// A beast already inside the clearance cannot skirt what it is standing in,
/// and a beast the hull is behind has nothing to skirt: both keep the bearing
/// they had.
///
/// `reach` is the range the stance noticed the boat at, and the pass is aimed
/// to clear it by [`BERTH`] of that — noticing further out than it insists on
/// passing is what leaves room for the turn to be gradual. The swing is capped
/// at a quarter turn off the hull's own bearing, which is the widest a *pass*
/// can be; beyond that it is running rather than passing.
///
/// What comes back is a bearing to *want*, not one the animal takes: [`swim`]
/// clamps the swing towards it like any other. So the one course where the
/// clearance falls short is the one laid dead through the hull, where the two
/// swings are equal and opposite and mostly cancel — a pod aiming to clear by
/// twelve and a half metres clears by six. Left alone rather than fixed by
/// unclamping the berth, which would be an animal that abandons its journey
/// for a boat; fixing it properly means suspending the want while giving way,
/// which is also what a stance that *closes* the range will need.
fn skirt(beast: &Beast, boat: Vec2, reach: f32) -> Vec2 {
    let to = boat - beast.position;
    let range = to.length();
    let clearance = reach * BERTH;
    if range <= clearance || beast.heading.dot(to) <= 0.0 {
        return beast.heading;
    }
    // How wide the clearance stands from here, as an angle, and half as much
    // again so the pass is outside it rather than grazing it.
    let spread = ((clearance / range).asin() * 1.5).min(FRAC_PI_2);
    let bearing = to / range;
    // Whichever way it was already going round: the side the hull lies to,
    // taken the other way.
    let side = if bearing.perp_dot(beast.heading) >= 0.0 {
        1.0
    } else {
        -1.0
    };
    turned(bearing, side * spread)
}

/// The way the water gets deeper from here, as a unit vector: the bearings a
/// beast could take, sounded a couple of turns' worth ahead, deepest winning.
/// How a leaving animal finds the blue without this file knowing where any
/// coast is — and it keeps its course where every sounding comes back the
/// same, which on open floor is every sounding.
fn deeper(shared: &Shared, beast: &Beast) -> Vec2 {
    let mut deepest = f32::INFINITY;
    let mut best = beast.heading;
    for step in 0..8 {
        let bearing = turned(beast.heading, step as f32 * (TAU / 8.0));
        let at = beast.position + bearing * SOUNDING * 2.0;
        if let Some(floor) = shared.world.ready_height(at.x, at.y) {
            if floor < deepest {
                deepest = floor;
                best = bearing;
            }
        }
    }
    best
}

/// The nearest player within `reach` of a point, if any — what both giving
/// way and being spooked ask, and the same question either way.
fn nearest_player(players: &[Wake], of: Vec2, reach: f32) -> Option<&Wake> {
    players
        .iter()
        .filter(|player| player.position.distance(of) < reach)
        .min_by(|a, b| a.position.distance(of).total_cmp(&b.position.distance(of)))
}

/// Whether the seafloor at a point falls inside a band, and is *known* to.
/// The sounding is [`Archipelago::ready_height`] rather than the generating
/// kind, because this thread ticks on a beat and an island takes real
/// milliseconds to raise — and ground nobody has been near enough to generate
/// is ground no beast needs to be swimming towards.
///
/// [`Archipelago::ready_height`]: world::archipelago::Archipelago::ready_height
fn floor_in(shared: &Shared, at: Vec2, band: (f32, f32)) -> bool {
    shared
        .world
        .ready_height(at.x, at.y)
        .is_some_and(|floor| (band.0..=band.1).contains(&floor))
}

/// Tries to raise one beast of a kind in a player's waters, born on the
/// habitat's ring and already going somewhere. `None` where the water this
/// player is in has nothing of the sort to offer, which costs nothing and is
/// tried afresh next beat.
///
/// The two kinds of animal differ here and only here. A **traveller** is born
/// in its own water on the ring and given the far end of a course past this
/// player. A **shark** is born in deep water *off* the shallows it will live
/// in, so the ring finds the home and the birth is sounded out from there: it
/// has to come from somewhere nobody is, which at a coast is offshore.
///
/// A try is a whole candidate — a ring spot *and* the somewhere else each kind
/// needs of it — because a ring spot with no course off it is not a beast and
/// the next spot along may well be.
fn raise(habitat: &'static Habitat, shared: &Shared, player: Vec2, entropy: u32) -> Option<Beast> {
    (0..RAISE_ATTEMPTS).find_map(|attempt| {
        let bearing = unit(entropy, attempt * 2 + 1) * TAU;
        let out =
            habitat.ring.0 + unit(entropy, attempt * 2 + 2) * (habitat.ring.1 - habitat.ring.0);
        let on_the_ring = player + Vec2::from_angle(bearing) * out;
        // Nothing swims past the end of the world: a player standing at the
        // brink has half their horizon outside it, and a beast raised out
        // there is one the file cannot write down — see [`Flock::records`].
        // Here for the cheapness of it, the ring spot being the one point both
        // kinds have before any sounding; the check below is the one that
        // actually covers both points a beast is written down as.
        if !crate::reachable(on_the_ring) {
            return None;
        }
        if !floor_in(shared, on_the_ring, habitat.band) {
            return None;
        }

        let raised = match habitat.journey {
            Some(far) => {
                let bound = journey(shared, habitat, on_the_ring, player, far, entropy)?;
                Beast::born(habitat, on_the_ring, Some(bound), entropy)
            }
            None => {
                let born = sound_out(
                    shared,
                    on_the_ring,
                    entropy,
                    &BIRTH_RINGS,
                    (f32::NEG_INFINITY, DEEP),
                )?;
                // The deep is sounded outward from the *home*, which already
                // sits most of the way to the edge of these waters, so it will
                // happily find water beyond them. A shark born out there makes
                // a bare half-metre a beat and would be forgotten before it
                // could swim in, so the birth has to be inside the reach that
                // keeps it minded.
                (born.distance(player) <= habitat.waters)
                    .then(|| Beast::born(habitat, born, Some(on_the_ring), entropy))?
            }
        };
        // Where it is *and* where it is going, because the file writes both
        // and the reader refuses either past the edge. The ring spot is only
        // one of the two, and which one differs by kind; the other comes from
        // a sounding that walks outward from it, so the edge is exactly where
        // a sounding will have gone looking.
        (crate::reachable(raised.position) && raised.goal.is_none_or(crate::reachable))
            .then_some(raised)
    })
}

/// A heading swung through `angle` radians — positive one way, negative the
/// other, nobody downstream caring which is which.
fn turned(heading: Vec2, angle: f32) -> Vec2 {
    Vec2::from_angle(angle).rotate(heading)
}

/// Stirs bits until they stop resembling what they were — SplitMix's mixing
/// rounds, without its sequence. Everything random the beasts do comes
/// through here.
///
/// The game crate has these same rounds, deliberately not shared: the only
/// crate both sides could reach for is `protocol`, and nothing here belongs on
/// the wire. Nothing this decides ever needs to agree with another machine —
/// where a shark is raised is *told*, not re-derived — so two copies answering
/// differently would cost nothing.
fn scramble(mut x: u32) -> u32 {
    x = x.wrapping_add(0x9E37_79B9);
    x ^= x >> 16;
    x = x.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 13;
    x = x.wrapping_mul(0xC2B2_AE35);
    x ^ (x >> 16)
}

/// A decorative number in `0.0..1.0` from some bits and a salt.
fn unit(entropy: u32, salt: u32) -> f32 {
    scramble(entropy ^ salt) as f32 / u32::MAX as f32
}

/// A decorative count of beats somewhere in a span, ends included.
fn span(entropy: u32, salt: u32, span: (u32, u32)) -> u32 {
    span.0 + (unit(entropy, salt) * (span.1 - span.0) as f32) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::Server;

    /// The seed the session tests sail, and a world of it with the ground
    /// around the spawn already made: the warden sounds with `ready_height`,
    /// which answers for open ocean always and for an island only once
    /// somebody has paid for it. A test that skipped this would be swimming
    /// beasts through a world that reads as unbroken ocean.
    fn a_sea() -> (Arc<Shared>, Vec2, Vec2) {
        let shared = Server::bind(("127.0.0.1", 0), 7)
            .expect("a server should bind")
            .shared;
        let (spawn, facing) = (shared.spawn, shared.facing);
        let middle = (spawn + facing) / 2.0;
        for row in -60..=60 {
            for column in -60..=60 {
                let at = middle + Vec2::new(column as f32, row as f32) * 20.0;
                shared.world.height(at.x, at.y);
            }
        }
        (shared, spawn, facing)
    }

    /// Somewhere in the shallows off the entry island, found the way the
    /// session tests find one: stepping in from the water players spawn on
    /// towards the island they face.
    fn shallows(shared: &Shared, spawn: Vec2, facing: Vec2) -> Vec2 {
        let span = facing - spawn;
        let steps = (span.length() / 2.0).ceil() as i32;
        (0..steps)
            .find_map(|step| {
                let at = spawn + span * (step as f32 / steps as f32);
                floor_in(shared, at, (-5.0, -2.5)).then_some(at)
            })
            .expect("the entry island should have a coast")
    }

    /// One player standing still, which is what almost every test here wants
    /// of one: somewhere for a beast to be near, and nothing more. A boat that
    /// is going somewhere has to be moved beat by beat, since a course is
    /// worked out from where it was — see `a_pod_rides_a_bow_and_then_goes_on`,
    /// which is the one test that does.
    fn at(position: Vec2) -> [(PlayerId, Vec2); 1] {
        [(PlayerId(0), position)]
    }

    /// One particular beast of a flock — by id, because a beat raises the
    /// animals a player is owed alongside whatever a test put there, and it
    /// is the one under test that every assertion here is about.
    fn beast(flock: &Flock, id: BeastId) -> &Beast {
        flock
            .beasts
            .get(&id)
            .expect("the beast under test is still minded")
    }

    /// A beast of a kind raised for a player, over as many rolls as it takes
    /// from `from` on. Which bits find water is nobody's business — a coast
    /// offers a ring that is mostly island or mostly open ocean, so a single
    /// try failing says nothing, exactly as it says nothing to the warden that
    /// tries again next beat.
    ///
    /// Where to start from is the caller's, because everything else a raise
    /// decides comes out of the same bits: taking the first rolls that work
    /// every time deals one length of journey and one spot on the ring over
    /// and over, which is a fine animal to watch and a poor sample to judge
    /// the numbers by.
    fn some_raise(shared: &Shared, kind: BeastKind, player: Vec2, from: u32) -> Beast {
        (from..from + 400)
            .find_map(|entropy| raise(habitat_of(kind), shared, player, scramble(entropy)))
            .expect("these waters should raise the kind that lives in them")
    }

    #[test]
    fn the_flock_survives_being_written_down_and_taken_back_up() {
        use crate::keeper::BeastRecord;

        // One living where it stands, one still bound somewhere: the two
        // shapes a record can take, adopted back into the parts of life
        // they were written out of.
        let dwelling = BeastRecord {
            kind: BeastKind::Shark,
            position: Vec2::new(10.0, 20.0),
            goal: None,
            left: 500,
        };
        let bound = BeastRecord {
            kind: BeastKind::Whale,
            position: Vec2::new(-100.0, 50.0),
            goal: Some(Vec2::new(900.0, 900.0)),
            left: 4_000,
        };
        let mut flock = Flock::new(1);
        flock.adopt(&dwelling);
        flock.adopt(&bound);

        let shark = beast(&flock, BeastId(0));
        assert_eq!(shark.doing, Doing::Dwelling, "no goal means arrived");
        assert_eq!(shark.life, 500, "a life spans sessions, not restarts");
        let whale = beast(&flock, BeastId(1));
        assert_eq!(whale.doing, Doing::Bound, "a goal means still going");
        assert_eq!(whale.goal, bound.goal);

        // Written down again, the flock reads exactly as it was adopted.
        let mut written = flock.records();
        written.sort_by_key(|record| record.left);
        assert_eq!(written, vec![dwelling, bound]);

        // But a leaving beast is nobody's to keep: its exit is finished
        // under the water either way.
        flock
            .beasts
            .get_mut(&BeastId(0))
            .expect("the shark is kept")
            .settle(Doing::Leaving, 1);
        assert_eq!(flock.records().len(), 1, "an exit was written down");
    }

    #[test]
    fn nothing_past_the_end_of_the_world_is_written_down() {
        // A world may only write what it can read back. The reader refuses a
        // beast swimming — or making — for anywhere no player could go, so a
        // beast out there has to be dropped rather than filed, or the world
        // would not open again. It takes a player at the very brink to make
        // one, which is exactly the player who would find their world gone.
        let past = crate::MAX_RANGE + 500.0;
        let over_the_edge = crate::keeper::BeastRecord {
            kind: BeastKind::Shark,
            position: Vec2::new(past, 0.0),
            goal: None,
            left: 500,
        };
        // And one swimming honestly enough, but *making* for out there: the
        // reader checks where a beast is going as well as where it is, and
        // this is the one the edge of the world actually produced.
        let bound_over_the_edge = crate::keeper::BeastRecord {
            kind: BeastKind::Whale,
            position: Vec2::new(crate::MAX_RANGE - 500.0, 0.0),
            goal: Some(Vec2::new(past, 0.0)),
            left: 4_000,
        };
        let mut flock = Flock::new(1);
        flock.adopt(&over_the_edge);
        flock.adopt(&bound_over_the_edge);

        assert_eq!(
            flock.records(),
            vec![],
            "the world wrote down a beast it would refuse to read"
        );
    }

    #[test]
    fn a_shark_is_born_in_the_deep_and_swims_to_the_shallows() {
        let (shared, spawn, facing) = a_sea();
        let coast = shallows(&shared, spawn, facing);

        // Raised for a player standing off the beach: it is born in deep
        // water, nowhere near where its life will be spent, and it is under
        // the surface while it makes its way in.
        let mut flock = Flock::new(1);
        let shark = some_raise(&shared, BeastKind::Shark, coast, 0);
        let born = shark.position;
        assert!(
            floor_in(&shared, born, (f32::NEG_INFINITY, DEEP)),
            "a shark was born over ground at {:?} m",
            shared.world.ready_height(born.x, born.y),
        );
        assert!(!shark.surfaced, "a newcomer arrived on the surface");
        assert_eq!(shark.doing, Doing::Bound);
        assert!(shark.goal.is_some(), "it was born with nowhere to be");
        let id = flock.keep(shark);

        // And it swims in: within a few minutes of beats it is living in the
        // band its kind claims, and up where a fin can be seen.
        for _ in 0..2_000 {
            flock.beat(&shared, &at(coast), &[]);
            if beast(&flock, id).doing == Doing::Dwelling {
                break;
            }
        }
        let shark = beast(&flock, id);
        assert_eq!(shark.doing, Doing::Dwelling, "the shark never arrived");
        assert!(shark.surfaced, "a shark that arrived is up in its shallows");
        assert!(
            floor_in(&shared, shark.position, habitat_of(BeastKind::Shark).band),
            "arrived over ground at {:?} m",
            shared
                .world
                .ready_height(shark.position.x, shark.position.y),
        );
        assert!(
            shark.position.distance(born) > 20.0,
            "the shark arrived without going anywhere"
        );
    }

    #[test]
    fn a_beast_that_has_lived_its_life_goes_down_and_out() {
        let (shared, spawn, facing) = a_sea();
        let coast = shallows(&shared, spawn, facing);

        // A shark in its shallows with a beat or two left to live, and a
        // player anchored where it can still be minded — the whole of what is
        // watched here is this one animal's exit.
        let mut flock = Flock::new(2);
        let mut shark = Beast::summoned(habitat_of(BeastKind::Shark), coast, 3);
        shark.life = 2;
        let id = flock.keep(shark);

        let mut told: Vec<(bool, Vec2)> = Vec::new();
        let mut gone = false;
        for _ in 0..LEAVING_PATIENCE + 40 {
            for word in flock.beat(&shared, &at(coast), &[]) {
                match word {
                    ToClient::Beast {
                        id: said,
                        surfaced,
                        position,
                        ..
                    } if said == id => told.push((surfaced, position)),
                    ToClient::BeastGone { id: said } if said == id => gone = true,
                    _ => {}
                }
            }
            if gone {
                break;
            }
        }

        assert!(gone, "the shark outlived its life");
        assert!(
            !flock.beasts.contains_key(&id),
            "it was told gone and kept anyway"
        );

        // It was under the water for the whole of its leaving, and the last
        // anyone was told of it, it had left the shallows it lived in and was
        // heading for the deep — a beat short of it, since a beast is let go
        // between one telling and the next. An animal sounds and does not
        // come back up; it is never seen to blink out.
        let (surfaced, last) = *told.last().expect("it was told of at all");
        assert!(!surfaced, "it was let go with its back out of the water");
        let floor = shared
            .world
            .ready_height(last.x, last.y)
            .expect("it was let go over ground somebody has made");
        assert!(
            floor <= habitat_of(BeastKind::Shark).band.0,
            "it was last seen over {floor} m, still in its own shallows"
        );
        let down = told.iter().filter(|(surfaced, _)| !surfaced).count();
        assert!(
            down >= SLIPPING as usize,
            "it went down and out in {down} beats"
        );
    }

    #[test]
    fn a_whale_dives_from_a_boat_and_comes_back_up() {
        let (shared, spawn, _) = a_sea();
        // Out to sea, where a whale lives: open floor, and no coast to be
        // steered off in the middle of the test.
        let deep = spawn + Vec2::new(-800.0, 0.0);
        assert!(floor_in(&shared, deep, habitat_of(BeastKind::Whale).band));

        let mut flock = Flock::new(4);
        let id = flock.keep(Beast::summoned(habitat_of(BeastKind::Whale), deep, 5));
        // The boat, while it is not the point: many times the berth away, so
        // the whale has nothing to say about it, and standing off the animal
        // rather than off a fixed spot, so that a whale which swims a few
        // hundred metres over the course of the test is minded throughout. A
        // boat pinned to where the whale *started* is a boat the whale can
        // leave, and being forgotten in the middle of a test about breathing
        // reads as a lookup bug.
        let abeam = Vec2::new(0.0, 600.0);
        let far = |flock: &Flock| beast(flock, id).position + abeam;

        // Left alone it breathes: a bout up, a bout down, and up again — the
        // surfacing is not a one-off state it settles into.
        let mut breaths = 0;
        let mut up = beast(&flock, id).surfaced;
        for _ in 0..2 * (SOUNDED_BOUT.1 + SURFACED_BOUT.1) {
            let watching = far(&flock);
            flock.beat(&shared, &at(watching), &[]);
            if beast(&flock, id).surfaced != up {
                up = !up;
                breaths += 1;
            }
        }
        assert!(
            breaths >= 2,
            "a whale left alone changed depth {breaths} times"
        );

        // Now bring the boat up to it while it is at the surface, and it
        // dives — within the beat, whatever was left of the bout.
        for _ in 0..SOUNDED_BOUT.1 + SURFACED_BOUT.1 {
            let watching = far(&flock);
            flock.beat(&shared, &at(watching), &[]);
            if beast(&flock, id).surfaced {
                break;
            }
        }
        assert!(
            beast(&flock, id).surfaced,
            "the whale never came up to be met"
        );
        let alongside = beast(&flock, id).position;
        flock.beat(&shared, &at(alongside), &[]);
        assert!(!beast(&flock, id).surfaced, "the whale ignored the boat");

        // And it stays down while the boat is there: no bout ends it, because
        // being driven down is not a bout.
        for _ in 0..SPOOKED + SURFACED_BOUT.1 {
            let alongside = beast(&flock, id).position;
            flock.beat(&shared, &at(alongside), &[]);
            assert!(
                !beast(&flock, id).surfaced,
                "the whale surfaced under the boat"
            );
        }

        // The boat leaves; the whale comes back up in its own time.
        let mut surfaced = false;
        for _ in 0..SPOOKED + SOUNDED_BOUT.1 + 40 {
            let watching = far(&flock);
            flock.beat(&shared, &at(watching), &[]);
            if beast(&flock, id).surfaced {
                surfaced = true;
                break;
            }
        }
        assert!(surfaced, "the whale never came back up");
    }

    #[test]
    fn a_pod_passes_a_boat_instead_of_fleeing_it() {
        let (shared, spawn, _) = a_sea();
        let habitat = habitat_of(BeastKind::Dolphins);
        let reach = Regard::of(BeastKind::Dolphins, A_PLAYER)
            .berth()
            .expect("a pod gives way to a boat it is not riding");

        // The boat here never moves, which is the point of it: a pod's stance
        // is `Play`, and a boat at anchor is making no water to ride, so what
        // is watched is the half of `Play` that is simply getting out of the
        // way. The riding half is `a_pod_rides_a_bow_and_then_goes_on`.
        //
        // A pod on a crossing that would take it a few metres off that boat:
        // born, the hull and the far end all but on one line, out where the
        // floor is open and there is nothing else in the way. Whatever room it
        // ends up with beyond those few metres, it made.
        //
        // A few metres and not none, because a course laid *exactly* over the
        // hull is degenerate twice over and neither half is about the berth.
        // The goal want and the berth are both clamped to the same quarter
        // radian a beat and, dead in line, they are the same size and opposite,
        // so the pass is whatever is left over rather than what `skirt` asked
        // for. And `skirt` takes the side the animal is already leaning, which
        // head on is no side at all, so the wander picks it. Every course that
        // is not the exact one clears the berth and then some.
        let born = spawn + Vec2::new(-1_000.0, 0.0);
        let boat = spawn + Vec2::new(-800.0, 5.0);
        let goal = spawn + Vec2::new(-600.0, 0.0);
        for at in [born, boat, goal] {
            assert!(
                floor_in(&shared, at, habitat.band),
                "{at:?} is not open sea"
            );
        }

        let mut flock = Flock::new(11);
        let id = flock.keep(Beast::born(habitat, born, Some(goal), 12));

        // Swum until it is abeam of the hull and past it, watching how close it
        // came and whether it ever gave up on where it was going.
        let along = (goal - born).normalize();
        let mut nearest = f32::INFINITY;
        let mut past = false;
        for _ in 0..600 {
            flock.beat(&shared, &at(boat), &[]);
            let pod = beast(&flock, id);
            nearest = nearest.min(pod.position.distance(boat));
            assert_eq!(
                pod.doing,
                Doing::Bound,
                "the pod stopped being on its way somewhere over a boat"
            );
            // The whole difference between passing and fleeing, and the one
            // thing a berth may never do: turn the animal back down its own
            // course. `skirt` caps the swing at a quarter turn off the hull's
            // bearing for exactly this reason.
            assert!(
                pod.heading.dot(along) > 0.0,
                "the pod turned back the way it came {:.0} m off the boat",
                pod.position.distance(boat),
            );
            if (pod.position - boat).dot(along) > 0.0 {
                past = true;
                break;
            }
        }

        assert!(past, "the pod never got past the boat");
        // And it kept the clearance the berth promises, which is more than
        // twice the room the course it was swimming would have given it: the
        // difference between the two is the bend, and the bend is the whole
        // behaviour.
        assert!(
            nearest >= reach * BERTH,
            "the pod passed {nearest:.1} m off a boat it clears by {:.1}",
            reach * BERTH,
        );
    }

    #[test]
    fn a_pod_rides_a_bow_and_then_goes_on() {
        let (shared, spawn, _) = a_sea();
        let habitat = habitat_of(BeastKind::Dolphins);
        let reach = Regard::of(BeastKind::Dolphins, A_PLAYER)
            .playing()
            .expect("a pod comes to a bow");

        // A boat motoring steadily out to sea — moved every beat, because the
        // whole of what makes a bow a bow is that the boat is going somewhere,
        // and the server works that out from where the boat *was*. And a pod on
        // a crossing of its own, born already inside the reach: how an animal
        // gets within noticing distance is the ordinary swimming the rest of
        // this module tests, and starting outside it only makes what happens
        // next depend on a chase this test is not about.
        let helm = Vec2::new(-1.0, 0.0);
        let knots = 4.0;
        let moorings = spawn + Vec2::new(-800.0, 0.0);
        let boat = |beat: u32| moorings + helm * knots * BEAST_TICK.as_secs_f32() * beat as f32;
        let wake = |beat: u32| Wake {
            position: boat(beat),
            course: helm,
            speed: knots,
        };

        let born = spawn + Vec2::new(-790.0, 10.0);
        let goal = spawn + Vec2::new(-2_500.0, 300.0);
        for at in [moorings, born, goal] {
            assert!(
                floor_in(&shared, at, habitat.band),
                "{at:?} is not open sea"
            );
        }
        assert!(knots >= UNDERWAY, "the boat is not making way");

        let mut flock = Flock::new(21);
        let id = flock.keep(Beast::born(habitat, born, Some(goal), 22));

        // Every riding beat: how far off station the pod was, and how far ahead
        // of the hull. Judged afterwards rather than as it goes, because the
        // start of a ride is the pod getting there and only the rest of it is
        // the pod keeping station.
        let mut held: Vec<(f32, f32)> = Vec::new();
        let mut peeled: Option<(u32, f32)> = None;
        for beat in 0..RIDE.1 + 400 {
            flock.beat(&shared, &[(PlayerId(0), boat(beat))], &[]);
            let pod = beast(&flock, id);

            // Whatever it is doing about the boat, it is still an animal on a
            // crossing: a stance moves an animal, and never moves it on through
            // its life. This is the assertion the whole design turns on.
            assert_eq!(
                pod.doing,
                Doing::Bound,
                "riding a bow took the pod out of its crossing"
            );
            assert_eq!(pod.goal, Some(goal), "riding a bow lost the pod its goal");

            match &pod.riding {
                Some(riding) => {
                    assert!(
                        peeled.is_none(),
                        "the pod came back for a second bow it should have wanted nothing to do with"
                    );
                    held.push((
                        pod.position.distance(wake(beat).bow(riding.side)),
                        (pod.position - boat(beat)).dot(helm),
                    ));
                }
                None if !held.is_empty() && peeled.is_none() => {
                    peeled = Some((beat, pod.position.distance(goal)));
                }
                None => {}
            }
        }

        assert!(!held.is_empty(), "the pod never came to the bow at all");
        let rode = held.len() as u32;
        assert!(
            (RIDE.0..=RIDE.1).contains(&rode),
            "the pod rode for {rode} beats, out of the {RIDE:?} it draws from",
        );

        // The tail of the ride is the station keeping — whatever the approach
        // cost, by the end of it the pod is on the bow and staying on it. The
        // reach is what it noticed the boat at, so holding inside `BERTH` of
        // that is holding closer than it would ever have passed.
        let holding = &held[held.len() - 100..];
        for (off, ahead) in holding {
            assert!(
                *off < reach * BERTH,
                "the pod let its station go to {off:.1} m in the last 100 beats of the ride",
            );
            assert!(
                *ahead > 0.0,
                "the pod dropped {:.0} m astern of the boat in the last 100 beats of the ride",
                -ahead,
            );
        }

        // And then it went on. The boat is still there and still making way for
        // the whole rest of the loop above, so a pod that had merely been
        // interrupted would be back on the bow inside a beat — the assertion
        // in the riding arm is what says it never was. What is left is whether
        // it took its crossing up again, which is the other half of the promise
        // that a stance is steering and not a life.
        let (when, from_goal) = peeled.expect("the pod never left the bow");
        let ashore = beast(&flock, id).position.distance(goal);
        assert!(
            ashore < from_goal,
            "the pod peeled off {when} beats in, {from_goal:.0} m from the far end of its \
             crossing, and was {ashore:.0} m from it at the end"
        );
    }

    #[test]
    fn a_pod_is_passing_through_and_its_journey_is_its_whole_life() {
        let (shared, spawn, _) = a_sea();
        // Open water off the entry island, where a pod would be.
        let anchored = spawn + Vec2::new(-500.0, 0.0);
        let habitat = habitat_of(BeastKind::Dolphins);
        assert!(floor_in(&shared, anchored, habitat.band));

        // Six pods and not one, because how far a crossing runs is drawn per
        // animal out of `journey`: watching a single pod is watching a single
        // draw of that span, and it is the long draws that would fail to fit
        // inside the waters keeping the animal minded. Six is enough to catch
        // an end laid past the edge of them.
        let mut lengths: Vec<f32> = Vec::new();
        for which in 0..6 {
            // Raised for the player anchored there: born on the ring, already
            // going somewhere, and up where it can be seen from the first beat
            // — a pod has no swim in from the blue, because being on its way
            // *is* what it is.
            let pod = some_raise(&shared, BeastKind::Dolphins, anchored, which * 137);
            let born = pod.position;
            let goal = pod.goal.expect("a pod raised with nowhere to go");
            assert_eq!(pod.doing, Doing::Bound);
            assert!(pod.surfaced, "a pod arrived underwater");
            assert!(floor_in(&shared, goal, habitat.crossing));
            let crossing = born.distance(goal);
            assert!(
                crossing >= habitat.journey.expect("pods journey").0,
                "a crossing of {crossing} m is not a journey"
            );
            lengths.push(crossing);

            // And the course is laid past the player rather than at them or
            // away from them: it is a crossing somebody is meant to *meet*,
            // which is the one thing about these animals that is staged.
            let along = (goal - born).normalize();
            let abeam = (anchored - born).reject_from(along).length();
            let ahead = (anchored - born).dot(along);
            assert!(
                ahead > 0.0 && abeam <= ABEAM,
                "a course passing {abeam:.0} m abeam and {ahead:.0} m along is not by the player"
            );

            // Reaching the far end is the end of the animal: it goes down and
            // is let go. No separate clock said so — it simply got there.
            //
            // Which is why this watches the *mechanism* rather than only
            // waiting for the pod to stop existing: being forgotten for want of
            // anybody near ends in a leaving and a `BeastGone` too, so a
            // journey laid beyond the waters that keep a pod minded would end
            // every crossing that way and a test that only counted endings
            // would call it a pass. What is asserted is that it stopped being
            // bound because it had arrived.
            let mut flock = Flock::new(6 + which);
            let id = flock.keep(pod);
            let mut arrived: Option<f32> = None;
            let mut gone = false;
            for _ in 0..3_000 {
                for word in flock.beat(&shared, &at(anchored), &[]) {
                    if matches!(word, ToClient::BeastGone { id: said } if said == id) {
                        gone = true;
                    }
                }
                if gone {
                    break;
                }
                let pod = beast(&flock, id);
                // Never let go for want of anybody near: the player is anchored
                // on its course, and both ends of the crossing lie inside a
                // pod's waters, so the count of beats nobody was near never
                // starts.
                assert_eq!(
                    pod.unminded,
                    0,
                    "a {crossing:.0} m crossing left the waters that keep it minded, \
                     {:.0} m from the player",
                    pod.position.distance(anchored),
                );
                if pod.doing == Doing::Leaving && arrived.is_none() {
                    arrived = Some(pod.position.distance(goal));
                }
            }
            assert!(gone, "the pod is still crossing");

            let short = arrived.expect("the pod went without ever being seen to leave");
            assert!(
                short < ARRIVED,
                "the pod broke off a {crossing:.0} m crossing {short:.0} m short of the end"
            );
        }

        // And those six were six different crossings, not one length dealt six
        // times — the assertions above are only worth their run if the span is
        // actually being drawn from.
        let (shortest, longest) = (
            lengths.iter().copied().fold(f32::INFINITY, f32::min),
            lengths.iter().copied().fold(0.0, f32::max),
        );
        assert!(
            longest - shortest > 100.0,
            "six pods drew crossings between {shortest:.0} and {longest:.0} m"
        );
    }

    #[test]
    fn a_journey_a_traveller_could_not_finish_is_not_a_journey() {
        // The far end of a crossing has to fall inside the waters that keep
        // the animal minded, and the numbers that decide whether it does are
        // three spans that a raise draws the middle of far more often than the
        // corners. So this works the geometry out rather than swimming one:
        // the corner cases are the ones that fail, and a pod that fails is not
        // a pod that misbehaves — it is a pod dropped in the middle of its own
        // life, which was how this was first written and is what the crossing
        // lengths are held down to now.
        for habitat in &HABITATS {
            let Some(journey) = habitat.journey else {
                continue;
            };
            // The same lay as `journey`: born `out` along the ring from the
            // player, aimed at a point abeam of them, and swum `far` metres.
            let mut furthest: f32 = 0.0;
            for ring in 0..=8 {
                let out = habitat.ring.0 + (habitat.ring.1 - habitat.ring.0) * ring as f32 / 8.0;
                for step in -8..=8 {
                    let abeam = ABEAM * step as f32 / 8.0;
                    for length in [journey.0, journey.1] {
                        let player = Vec2::ZERO;
                        let from = Vec2::new(out, 0.0);
                        let along = (player - from).normalize();
                        let aim = player + along.perp() * abeam;
                        let end = from + (aim - from).normalize() * length;
                        furthest = furthest.max(end.distance(player));
                    }
                }
            }
            assert!(
                furthest <= habitat.waters,
                "{:?} can be sent {furthest:.0} m off, past the {:.0} m that keeps it minded",
                habitat.kind,
                habitat.waters,
            );

            // And the life it carries is a backstop rather than a rival
            // ending: it has to outlast the longest crossing by enough that a
            // course bent round a headland still gets to the end of itself.
            let beats = journey.1 / (habitat.cruise * BEAST_TICK.as_secs_f32());
            assert!(
                habitat.life.0 as f32 > beats * 1.25,
                "{:?} draws lives of {} beats for crossings of {beats:.0}",
                habitat.kind,
                habitat.life.0,
            );
        }
    }

    #[test]
    fn a_summons_is_already_living_where_it_was_put() {
        // The console wants the animal seen — see `conjure`. A summoned beast
        // skips the swim in from the blue that a raised one makes.
        let beast = Beast::summoned(habitat_of(BeastKind::Dolphins), Vec2::ZERO, 9);
        assert_eq!(beast.doing, Doing::Dwelling);
        assert!(beast.surfaced);
    }

    #[test]
    fn every_beast_has_an_opinion_on_everything_it_could_meet() {
        // `REGARDS` is a grid rather than a lookup, so what it can fail to be
        // total about is not a missing cell — the shape of the array sees to
        // that — but the two axes agreeing with what there actually is. A row
        // per habitat, in habitat order, and a column per thing there is to
        // meet: get either of those out of step and every cell still exists and
        // holds the opinion belonging to somebody else.
        assert_eq!(
            REGARDS.len(),
            HABITATS.len(),
            "the table has {} rows for {} kinds of beast",
            REGARDS.len(),
            HABITATS.len(),
        );
        for (index, habitat) in HABITATS.iter().enumerate() {
            assert_eq!(
                Regard::row(habitat.kind),
                index,
                "{:?} lives on row {index} of the habitats and row {} of the table",
                habitat.kind,
                Regard::row(habitat.kind),
            );
        }
        for (index, met) in Met::ALL.iter().enumerate() {
            assert_eq!(
                met.column(),
                index,
                "{met:?} is column {} of the table and {index} of everything there is to meet",
                met.column(),
            );
        }

        // And then the totality itself, asked for rather than reasoned about: a
        // variant added to `Met` and given a column without being added to
        // `ALL` leaves the table a column narrow, which is this indexing and
        // nothing else.
        for habitat in &HABITATS {
            for met in Met::ALL {
                let _ = Regard::of(habitat.kind, met);
            }
        }
    }

    #[test]
    fn nothing_in_the_table_asks_for_a_stance_that_is_not_written() {
        // `Harry` and `Play` are in `Regard` because the table is the place
        // this design gets written down, and they are not in any cell because
        // neither has an implementation — a cell holding one would be an animal
        // that says it does something and does nothing, which is the one way a
        // table of stances can be quietly wrong. So this is the tripwire for
        // filling one in: the day a cell wants `Harry`, this fails, and what it
        // is asking for is the steering to be written before the cell is.
        for (row, habitat) in HABITATS.iter().enumerate() {
            for met in Met::ALL {
                let regard = Regard::of(habitat.kind, met);
                assert!(
                    matches!(
                        regard,
                        Regard::Ignore | Regard::Berth(_) | Regard::Sound(_) | Regard::Play(_)
                    ),
                    "{:?} answers {met:?} with {regard:?}, which nothing swims yet",
                    habitat.kind,
                );
                // Every stance that carries a reach has to carry a real one:
                // zero would be a cell that reads as an opinion and behaves as
                // `Ignore`, which is the other way round from the above and
                // just as quiet.
                if let Some(reach) = regard.berth().or(regard.sounding()) {
                    assert!(
                        reach > 0.0,
                        "row {row} answers {met:?} at {reach} m, which is not a reach",
                    );
                }
            }
        }
    }

    #[test]
    fn every_kind_lives_a_life_it_could_be_met_in() {
        // The spans are decorative, but two things about them are not: a life
        // has to be long enough to sail past and short enough that the waters
        // turn over, and every draw has to land inside the span it was drawn
        // from — an off-by-one here would be a beast that lived forever.
        for habitat in &HABITATS {
            assert!(habitat.life.0 >= 600, "{:?} is met and gone", habitat.kind);
            for entropy in 0..256 {
                let drawn = span(scramble(entropy), 0x11FE, habitat.life);
                assert!(
                    (habitat.life.0..=habitat.life.1).contains(&drawn),
                    "{:?} was dealt {drawn} beats out of {:?}",
                    habitat.kind,
                    habitat.life,
                );
            }
        }
    }
}
