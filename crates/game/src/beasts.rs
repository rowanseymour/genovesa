//! What the beasts look like: the server's creatures, drawn.
//!
//! A beast is the opposite arrangement from the wildlife. That is scenery this
//! client invents and the server has never heard of; a beast is a creature the
//! *server* means. The server owns where one is and where it is going and tells
//! everyone on a beat ([`protocol::ToClient::Beast`], one message that is both
//! introduction and movement); this module's job is the half a server has no
//! opinion on — what each kind looks like being there.
//!
//! The wire carries a point and a velocity on the ground plane; everything the
//! player sees is decided here — the body eased along the tellings the way
//! remote players' markers are, the bearing swung to follow the water, and
//! everything about depth. The shark holds just under the surface and breathes
//! its depth on a slow cycle, so the dorsal fin cuts the water for a while,
//! slides under, and comes back: the fin standing out of the swell *is* the
//! shark as far as most encounters go, which is why the model's fin is built
//! tall. The dolphins and the whale porpoise — a sine about cruising depth,
//! pitched by its own slope.
//!
//! A pod is one beast. The wire says where the pod is; how many dolphins
//! that is, and where each swims in the formation, is dealt from the
//! [`BeastId`] — which every machine was told — so every client draws the
//! same pod without another byte crossing the wire. The same trick the
//! eagles play with chunk coordinates, one rung up.
//!
//! The shark's tail is the model's own: one clip, `swim`, played on its own
//! clock — scaled by how fast the water is going by, but never seeked. The
//! player's gait inverted, deliberately: feet on ground have contact to keep,
//! where a tail in water only has to look like the thing pushing. Both
//! conductors climb out of an arriving model to see what it is part of — see
//! [`crate::models::above`].

use std::collections::HashMap;
use std::f32::consts::TAU;

use bevy::animation::graph::{AnimationGraph, AnimationGraphHandle, AnimationNodeIndex};
use bevy::animation::AnimationPlayer;
use bevy::gltf::GltfAssetLabel;
use bevy::prelude::*;

use protocol::{BeastId, BeastKind};

use crate::models::above;
use crate::sea::SeaConditions;
use crate::terrain::Ground;
use crate::{between, eased, matte, signed, unit, AppState, Size};

/// The shark, rigged and with its one clip in it. The other kinds are rigid
/// meshes whose whole-body motion is computed here, as it was when they were
/// wildlife — only the shark carries an armature, its tail being the one
/// piece of animal anatomy the camera watches work.
const SHARK_MODEL: &str = "models/shark.glb";
const DOLPHIN_MODEL: &str = "models/dolphin.glb";
const WHALE_MODEL: &str = "models/whale.glb";

/// How long each kind runs, nose to tail, and what its file measures — see
/// [`Size`], which is where the arithmetic and the reasoning both live.
///
/// The shark's range is the point of the exercise. Drawn at the model's own
/// length it was eleven centimetres longer than a dolphin and a quarter of
/// the whale, which is a reef shark: the animal read as harmless at exactly
/// the moment it is meant to be the reason to keep the hull between yourself
/// and the water. Three to four metres is a bull or a tiger, and it is the
/// dolphin beside it that makes the difference legible.
///
/// Which is why the dolphin stops at 2.9 rather than at the 3.2 a bottlenose
/// reaches. The two ranges are read against each other far more often than
/// either is read against life — a pod and a fin are the same water — and
/// overlapping them puts the occasional dolphin above the occasional shark,
/// which is exactly the impression the shark's range exists to end.
const SHARK_SIZE: Size = Size {
    model: 2.64,
    range: (3.0, 4.0),
};
const DOLPHIN_SIZE: Size = Size {
    model: 2.38,
    range: (2.4, 2.9),
};
const WHALE_SIZE: Size = Size {
    model: 11.1,
    range: (11.0, 15.0),
};

/// The clip, by its position in the file — held to its name by
/// `the_model_carries_the_swim_the_game_plays`. Swimming is the whole
/// vocabulary: a shark that stopped swimming would be a drowning shark.
const SWIM: usize = 0;

/// Metres per second of water the swim cycle was drawn against: at this
/// pace the clip plays at exactly the rate the master keyed. Not in the
/// file, for the same reason the player's stride is not — a clip knows how
/// long it lasts, not how fast the animal drawn in it was going.
const SWISH_PACE: f32 = 1.4;

/// How far under the local water surface the origin rides when the shark is
/// up, measured on the model rather than in world metres: [`glide`] scales it
/// by the size this shark was dealt, so what stays constant between a three
/// metre shark and a four metre one is the *proportion* of fin showing, which
/// is what the eye is reading. The dorsal fin tip stands 0.65 over the origin,
/// so riding awash puts a forearm's length of fin out of the water — it was
/// first set deeper, a hand's width of fin, and from the camera's height that
/// read as no fin at all. The back stays just under at this depth, a paleness
/// the fin is cutting out of.
const AWASH: f32 = 0.38;

/// And when it has sounded: fin under by a fin's own height, body still
/// shallow enough to stay a shape in the water rather than vanishing —
/// a shark that disappeared entirely would read as despawned, and the
/// unsettling thing about a shark is knowing it is still there. Scaled with
/// the animal, like [`AWASH`].
const SOUNDED: f32 = 0.88;

/// Seconds of one breath of depth, awash to sounded and back. Slow enough
/// that a fin is *there* to be noticed and tracked, quick enough that a
/// minute of watching a coast catches one.
const RISE_PERIOD: f32 = 9.0;

/// How far under the surface a beast rides once the server says it is no
/// longer showing itself, in metres — see [`protocol::ToClient::Beast`]'s
/// `surfaced`, which is a decision rather than a depth, this being the depth.
/// Deep enough that a body at it is a shadow in the blue and then nothing,
/// which is what makes it worth the wire: a whale that has been driven down
/// by a boat has *gone*, and so has one that has come to the end of its life.
const SOUNDED_DEEP: f32 = 7.0;

/// The least water a diving beast keeps under it, in metres. A shark leaving
/// the shallows is over sand for as long as the shelf runs, and would put
/// itself through it at the depth above; hugging the bottom until the floor
/// falls away is both what it can do and what the animal would.
const KEEL: f32 = 0.8;

/// How quickly a dive is drawn, in e-foldings per second. Slower than the
/// easing of a position below, and much slower than a breath: sounding is the
/// one thing these animals do that is meant to be *watched* happening, and a
/// body that sank at the pace it glides would have popped out of existence
/// with extra steps.
const DIVING: f32 = 0.55;

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
const POD_SIZE: (usize, usize) = (2, 5);

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
        surfaced: bool,
    ) {
        let told = Told {
            target: position,
            velocity,
            surfaced,
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
                // At the depth the first telling implies rather than eased
                // into it: a beast heard of for the first time is simply
                // where it is, and the easing below is for a beast that
                // *changes* — which is a dive, and is meant to be watched.
                Sounding(f32::from(!surfaced)),
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

/// Where the server last put a beast, where it said it was going, and whether
/// it said the animal is showing itself.
#[derive(Component, Clone, Copy)]
struct Told {
    target: Vec2,
    velocity: Vec2,
    surfaced: bool,
}

/// How far into a dive a beast is *drawn*, from `0.0` up to `1.0` down —
/// eased towards what the last telling said, exactly as its place is eased
/// towards where the last telling put it. The server decides that an animal
/// has gone down; how long that takes to look like, and how far down "down"
/// is, are this side's.
#[derive(Component)]
struct Sounding(f32);

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
            .add_systems(
                Update,
                // Nothing outside a match has beasts in it. Inside one, none
                // of this is the player's hands, so none of it pauses: the
                // sharks are the server's, and the server does not stop
                // swimming them because somebody opened a menu.
                (dress, conduct, glide, porpoise, swish).run_if(in_state(AppState::InWorld)),
            )
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
    // One white material for both, and for the same reason the birds share
    // one: each animal's colours are on its own vertices, so what is left for
    // a material to say is nothing.
    let painted = materials.add(matte(Color::WHITE));
    commands.insert_resource(BeastModels {
        dolphin: (
            assets.load(crate::model_mesh(DOLPHIN_MODEL, 0)),
            painted.clone(),
        ),
        whale: (assets.load(crate::model_mesh(WHALE_MODEL, 0)), painted),
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
                    // Hung at the size the id deals it — the same size
                    // [`glide`] rides it at, both of them asking [`Size`]
                    // rather than one of them being told by the other.
                    Transform::from_scale(Vec3::splat(shark_size(of.seed))),
                    WorldAssetRoot(assets.load(GltfAssetLabel::Scene(0).from_asset(SHARK_MODEL))),
                ));
            }
            BeastKind::Dolphins => {
                let (mesh, material) = models.dolphin.clone();
                for (member, station, size, swimming) in pod_members(of.seed) {
                    commands.entity(beast).with_child((
                        Name::new(format!("dolphin {member}")),
                        swimming,
                        Transform::from_translation(station).with_scale(Vec3::splat(size)),
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
                    Transform::from_xyz(0.0, -(WHALE_SWIM.cruise + WHALE_SWIM.leap), 0.0)
                        .with_scale(Vec3::splat(whale_size(of.seed))),
                    Mesh3d(mesh),
                    MeshMaterial3d(material),
                ));
            }
        }
    }
}

/// How big a shark of this id is drawn, as a multiple of the model. Asked
/// twice — once where the figure is hung and once every frame as it is
/// ridden — and a pure function of the id both times, so the two agree
/// without either storing the answer, exactly as the breath phase does.
fn shark_size(seed: u32) -> f32 {
    SHARK_SIZE.dealt(seed, 0x5A1E)
}

/// And a whale's, which is asked for in one place only — but a salt spelled
/// out at the call and again in the test that checks it is two spellings of
/// one number, and the test would go on passing after one of them changed.
fn whale_size(seed: u32) -> f32 {
    WHALE_SIZE.dealt(seed, 0x8A1E)
}

/// The members of the pod a [`BeastId`]'s bits deal: station index, where it
/// swims in the pod's frame, how big it is drawn, and its porpoising. A pure
/// function of the seed, which is the point — see the module doc — and
/// starting under the water's own opacity, so however a pod first appears it
/// *surfaces*.
///
/// Size is dealt per member rather than per pod, from the same bits the
/// station jitter uses: a pod is a family and holds a big one and a small
/// one, and sizing the whole pod together would put five identical animals
/// abreast, which is the thing the jitter exists to avoid.
fn pod_members(seed: u32) -> Vec<(usize, Vec3, f32, Porpoising)> {
    let count = between(seed, 0x90D5, POD_SIZE);
    POD_STATIONS
        .iter()
        .enumerate()
        .take(count)
        .map(|(member, (side, lag))| {
            let bits = seed ^ ((member as u32) << 8);
            let jitter = |salt: u32| signed(bits, salt);
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
                DOLPHIN_SIZE.dealt(bits, 0xD04),
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
        if above(&hierarchy, &figures, entity).is_none() {
            continue;
        }
        let Some(beast) = above(&hierarchy, &beasts, entity) else {
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

/// Swims each beast towards where the server last put it: eased over the
/// ground plane, swung gradually onto its bearing, sunk or raised as the
/// telling says it is showing itself or not, and — for the shark — ridden at
/// the depth its breath has reached, just under the swell, fin out for a
/// while in every cycle. The porpoising kinds keep their root on the
/// waterline and let [`porpoise`] give every member its own depth, the dive
/// included.
fn glide(
    time: Res<Time>,
    ground: Option<Res<Ground>>,
    sea: Res<SeaConditions>,
    mut beasts: Query<(&Beast, &Told, &mut Sounding, &mut Transform)>,
) {
    let dt = time.delta_secs();
    let elapsed = time.elapsed_secs_wrapped();

    for (beast, told, mut sounding, mut transform) in &mut beasts {
        let at = Vec2::new(transform.translation.x, transform.translation.z);
        let at = at.lerp(told.target, eased(SMOOTHING, dt));
        sounding.0 += (f32::from(!told.surfaced) - sounding.0) * eased(DIVING, dt);

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
                // Both depths are on the model, so a bigger shark rides
                // proportionally deeper and shows the same fin — see
                // [`AWASH`]. Riding every shark at one depth would put a
                // four-metre animal's whole shoulder out of the water.
                let deep = (AWASH + (SOUNDED - AWASH) * breathing) * shark_size(beast.seed);
                let riding = surface - deep;
                sunk(ground.as_deref(), at, riding, sounding.0)
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
///
/// A sounding animal loses the arc as it goes down, rather than carrying on
/// porpoising invisibly a few metres lower: the leap is scaled away by how
/// far into the dive it is, which takes the pitch flat with it, so a whale
/// that has been driven under levels off and sinks — one last back, and then
/// the blue.
fn porpoise(
    time: Res<Time>,
    ground: Option<Res<Ground>>,
    sea: Res<SeaConditions>,
    beasts: Query<(&Told, &Sounding, &Transform), With<Beast>>,
    mut members: Query<(&Porpoising, &ChildOf, &mut Transform), Without<Beast>>,
) {
    let elapsed = time.elapsed_secs_wrapped();
    for (swimming, of, mut transform) in &mut members {
        let Ok((told, sounding, carrier)) = beasts.get(of.parent()) else {
            continue;
        };
        let (at, water) = sea.under_station(ground.as_deref(), carrier, &transform, elapsed);

        let (rise, run) = (TAU / swimming.period * elapsed + swimming.phase).sin_cos();
        let leap = swimming.leap * (1.0 - sounding.0);
        let riding = water - swimming.cruise + leap * rise;
        transform.translation.y = sunk(ground.as_deref(), at.xz(), riding, sounding.0);
        let pitch = (leap * TAU / swimming.period * run).atan2(told.velocity.length());
        transform.rotation = Quat::from_rotation_x(pitch);
    }
}

/// A ride taken down by how far into a dive the animal is, but never through
/// the bottom: over a shelf it hugs the sand at [`KEEL`] and only truly
/// disappears where the floor falls away, which is where the server sends
/// anything that is leaving for good. Ground this client has not been sent
/// counts as no floor at all — it is beyond the haze by construction, and a
/// beast out there is nothing to look at either way.
///
/// The two limits are applied in this order because they are not equals. The
/// dive may not lift a body *above* where it was riding, which would be a
/// sounding animal jumping; and it may not put one through the sand, which is
/// worse, so the floor gets the last word. Taking them the other way about
/// discards the floor wherever a body already rides below the keel line — the
/// trough of a dolphin's own porpoising over the shallowest floor its band
/// allows is exactly that — and a swell then walks the tail through the
/// bottom.
fn sunk(ground: Option<&Ground>, at: Vec2, riding: f32, sounding: f32) -> f32 {
    let deep = riding - SOUNDED_DEEP * sounding;
    match ground.and_then(|ground| ground.height(at.x, at.y)) {
        Some(floor) => deep.min(riding).max(floor + KEEL),
        None => deep,
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::state::app::StatesPlugin;
    use bevy::time::{TimePlugin, TimeUpdateStrategy};

    use super::*;
    use crate::net::{fake_server, NetPlugin, Online};
    use crate::testing::{
        assert_model_draws, assert_model_is_painted, assert_rigid_skin, clip_names,
        creature_named_by, extent, run_frames, run_until, span, triangles,
    };
    use crate::Helm;
    use protocol::ToClient;

    #[test]
    fn the_model_is_a_shark_fit_to_draw() {
        assert_model_draws(SHARK_MODEL, &[(0, "hide")]);
        assert_model_is_painted(SHARK_MODEL, 0);
    }

    #[test]
    fn the_dolphin_and_the_whale_bring_their_own_colours() {
        for file in [DOLPHIN_MODEL, WHALE_MODEL] {
            assert_model_is_painted(file, 0);
        }
    }

    #[test]
    fn the_skin_is_rigid_so_the_facets_stay_flat() {
        assert_rigid_skin(SHARK_MODEL);
    }

    #[test]
    fn the_shark_is_a_shark_swimming_forward() {
        // Shark-shaped — not a whale and not a minnow — with the girth
        // forward of amidships, which is what points it: the file faces -Z
        // like everything here, so the fat end must lean that way. How long
        // the animal actually swims is [`SHARK_SIZE`]'s business, this being
        // the file it is measured from.
        let (nose, tail) = extent(SHARK_MODEL, 0, 2);
        let length = tail - nose;
        assert!(
            (2.2..=3.2).contains(&length),
            "nose to tail is {length} m, which is not this game's shark"
        );

        let girth = triangles(SHARK_MODEL, 0, "POSITION")
            .into_iter()
            .flatten()
            .fold((0.0_f32, 0.0_f32), |(widest, at), c| {
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
        // And nothing hangs deep enough to plough the sand of the shallow
        // band it patrols.
        let (keel, fin) = extent(SHARK_MODEL, 0, 1);
        assert!(
            fin > AWASH && fin < 1.0,
            "the fin tops out {fin} m over the spine"
        );
        assert!(keel > -0.6, "the shark draws {} m", -keel);
    }

    #[test]
    fn the_model_carries_the_swim_the_game_plays() {
        // Asked for by position, held to its name — the figure's own rule.
        assert_eq!(
            clip_names(SHARK_MODEL).get(SWIM).map(String::as_str),
            Some("swim")
        );
    }

    #[test]
    fn the_rigid_kinds_are_one_creature_each() {
        // These pins lived in the wildlife's tests while these animals were
        // wildlife.
        for file in [DOLPHIN_MODEL, WHALE_MODEL] {
            assert_model_draws(file, &[(0, creature_named_by(file))]);
        }
    }

    #[test]
    fn the_models_measure_what_their_sizes_say_they_do() {
        // The one thing about a [`Size`] nothing at runtime can check. It
        // turns a length in world metres into a scale by dividing by what
        // the file measures, so a remodel that came through in centimetres —
        // or with the exporter's axes wrong — would swim a shark a hundred
        // times over while every number in this module still read three to
        // four metres. Nose to tail lies along Z, the forward axis.
        for (file, size) in [
            (SHARK_MODEL, &SHARK_SIZE),
            (DOLPHIN_MODEL, &DOLPHIN_SIZE),
            (WHALE_MODEL, &WHALE_SIZE),
        ] {
            let measured = span(file, 0, 2);
            assert!(
                (measured - size.model).abs() < 0.01,
                "{file} measures {measured} m nose to tail, but its Size divides by {}",
                size.model
            );
        }
    }

    #[test]
    fn every_beast_is_dealt_a_size_inside_its_kind_s_range() {
        // The sizes are the whole of what `dress` hangs and what `glide`
        // rides at, and a deal that fell outside its range would be an
        // animal that is simply the wrong size, which is only ever noticed
        // by eye. Both ends are asked for as well: a `dealt` that answered
        // one value would satisfy the range and lose the variety, which is
        // the reason any of this is dealt rather than fixed.
        let mut seen: Vec<(f32, f32)> = vec![(f32::MAX, f32::MIN); 3];
        for seed in 0..1024 {
            let sizes = [
                (0, SHARK_SIZE.model * shark_size(seed), &SHARK_SIZE),
                (2, WHALE_SIZE.model * whale_size(seed), &WHALE_SIZE),
            ];
            let pod: Vec<_> = pod_members(seed)
                .into_iter()
                .map(|(_, _, size, _)| (1, DOLPHIN_SIZE.model * size, &DOLPHIN_SIZE))
                .collect();
            for (kind, drawn, size) in sizes.into_iter().chain(pod) {
                let (low, high) = size.range;
                assert!(
                    (low..=high).contains(&drawn),
                    "a beast was dealt {drawn} m, outside {low}–{high}"
                );
                seen[kind].0 = seen[kind].0.min(drawn);
                seen[kind].1 = seen[kind].1.max(drawn);
            }
        }
        for (kind, size) in [&SHARK_SIZE, &DOLPHIN_SIZE, &WHALE_SIZE]
            .into_iter()
            .enumerate()
        {
            let (low, high) = size.range;
            let span = high - low;
            assert!(
                seen[kind].0 < low + span * 0.1 && seen[kind].1 > high - span * 0.1,
                "kind {kind} only ever came out {:?} of {low}–{high}",
                seen[kind]
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
            assert!((POD_SIZE.0..=POD_SIZE.1).contains(&dealt.len()));
            for ((member, station, size, _), again) in dealt.iter().zip(pod_members(seed)) {
                assert_eq!(*station, again.1, "member {member} moved between deals");
                assert_eq!(*size, again.2, "member {member} changed size between deals");
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
        // Headless frames take next to no real time, and half of what this
        // module does is eased *per second* — a dive most of all. The clock
        // is stepped by hand so that a frame here is worth what a frame is
        // worth on a screen.
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            16,
        )))
        .insert_resource(Online::new(connection));
        app.update();
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::InWorld);
        app.update();
        app
    }

    /// How deep the one beast in the world is riding — the whole of what a
    /// dive is, from outside.
    fn riding(app: &mut App) -> f32 {
        app.world_mut()
            .query_filtered::<&Transform, With<Beast>>()
            .single(app.world())
            .expect("one beast")
            .translation
            .y
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
            surfaced: true,
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
            surfaced: true,
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
            surfaced: true,
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
    fn a_beast_told_to_have_sounded_goes_down_and_takes_its_time() {
        // The one thing about depth the server has an opinion on — see
        // `protocol::ToClient::Beast`. A whale driven under by a boat, and a
        // beast at the end of its life, are both told the same way, and this
        // is where that becomes a body going out of sight.
        let (addr, socket) = fake_server(Vec2::ZERO, Vec2::ZERO);
        let connection = crate::net::Connection::join(&addr).expect("join");
        let server = socket.recv().expect("the fake server keeps its socket");
        let mut app = beast_app(connection);

        let told = |surfaced: bool| {
            (ToClient::Beast {
                id: BeastId(2),
                kind: BeastKind::Shark,
                position: Vec2::new(6.0, 0.0),
                velocity: Vec2::new(1.4, 0.0),
                surfaced,
            })
            .write(&mut &server)
            .expect("beast");
        };

        // Up: riding awash, where a fin has something to cut.
        told(true);
        run_until(&mut app, "the beast appears", |app| {
            !beasts_afoot(app).is_empty()
        });
        run_frames(&mut app, 120);
        let awash = riding(&mut app);
        assert!(
            (-2.0..=0.0).contains(&awash),
            "a shark on the surface riding at {awash} m"
        );

        // Told it has gone down, it goes — but over a second or two, not
        // between frames. A body that fell to depth in one frame would read
        // as the animal being deleted, which is the whole thing this exists
        // to avoid.
        told(false);
        run_frames(&mut app, 6);
        let starting = riding(&mut app);
        assert!(
            starting < awash && starting > awash - SOUNDED_DEEP / 2.0,
            "a tenth of a second took it from {awash} m to {starting} m"
        );

        run_frames(&mut app, 500);
        let sounded = riding(&mut app);
        assert!(
            sounded < awash - SOUNDED_DEEP * 0.8,
            "several seconds of diving only reached {sounded} m"
        );

        // And it comes back up when it is told it has: a dive is a state the
        // server holds, not a one-way trip this side remembers.
        told(true);
        run_frames(&mut app, 500);
        let up = riding(&mut app);
        assert!(
            up > sounded + SOUNDED_DEEP * 0.8,
            "it stayed down at {up} m"
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
            surfaced: true,
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
            (POD_SIZE.0..=POD_SIZE.1).contains(&members.len()),
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
