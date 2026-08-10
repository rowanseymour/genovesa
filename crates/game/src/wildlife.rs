//! The wildlife: eagles over the summits, and formations crossing the sea —
//! pods of dolphins, lines of seabirds, the odd whale.
//!
//! None of it can be touched, and that one fact decides the architecture. A
//! creature a player could interact with would have to be the server's —
//! authoritative, synchronised, on the wire the way other players are —
//! because two machines are only free to disagree about what nobody can act
//! on. These are scenery, so the server has never heard of them: each client
//! raises its own out of nothing but the ground it was already sent and its
//! own clock, and the protocol is untouched. Any creature that ever earns
//! behaviour worth reaching for moves to the server and the wire *first* and
//! becomes a different kind of thing; it does not grow out of these.
//!
//! Wildlife comes in two shapes, at opposite ends of what decoration can be:
//!
//! - An **eagle** belongs to a *place*. A chunk whose ground holds a summit
//!   worth the name gets a bird — sometimes a pair — circling it. A summit
//!   is a fact about the chunk grid, so every client raises eagles over the
//!   same peaks without a word crossing the wire, and the birds hang off the
//!   chunk entity so streaming despawns them with the ground. Each bird
//!   carries its own circle: a pair is two birds on one thermal rather than
//!   a thing of its own.
//! - A **crossing** belongs to a *moment*: a pod porpoising past the boat, a
//!   line of seabirds undulating along the shallows, one or two whales
//!   shouldering through open water. It surfaces near whoever is looking and
//!   is gone; a client anchored a mile away gets its own. Here the unit is
//!   the [`Formation`] rather than the individual — the group carries the
//!   course and the lifetime, and a member only ever knows its station in it
//!   and its phase of the group's own rhythm. A swimming formation retires
//!   under water, where a despawn cannot be seen; a flying one retires out
//!   at the edge of the haze.
//!
//! And though nothing here can be touched, it can be *approached* — so the
//! one behaviour wildlife owes the player is absence: birds give way upward
//! and swimmers give way downward as the player nears, eased rather than
//! snapped, so an encounter reads as the creature minding them and never as
//! the boat passing through it. That is one rule for both shapes, eagle and
//! pod alike: [`Shy`] carries how much of the player a creature is currently
//! minding and [`give_way`] eases it on and off, while what being shy
//! *means* — climbing, sounding — stays each kind's own business.

use std::f32::consts::{FRAC_PI_2, TAU};
use std::ops::Index;

use bevy::math::Vec3Swizzles;
use bevy::prelude::*;
use protocol::ground::CHUNK_METRES;

use crate::camera::MapCamera;
use crate::player::PlayerPlace;
use crate::sea::SeaConditions;
use crate::terrain::{Ground, TerrainChunk};
use crate::{eased, matte, model_mesh, AppState};

/// The kinds of creature. Also an index into [`WildlifeModels`], so anything
/// that knows what it is can find what to draw it with.
#[derive(Clone, Copy)]
enum Kind {
    Eagle,
    Dolphin,
    Seabird,
    Whale,
}

/// What each kind is made of, in [`Kind`]'s own order: the file its mesh
/// comes out of, and the colour it is painted.
///
/// The models hold one mesh each, at position 0 — pinned by
/// `the_models_are_one_creature_each_fit_to_draw` the way the palm's order
/// is, which reads the file names here to know what it is looking for.
const KINDS: [(&str, Color); 4] = [
    ("eagle.glb", EAGLE_COLOR),
    ("dolphin.glb", DOLPHIN_COLOR),
    ("seabird.glb", SEABIRD_COLOR),
    ("whale.glb", WHALE_COLOR),
];

/// Dark umber. An eagle is seen against sky or against sunlit rock, and in
/// both it is its silhouette — real plumage colour would only muddy a shape
/// a few pixels across.
const EAGLE_COLOR: Color = Color::srgb(0.24, 0.18, 0.13);

/// Wet slate. Lighter than the deep sea it breaks out of and darker than the
/// spray-white a leap suggests, so the arc reads against the water at the
/// distances pods keep.
const DOLPHIN_COLOR: Color = Color::srgb(0.42, 0.50, 0.55);

/// Chalk grey. A seabird is seen low against bright water, where a pale bird
/// is the one that reads — the real birds are mostly white for their own
/// reasons.
const SEABIRD_COLOR: Color = Color::srgb(0.84, 0.84, 0.80);

/// Deep blue-grey, darker than the dolphin's: a whale's back barely clears
/// the water, and what sells the size is a long dark mass rather than a
/// bright shape.
const WHALE_COLOR: Color = Color::srgb(0.27, 0.31, 0.37);

// --- Minding the player ------------------------------------------------------

/// How much of the player a creature is currently minding, and what it takes
/// to mind them at all. The one give-way rule the module doc promises: eased
/// on and off by [`give_way`], and read by whatever draws the creature.
#[derive(Component)]
pub struct Shy {
    /// `0.0` unbothered, `1.0` fully given way. What that *means* is the
    /// creature's own — a bird climbs, a pod sounds — so nothing here says.
    minding: f32,
    /// How near the player may come across the map, in metres, before it is
    /// felt. Generous enough on a formation to cover the members' spread
    /// around the leader the distance is measured to.
    wary: f32,
    /// How far above the player it stops being worth minding, in metres: a
    /// bird already this much higher than the masthead has nothing to give
    /// way to. Infinite for anything keeping to the water, where the player
    /// is never below in a sense worth measuring.
    headroom: f32,
}

impl Shy {
    /// Unbothered, minding a player within `wary` metres — the sea's
    /// version, with no height to it.
    fn of(wary: f32) -> Self {
        Self {
            minding: 0.0,
            wary,
            headroom: f32::INFINITY,
        }
    }
}

/// How fast a creature gives way and how fast it settles back, in e-foldings
/// per second: a couple of seconds either way, which is long enough to read
/// as the animal deciding and short enough to happen while the boat is still
/// there.
const MINDING: f32 = 0.6;

/// Whether a creature at `at` is minding a player at `player` — the whole of
/// the rule, kept out of the system so it can be asked about directly.
fn minds(shy: &Shy, at: Vec3, player: Option<Vec3>) -> bool {
    player.is_some_and(|player| {
        at.xz().distance(player.xz()) < shy.wary && at.y - player.y < shy.headroom
    })
}

/// Eases every creature's [`Shy::minding`] towards whether the player is
/// there to be minded.
///
/// Position comes from the propagated transform, so an eagle hanging off a
/// chunk is compared in the same world frame a formation is. That is last
/// frame's — propagation runs after this — and on the frame a creature is
/// spawned it is the origin, both of which are a fraction of one ease on a
/// quantity that takes seconds to travel.
fn give_way(
    time: Res<Time>,
    player: PlayerPlace,
    mut creatures: Query<(&mut Shy, &GlobalTransform)>,
) {
    let player = player.at();
    let ease = eased(MINDING, time.delta_secs());
    for (mut shy, at) in &mut creatures {
        let target = f32::from(minds(&shy, at.translation(), player));
        shy.minding += (target - shy.minding) * ease;
    }
}

// --- Eagles ----------------------------------------------------------------

/// Ground that must stand under a summit before it earns an eagle, in metres.
///
/// Set against the maps rather than derived: island tops run from a few
/// metres on an islet to a hundred and twenty-odd on a continent, and the
/// bare-rock look starts somewhere in the forties. This sits above that, so
/// an eagle always means *mountain* — a green hill never has one, a low
/// island has none at all, and the bird only appears where the request for it
/// ("if there's sufficiently high ground") is honestly met.
const EYRIE_HEIGHT: f32 = 55.0;

/// The circle an eagle rides, in metres — wide enough to read as patrolling
/// the summit rather than orbiting a point, small enough to stay over the
/// massif that raised it. A second bird of a pair rides a slightly wider one.
const SOAR_RADIUS: f32 = 24.0;

/// How far above its summit the circle is flown.
const SOAR_CLEARANCE: f32 = 16.0;

/// Speed along the circle, in metres per second — a lap in about seventeen
/// seconds. Soaring pace: fast enough that the motion is what catches the
/// eye at two hundred metres, slow enough to be riding a thermal rather than
/// chasing something.
const SOAR_SPEED: f32 = 9.0;

/// How far the bird rolls into its turn, in radians. Enough to break the
/// wings' flat line when seen edge-on, which is what says "banking" rather
/// than "hovering ornament".
const SOAR_BANK: f32 = 0.20;

/// The least air the circle may keep between the bird and any ground, in
/// metres. The circle is sized from its own chunk's summit, but a ridge in
/// the next chunk is entitled to rise through it.
const SOAR_GROUND_CLEARANCE: f32 = 7.0;

/// How near the player may come, in metres across the map, before an eagle
/// minds them; how far above them it stops caring; and how much air it puts
/// on when it does. A boat under a summit is a boat aground on a mountain,
/// which the game permits, so the bird has to permit it too: it climbs
/// rather than ever sharing its altitude with a masthead.
const EAGLE_WARY: f32 = 40.0;
const EAGLE_HEADROOM: f32 = 25.0;
const EAGLE_LIFT: f32 = 14.0;

/// An eagle, circling the summit of the chunk it hangs off.
#[derive(Component)]
pub struct Eagle {
    /// The centre of its circle, in the chunk's own frame: the summit, plus
    /// [`SOAR_CLEARANCE`] — plus a little more for the second of a pair.
    centre: Vec3,
    /// Its own circle's radius — [`SOAR_RADIUS`], widened for a pair's
    /// second bird so the two never meet.
    radius: f32,
    /// Where on the circle this bird was at time zero, in radians — from the
    /// chunk's coordinates, so the same bird is mid-lap in the same place
    /// however often its ground streams out and back in.
    phase: f32,
    /// Which way round: `1.0` or `-1.0`. A pair shares one direction, being
    /// on the same thermal.
    turn: f32,
}

/// Where an eagle would circle over one chunk, in the chunk's own frame —
/// or `None`, which is the usual answer.
///
/// The rule is the summit itself, not high ground in general: a massif spans
/// many chunks that all hold mountain, and a bird over each would read as a
/// flock. So a chunk only qualifies on a peak in its grid's *interior* — a
/// highest corner on the border is the shoulder of a summit whose top lies in
/// the neighbouring chunk, which will raise the eagle itself. A summit
/// landing exactly on a seam is thereby nobody's and goes unwatched; at one
/// corner among four thousand it is a price worth the rule staying one line.
pub fn eyrie(ground: &Ground, chunk: IVec2) -> Option<Vec3> {
    let (at, height) = ground.peak(chunk)?;
    if height < EYRIE_HEIGHT {
        return None;
    }
    let local = at - chunk.as_vec2() * CHUNK_METRES;
    if local.min_element() <= 0.0 || local.max_element() >= CHUNK_METRES {
        return None;
    }
    Some(Vec3::new(local.x, height + SOAR_CLEARANCE, local.y))
}

/// Gives every newly arrived chunk with a summit its eagle — or its pair,
/// every third summit or so holding two.
///
/// A pair is two birds on one thermal rather than a [`Formation`]: they come
/// out of the same bits, so they share a circle centre and a direction, and
/// the second is simply put a little higher, on a slightly wider circle, and
/// most of a sixth of a lap behind — enough to read as company rather than
/// as a collision waiting to happen. Nothing after this knows the two are a
/// pair, and nothing needs to: unlike a crossing, they hold no course
/// between them and no lifetime, having the chunk's.
///
/// Children of the chunk entity, like palms: drawn when the ground is drawn,
/// forgotten when streaming forgets it, and re-arrival runs this again —
/// same chunk, same summit, same birds.
fn watch_summits(
    mut commands: Commands,
    models: Res<WildlifeModels>,
    ground: Res<Ground>,
    chunks: Query<(Entity, &TerrainChunk), Added<TerrainChunk>>,
) {
    for (entity, chunk) in &chunks {
        let Some(centre) = eyrie(&ground, chunk.coords) else {
            continue;
        };
        let bits = scramble((chunk.coords.x as u32) ^ (chunk.coords.y as u32).rotate_left(16));
        let pair = (bits >> 25).is_multiple_of(3);
        for wing in 0..if pair { 2 } else { 1 } {
            let trailing = wing as f32;
            commands.entity(entity).with_child((
                Name::new("Eagle"),
                Eagle {
                    centre: centre + Vec3::Y * 4.0 * trailing,
                    radius: SOAR_RADIUS + 3.0 * trailing,
                    phase: TAU * (bits >> 8) as f32 / (1 << 24) as f32 + 0.9 * trailing,
                    turn: if bits & 1 == 0 { 1.0 } else { -1.0 },
                },
                Shy {
                    headroom: EAGLE_HEADROOM,
                    ..Shy::of(EAGLE_WARY)
                },
                Transform::from_translation(centre),
                models[Kind::Eagle].drawn_as(),
            ));
        }
    }
}

/// Flies every eagle one frame further round its circle.
///
/// Driven off the elapsed clock rather than integrated, so a bird's place is
/// a function of time and nothing accumulates. The clock is the wrapped one
/// the sea also runs on; when it wraps, once an hour, every bird skips to
/// another point of its circle — the same shrug the swell gives, and as
/// unlikely to be watched when it happens.
fn soar(
    time: Res<Time>,
    ground: Res<Ground>,
    chunks: Query<&TerrainChunk>,
    mut eagles: Query<(&Eagle, &Shy, &ChildOf, &mut Transform)>,
) {
    for (eagle, shy, chunk, mut transform) in &mut eagles {
        let angle =
            eagle.phase + eagle.turn * (SOAR_SPEED / eagle.radius) * time.elapsed_secs_wrapped();
        let (sin, cos) = angle.sin_cos();
        let mut at = eagle.centre + Vec3::new(cos, 0.0, sin) * eagle.radius;

        let Ok(on) = chunks.get(chunk.parent()) else {
            continue;
        };
        let origin = on.coords.as_vec2() * CHUNK_METRES;
        if let Some(under) = ground.height(origin.x + at.x, origin.y + at.z) {
            at.y = at.y.max(under + SOAR_GROUND_CLEARANCE);
        }

        // Giving way is upward, and eased by [`give_way`] both on and off, so
        // the bird soars clear rather than teleporting there.
        at.y += EAGLE_LIFT * shy.minding;

        let along = Vec3::new(-sin, 0.0, cos) * eagle.turn;
        transform.translation = at;
        // Nose along the flight, then rolled about it — into the turn, which
        // for either handedness is the wing nearer the summit dipping.
        transform.rotation = Transform::default().looking_to(along, Vec3::Y).rotation
            * Quat::from_rotation_z(-eagle.turn * SOAR_BANK);
    }
}

// --- Crossings -------------------------------------------------------------

/// One kind of crossing, whole: what it asks of the sea before it will be
/// made, and what it is made of. Every number that differs between a pod, a
/// line of seabirds and a whale is here, so the differences can be read in
/// one place — and so the spawner below can be written once.
struct Crossing {
    /// What the formation is called in the entity tree.
    name: &'static str,
    /// The members, and how they sit in it.
    members: Members,
    /// The ring around the camera's focus the formation enters on, in
    /// metres: far enough to appear as "out there" rather than materialising
    /// alongside, near enough to be seen without being looked for.
    ring: (f32, f32),
    /// How far abeam of the focus the course is laid, at most. Aimed near
    /// the watcher rather than at or away: the crossing passes the boat,
    /// which is the whole spectacle.
    abeam: f32,
    /// The formation's way, in metres per second — jittered a little per
    /// spawning so no two crossings keep exactly the same time.
    speed: f32,
    /// Seconds the crossing runs before it may retire.
    life: f32,
    /// The heights the ground under the course must keep to, in metres —
    /// how each kind claims its own water. Dolphins ask only for depth;
    /// whales for real depth; seabirds for the narrow band that *is* the
    /// shallows, which confines their lines to just off a coast.
    band: (f32, f32),
    /// Metres past the crossing's own length the course is sounded for —
    /// the grace a retirement may spend waiting for the last back to go
    /// under. Zero for flyers, which keep themselves clear of the ground
    /// live instead.
    overrun: f32,
    /// How close the player may come, in metres, before the formation gives
    /// way — its members' [`Shy::wary`].
    wary: f32,
}

/// What a crossing's members are, and how they hold their places in it. The
/// two shapes retire differently and move differently, and nothing else
/// about them differs at all.
enum Members {
    /// A school: dolphins or whales, porpoising at their stations.
    Swimming(Pod),
    /// A line: seabirds strung out along the course.
    Flying(Line),
}

/// A swimming formation's members. A dolphin and a whale are the same animal
/// at different sizes and tempos, so the difference between a pod and a pair
/// of whales is these numbers and not two spawners.
struct Pod {
    kind: Kind,
    /// What one member is called in the entity tree.
    name: &'static str,
    /// Where members swim relative to the leader — abreast-and-behind in a
    /// loose echelon — and how many of the stations get used, fewest to
    /// most inclusive. The leader's own station comes first.
    stations: &'static [(f32, f32)],
    count: (usize, usize),
    /// Radians between neighbours in the porpoising cycle — enough that a
    /// pod surfaces as a run of arcs rather than a synchronised display
    /// team.
    stagger: f32,
    /// Metres of station jitter, so no member sits exactly where its
    /// station says.
    slop: f32,
    /// The motion every member shares. Its [`Swimmer::phase`] is the
    /// leader's; the rest are staggered off it.
    swimmer: Swimmer,
}

/// A flying formation's members: birds strung out behind the leader, all on
/// one undulation.
struct Line {
    kind: Kind,
    name: &'static str,
    count: (usize, usize),
    /// Metres of course between one bird and the next.
    spacing: f32,
    slop: f32,
}

static DOLPHINS: Crossing = Crossing {
    name: "Pod",
    members: Members::Swimming(Pod {
        kind: Kind::Dolphin,
        name: "Dolphin",
        stations: &[(0.0, 0.0), (-1.7, 2.1), (1.7, 2.4), (-3.3, 4.6), (3.4, 4.9)],
        count: (2, 5),
        stagger: 0.45,
        slop: 0.6,
        swimmer: Swimmer {
            // All under water at the spawn — the sine's trough — so a pod
            // enters the world unseen and *surfaces*.
            phase: -FRAC_PI_2,
            period: 3.2,
            leap: 2.2,
            cruise: 1.3,
            refuge: 2.0,
            hidden_below: -1.2,
        },
    }),
    ring: (90.0, 150.0),
    abeam: 50.0,
    speed: 4.5,
    life: 36.0,
    band: (f32::NEG_INFINITY, -4.0),
    overrun: 30.0,
    wary: 25.0,
};

static SEABIRDS: Crossing = Crossing {
    name: "Seabird line",
    members: Members::Flying(Line {
        kind: Kind::Seabird,
        name: "Seabird",
        count: (3, 7),
        spacing: 3.4,
        slop: 0.5,
    }),
    ring: (70.0, 140.0),
    abeam: 50.0,
    speed: 6.0,
    life: 20.0,
    // Shallower than open floor, wetter than the beach: the strip of sea
    // that holds surf. An open-ocean course has no such band to offer, so
    // seabirds are a thing that happens near coasts — which is where the
    // real ones fly their lines. The ceiling is barely under the waterline
    // on purpose: a knee-deep lagoon is exactly seabird country, and any
    // sandbar the striding samples miss is [`skim`]'s live clearance to
    // rise over, not this sounding's to forbid.
    band: (-7.0, -0.3),
    overrun: 0.0,
    wary: 35.0,
};

static WHALES: Crossing = Crossing {
    name: "Whales",
    members: Members::Swimming(Pod {
        kind: Kind::Whale,
        name: "Whale",
        // A second whale swims off the leader's quarter, well clear.
        stations: &[(0.0, 0.0), (7.0, 12.0)],
        count: (1, 2),
        stagger: 0.9,
        slop: 1.0,
        swimmer: Swimmer {
            phase: -FRAC_PI_2,
            period: 9.0,
            leap: 1.9,
            cruise: 2.4,
            refuge: 2.0,
            hidden_below: -2.8,
        },
    }),
    ring: (110.0, 190.0),
    abeam: 60.0,
    speed: 2.2,
    life: 45.0,
    band: (f32::NEG_INFINITY, -7.0),
    overrun: 40.0,
    wary: 45.0,
};

/// Stride at which a course is sounded before it is swum or flown, in
/// metres. Finer than the facet grid, so a one-facet pinnacle cannot slip
/// between samples.
const COURSE_SOUNDING: f32 = 12.0;

/// Quiet after a crossing retires before the next may be tried, in seconds —
/// and the much shorter pause after a try that found no fitting water, which
/// is not worth a minute of silence.
const CROSSING_REST: f32 = 45.0;
const CROSSING_RETRY: f32 = 2.0;

/// A flying formation's height over the water, the metres of rise and fall
/// its line undulates through, and the metres of course one undulation
/// spans. The phase is laid along the course rather than along time, so
/// each bird crosses a crest where the bird ahead of it did — which is what
/// a line of seabirds does, and why it reads as one animal.
const SKIM_HEIGHT: f32 = 2.6;
const UNDULATION: f32 = 0.9;
const UNDULATION_LENGTH: f32 = 26.0;

/// The climb a flying formation puts on when the hull nears, in metres.
const SKIM_CLIMB: f32 = 7.0;

/// How far from the camera's focus a flying formation may retire, in
/// metres: far enough that a bird winking out is a speck winking out.
const FLOCK_GONE: f32 = 450.0;

/// One formation on its crossing. The transform carries where its leader is
/// and which way the whole faces; this carries what the transform must not
/// be asked to remember.
#[derive(Component)]
pub struct Formation {
    /// Which kind of crossing this is — every number the kind fixes is read
    /// off it rather than copied in here.
    kind: &'static Crossing,
    /// The course, as a unit vector on the map.
    heading: Vec2,
    /// Its way along it, in metres per second — the kind's speed, jittered a
    /// little per spawning, so this one is its own.
    speed: f32,
    /// `Time::elapsed_secs` at the spawn, which every member's cycle counts
    /// from.
    born: f32,
}

/// Marks a formation that swims — it retires under water.
#[derive(Component)]
pub struct School;

/// Marks a formation that flies — it retires out at the edge of sight.
#[derive(Component)]
pub struct Flock;

/// One swimming member, at a fixed station in the formation's frame. Only
/// its height and pitch are its own: a sine about cruising depth, and the
/// sine's own slope. A dolphin and a whale are the same motion at different
/// sizes and tempos, so one component carries the numbers.
#[derive(Component)]
pub struct Swimmer {
    /// Where in the porpoising cycle this one is.
    phase: f32,
    /// Seconds one cycle takes.
    period: f32,
    /// The sine's size, in metres.
    leap: f32,
    /// How far under the waterline the cycle is centred.
    cruise: f32,
    /// Extra depth taken up when the formation has given way.
    refuge: f32,
    /// The height below which the whole animal is out of sight through the
    /// water's near-opacity — where retiring, and nothing else, may happen.
    /// Deeper for a whale than a dolphin: more back to hide.
    hidden_below: f32,
}

/// One flying member of a line, at a fixed station. Its phase places it on
/// the line's undulation — see [`UNDULATION_LENGTH`].
#[derive(Component)]
pub struct Seabird {
    phase: f32,
}

/// A course for a crossing near `focus`, or `None` — from where it enters
/// to the heading it holds, every stride of it sounded and found inside the
/// kind's own band.
///
/// `entropy` is decorative randomness: which bearing, how far out, how far
/// abeam the course is laid. A try that fails is simply tried again later
/// with fresh bits, so this needs no cleverness about *finding* fitting
/// water — only honesty about refusing a course it cannot vouch for,
/// unarrived chunks included.
fn plan_course(
    ground: &Ground,
    focus: Vec2,
    entropy: u32,
    crossing: &Crossing,
) -> Option<(Vec2, Vec2)> {
    let slice = |salt: u32| scramble(entropy ^ salt) as f32 / u32::MAX as f32;

    let bearing = TAU * slice(0x0B5E);
    let (near, far) = crossing.ring;
    let start = focus + Vec2::from_angle(bearing) * (near + (far - near) * slice(0x51DE));
    let abeam = focus
        + Vec2::from_angle(bearing + FRAC_PI_2) * crossing.abeam * (slice(0xABEA) * 2.0 - 1.0);
    let heading = (abeam - start).normalize_or_zero();
    if heading == Vec2::ZERO {
        return None;
    }

    // The whole crossing plus its overrun, and two strides behind the leader
    // for the tail of the formation — none of it may leave the band.
    let length = crossing.speed * crossing.life + crossing.overrun;
    let strides = (length / COURSE_SOUNDING).ceil() as i32;
    let (lowest, highest) = crossing.band;
    for stride in -2..=strides {
        let at = start + heading * (length * stride as f32 / strides as f32);
        match ground.height(at.x, at.y) {
            Some(height) if (lowest..=highest).contains(&height) => {}
            _ => return None,
        }
    }
    Some((start, heading))
}

/// The metronome between crossings, counted on `Time::elapsed_secs`.
#[derive(Resource, Default)]
struct CrossingClock {
    next_try: f32,
}

/// Sends a formation across the neighbourhood whenever the sea has been
/// quiet for long enough — one crossing at a time; wildlife is an event, not
/// a population. Which kind is the entropy's choice: pods and seabird lines
/// often, a whale seldom, and whatever was rolled only happens if the water
/// near the focus fits it — so open ocean gets no seabirds and a shallow
/// anchorage no whales, without anything here knowing where the coast is.
fn send_crossings(
    mut commands: Commands,
    time: Res<Time>,
    mut clock: ResMut<CrossingClock>,
    models: Res<WildlifeModels>,
    ground: Res<Ground>,
    cameras: Query<&MapCamera>,
    crossings: Query<(), With<Formation>>,
) {
    let now = time.elapsed_secs();
    if !crossings.is_empty() || now < clock.next_try {
        return;
    }
    let Ok(camera) = cameras.single() else {
        return;
    };
    let focus = camera.focus.xz();

    let entropy = scramble(time.elapsed().as_millis() as u32);
    let kind = match scramble(entropy ^ 0x5EA5) % 100 {
        0..45 => &DOLPHINS,
        45..80 => &SEABIRDS,
        _ => &WHALES,
    };
    let Some((start, heading)) = plan_course(&ground, focus, entropy, kind) else {
        clock.next_try = now + CROSSING_RETRY;
        return;
    };
    clock.next_try = now + kind.life + CROSSING_REST;

    // The station jitter — "a bit of random variation": no member sits
    // exactly on its station, in place or in rhythm, or a formation would
    // read as one animal stamped repeatedly, the way a regular palm crown
    // would tile.
    let jitter = |member: usize, salt: u32| signed(entropy ^ ((member as u32) << 8), salt);

    let formation = (
        Name::new(kind.name),
        Formation {
            kind,
            heading,
            speed: kind.speed * (0.92 + 0.16 * unit(entropy, 0x3EED)),
            born: now,
        },
        Shy::of(kind.wary),
        DespawnOnExit(AppState::InWorld),
        Transform::from_xyz(start.x, 0.0, start.y)
            .looking_to(Vec3::new(heading.x, 0.0, heading.y), Vec3::Y),
        Visibility::default(),
    );

    match &kind.members {
        Members::Swimming(pod) => {
            let count = between(entropy, 0x90D5, pod.count).min(pod.stations.len());
            commands.spawn((School, formation)).with_children(|school| {
                for (member, (side, lag)) in pod.stations.iter().enumerate().take(count) {
                    let swimmer = Swimmer {
                        phase: pod.swimmer.phase
                            + member as f32 * pod.stagger
                            + 0.12 * jitter(member, 0xD01),
                        ..pod.swimmer
                    };
                    school.spawn((
                        Name::new(pod.name),
                        Transform::from_xyz(
                            side + pod.slop * jitter(member, 0xD02),
                            -(swimmer.cruise + swimmer.leap),
                            lag + pod.slop * jitter(member, 0xD03),
                        ),
                        swimmer,
                        models[pod.kind].drawn_as(),
                    ));
                }
            });
        }
        Members::Flying(line) => {
            let count = between(entropy, 0x5B1D, line.count);
            commands.spawn((Flock, formation)).with_children(|flock| {
                for member in 0..count {
                    let lag = member as f32 * line.spacing + line.slop * jitter(member, 0x5B2);
                    flock.spawn((
                        Name::new(line.name),
                        Seabird {
                            // Phase from *place* on the line, not from the
                            // bird: each crosses a crest where the bird
                            // ahead did.
                            phase: -lag * TAU / UNDULATION_LENGTH,
                        },
                        Transform::from_xyz(line.slop * jitter(member, 0x5B3), SKIM_HEIGHT, lag),
                        models[line.kind].drawn_as(),
                    ));
                }
            });
        }
    }
}

/// Carries every formation along its course.
fn advance(time: Res<Time>, mut formations: Query<(&Formation, &mut Transform)>) {
    for (formation, mut transform) in &mut formations {
        transform.translation += Vec3::new(formation.heading.x, 0.0, formation.heading.y)
            * formation.speed
            * time.delta_secs();
    }
}

/// Where a member of a formation stands on the map, and where the water is
/// there — the opening move of both [`porpoise`] and [`skim`], which differ
/// only in what they do with the answer.
///
/// The station is read out of the member's own transform in the plane and
/// carried into the world through the formation's, so a member swims where
/// its station says however the group is headed. The swell is the wrapped
/// clock's, the same one the water is drawn on, so a leap crests a wave
/// rather than some flat remembered ocean.
fn over_the_water(
    ground: &Ground,
    conditions: &SeaConditions,
    carrier: &Transform,
    station: &Transform,
    elapsed: f32,
) -> (Vec3, f32) {
    let at = carrier.transform_point(Vec3::new(station.translation.x, 0.0, station.translation.z));
    (at, conditions.water_over(Some(ground), at.xz(), elapsed))
}

/// Rides every swimmer through its arcs: a sine about cruising depth for the
/// height, its own derivative for the pitch — so the nose enters the water
/// where the leap is falling, which is the whole of what makes an arc read
/// as a leap rather than a bob.
///
/// The sine stands on the swell at the swimmer's own spot of sea, on the
/// same wrapped clock the water is drawn with, so a leap crests a wave
/// rather than some flat remembered ocean. A shy formation sounds: the
/// cycle's centre sinks by the refuge and its size closes toward nothing,
/// so a pod the boat bears down on simply slips under and cruises.
fn porpoise(
    time: Res<Time>,
    ground: Res<Ground>,
    conditions: Res<SeaConditions>,
    formations: Query<(&Formation, &Shy, &Transform)>,
    mut swimmers: Query<(&Swimmer, &ChildOf, &mut Transform), Without<Formation>>,
) {
    let elapsed = time.elapsed_secs_wrapped();
    for (swimmer, of, mut transform) in &mut swimmers {
        let Ok((formation, shy, carrier)) = formations.get(of.parent()) else {
            continue;
        };
        let (_, water) = over_the_water(&ground, &conditions, carrier, &transform, elapsed);

        let bold = 1.0 - shy.minding;
        let (rise, run) = (TAU / swimmer.period * (time.elapsed_secs() - formation.born)
            + swimmer.phase)
            .sin_cos();
        transform.translation.y =
            water - swimmer.cruise - shy.minding * swimmer.refuge + swimmer.leap * bold * rise;
        let pitch = (swimmer.leap * bold * TAU / swimmer.period * run).atan2(formation.speed);
        transform.rotation = Quat::from_rotation_x(pitch);
    }
}

/// Flies every seabird along its line: a fixed height over the swell, the
/// line's own undulation on top, and the whole line lifting while the
/// formation is shy — birds give way upward, being the one direction the
/// boat cannot follow.
///
/// The ground is sampled live rather than trusted to the sounded course:
/// past its crossing a line keeps flying until it is out of sight, over
/// whatever happens to be there, and a bird meets a headland by rising over
/// it — it is a bird.
fn skim(
    time: Res<Time>,
    ground: Res<Ground>,
    conditions: Res<SeaConditions>,
    formations: Query<(&Formation, &Shy, &Transform)>,
    mut seabirds: Query<(&Seabird, &ChildOf, &mut Transform), Without<Formation>>,
) {
    let elapsed = time.elapsed_secs_wrapped();
    for (seabird, of, mut transform) in &mut seabirds {
        let Ok((formation, shy, carrier)) = formations.get(of.parent()) else {
            continue;
        };
        let (at, water) = over_the_water(&ground, &conditions, carrier, &transform, elapsed);

        let along =
            TAU / UNDULATION_LENGTH * formation.speed * (time.elapsed_secs() - formation.born);
        let (rise, run) = (along + seabird.phase).sin_cos();
        let flying = water + SKIM_HEIGHT + UNDULATION * rise + shy.minding * SKIM_CLIMB;
        // Unknown ground counts as sea: it is far away by construction, and
        // a decorative bird guessing low is a bird the haze already hides.
        let standing = ground.surface(at.x, at.z).unwrap_or(0.0);
        transform.translation.y = flying.max(standing + SKIM_HEIGHT);
        let pitch =
            (UNDULATION * TAU / UNDULATION_LENGTH * formation.speed * run).atan2(formation.speed);
        transform.rotation = Quat::from_rotation_x(pitch);
    }
}

/// Retires swimming formations whose crossing is done — while every member
/// is under water, so the despawn happens where it cannot be watched. The
/// grace period is for freak seas: if the swell somehow keeps a back wet
/// long past time, the formation goes anyway rather than swimming off the
/// end of its sounded course.
fn retire_schools(
    mut commands: Commands,
    time: Res<Time>,
    schools: Query<(Entity, &Formation, &Children), With<School>>,
    swimmers: Query<(&Swimmer, &Transform)>,
) {
    let now = time.elapsed_secs();
    for (entity, formation, members) in &schools {
        let age = now - formation.born;
        if age < formation.kind.life {
            continue;
        }
        let hidden = members.iter().all(|member| {
            swimmers
                .get(member)
                .is_ok_and(|(swimmer, t)| t.translation.y < swimmer.hidden_below)
        });
        if hidden || age > formation.kind.life + 25.0 {
            commands.entity(entity).despawn();
        }
    }
}

/// Retires flying formations whose crossing is done — once they are far
/// enough from the focus that a vanishing bird is a vanishing speck. A bird
/// cannot hide the way a swimmer can, so distance is its only exit; the
/// hard cap is for a player who takes to chasing one.
fn retire_flocks(
    mut commands: Commands,
    time: Res<Time>,
    cameras: Query<&MapCamera>,
    flocks: Query<(Entity, &Formation, &Transform), With<Flock>>,
) {
    let Ok(camera) = cameras.single() else {
        return;
    };
    let focus = camera.focus.xz();
    let now = time.elapsed_secs();
    for (entity, formation, transform) in &flocks {
        let age = now - formation.born;
        let gone = transform.translation.xz().distance(focus) > FLOCK_GONE;
        if (age > formation.kind.life && gone) || age > 3.0 * formation.kind.life {
            commands.entity(entity).despawn();
        }
    }
}

// --- The plumbing they share -----------------------------------------------

/// The meshes and materials every creature of a kind shares, loaded once.
#[derive(Resource)]
struct WildlifeModels([Creature; KINDS.len()]);

/// What one kind is drawn with — shared by every creature of it, so a whole
/// pod draws in one call rather than one apiece.
struct Creature {
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
}

impl Creature {
    /// The components that put one on the screen. Visibility is spelled out
    /// rather than left to the spawner: a creature that inherited nothing
    /// would never be drawn, and the ones hanging off a chunk have a parent
    /// to inherit from.
    fn drawn_as(&self) -> (Mesh3d, MeshMaterial3d<StandardMaterial>, Visibility) {
        (
            Mesh3d(self.mesh.clone()),
            MeshMaterial3d(self.material.clone()),
            Visibility::default(),
        )
    }
}

impl Index<Kind> for WildlifeModels {
    type Output = Creature;

    fn index(&self, kind: Kind) -> &Creature {
        &self.0[kind as usize]
    }
}

fn load_the_models(
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    assets: Res<AssetServer>,
) {
    commands.insert_resource(WildlifeModels(KINDS.map(|(file, colour)| Creature {
        mesh: assets.load(model_mesh(file, 0)),
        material: materials.add(matte(colour)),
    })));
}

/// Stirs bits until they stop resembling what they were — SplitMix's mixing
/// rounds, without its sequence. Decorative randomness only: nothing fed
/// through this may ever need to agree with another machine, and the one
/// caller that wants agreement anyway (the eagles) gets it by feeding in
/// chunk coordinates, which already agree.
fn scramble(mut x: u32) -> u32 {
    x = x.wrapping_add(0x9E37_79B9);
    x ^= x >> 16;
    x = x.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 13;
    x = x.wrapping_mul(0xC2B2_AE35);
    x ^ (x >> 16)
}

/// A decorative number in `0.0..1.0` from some bits and a salt — the one way
/// this module turns entropy into a quantity, so a bearing, a jitter and a
/// speed are all drawn the same way.
fn unit(entropy: u32, salt: u32) -> f32 {
    scramble(entropy ^ salt) as f32 / u32::MAX as f32
}

/// The same, in `-1.0..1.0`: a wobble either way about whatever it is added
/// to.
fn signed(entropy: u32, salt: u32) -> f32 {
    unit(entropy, salt) * 2.0 - 1.0
}

/// One of `range.0..=range.1`, evenly.
fn between(entropy: u32, salt: u32, range: (usize, usize)) -> usize {
    range.0 + scramble(entropy ^ salt) as usize % (range.1 + 1 - range.0)
}

pub struct WildlifePlugin;

impl Plugin for WildlifePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CrossingClock>()
            .add_systems(Startup, load_the_models)
            .add_systems(
                Update,
                (
                    watch_summits,
                    send_crossings,
                    advance,
                    give_way,
                    soar,
                    porpoise,
                    skim,
                    retire_schools,
                    retire_flocks,
                )
                    .chain()
                    .run_if(in_state(AppState::InWorld).and_then(resource_exists::<Ground>)),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration;

    use bevy::state::app::StatesPlugin;
    use bevy::time::{TimePlugin, TimeUpdateStrategy};
    use protocol::ground::{quantize, ChunkPayload, Surface, Tone, FACET_TRIS, FACET_VERTS};

    use crate::testing::{assert_model_draws, test_ground, triangles};

    /// Every kind of crossing there is — which of them a moment gets is
    /// `send_crossings`' roll, and this is for saying something about the lot
    /// of them at once.
    const CROSSINGS: [&Crossing; 3] = [&DOLPHINS, &SEABIRDS, &WHALES];

    /// Every course this entropy can lay for a kind, tried until one is
    /// found. Which bits fit is nobody's business, so a claim about the water
    /// near a spot is a claim about the whole run of tries rather than about
    /// any one of them.
    fn some_course(ground: &Ground, focus: Vec2, kind: &Crossing) -> Option<(Vec2, Vec2)> {
        (0..200).find_map(|entropy| plan_course(ground, focus, scramble(entropy), kind))
    }

    /// A chunk payload holding one round hill: `height` metres at grid corner
    /// `peak`, falling off with distance, over a low plain.
    fn a_hill(peak: (usize, usize), height: f32) -> ChunkPayload {
        ChunkPayload {
            heights: (0..FACET_VERTS * FACET_VERTS)
                .map(|i| {
                    let dx = (i % FACET_VERTS) as f32 - peak.0 as f32;
                    let dz = (i / FACET_VERTS) as f32 - peak.1 as f32;
                    quantize((height - (dx * dx + dz * dz).sqrt()).max(1.0))
                })
                .collect(),
            surfaces: vec![Surface::plain(Tone::Grass); FACET_TRIS],
            water: None,
            palms: Vec::new(),
        }
    }

    /// A wide flat floor of sea at one depth, in metres below the waterline —
    /// which is the whole of what a course is sounded against, so a depth is
    /// a choice of which kinds may cross it.
    fn a_floor(depth: f32) -> Ground {
        let mut ground = Ground::default();
        for cz in -6..=6 {
            for cx in -6..=6 {
                ground.deliver(
                    IVec2::new(cx, cz),
                    Some(ChunkPayload {
                        heights: vec![quantize(-depth); FACET_VERTS * FACET_VERTS],
                        surfaces: vec![Surface::plain(Tone::Sand); FACET_TRIS],
                        water: None,
                        palms: Vec::new(),
                    }),
                );
            }
        }
        ground
    }

    /// Three metres of water: the band a seabird's line accepts and a pod's
    /// or a whale's refuses.
    fn a_shelf() -> Ground {
        a_floor(3.0)
    }

    #[test]
    fn an_eagle_watches_an_interior_summit() {
        let mut ground = Ground::default();
        let chunk = IVec2::new(3, -2);
        ground.deliver(chunk, Some(a_hill((20, 40), 80.0)));
        let centre = eyrie(&ground, chunk).expect("a summit this high holds an eagle");
        // The summit's corner is at facet coordinates times the facet stride,
        // in the chunk's own frame; the circle is flown above it.
        assert_eq!(centre, Vec3::new(40.0, 80.0 + SOAR_CLEARANCE, 80.0));
    }

    #[test]
    fn no_eagle_below_the_mountains() {
        let mut ground = Ground::default();
        let chunk = IVec2::new(3, -2);
        ground.deliver(chunk, Some(a_hill((20, 40), EYRIE_HEIGHT - 5.0)));
        assert_eq!(eyrie(&ground, chunk), None);
    }

    #[test]
    fn a_summit_on_the_border_is_the_neighbours() {
        // The test island peaks exactly at the origin — a corner all four
        // chunks around it share, so each sees its own highest point on its
        // border and none of them raises the bird. The seam case; a real
        // summit lands inside somebody's chunk.
        let ground = test_ground();
        for chunk in [
            IVec2::new(0, 0),
            IVec2::new(-1, 0),
            IVec2::new(0, -1),
            IVec2::new(-1, -1),
        ] {
            assert_eq!(eyrie(&ground, chunk), None);
        }
    }

    #[test]
    fn crossings_keep_off_the_island() {
        // Anchored dead over the island, every ring a course could start on
        // is ground or shallows: nothing may be found for any kind, whatever
        // the entropy says.
        let ground = test_ground();
        for kind in CROSSINGS {
            assert_eq!(some_course(&ground, Vec2::ZERO, kind), None);
        }
    }

    #[test]
    fn pods_and_whales_cross_open_water() {
        // Anchored off the coast there is honest deep water on the seaward
        // side, and some try finds it — and what it finds it has sounded, so
        // the course it answers stays clear of the island by construction.
        let ground = test_ground();
        for kind in [&DOLPHINS, &WHALES] {
            let (start, _) = some_course(&ground, Vec2::new(400.0, 0.0), kind)
                .expect("open water near the focus holds some course");
            assert!(
                start.length() > crate::testing::TEST_ISLAND_REACH,
                "a crossing started over the island at {start}"
            );
        }
    }

    #[test]
    fn seabirds_hug_the_shallows() {
        // Over a three-metre shelf the seabirds' band fits and the swimmers'
        // do not — which is the whole sorting: lines of birds happen off
        // beaches, pods and whales out at sea.
        let shelf = a_shelf();
        assert!(some_course(&shelf, Vec2::ZERO, &SEABIRDS).is_some());
        for kind in [&DOLPHINS, &WHALES] {
            assert_eq!(some_course(&shelf, Vec2::ZERO, kind), None);
        }

        // And the open sea offers a seabird line nothing: the floor of the
        // test island's surroundings is below the band, like all open floor.
        let open = test_ground();
        assert_eq!(some_course(&open, Vec2::new(400.0, 0.0), &SEABIRDS), None);
    }

    /// A headless app with the wildlife running over `ground`, in a match and
    /// with a camera for the crossings to surface near. No render app and no
    /// waiting on the models: what this is about is where creatures are put,
    /// not what they look like.
    ///
    /// The clock is stepped by hand, at a tenth of a second a frame. Headless
    /// frames take next to no real time, and which kind of crossing is tried
    /// is drawn from the elapsed milliseconds — so a run on the true clock
    /// would roll the same kind over and over, and a fixed step makes the
    /// sequence of tries the same on every machine as well as varied.
    fn test_app(ground: Ground) -> App {
        let mut app = App::new();
        app.add_plugins((
            TaskPoolPlugin::default(),
            AssetPlugin::default(),
            TimePlugin,
            StatesPlugin,
            WildlifePlugin,
        ))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            100,
        )))
        .init_state::<AppState>()
        .init_asset::<Mesh>()
        .init_resource::<Assets<StandardMaterial>>()
        .init_resource::<SeaConditions>()
        .insert_resource(ground);
        app.world_mut().spawn(MapCamera::default());
        app.update();
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::InWorld);
        app.update();
        app
    }

    #[test]
    fn a_crossing_puts_every_member_somewhere_real() {
        // The whole spawn-and-swim path, end to end, and the assertion worth
        // making about it from outside: every creature is at a place. A
        // member reading the swell over ground the client has not been sent
        // used to come back NaN — see `sea::water_over` — and a NaN
        // translation is a creature that silently stops being drawn, with
        // nothing failing anywhere to say so.
        //
        // Deep water, so the kinds that want it can cross; a seabird roll
        // over it simply finds nothing and is tried again a moment later,
        // which is the retry doing its job.
        let mut app = test_app(a_floor(20.0));
        let mut members = Vec::new();
        for _ in 0..200 {
            app.update();
            let mut formations = app
                .world_mut()
                .query_filtered::<&Children, With<Formation>>();
            if let Ok(children) = formations.single(app.world()) {
                members = children.iter().collect();
                break;
            }
        }
        assert!(
            !members.is_empty(),
            "twenty seconds of open water sent nothing across it"
        );

        // And they keep being somewhere as the crossing runs, not just at the
        // spawn: the swell is read afresh under every member every frame.
        for _ in 0..100 {
            app.update();
            for member in &members {
                let at = app
                    .world()
                    .entity(*member)
                    .get::<Transform>()
                    .expect("a member stands somewhere")
                    .translation;
                assert!(at.is_finite(), "a member of the crossing is at {at}");
            }
        }
    }

    #[test]
    fn a_creature_minds_a_player_within_its_wary_radius() {
        // The give-way rule itself, which both an eagle and a pod hang off.
        let swimmer = Shy::of(20.0);
        let boat = Vec3::new(10.0, 0.0, 0.0);
        assert!(minds(&swimmer, Vec3::ZERO, Some(boat)));
        assert!(!minds(&swimmer, Vec3::new(-30.0, 0.0, 0.0), Some(boat)));
        // Nobody playing is nobody to mind — a shot of the menu.
        assert!(!minds(&swimmer, Vec3::ZERO, None));

        // Height is a bird's business alone: a swimmer minds a hull overhead
        // exactly as it minds one alongside, while an eagle already well
        // above the masthead has nothing to climb away from.
        let overhead = Vec3::new(0.0, 60.0, 0.0);
        assert!(minds(&swimmer, Vec3::ZERO, Some(overhead)));
        let eagle = Shy {
            headroom: EAGLE_HEADROOM,
            ..Shy::of(EAGLE_WARY)
        };
        assert!(minds(&eagle, Vec3::new(0.0, 70.0, 0.0), Some(overhead)));
        assert!(!minds(&eagle, Vec3::new(0.0, 100.0, 0.0), Some(overhead)));
    }

    #[test]
    fn the_models_are_one_creature_each_fit_to_draw() {
        // See `assert_model_draws`. Each file holds one mesh, at position 0,
        // and it is the animal the file is named for — which is the one thing
        // about them the game cannot see for itself.
        for (file, _) in KINDS {
            let creature = file.strip_suffix(".glb").expect("a glTF binary");
            assert_model_draws(file, &[(0, creature)]);
        }
    }

    #[test]
    fn the_wildlife_is_built_to_scale() {
        // What the spawners assume when they place the models unscaled: a
        // remodel that came through in centimetres — or with the exporter's
        // axes wrong — would fly a hundred-metre bird. Wingspans lie along X;
        // nose-to-tail lengths along Z, the forward axis.
        let span = |kind: Kind, axis: usize| -> f32 {
            let corners: Vec<f32> = triangles(KINDS[kind as usize].0, 0, "POSITION")
                .into_iter()
                .flatten()
                .map(|corner| corner[axis])
                .collect();
            corners.iter().fold(f32::MIN, |a, b| a.max(*b))
                - corners.iter().fold(f32::MAX, |a, b| a.min(*b))
        };
        for (kind, axis, wanted) in [
            (Kind::Eagle, 0, 3.0..4.5),
            (Kind::Dolphin, 2, 2.0..3.0),
            (Kind::Seabird, 0, 1.6..2.6),
            (Kind::Whale, 2, 9.0..13.0),
        ] {
            let measured = span(kind, axis);
            assert!(
                wanted.contains(&measured),
                "{} measures {measured}m across axis {axis}, not {wanted:?}",
                KINDS[kind as usize].0
            );
        }
    }
}
