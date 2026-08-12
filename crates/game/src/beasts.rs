//! What the beasts look like: the server's creatures, drawn.
//!
//! A beast is the opposite arrangement from the wildlife in every way that
//! matters. The wildlife is scenery this client invents for itself and the
//! server has never heard of; a beast is a creature the *server* means —
//! the shark because it will one day act on a player, the dolphins and the
//! whale because they are rare enough to point at, and "look, a whale!"
//! only lands if both players are under the same sea. The server owns where
//! a beast is and where it is going, tells everyone on a beat
//! ([`protocol::ToClient::Beast`], one message that is both introduction and
//! movement), and this module's whole job is the half a server has no
//! opinion on: what each kind looks like being there.
//!
//! That half is not nothing. The wire carries a point and a velocity on the
//! ground plane; everything the player actually sees is decided here — the
//! body eased along the tellings the way remote players' markers are, the
//! bearing swung to follow the water it is swimming, and everything about
//! depth. The shark holds just under the surface and breathes its depth on
//! a slow cycle, so the dorsal fin cuts the water for a while, slides
//! under, and comes back — the fin standing out of the swell *is* the shark
//! as far as most encounters go, which is why the model's fin is built tall
//! (see the master's NOTES). The dolphins and the whale porpoise: a sine
//! about cruising depth, pitched by its own slope, the motion these animals
//! had as client-side wildlife carried over whole now that the *place* is
//! the server's word.
//!
//! A pod is one beast. The wire says where the pod is; how many dolphins
//! that is, and where each swims in the formation, is dealt from the
//! [`BeastId`] — which every machine was told — so every client draws the
//! same pod without another byte crossing the wire. The same trick the
//! eagles play with chunk coordinates, one rung up.
//!
//! The shark's tail is the model's own: the file carries one clip, `swim`,
//! a lateral wave that travels nose to tail, and it plays on its own clock —
//! scaled by how fast the water is going by, but never seeked. That is the
//! player's gait inverted, and deliberately so: feet on ground have contact
//! to keep, where a tail in water only has to look like the thing pushing,
//! so a clip at roughly the right rate reads perfectly and costs no
//! bookkeeping. The figure's `conduct` already leaves other rigged models
//! alone for exactly this day; this module's own conductor takes only the
//! players hung under a beast.

use std::collections::HashMap;
use std::f32::consts::TAU;

use bevy::animation::graph::{AnimationGraph, AnimationGraphHandle, AnimationNodeIndex};
use bevy::animation::AnimationPlayer;
use bevy::gltf::{GltfAssetLabel, GltfMeshName};
use bevy::prelude::*;

use protocol::{BeastId, BeastKind};

use crate::sea::SeaConditions;
use crate::terrain::Ground;
use crate::{eased, matte, AppState};

/// The shark, rigged and with its one clip in it. The other kinds are rigid
/// meshes whose whole-body motion is computed here, as it was when they were
/// wildlife — only the shark carries an armature, its tail being the one
/// piece of animal anatomy the camera watches work.
const SHARK_MODEL: &str = "models/shark.glb";
const DOLPHIN_MODEL: &str = "models/dolphin.glb";
const WHALE_MODEL: &str = "models/whale.glb";

/// The clip, by its position in the file — held to its name by
/// `the_model_carries_the_swim_the_game_plays`. Swimming is the whole
/// vocabulary: a shark that stopped swimming would be a drowning shark.
const SWIM: usize = 0;

/// What each mesh in the file is painted, by the name it carries there —
/// the same arrangement as the player's figure, one entry per tone.
const TONES: [(&str, Color); 1] = [("hide", HIDE_COLOR)];

/// Sand-grey. A shark here is seen through a metre of sunlit shallow water
/// or as a fin against it, and both read best a shade paler than the
/// dolphin's wet slate — the dolphin is a dark arc over deep water, where
/// the shark is a pale shape over sand.
const HIDE_COLOR: Color = Color::srgb(0.46, 0.45, 0.40);

/// Wet slate. Lighter than the deep sea it breaks out of and darker than the
/// spray-white a leap suggests, so the arc reads against the water at the
/// distances pods keep.
const DOLPHIN_COLOR: Color = Color::srgb(0.42, 0.50, 0.55);

/// Deep blue-grey, darker than the dolphin's: a whale's back barely clears
/// the water, and what sells the size is a long dark mass rather than a
/// bright shape.
const WHALE_COLOR: Color = Color::srgb(0.27, 0.31, 0.37);

/// Metres per second of water the swim cycle was drawn against: at this
/// pace the clip plays at exactly the rate the master keyed. Not in the
/// file, for the same reason the player's stride is not — a clip knows how
/// long it lasts, not how fast the animal drawn in it was going.
const SWISH_PACE: f32 = 1.4;

/// How far under the local water surface the origin rides when the shark is
/// up, in metres. The dorsal fin tip stands 0.65 over the origin, so riding
/// awash puts a forearm's length of fin out of the water — it was first set
/// deeper, a hand's width of fin, and from the camera's height that read as
/// no fin at all. The back stays just under at this depth, a paleness the
/// fin is cutting out of.
const AWASH: f32 = 0.38;

/// And when it has sounded: fin under by a fin's own height, body still
/// shallow enough to stay a shape in the water rather than vanishing —
/// a shark that disappeared entirely would read as despawned, and the
/// unsettling thing about a shark is knowing it is still there.
const SOUNDED: f32 = 0.88;

/// Seconds of one breath of depth, awash to sounded and back. Slow enough
/// that a fin is *there* to be noticed and tracked, quick enough that a
/// minute of watching a coast catches one.
const RISE_PERIOD: f32 = 9.0;

/// How quickly a beast closes on where the server last put it, in
/// e-foldings per second — see [`eased`]. Tellings come a few times a
/// second and the easing is what turns them back into a glide.
const SMOOTHING: f32 = 6.0;

/// How quickly the body swings onto a new bearing, likewise. Deliberately
/// slower than the position: a shark turns like a keel, not a compass
/// needle, and the lag is most of what makes the turn look swum.
const TURNING: f32 = 3.0;

/// Where dolphins swim relative to the pod's own point — abreast-and-behind
/// in a loose echelon, the leader's station first — and how many of the
/// stations a pod uses, fewest to most inclusive. Which count, and every
/// jitter below, is dealt from the pod's [`BeastId`], so the pod this client
/// draws is the pod every client draws.
const POD_STATIONS: [(f32, f32); 5] =
    [(0.0, 0.0), (-1.7, 2.1), (1.7, 2.4), (-3.3, 4.6), (3.4, 4.9)];
const POD_SIZE: (u32, u32) = (2, 5);

/// Radians between neighbours in the porpoising cycle — enough that a pod
/// surfaces as a run of arcs rather than a synchronised display team — and
/// metres of station jitter, so no member sits exactly where its station
/// says.
const POD_STAGGER: f32 = 0.45;
const POD_SLOP: f32 = 0.6;

/// The porpoising a dolphin does, and a whale's — the same animal at
/// different sizes and tempos, exactly as when they were wildlife: seconds a
/// cycle takes, metres of sine, metres under the waterline it is centred.
const DOLPHIN_SWIM: Porpoising = Porpoising {
    phase: 0.0,
    period: 3.2,
    leap: 2.2,
    cruise: 1.3,
};
const WHALE_SWIM: Porpoising = Porpoising {
    phase: 0.0,
    period: 9.0,
    leap: 1.9,
    cruise: 2.4,
};

/// The beasts this client has been told of, by the entity standing for each.
///
/// Kept for the reason [`crate::net::Online`] keeps its markers: entities
/// spawn through `Commands`, so two words about one beast in a single
/// frame's drain would otherwise be searching for an entity that is still
/// only a queued command. Cleared on leaving the world; the entities clear
/// themselves, being `DespawnOnExit`.
#[derive(Resource, Default)]
pub struct Beasts {
    seen: HashMap<BeastId, Entity>,
}

impl Beasts {
    /// A word about a beast: the first one spawns it, every later one only
    /// moves the goalposts it eases towards. Called by the session's
    /// [`crate::net::receive`], which is where everything a server says
    /// lands.
    pub fn seen(
        &mut self,
        commands: &mut Commands,
        id: BeastId,
        kind: BeastKind,
        position: Vec2,
        velocity: Vec2,
    ) {
        let told = Told {
            target: position,
            velocity,
        };
        if let Some(&beast) = self.seen.get(&id) {
            // Overwriting is the whole of the update, exactly as it is for a
            // marker — where the server last put a beast is all this side
            // knows about it.
            commands.entity(beast).insert(told);
            return;
        }
        let beast = commands
            .spawn((
                Name::new(id.to_string()),
                Beast { kind, seed: id.0 },
                told,
                DespawnOnExit(AppState::InWorld),
                Transform::from_xyz(position.x, 0.0, position.y).looking_to(
                    Vec3::new(velocity.x, 0.0, velocity.y).normalize_or(Vec3::NEG_Z),
                    Vec3::Y,
                ),
                Visibility::default(),
            ))
            .id();
        self.seen.insert(id, beast);
    }

    /// The server has stopped minding one: it goes, wherever it stood — no
    /// death throes, because nothing died. The sea is emptier by one.
    pub fn gone(&mut self, commands: &mut Commands, id: BeastId) {
        if let Some(beast) = self.seen.remove(&id) {
            commands.entity(beast).despawn();
        }
    }
}

/// One beast as this client draws it: the facts that never change between
/// tellings. Where it is heading lives in [`Told`], overwritten wholesale by
/// every word from the server.
#[derive(Component)]
struct Beast {
    kind: BeastKind,
    /// The id's own bits, which are the one die every machine was dealt
    /// alike: everything this client invents about a beast — a shark's
    /// breath phase, a pod's size and stations — is drawn from here, so
    /// every other client invents the same.
    seed: u32,
}

/// Where the server last put a beast, and where it said it was going.
#[derive(Component, Clone, Copy)]
struct Told {
    target: Vec2,
    velocity: Vec2,
}

/// The model hung under a beast — what [`conduct`] looks for above an
/// animation player, so the walker's own figure and any rigged thing the
/// world grows later are left alone.
#[derive(Component)]
struct BeastFigure;

/// One swimming member of a beast that porpoises — a dolphin at its station,
/// or the whale itself. Only its height and pitch are its own: a sine about
/// cruising depth, and the sine's own slope, so the nose enters the water
/// where the leap is falling — which is the whole of what makes an arc read
/// as a leap rather than a bob.
#[derive(Component, Clone, Copy)]
struct Porpoising {
    /// Where in the cycle this one is, offset per member so a pod surfaces
    /// as a run of arcs.
    phase: f32,
    /// Seconds one cycle takes.
    period: f32,
    /// The sine's size, in metres.
    leap: f32,
    /// How far under the waterline the cycle is centred.
    cruise: f32,
}

/// An animation player that is a beast's, and whose beast it is: [`swish`]
/// reads the beast's last telling to pace the clip.
#[derive(Component)]
struct Swishing(Entity);

/// The swim, mixed and ready: the graph the one clip hangs in and the node
/// it occupies. One graph for every shark that will ever swim past.
#[derive(Resource)]
struct Swim {
    graph: Handle<AnimationGraph>,
    node: AnimationNodeIndex,
}

/// The mesh and material every beast of the rigid kinds shares, loaded once
/// — a whole pod draws in one call rather than one apiece.
#[derive(Resource)]
struct BeastModels {
    dolphin: (Handle<Mesh>, Handle<StandardMaterial>),
    whale: (Handle<Mesh>, Handle<StandardMaterial>),
}

pub struct BeastsPlugin;

impl Plugin for BeastsPlugin {
    fn build(&self, app: &mut App) {
        // Also initialised by NetPlugin, whose `receive` writes into it;
        // initialising a resource twice is free, and each plugin's tests run
        // it alone.
        app.init_resource::<Beasts>()
            .init_resource::<SeaConditions>()
            .add_systems(Startup, school)
            // None of this is the player's hands, so none of it pauses: the
            // sharks are the server's, and the server does not stop swimming
            // them because somebody opened a menu.
            .add_systems(Update, (dress, conduct, paint, glide, porpoise, swish))
            .add_systems(OnExit(AppState::InWorld), forget);
    }
}

/// Loads what every beast will be drawn with, up front: the graph the
/// shark's one clip hangs in — a graph describes the clip, not any
/// particular shark swimming it — and the rigid kinds' meshes, painted once
/// in this module's tones.
fn school(
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
) {
    let mut graph = AnimationGraph::new();
    let root = graph.root;
    let node = graph.add_clip(
        assets.load(GltfAssetLabel::Animation(SWIM).from_asset(SHARK_MODEL)),
        1.0,
        root,
    );
    commands.insert_resource(Swim {
        graph: graphs.add(graph),
        node,
    });
    commands.insert_resource(BeastModels {
        dolphin: (
            assets.load(crate::model_mesh(DOLPHIN_MODEL, 0)),
            materials.add(matte(DOLPHIN_COLOR)),
        ),
        whale: (
            assets.load(crate::model_mesh(WHALE_MODEL, 0)),
            materials.add(matte(WHALE_COLOR)),
        ),
    });
}

/// Hangs under any beast that has just been told of whatever its kind is
/// drawn as.
///
/// The shark arrives as a whole scene rather than meshes pulled out one at
/// a time, for the reason the player's figure does: a skinned mesh has to
/// arrive as a hierarchy or it is a shape with no skeleton behind it. The
/// file's own materials come along and [`paint`] undoes them. The rigid
/// kinds are hung as plain meshes at their stations — a pod's members dealt
/// from the id, so every client hangs the same dolphins in the same order.
fn dress(
    mut commands: Commands,
    assets: Res<AssetServer>,
    models: Res<BeastModels>,
    beasts: Query<(Entity, &Beast), Added<Beast>>,
) {
    for (beast, of) in &beasts {
        match of.kind {
            BeastKind::Shark => {
                commands.entity(beast).with_child((
                    Name::new("figure"),
                    BeastFigure,
                    WorldAssetRoot(assets.load(GltfAssetLabel::Scene(0).from_asset(SHARK_MODEL))),
                ));
            }
            BeastKind::Dolphins => {
                let (mesh, material) = models.dolphin.clone();
                for (member, station, swimming) in pod_members(of.seed) {
                    commands.entity(beast).with_child((
                        Name::new(format!("dolphin {member}")),
                        swimming,
                        Transform::from_translation(station),
                        Mesh3d(mesh.clone()),
                        MeshMaterial3d(material.clone()),
                    ));
                }
            }
            BeastKind::Whale => {
                let (mesh, material) = models.whale.clone();
                commands.entity(beast).with_child((
                    Name::new("whale"),
                    Porpoising {
                        // Its own point of the cycle, from the id: two
                        // whales met in one session should not blow in
                        // unison.
                        phase: unit(of.seed, 0x817A1E) * TAU,
                        ..WHALE_SWIM
                    },
                    Transform::from_xyz(0.0, -(WHALE_SWIM.cruise + WHALE_SWIM.leap), 0.0),
                    Mesh3d(mesh),
                    MeshMaterial3d(material),
                ));
            }
        }
    }
}

/// The members of the pod a [`BeastId`]'s bits deal: station index, where it
/// swims in the pod's frame, and its porpoising. A pure function of the
/// seed, which is the point — see the module doc — and starting under the
/// water's own opacity, so however a pod first appears it *surfaces*.
fn pod_members(seed: u32) -> Vec<(usize, Vec3, Porpoising)> {
    let count = POD_SIZE.0 + scramble(seed ^ 0x90D5) % (POD_SIZE.1 + 1 - POD_SIZE.0);
    POD_STATIONS
        .iter()
        .enumerate()
        .take(count as usize)
        .map(|(member, (side, lag))| {
            let jitter = |salt: u32| (unit(seed ^ ((member as u32) << 8), salt) - 0.5) * 2.0;
            let swimming = Porpoising {
                phase: member as f32 * POD_STAGGER + 0.12 * jitter(0xD01),
                ..DOLPHIN_SWIM
            };
            (
                member,
                Vec3::new(
                    side + POD_SLOP * jitter(0xD02),
                    -(swimming.cruise + swimming.leap),
                    lag + POD_SLOP * jitter(0xD03),
                ),
                swimming,
            )
        })
        .collect()
}

/// Sets a beast's clip playing as its scene finishes arriving — the loader
/// puts an [`AnimationPlayer`] on the root of whatever it found animated,
/// and this takes only the ones hung under a beast, exactly as the figure's
/// conductor takes only the walker's.
fn conduct(
    mut commands: Commands,
    swim: Res<Swim>,
    hierarchy: Query<&ChildOf>,
    beasts: Query<(), With<Beast>>,
    figures: Query<(), With<BeastFigure>>,
    mut arrivals: Query<(Entity, &mut AnimationPlayer), Added<AnimationPlayer>>,
) {
    for (entity, mut player) in &mut arrivals {
        if !hierarchy
            .iter_ancestors(entity)
            .any(|above| figures.contains(above))
        {
            continue;
        }
        let Some(beast) = hierarchy
            .iter_ancestors(entity)
            .find(|above| beasts.contains(*above))
        else {
            continue;
        };

        commands
            .entity(entity)
            .insert((AnimationGraphHandle(swim.graph.clone()), Swishing(beast)));
        // Repeating at its own pace, where the walker's run is paused and
        // seeked: a tail in water has no footfalls to keep honest.
        player.play(swim.node).repeat();
    }
}

/// Paints the model in the world's tones as its meshes arrive, throwing away
/// what came out of the file — the same undoing the figure does, against
/// this module's own palette.
fn paint(
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut arrivals: Query<
        (&GltfMeshName, &mut MeshMaterial3d<StandardMaterial>),
        Added<GltfMeshName>,
    >,
) {
    for (mesh, mut material) in &mut arrivals {
        if let Some((_, tone)) = TONES.iter().find(|(named, _)| *named == mesh.0) {
            *material = MeshMaterial3d(materials.add(matte(*tone)));
        }
    }
}

/// Swims each beast towards where the server last put it: eased over the
/// ground plane, swung gradually onto its bearing, and — for the shark —
/// ridden at the depth its breath has reached, just under the swell, fin out
/// for a while in every cycle. The porpoising kinds keep their root on the
/// waterline and let [`porpoise`] give every member its own depth.
fn glide(
    time: Res<Time>,
    ground: Option<Res<Ground>>,
    sea: Res<SeaConditions>,
    mut beasts: Query<(&Beast, &Told, &mut Transform)>,
) {
    let dt = time.delta_secs();
    let elapsed = time.elapsed_secs_wrapped();

    for (beast, told, mut transform) in &mut beasts {
        let at = Vec2::new(transform.translation.x, transform.translation.z);
        let at = at.lerp(told.target, eased(SMOOTHING, dt));

        let ride = match beast.kind {
            // The water surface here, swell and all: the ride is measured
            // down from the moving surface rather than from flat calm, so
            // the fin keeps its freeboard through a wave instead of being
            // swallowed by every crest. The breath phase is the id's, so a
            // pair of sharks rising and sounding in unison would have to be
            // dealt the same bits.
            BeastKind::Shark => {
                let breath = unit(beast.seed, 0xB0B) * TAU;
                let surface = sea.water_over(ground.as_deref(), at, elapsed);
                let breathing = 0.5 - 0.5 * ((elapsed / RISE_PERIOD * TAU) + breath).cos();
                surface - AWASH - (SOUNDED - AWASH) * breathing
            }
            BeastKind::Dolphins | BeastKind::Whale => 0.0,
        };

        transform.translation = Vec3::new(at.x, ride, at.y);
        if let Ok(heading) = Dir3::new(Vec3::new(told.velocity.x, 0.0, told.velocity.y)) {
            let onto = Transform::default().looking_to(heading, Vec3::Y).rotation;
            let swing = transform.rotation.slerp(onto, eased(TURNING, dt));
            transform.rotation = swing;
        }
    }
}

/// Rides every porpoising member through its arcs: a sine about cruising
/// depth for the height, its own derivative for the pitch — so the nose
/// enters the water where the leap is falling. The sine stands on the swell
/// at the member's own spot of sea, on the same wrapped clock the water is
/// drawn with, so a leap crests a wave rather than some flat remembered
/// ocean; when that clock wraps, once an hour, every arc skips to another
/// point of its cycle — the same shrug the swell gives, and as unlikely to
/// be watched.
fn porpoise(
    time: Res<Time>,
    ground: Option<Res<Ground>>,
    sea: Res<SeaConditions>,
    beasts: Query<(&Told, &Transform), With<Beast>>,
    mut members: Query<(&Porpoising, &ChildOf, &mut Transform), Without<Beast>>,
) {
    let elapsed = time.elapsed_secs_wrapped();
    for (swimming, of, mut transform) in &mut members {
        let Ok((told, carrier)) = beasts.get(of.parent()) else {
            continue;
        };
        let station = carrier.transform_point(Vec3::new(
            transform.translation.x,
            0.0,
            transform.translation.z,
        ));
        let water = sea.water_over(ground.as_deref(), station.xz(), elapsed);

        let (rise, run) = (TAU / swimming.period * elapsed + swimming.phase).sin_cos();
        transform.translation.y = water - swimming.cruise + swimming.leap * rise;
        let pitch = (swimming.leap * TAU / swimming.period * run).atan2(told.velocity.length());
        transform.rotation = Quat::from_rotation_x(pitch);
    }
}

/// Paces every playing swim by how fast its beast's water is going by, so a
/// shark pushed harder some day swishes harder instead of gliding like a
/// submarine. The clip rate is *roughly* right rather than seeked exactly —
/// see the module's opening for why that is enough for a tail.
fn swish(
    swim: Res<Swim>,
    tellings: Query<&Told>,
    mut swimming: Query<(&Swishing, &mut AnimationPlayer)>,
) {
    for (Swishing(beast), mut player) in &mut swimming {
        let Ok(told) = tellings.get(*beast) else {
            continue;
        };
        if let Some(playing) = player.animation_mut(swim.node) {
            playing.set_speed(told.velocity.length() / SWISH_PACE);
        }
    }
}

/// Leaving the world forgets who was told of: the entities despawn with the
/// state, and a map into a world that has ended must not name entities in
/// the next one.
fn forget(mut beasts: ResMut<Beasts>) {
    beasts.seen.clear();
}

/// Stirs bits until they stop resembling what they were — SplitMix's mixing
/// rounds, without its sequence. Everything this module invents about a
/// beast comes through here, seeded from the beast's id — which is how the
/// inventions *agree* across machines: same bits in, same pod out. The
/// eagles play the same trick with chunk coordinates.
fn scramble(mut x: u32) -> u32 {
    x = x.wrapping_add(0x9E37_79B9);
    x ^= x >> 16;
    x = x.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 13;
    x = x.wrapping_mul(0xC2B2_AE35);
    x ^ (x >> 16)
}

/// A number in `0.0..1.0` from some bits and a salt.
fn unit(seed: u32, salt: u32) -> f32 {
    scramble(seed ^ salt) as f32 / u32::MAX as f32
}

#[cfg(test)]
mod tests {
    use bevy::state::app::StatesPlugin;
    use bevy::time::TimePlugin;

    use super::*;
    use crate::net::{fake_server, NetPlugin, Online};
    use crate::testing::{
        is_flat_shaded, model, run_frames, run_until, skin_weights, triangles, winds_outwards,
    };
    use crate::Helm;
    use protocol::ToClient;

    /// The corners of every mesh in the model, in the file's own frame.
    fn corners() -> Vec<Vec3> {
        (0..TONES.len())
            .flat_map(|mesh| triangles(SHARK_MODEL, mesh, "POSITION"))
            .flatten()
            .collect()
    }

    fn meshes() -> Vec<String> {
        model(SHARK_MODEL).0["meshes"]
            .as_array()
            .expect("the model has meshes")
            .iter()
            .map(|mesh| mesh["name"].as_str().expect("a named mesh").to_owned())
            .collect()
    }

    #[test]
    fn the_model_is_the_shark_the_game_paints() {
        // Every mesh in the file is one the palette has a tone for, and every
        // tone has a mesh — a mesh outside the pairing arrives wearing
        // whatever Blender last gave it.
        let mut named = meshes();
        named.sort();
        let mut wanted: Vec<String> = TONES.iter().map(|(name, _)| (*name).to_owned()).collect();
        wanted.sort();
        assert_eq!(named, wanted, "the model's meshes are not the palette's");

        for (index, name) in meshes().iter().enumerate() {
            let faces = triangles(SHARK_MODEL, index, "POSITION");
            assert!(winds_outwards(&faces), "the {name} is wound inside-out");
            assert!(
                is_flat_shaded(&faces, &triangles(SHARK_MODEL, index, "NORMAL")),
                "the {name} is smooth-shaded"
            );
        }
    }

    #[test]
    fn the_skin_is_rigid_so_the_facets_stay_flat() {
        // The rule every rigged model here lives by: one bone per vertex at
        // full weight, or facets bend as the tail swishes.
        for mesh in 0..TONES.len() {
            for weights in skin_weights(SHARK_MODEL, mesh) {
                let carrying = weights.iter().filter(|w| **w > 0.0).count();
                assert_eq!(
                    carrying,
                    1,
                    "a vertex of {} is shared between bones: {weights:?}",
                    meshes()[mesh]
                );
                assert!(
                    weights.iter().any(|w| (*w - 1.0).abs() < 1e-3),
                    "a vertex of {} is carried at {weights:?}",
                    meshes()[mesh]
                );
            }
        }
    }

    #[test]
    fn the_shark_is_a_shark_swimming_forward() {
        // Shark-sized — a reef shark, not a whale and not a minnow — with
        // the girth forward of amidships, which is what points it: the file
        // faces -Z like everything here, so the fat end must lean that way.
        let corners = corners();
        let (nose, tail) = corners
            .iter()
            .fold((f32::MAX, f32::MIN), |(n, t), c| (n.min(c.z), t.max(c.z)));
        let length = tail - nose;
        assert!(
            (2.2..=3.2).contains(&length),
            "nose to tail is {length} m, which is not this game's shark"
        );

        let girth = corners.iter().fold((0.0_f32, 0.0_f32), |(widest, at), c| {
            if c.x.abs() > widest {
                (c.x.abs(), c.z)
            } else {
                (widest, at)
            }
        });
        assert!(
            girth.1 < (nose + tail) / 2.0,
            "the girth peaks at z {} — the shark is swimming backwards",
            girth.1
        );

        // The dorsal fin is the shark for most of its screen life, so the
        // model must hold it high enough to cut the surface at the depth
        // `glide` rides it: taller than AWASH, or no fin would ever show.
        let fin = corners.iter().fold(f32::MIN, |f, c| f.max(c.y));
        assert!(
            fin > AWASH && fin < 1.0,
            "the fin tops out {fin} m over the spine"
        );
        // And nothing hangs deep enough to plough the sand of the shallow
        // band it patrols.
        let keel = corners.iter().fold(f32::MAX, |k, c| k.min(c.y));
        assert!(keel > -0.6, "the shark draws {} m", -keel);
    }

    #[test]
    fn the_model_carries_the_swim_the_game_plays() {
        // Asked for by position, held to its name — the figure's own rule.
        let clips: Vec<String> = model(SHARK_MODEL).0["animations"]
            .as_array()
            .expect("the model has animations")
            .iter()
            .map(|clip| clip["name"].as_str().expect("a named clip").to_owned())
            .collect();
        assert_eq!(clips.get(SWIM).map(String::as_str), Some("swim"));
    }

    #[test]
    fn the_rigid_kinds_are_one_creature_each_built_to_scale() {
        // What `dress` assumes when it hangs the models unscaled — a remodel
        // that came through in centimetres, or with the exporter's axes
        // wrong, would swim a hundred-metre whale. Nose-to-tail lengths lie
        // along Z, the forward axis. These pins lived in the wildlife's
        // tests while these animals were wildlife.
        for (file, wanted) in [(DOLPHIN_MODEL, 2.0..3.0), (WHALE_MODEL, 9.0..13.0)] {
            let creature = file
                .strip_prefix("models/")
                .and_then(|name| name.strip_suffix(".glb"))
                .expect("a glTF binary under assets/models/");
            crate::testing::assert_model_draws(file, &[(0, creature)]);

            let lengths: Vec<f32> = triangles(file, 0, "POSITION")
                .into_iter()
                .flatten()
                .map(|corner| corner.z)
                .collect();
            let measured = lengths.iter().fold(f32::MIN, |a, b| a.max(*b))
                - lengths.iter().fold(f32::MAX, |a, b| a.min(*b));
            assert!(
                wanted.contains(&measured),
                "{file} measures {measured}m nose to tail, not {wanted:?}"
            );
        }
    }

    #[test]
    fn a_pod_is_dealt_whole_from_its_id() {
        // The wire says where a pod is and nothing else; the members are
        // this function of the id, so two clients told the same id hang the
        // same dolphins. Determinism is the whole promise, and the count
        // staying within the stations is what keeps the deal honest.
        for seed in 0..64 {
            let dealt = pod_members(seed);
            assert_eq!(
                dealt.len(),
                pod_members(seed).len(),
                "one id dealt two pods"
            );
            assert!((POD_SIZE.0 as usize..=POD_SIZE.1 as usize).contains(&dealt.len()));
            for ((member, station, _), again) in dealt.iter().zip(pod_members(seed)) {
                assert_eq!(*station, again.1, "member {member} moved between deals");
                // On station give or take the slop, under water to start.
                let (side, lag) = POD_STATIONS[*member];
                assert!((station.x - side).abs() <= POD_SLOP);
                assert!((station.z - lag).abs() <= POD_SLOP);
                assert!(station.y < 0.0, "a dolphin was dealt in the air");
            }
        }
    }

    // --- What the game does with the tellings --------------------------------

    /// A headless app in a served world, with the session and the beasts
    /// running — no render app, so scenes never finish arriving, but the
    /// entities and their tellings are all these tests ask after.
    fn beast_app(connection: crate::net::Connection) -> App {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
            TimePlugin,
            StatesPlugin,
            bevy::animation::AnimationPlugin,
            NetPlugin,
            BeastsPlugin,
        ))
        .init_state::<AppState>()
        .add_sub_state::<Helm>()
        .init_asset::<Mesh>()
        .init_asset::<bevy::world_serialization::WorldAsset>()
        .init_resource::<Assets<StandardMaterial>>()
        .init_resource::<ButtonInput<KeyCode>>()
        .insert_resource(Online::new(connection));
        app.update();
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::InWorld);
        app.update();
        app
    }

    fn beasts_afoot(app: &mut App) -> Vec<(Vec2, Vec2)> {
        app.world_mut()
            .query::<&Told>()
            .iter(app.world())
            .map(|told| (told.target, told.velocity))
            .collect()
    }

    #[test]
    fn beasts_come_move_and_go_as_the_server_tells() {
        let (addr, socket) = fake_server(Vec2::ZERO, Vec2::ZERO);
        let connection = crate::net::Connection::join(&addr).expect("join");
        let server = socket.recv().expect("the fake server keeps its socket");
        let mut app = beast_app(connection);

        // The first word spawns it, already knowing where it is bound.
        (ToClient::Beast {
            id: BeastId(3),
            kind: BeastKind::Shark,
            position: Vec2::new(40.0, -20.0),
            velocity: Vec2::new(1.0, 0.5),
        })
        .write(&mut &server)
        .expect("beast");
        run_until(&mut app, "the beast appears", |app| {
            !beasts_afoot(app).is_empty()
        });
        assert_eq!(
            beasts_afoot(&mut app),
            [(Vec2::new(40.0, -20.0), Vec2::new(1.0, 0.5))]
        );

        // A later word only moves the goalposts — same entity, new telling.
        (ToClient::Beast {
            id: BeastId(3),
            kind: BeastKind::Shark,
            position: Vec2::new(42.0, -19.0),
            velocity: Vec2::new(0.5, 1.0),
        })
        .write(&mut &server)
        .expect("beast again");
        run_until(&mut app, "the beast retargets", |app| {
            beasts_afoot(app) == [(Vec2::new(42.0, -19.0), Vec2::new(0.5, 1.0))]
        });
        let entities = app
            .world_mut()
            .query_filtered::<Entity, With<Beast>>()
            .iter(app.world())
            .count();
        assert_eq!(entities, 1, "a moved beast was taken for a second one");

        // And the word that it is no longer minded takes it away.
        (ToClient::BeastGone { id: BeastId(3) })
            .write(&mut &server)
            .expect("gone");
        run_until(&mut app, "the beast despawns", |app| {
            beasts_afoot(app).is_empty()
        });
    }

    #[test]
    fn a_beast_rides_under_the_surface_facing_where_it_is_going() {
        let (addr, socket) = fake_server(Vec2::ZERO, Vec2::ZERO);
        let connection = crate::net::Connection::join(&addr).expect("join");
        let server = socket.recv().expect("the fake server keeps its socket");
        let mut app = beast_app(connection);

        // Bound due +X, told from close by so the easing has settled well
        // inside the test's patience.
        (ToClient::Beast {
            id: BeastId(1),
            kind: BeastKind::Shark,
            position: Vec2::new(10.0, 0.0),
            velocity: Vec2::new(1.4, 0.0),
        })
        .write(&mut &server)
        .expect("beast");
        run_until(&mut app, "the beast appears", |app| {
            !beasts_afoot(app).is_empty()
        });
        run_frames(&mut app, 120);

        let place = *app
            .world_mut()
            .query_filtered::<&Transform, With<Beast>>()
            .single(app.world())
            .expect("one beast");

        // Under the water and near it: never surfaced, never on the sand.
        // No ground was ever delivered here, so the sea is open ocean and
        // its surface sits within the swell of height zero.
        assert!(
            (-2.0..=0.0).contains(&place.translation.y),
            "a shark riding at {} m",
            place.translation.y
        );

        // Facing the way the server said it was going: -Z is forward, so a
        // velocity along +X must have swung the nose to +X.
        let forward = place.forward();
        assert!(
            forward.x > 0.9,
            "told to swim +X, the shark faces {forward:?}"
        );
    }

    #[test]
    fn a_pod_told_once_is_several_dolphins_swimming() {
        let (addr, socket) = fake_server(Vec2::ZERO, Vec2::ZERO);
        let connection = crate::net::Connection::join(&addr).expect("join");
        let server = socket.recv().expect("the fake server keeps its socket");
        let mut app = beast_app(connection);

        (ToClient::Beast {
            id: BeastId(5),
            kind: BeastKind::Dolphins,
            position: Vec2::new(30.0, -10.0),
            velocity: Vec2::new(2.0, 0.0),
        })
        .write(&mut &server)
        .expect("pod");
        run_until(&mut app, "the pod appears", |app| {
            !beasts_afoot(app).is_empty()
        });
        run_frames(&mut app, 20);

        // One beast, several animals — each at a station, each under or
        // about the water and somewhere real, porpoising on its own phase.
        let members: Vec<Transform> = app
            .world_mut()
            .query_filtered::<&Transform, With<Porpoising>>()
            .iter(app.world())
            .copied()
            .collect();
        assert!(
            (POD_SIZE.0 as usize..=POD_SIZE.1 as usize).contains(&members.len()),
            "a pod of {}",
            members.len()
        );
        for member in &members {
            assert!(member.translation.is_finite());
            assert!(
                member.translation.y < 3.0,
                "a dolphin at {} m is a flying fish",
                member.translation.y
            );
        }
    }

    #[test]
    fn something_else_that_moves_is_not_taken_for_a_beast() {
        // The loader puts an animation player on anything animated — the
        // walking figure most of all — and only the ones hung under a beast
        // are the beasts' to pace.
        let (addr, socket) = fake_server(Vec2::ZERO, Vec2::ZERO);
        let connection = crate::net::Connection::join(&addr).expect("join");
        let _server = socket.recv().expect("the fake server keeps its socket");
        let mut app = beast_app(connection);

        let stranger = app.world_mut().spawn(AnimationPlayer::default()).id();
        run_frames(&mut app, 2);

        let stranger = app.world().entity(stranger);
        assert!(
            stranger.get::<Swishing>().is_none(),
            "the walker was taken for a shark"
        );
        assert!(
            stranger.get::<AnimationGraphHandle>().is_none(),
            "the swim was pressed on something else"
        );
    }
}
