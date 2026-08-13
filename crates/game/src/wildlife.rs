//! The wildlife: eagles over the summits, and lines of seabirds along the
//! shallows. The birds, in short — everything of fur and fin has moved on.
//!
//! None of it can be touched or pointed at, and those two facts decide the
//! architecture. A creature a player could interact with has to be the
//! server's — authoritative, synchronised, on the wire the way other players
//! are — because two machines are only free to disagree about what nobody
//! can act on. And a creature worth *pointing at* has to be the server's
//! too, or "look!" is a thing that happens to one player at a time. Both of
//! those kinds are the *beasts* now — sharks, pods of dolphins, the odd
//! whale, server-owned and wire-borne, drawn by [`crate::beasts`]; the
//! dolphins and whales lived here once, and moved out when the second rule
//! was understood. What stays is the texture nobody compares notes on:
//! birds are everywhere and nowhere in particular, so each client raises
//! its own out of nothing but the ground it was already sent and its own
//! clock, and the protocol is untouched.
//!
//! The birds come in two shapes, at opposite ends of what decoration can be:
//!
//! - An **eagle** belongs to a *place*. A chunk whose ground holds a summit
//!   worth the name gets a bird — sometimes a pair — circling it. A summit
//!   is a fact about the chunk grid, so every client raises eagles over the
//!   same peaks without a word crossing the wire, and the birds hang off the
//!   chunk entity so streaming despawns them with the ground. Each bird
//!   carries its own circle: a pair is two birds on one thermal rather than
//!   a thing of its own.
//! - A **crossing** belongs to a *moment*: a line of seabirds undulating
//!   along the shallows. It surfaces near whoever is looking and is gone; a
//!   client anchored a mile away gets its own. Here the unit is the
//!   [`Formation`] rather than the individual — the line carries the course
//!   and the lifetime, and a bird only ever knows its station in it and its
//!   phase of the line's own rhythm. It retires out at the edge of the
//!   haze, where a vanishing bird is a vanishing speck.
//!
//! And though nothing here can be touched, it can be *approached* — so the
//! one behaviour wildlife owes the player is absence: birds give way upward
//! as the player nears, eased rather than snapped, so an encounter reads as
//! the bird minding them and never as the boat passing through it. [`Shy`]
//! carries how much of the player a creature is currently minding and
//! [`give_way`] eases it on and off, while what being shy *means* stays
//! each kind's own business. (The beasts run their giving-way on the
//! server, where behaviour about a player belongs.)

use std::f32::consts::{FRAC_PI_2, TAU};
use std::ops::Index;

use bevy::math::Vec3Swizzles;
use bevy::prelude::*;
use protocol::ground::CHUNK_METRES;

use crate::camera::MapCamera;
use crate::player::PlayerPlace;
use crate::sea::SeaConditions;
use crate::terrain::{Ground, TerrainChunk};
use crate::{between, eased, matte, model_mesh, scramble, signed, unit, AppState};

/// The kinds of creature. Also an index into [`WildlifeModels`], so anything
/// that knows what it is can find what to draw it with.
#[derive(Clone, Copy)]
enum Kind {
    Eagle,
    Seabird,
}

/// What each kind is made of, in [`Kind`]'s own order: the file its mesh
/// comes out of, and the colour it is painted.
///
/// The models hold one mesh each, at position 0 — pinned by
/// `the_models_are_one_creature_each_fit_to_draw` the way the palm's order
/// is, which reads the file names here to know what it is looking for.
const KINDS: [(&str, Color); 2] = [
    ("models/eagle.glb", EAGLE_COLOR),
    ("models/seabird.glb", SEABIRD_COLOR),
];

/// Dark umber. An eagle is seen against sky or against sunlit rock, and in
/// both it is its silhouette — real plumage colour would only muddy a shape
/// a few pixels across.
const EAGLE_COLOR: Color = Color::srgb(0.24, 0.18, 0.13);

/// Chalk grey. A seabird is seen low against bright water, where a pale bird
/// is the one that reads — the real birds are mostly white for their own
/// reasons.
const SEABIRD_COLOR: Color = Color::srgb(0.84, 0.84, 0.80);

// --- Minding the player ------------------------------------------------------

/// How much of the player a creature is currently minding, and what it takes
/// to mind them at all. The one give-way rule the module doc promises: eased
/// on and off by [`give_way`], and read by whatever draws the creature.
#[derive(Component)]
pub struct Shy {
    /// `0.0` unbothered, `1.0` fully given way. What that *means* is the
    /// creature's own — an eagle climbs, a line lifts — so nothing here says.
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
/// made, and what it is made of. Written as data rather than in the spawner
/// because it used to be three kinds — pods and whales crossed here before
/// they became beasts — and the shape is worth keeping for whatever flies
/// next: a second bird is a second static, not a second spawner.
struct Crossing {
    /// What the formation is called in the entity tree.
    name: &'static str,
    /// The birds, and how they sit in the line.
    line: Line,
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
    /// how the crossing claims its own water.
    band: (f32, f32),
    /// How close the player may come, in metres, before the formation gives
    /// way — its members' [`Shy::wary`].
    wary: f32,
}

/// A flying formation's members: birds strung out behind the leader, all on
/// one undulation.
struct Line {
    kind: Kind,
    name: &'static str,
    count: (usize, usize),
    /// Metres of course between one bird and the next.
    spacing: f32,
    /// Metres of station jitter, so no bird sits exactly where its station
    /// says.
    slop: f32,
}

static SEABIRDS: Crossing = Crossing {
    name: "Seabird line",
    line: Line {
        kind: Kind::Seabird,
        name: "Seabird",
        count: (3, 7),
        spacing: 3.4,
        slop: 0.5,
    },
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
    wary: 35.0,
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
    let bearing = TAU * unit(entropy, 0x0B5E);
    let (near, far) = crossing.ring;
    let start = focus + Vec2::from_angle(bearing) * (near + (far - near) * unit(entropy, 0x51DE));
    let abeam =
        focus + Vec2::from_angle(bearing + FRAC_PI_2) * crossing.abeam * signed(entropy, 0xABEA);
    let heading = (abeam - start).normalize_or_zero();
    if heading == Vec2::ZERO {
        return None;
    }

    // The whole crossing, and two strides behind the leader for the tail of
    // the line — none of it may leave the band.
    let length = crossing.speed * crossing.life;
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
/// a population. The roll only lands if the water near the focus fits the
/// birds' band — so open ocean gets no seabirds, without anything here
/// knowing where the coast is.
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
    let kind = &SEABIRDS;
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

    let line = &kind.line;
    let count = between(entropy, 0x5B1D, line.count);
    commands
        .spawn((
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
        ))
        .with_children(|flock| {
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

/// Carries every formation along its course.
fn advance(time: Res<Time>, mut formations: Query<(&Formation, &mut Transform)>) {
    for (formation, mut transform) in &mut formations {
        transform.translation += Vec3::new(formation.heading.x, 0.0, formation.heading.y)
            * formation.speed
            * time.delta_secs();
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
        // The swell is the wrapped clock's, the same one the water is drawn
        // on, so an undulation crests a wave rather than some flat remembered
        // ocean.
        let (at, water) = conditions.under_station(Some(&ground), carrier, &transform, elapsed);

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

/// Retires formations whose crossing is done — once they are far enough
/// from the focus that a vanishing bird is a vanishing speck. A bird cannot
/// hide the way a swimmer could, so distance is its only exit; the hard cap
/// is for a player who takes to chasing one.
fn retire_flocks(
    mut commands: Commands,
    time: Res<Time>,
    cameras: Query<&MapCamera>,
    flocks: Query<(Entity, &Formation, &Transform)>,
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
/// line of birds draws in one call rather than one apiece.
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
                    skim,
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

    use crate::testing::{assert_model_draws, creature_named_by, span, test_ground};

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

    /// Three metres of water: the band a seabird's line accepts.
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
        // is ground or shallows too thin even for birds: nothing may be
        // found, whatever the entropy says.
        let ground = test_ground();
        assert_eq!(some_course(&ground, Vec2::ZERO, &SEABIRDS), None);
    }

    #[test]
    fn seabirds_hug_the_shallows() {
        // Over a three-metre shelf the seabirds' band fits — lines of birds
        // happen off beaches, and nowhere else.
        let shelf = a_shelf();
        assert!(some_course(&shelf, Vec2::ZERO, &SEABIRDS).is_some());

        // The open sea offers a seabird line nothing: the floor of the
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
    /// frames take next to no real time, and every roll a crossing makes is
    /// drawn from the elapsed milliseconds — so a fixed step makes the
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
        // The whole spawn-and-fly path, end to end, and the assertion worth
        // making about it from outside: every creature is at a place. A
        // member reading the swell over ground the client has not been sent
        // used to come back NaN — see `sea::water_over` — and a NaN
        // translation is a creature that silently stops being drawn, with
        // nothing failing anywhere to say so.
        //
        // Shelf water, which is the one band the birds will cross.
        let mut app = test_app(a_shelf());
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
        // The give-way rule itself, which the eagle and the line both hang
        // off.
        let line = Shy::of(20.0);
        let boat = Vec3::new(10.0, 0.0, 0.0);
        assert!(minds(&line, Vec3::ZERO, Some(boat)));
        assert!(!minds(&line, Vec3::new(-30.0, 0.0, 0.0), Some(boat)));
        // Nobody playing is nobody to mind — a shot of the menu.
        assert!(!minds(&line, Vec3::ZERO, None));

        // Headroom is the eagle's refinement: a line low over the water
        // minds a hull whatever the heights, while an eagle already well
        // above the masthead has nothing to climb away from.
        let overhead = Vec3::new(0.0, 60.0, 0.0);
        assert!(minds(&line, Vec3::ZERO, Some(overhead)));
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
            assert_model_draws(file, &[(0, creature_named_by(file))]);
        }
    }

    #[test]
    fn the_wildlife_is_built_to_scale() {
        // What the spawners assume when they place the models unscaled: a
        // remodel that came through in centimetres — or with the exporter's
        // axes wrong — would fly a hundred-metre bird. Wingspans lie along X.
        // The swimming kinds' pins moved to the beasts' tests with the
        // animals themselves.
        for (kind, axis, wanted) in [(Kind::Eagle, 0, 3.0..4.5), (Kind::Seabird, 0, 1.6..2.6)] {
            let file = KINDS[kind as usize].0;
            let measured = span(file, 0, axis);
            assert!(
                wanted.contains(&measured),
                "{file} measures {measured}m across axis {axis}, not {wanted:?}"
            );
        }
    }
}
