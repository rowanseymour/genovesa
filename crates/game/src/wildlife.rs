//! The wildlife: eagles over the summits and dolphins out at sea.
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
//! Within that, the two kinds sit at opposite ends of what decoration can be:
//!
//! - An **eagle** belongs to a *place*. A chunk whose ground holds a summit
//!   worth the name gets one, circling it, so a mountain reads as inhabited
//!   from the sea. A summit is a fact about the chunk grid, so every client
//!   raises eagles over the same peaks without a word crossing the wire —
//!   they agree the way the drawn ground agrees, for free.
//! - A **pod of dolphins** belongs to a *moment*. It surfaces near whoever
//!   is looking, crosses their view, and is gone; a client anchored a mile
//!   away gets its own. Nothing about it is shared, or needs to be.
//!
//! An eagle hangs off the chunk whose summit it circles, as the palms hang
//! off theirs, so streaming despawns it with the ground and nothing here
//! keeps a ledger of birds. A pod has no ground to belong to and carries its
//! own lifetime instead: it retires shortly after its crossing, under water,
//! where a despawn cannot be seen.

use std::f32::consts::{FRAC_PI_2, TAU};

use bevy::asset::AssetPath;
use bevy::gltf::GltfAssetLabel;
use bevy::prelude::*;
use protocol::ground::CHUNK_METRES;

use crate::camera::MapCamera;
use crate::sea::SeaConditions;
use crate::terrain::{Ground, TerrainChunk};
use crate::{matte, AppState};

/// The two models, one mesh each. Position 0 in each file, pinned by
/// `the_models_hold_one_mesh_each` the way the palm's order is.
const EAGLE_MODEL: &str = "eagle.glb";
const DOLPHIN_MODEL: &str = "dolphin.glb";

/// Dark umber. An eagle is seen against sky or against sunlit rock, and in
/// both it is its silhouette — real plumage colour would only muddy a shape
/// a few pixels across.
const EAGLE_COLOR: Color = Color::srgb(0.24, 0.18, 0.13);

/// Wet slate. Lighter than the deep sea it breaks out of and darker than the
/// spray-white a leap suggests, so the arc reads against the water at the
/// distances pods keep.
const DOLPHIN_COLOR: Color = Color::srgb(0.42, 0.50, 0.55);

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
/// massif that raised it.
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

/// An eagle, circling the summit of the chunk it hangs off.
#[derive(Component)]
pub struct Eagle {
    /// The centre of its circle, in the chunk's own frame: the summit, plus
    /// [`SOAR_CLEARANCE`].
    centre: Vec3,
    /// Where on the circle this bird was at time zero, in radians — from the
    /// chunk's coordinates, so the same bird is mid-lap in the same place
    /// however often its ground streams out and back in.
    phase: f32,
    /// Which way round: `1.0` or `-1.0`.
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

/// Gives every newly arrived chunk with a summit its eagle.
///
/// A child of the chunk entity, like a palm: it is drawn when the ground is
/// drawn and forgotten when streaming forgets the ground, and re-arrival runs
/// this again — same chunk, same summit, same bird.
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
        commands.entity(entity).with_child((
            Name::new("Eagle"),
            Eagle {
                centre,
                phase: TAU * (bits >> 8) as f32 / (1 << 24) as f32,
                turn: if bits & 1 == 0 { 1.0 } else { -1.0 },
            },
            Transform::from_translation(centre),
            Visibility::default(),
            Mesh3d(models.eagle.clone()),
            MeshMaterial3d(models.eagle_material.clone()),
        ));
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
    mut eagles: Query<(&Eagle, &ChildOf, &mut Transform)>,
) {
    for (eagle, chunk, mut transform) in &mut eagles {
        let angle =
            eagle.phase + eagle.turn * (SOAR_SPEED / SOAR_RADIUS) * time.elapsed_secs_wrapped();
        let (sin, cos) = angle.sin_cos();
        let mut at = eagle.centre + Vec3::new(cos, 0.0, sin) * SOAR_RADIUS;

        if let Ok(on) = chunks.get(chunk.parent()) {
            let origin = on.coords.as_vec2() * CHUNK_METRES;
            if let Some(under) = ground.height(origin.x + at.x, origin.y + at.z) {
                at.y = at.y.max(under + SOAR_GROUND_CLEARANCE);
            }
        }

        let along = Vec3::new(-sin, 0.0, cos) * eagle.turn;
        transform.translation = at;
        // Nose along the flight, then rolled about it — into the turn, which
        // for either handedness is the wing nearer the summit dipping.
        transform.rotation = Transform::default().looking_to(along, Vec3::Y).rotation
            * Quat::from_rotation_z(-eagle.turn * SOAR_BANK);
    }
}

// --- Dolphins --------------------------------------------------------------

/// A pod's way through the water, in metres per second. Brisker than the
/// boat's amble, so a crossing overtakes rather than shadows it.
const POD_SPEED: f32 = 4.5;

/// How long a pod swims before it retires, in seconds — with [`POD_SPEED`],
/// about a hundred and sixty metres of crossing.
const POD_LIFE: f32 = 36.0;

/// The ring around the camera's focus a pod surfaces in, in metres: far
/// enough that it appears as "out there" rather than materialising alongside,
/// near enough to be seen without being looked for.
const POD_RING: (f32, f32) = (90.0, 150.0);

/// How far abeam of the focus a pod's course is laid, in metres, at most.
/// Aimed near the watcher rather than at or away: the crossing passes the
/// boat, which is the whole spectacle.
const POD_ABEAM: f32 = 50.0;

/// Water a pod's whole course must have under it, in metres. Deep enough
/// that no leap grazes a shoal and no course noses into a beach — and, being
/// depth *below sea level*, it can never be met by a lake, whose bed stands
/// with its island.
const POD_DEPTH: f32 = 4.0;

/// Stride at which the course is sounded before it is swum, in metres. Finer
/// than the facet grid, so a one-facet pinnacle cannot slip between samples.
const POD_SOUNDING: f32 = 12.0;

/// Quiet after a pod retires before the next may be tried, in seconds — and
/// the much shorter pause after a try that found no sea, which is not worth
/// a minute of silence.
const POD_REST: f32 = 45.0;
const POD_RETRY: f32 = 2.0;

/// One porpoising cycle, in seconds, and the sine's size and setting in
/// metres: risen `LEAP_HEIGHT - CRUISE_DEPTH` proud of the water at the top
/// of a leap, `LEAP_HEIGHT + CRUISE_DEPTH` under it between leaps.
const LEAP_PERIOD: f32 = 3.2;
const LEAP_HEIGHT: f32 = 2.2;
const CRUISE_DEPTH: f32 = 1.3;

/// How far apart neighbouring dolphins are in their cycle, in radians —
/// enough that the pod surfaces as a run of arcs rather than a synchronised
/// display team.
const LEAP_STAGGER: f32 = 0.45;

/// Deeper than this, in metres, a dolphin cannot be seen through the water's
/// near-opacity even with a trough over it — where retiring is allowed to
/// happen.
const OUT_OF_SIGHT: f32 = -1.2;

/// Where each pod member swims, relative to the leader: abreast-and-behind in
/// a loose echelon. As many dolphins as the entropy asks for, up to all five.
const STATIONS: [(f32, f32); 5] = [(0.0, 0.0), (-1.7, 2.1), (1.7, 2.4), (-3.3, 4.6), (3.4, 4.9)];

/// A pod on its crossing. The transform carries where it is and which way it
/// faces; this carries what the transform must not be asked to remember.
#[derive(Component)]
pub struct Pod {
    /// The course, as a unit vector on the map.
    heading: Vec2,
    /// `Time::elapsed_secs` at the spawn, which every member's cycle counts
    /// from.
    born: f32,
}

/// One dolphin of a pod, at a fixed station in the pod's frame. Only its
/// height and pitch are its own.
#[derive(Component)]
pub struct Dolphin {
    /// Where in the porpoising cycle this one is.
    phase: f32,
}

/// A course for a pod near `focus`, or `None` — from where it enters the
/// water to the heading it holds, every stride of it sounded and found deep.
///
/// `entropy` is decorative randomness: which bearing, how far out, how many
/// strides abeam the course is laid. A try that fails is simply tried again
/// later with fresh bits, so this needs no cleverness about *finding* sea —
/// only honesty about refusing a course it cannot vouch for, unarrived
/// chunks included.
pub fn plan_pod(ground: &Ground, focus: Vec2, entropy: u32) -> Option<(Vec2, Vec2)> {
    let slice = |salt: u32| scramble(entropy ^ salt) as f32 / u32::MAX as f32;

    let bearing = TAU * slice(0x0B5E);
    let (near, far) = POD_RING;
    let start = focus + Vec2::from_angle(bearing) * (near + (far - near) * slice(0x51DE));
    let abeam =
        focus + Vec2::from_angle(bearing + FRAC_PI_2) * POD_ABEAM * (slice(0xABEA) * 2.0 - 1.0);
    let heading = (abeam - start).normalize_or_zero();
    if heading == Vec2::ZERO {
        return None;
    }

    // The whole crossing, plus the grace the retirement below may add waiting
    // for the last leaper to submerge, plus the tail of the echelon behind
    // the leader — none of it may find ground.
    let length = POD_SPEED * (POD_LIFE + 2.0 * LEAP_PERIOD);
    let strides = (length / POD_SOUNDING).ceil() as i32;
    for stride in -1..=strides {
        let at = start + heading * (length * stride as f32 / strides as f32);
        match ground.height(at.x, at.y) {
            Some(depth) if depth <= -POD_DEPTH => {}
            _ => return None,
        }
    }
    Some((start, heading))
}

/// The metronome between pods, counted on `Time::elapsed_secs`.
#[derive(Resource, Default)]
struct PodClock {
    next_try: f32,
}

/// Sends a pod across the neighbourhood whenever the sea has been quiet for
/// long enough — one at a time; dolphins are an event, not a population.
fn send_pods(
    mut commands: Commands,
    time: Res<Time>,
    mut clock: ResMut<PodClock>,
    models: Res<WildlifeModels>,
    ground: Res<Ground>,
    cameras: Query<&MapCamera>,
    pods: Query<(), With<Pod>>,
) {
    let now = time.elapsed_secs();
    if !pods.is_empty() || now < clock.next_try {
        return;
    }
    let Ok(camera) = cameras.single() else {
        return;
    };
    let focus = Vec2::new(camera.focus.x, camera.focus.z);

    let entropy = scramble(time.elapsed().as_millis() as u32);
    let Some((start, heading)) = plan_pod(&ground, focus, entropy) else {
        clock.next_try = now + POD_RETRY;
        return;
    };
    clock.next_try = now + POD_LIFE + POD_REST;

    let count = 3 + scramble(entropy ^ 0x90D5) as usize % 3;
    commands
        .spawn((
            Name::new("Pod"),
            Pod { heading, born: now },
            DespawnOnExit(AppState::InWorld),
            Transform::from_xyz(start.x, 0.0, start.y)
                .looking_to(Vec3::new(heading.x, 0.0, heading.y), Vec3::Y),
            Visibility::default(),
        ))
        .with_children(|school| {
            for (member, (side, lag)) in STATIONS.iter().enumerate().take(count) {
                school.spawn((
                    Name::new("Dolphin"),
                    Dolphin {
                        // All under water at the spawn — the sine's trough —
                        // so a pod enters the world unseen and *surfaces*.
                        phase: -FRAC_PI_2 + member as f32 * LEAP_STAGGER,
                    },
                    Transform::from_xyz(*side, -CRUISE_DEPTH - LEAP_HEIGHT, *lag),
                    Visibility::default(),
                    Mesh3d(models.dolphin.clone()),
                    MeshMaterial3d(models.dolphin_material.clone()),
                ));
            }
        });
}

/// Carries every pod along its course.
fn swim(time: Res<Time>, mut pods: Query<(&Pod, &mut Transform)>) {
    for (pod, mut transform) in &mut pods {
        transform.translation +=
            Vec3::new(pod.heading.x, 0.0, pod.heading.y) * POD_SPEED * time.delta_secs();
    }
}

/// Rides every dolphin through its arcs: a sine about cruising depth for the
/// height, its own derivative for the pitch — so the nose enters the water
/// where the leap is falling, which is the whole of what makes an arc read
/// as a leap rather than a bob.
///
/// The sine stands on the swell at the dolphin's own spot of sea, on the same
/// wrapped clock the water is drawn with, so a leap crests a wave rather than
/// some flat remembered ocean.
fn porpoise(
    time: Res<Time>,
    ground: Res<Ground>,
    conditions: Res<SeaConditions>,
    pods: Query<(&Pod, &Transform)>,
    mut dolphins: Query<(&Dolphin, &ChildOf, &mut Transform), Without<Pod>>,
) {
    for (dolphin, of, mut transform) in &mut dolphins {
        let Ok((pod, pod_transform)) = pods.get(of.parent()) else {
            continue;
        };
        let at = pod_transform.transform_point(Vec3::new(
            transform.translation.x,
            0.0,
            transform.translation.z,
        ));
        let depth = ground.height(at.x, at.z).map_or(f32::MAX, |height| -height);
        let water = conditions.swell(Vec2::new(at.x, at.z), time.elapsed_secs_wrapped(), depth);

        let (rise, run) =
            (TAU / LEAP_PERIOD * (time.elapsed_secs() - pod.born) + dolphin.phase).sin_cos();
        transform.translation.y = water - CRUISE_DEPTH + LEAP_HEIGHT * rise;
        let pitch = (LEAP_HEIGHT * TAU / LEAP_PERIOD * run).atan2(POD_SPEED);
        transform.rotation = Quat::from_rotation_x(pitch);
    }
}

/// Retires pods whose crossing is done — while every member is under water,
/// so the despawn happens where it cannot be watched. The grace period is
/// for freak seas: if the swell somehow keeps a back wet for two whole
/// cycles past time, the pod goes anyway rather than swimming off the end of
/// its sounded course.
fn retire_pods(
    mut commands: Commands,
    time: Res<Time>,
    pods: Query<(Entity, &Pod, &Children)>,
    dolphins: Query<&Transform, With<Dolphin>>,
) {
    let now = time.elapsed_secs();
    for (entity, pod, school) in &pods {
        let age = now - pod.born;
        if age < POD_LIFE {
            continue;
        }
        let hidden = school.iter().all(|member| {
            dolphins
                .get(member)
                .is_ok_and(|t| t.translation.y < OUT_OF_SIGHT)
        });
        if hidden || age > POD_LIFE + 2.0 * LEAP_PERIOD {
            commands.entity(entity).despawn();
        }
    }
}

// --- The plumbing they share -----------------------------------------------

/// The meshes and materials every eagle and every dolphin share, loaded once.
#[derive(Resource)]
struct WildlifeModels {
    eagle: Handle<Mesh>,
    eagle_material: Handle<StandardMaterial>,
    dolphin: Handle<Mesh>,
    dolphin_material: Handle<StandardMaterial>,
}

fn load_the_models(
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    assets: Res<AssetServer>,
) {
    let mesh = |model: &str| -> AssetPath<'static> {
        GltfAssetLabel::Primitive {
            mesh: 0,
            primitive: 0,
        }
        .from_asset(model.to_owned())
    };
    commands.insert_resource(WildlifeModels {
        eagle: assets.load(mesh(EAGLE_MODEL)),
        eagle_material: materials.add(matte(EAGLE_COLOR)),
        dolphin: assets.load(mesh(DOLPHIN_MODEL)),
        dolphin_material: materials.add(matte(DOLPHIN_COLOR)),
    });
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

pub struct WildlifePlugin;

impl Plugin for WildlifePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PodClock>()
            .add_systems(Startup, load_the_models)
            .add_systems(
                Update,
                (watch_summits, soar, send_pods, swim, porpoise, retire_pods)
                    .chain()
                    .run_if(in_state(AppState::InWorld).and_then(resource_exists::<Ground>)),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use protocol::ground::{quantize, ChunkPayload, Surface, Tone, FACET_TRIS, FACET_VERTS};

    use crate::testing::{is_flat_shaded, model, test_ground, triangles, winds_outwards};

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
    fn pods_keep_off_the_island() {
        // Anchored dead over the island, every ring the plan could start a
        // pod on is ground or shallows: nothing may be found, whatever the
        // entropy says.
        let ground = test_ground();
        for entropy in 0..200 {
            assert_eq!(plan_pod(&ground, Vec2::ZERO, scramble(entropy)), None);
        }
    }

    #[test]
    fn pods_cross_open_water() {
        // Anchored off the coast there is honest sea on the seaward side, and
        // some try finds it — and what it finds it has sounded, so the course
        // it answers stays clear of the island by construction.
        let ground = test_ground();
        let found = (0..200)
            .find_map(|entropy| plan_pod(&ground, Vec2::new(400.0, 0.0), scramble(entropy)));
        let (start, _) = found.expect("open water near the focus holds some course");
        assert!(
            start.length() > crate::testing::TEST_ISLAND_REACH,
            "a pod started over the island at {start}"
        );
    }

    #[test]
    fn the_models_hold_one_mesh_each() {
        // The one thing about each file the game cannot see for itself —
        // that mesh 0 is the animal — held the same way the palm's order is.
        let (eagle, _) = model(EAGLE_MODEL);
        assert_eq!(eagle["meshes"][0]["name"], "eagle");
        let (dolphin, _) = model(DOLPHIN_MODEL);
        assert_eq!(dolphin["meshes"][0]["name"], "dolphin");
    }

    #[test]
    fn the_wildlife_is_built_to_scale() {
        // What `watch_summits` and `send_pods` assume when they place the
        // models unscaled: a remodel that came through in centimetres — or
        // with the exporter's axes wrong — would fly a hundred-metre bird.
        let span = |name: &str, axis: usize| -> f32 {
            let corners: Vec<f32> = triangles(name, 0, "POSITION")
                .into_iter()
                .flatten()
                .map(|corner| corner[axis])
                .collect();
            corners.iter().fold(f32::MIN, |a, b| a.max(*b))
                - corners.iter().fold(f32::MAX, |a, b| a.min(*b))
        };
        let wingspan = span(EAGLE_MODEL, 0);
        assert!(
            (3.0..4.5).contains(&wingspan),
            "the eagle spans {wingspan}m"
        );
        // Nose to flukes lies along Z, the forward axis.
        let length = span(DOLPHIN_MODEL, 2);
        assert!((2.0..3.0).contains(&length), "the dolphin runs {length}m");
    }

    #[test]
    fn every_face_of_the_wildlife_looks_outwards() {
        for name in [EAGLE_MODEL, DOLPHIN_MODEL] {
            assert!(
                winds_outwards(&triangles(name, 0, "POSITION")),
                "{name} is wound inside-out"
            );
        }
    }

    #[test]
    fn the_wildlife_is_flat_shaded() {
        for name in [EAGLE_MODEL, DOLPHIN_MODEL] {
            assert!(
                is_flat_shaded(
                    &triangles(name, 0, "POSITION"),
                    &triangles(name, 0, "NORMAL")
                ),
                "{name} is smooth-shaded"
            );
        }
    }
}
