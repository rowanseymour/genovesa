//! The boat the player gets about in, and the keys that steer it.
//!
//! The hull is modelled rather than drawn here: [`MODEL`] is a glTF file built
//! from a Blender master under `assets-src/`, and this module spawns its meshes
//! and steers what they hang off. What is here to stay is the *entity*: the
//! player's place in the world, which the movement keys drive ([`steer`]) and
//! the camera stays centred on.
//!
//! The few dimensions still named below are the ones the *rules* are written
//! against — where the keel is, and how deep. Those are not the model's to
//! change quietly, so `the_model_is_the_hull_the_keel_is_probed_along` holds
//! the file to them; everything else about the shape is the modeller's, and
//! this file has no opinion on it.
//!
//! It faces down its own -Z, so [`Transform::forward`] is the way it is
//! pointing and steering can leave the axis convention alone. Its origin is on
//! the waterline rather than at the keel or the deck, which is what lets
//! [`float`] put it down by simply setting the height of the surface it is on.

use bevy::asset::AssetPath;
use bevy::gltf::GltfAssetLabel;
use bevy::prelude::*;

use crate::bindings::{Action, KeyBindings};
use crate::camera::View;
use crate::terrain::Ground;
use crate::{eased, matte, AppState};

/// The boat, as a file. Built from `assets-src/boat.glb/boat.blend` by the
/// `build.sh` beside it, which is also where the export settings the look
/// depends on are written down.
const MODEL: &str = "boat.glb";

/// Which mesh in [`MODEL`] is which. glTF numbers its meshes rather than naming
/// them in a way the loader can ask for, so these are positions in the file —
/// which means reordering the objects in Blender would silently swap the hull
/// for the spar. `the_model_holds_a_hull_and_a_spar_in_that_order` is what
/// stops that being found by looking at it.
const HULL_MESH: usize = 0;
const SPAR_MESH: usize = 1;

/// Length overall, in metres. A small sailing boat: at the default zoom the
/// visible ground is some tens of metres across, so this reads as a boat
/// rather than as a speck, and the same at the far end of the zoom range it is
/// still a mark on the water rather than gone.
const LENGTH: f32 = 7.0;

/// Keel depth below the waterline. The sea is translucent, so this much of the
/// hull shows through the water as a darker shape under the deck — but what
/// makes it the game's business rather than the model's is [`GROUNDING_DRAFT`],
/// which is measured from it.
const DRAFT: f32 = 0.8;

/// Where the keel begins and ends, in metres from amidships — negative
/// forward, the same axis the hull is modelled on. The forefoot stops short of
/// the bow, which is what gives the stem its rake; the heel runs right aft to
/// the transom. [`grounding`] probes along these, so what runs aground is the
/// line that is drawn.
const FOREFOOT_STATION: f32 = -LENGTH * 0.5 * 0.7;
const HEEL_STATION: f32 = LENGTH * 0.5;

/// How much of the keel the ground is allowed to take before the hull is
/// stopped. Stopping the boat the instant the ground rises to meet the keel is
/// an invisible wall a boat's length offshore, whereas a fifth of a metre of
/// bite is a boat *beaching*: the keel is seen to touch, and then it stops.
/// Well clear of the two centimetres the heights are quantised to, so the
/// threshold cannot chatter.
const KEEL_BITE: f32 = 0.2;

/// How little water the hull is held in: ground standing higher than this far
/// below the waterline stops it.
///
/// On the coasts the generator draws this puts the hull within a metre or two
/// of the waterline; where it holds a boat further off, it is off a shelf too
/// thin to float one, and the shallows are painted as shallows long before
/// they are this thin — so a boat held out is held out of water it can be
/// seen to be held out of.
const GROUNDING_DRAFT: f32 = DRAFT - KEEL_BITE;

/// How many points along the keel are asked about the bottom. Spread from the
/// forefoot to the heel inclusive, so the gap between them comes out just under
/// the two metres the ground is sampled at: no facet of the height field can
/// lie wholly between two probes, so ground that rises across a facet is read
/// on the way up rather than stepped over.
///
/// That is what the spacing buys, and it is worth being plain that it is less
/// than "nothing gets past". A crest only one lattice line wide is *not* seen:
/// the field is linear between its corners, so two probes either side of such a
/// crest read its flanks, and the hull sails through a rock standing at the
/// waterline. Coasts are safe from it by being coasts — the bottom shelves, so
/// the ground under the keel is near enough monotone, and the innermost probe
/// is reading the shallowest water and reading it honestly. What is exposed is
/// the isolated skerry, which the generator draws on purpose and draws about a
/// facet across. Seeing one reliably would mean probing at a fraction of a
/// metre rather than at two, on every frame and at both poses, to buy a rock in
/// open water — while the coasts, which are what a boat is actually stopped by,
/// need none of it. Sailing through a skerry is the smaller wrong, and the one
/// that can be paid off from the other end, by giving the skerries some width.
///
/// The sides are not probed: the hull is a shallow V, drawing only [`DRAFT`] on
/// the centreline and nothing at all at the beam, so a probe out there would
/// have to carry a draught of its own to say anything the keel has not said.
/// That is a standing condition on the model rather than an observation about
/// one — a hull remodelled with a flat bottom carried out to the beam would
/// need probes out there too.
const KEEL_PROBES: usize = 4;

/// Metres per second under way. Brisk beyond honesty for a seven-metre hull,
/// but the boat is how the world is crossed: at this speed the ground in view
/// at the default zoom slides by in a few seconds, and the next island is
/// minutes away rather than tens of minutes.
const SPEED: f32 = 10.0;

/// Metres per second going astern — enough to back off a beach or out of a
/// cove, and slow enough that nobody crosses an ocean in reverse.
const ASTERN_SPEED: f32 = 4.0;

/// Seconds of lag between the speed the keys ask for and the speed the hull
/// makes — the time constant of an exponential ease, so most of any change
/// arrives within this long and it is all but done in three times it. Named
/// as a duration rather than as the rate [`eased`] takes, seven metres of
/// timber having a weight that is easier to think about in seconds. The
/// ease is what gives seven metres of timber its weight: the hull gathers
/// way over a few seconds instead of leaping to [`SPEED`] on the frame the
/// key goes down, and carries it for a couple of lengths' glide when the
/// key comes up.
const WAY_RESPONSE: f32 = 1.5;

/// Way below this, with no drive asked for, is stopped, and [`steer`] snaps
/// it to exactly zero. The ease only ever halves the remainder — left alone
/// the boat would creep forever, never quite done stopping — and a hull at
/// rest should be *at rest*: the same spot every frame, nothing moving.
const WAY_STOPPED: f32 = 0.02;

/// How fast the helm brings the bow round, in radians per second. Together
/// with [`SPEED`] this fixes the turning circle at about five metres — tight
/// enough to feel answerable from a camera forty metres up, wide enough that
/// coming about reads as a turn rather than a spin.
const TURN_RATE: f32 = 2.0;

/// How far the hull heels in a full-helm turn at full speed, in radians —
/// enough to swing the masthead more than a metre, which is what makes a turn
/// visible from forty metres up, and shy of anything that reads as capsizing.
/// It heels *outwards*, the way a keeled hull does: the water grips the keel
/// below the waterline while the turn flings the mass above it, so the boat
/// leans out of the corner, not into it like a bicycle.
const HEEL_AT_FULL_TURN: f32 = 0.22;

/// Seconds of lag between the heel a turn asks for and the heel the hull
/// shows, the same exponential shape as [`WAY_RESPONSE`] and much quicker:
/// rolling is the lightest thing seven metres of timber does. Quick enough
/// that the lean arrives while the turn is still news, slow enough that the
/// hull rolls rather than snaps — and the same curve is the straightening,
/// run back down to level when the helm comes off.
const HEEL_RESPONSE: f32 = 0.4;

/// Within this of the heel the turn is asking for, the hull snaps to it
/// exactly — the same tail-closing that [`WAY_STOPPED`] does for the way,
/// and for the same reason: the ease only ever halves the remainder, and a
/// hull done straightening should be *level*, holding one rotation frame
/// after frame rather than forever creeping towards it. A third of a degree,
/// invisible at any zoom.
const HEEL_SETTLED: f32 = 0.005;

/// Timber. Nothing on an island or in the sea is anywhere near this hue, so the
/// boat is findable in a landscape of greens and blues without being lit any
/// differently from them.
const HULL_COLOR: Color = Color::srgb(0.62, 0.28, 0.22);
/// Bare spar, pale enough to stand off both the water and the hull.
const SPAR_COLOR: Color = Color::srgb(0.86, 0.80, 0.68);

/// The player's boat. One per match, spawned where the world is entered.
///
/// `way` is the speed the hull is actually making along its heading, in
/// metres per second, ahead positive — the state the eased throttle lives
/// in. The keys name a speed; [`steer`] brings `way` towards it.
///
/// `heel` is the roll the hull is showing, in radians about its own forward,
/// positive with the masthead to port. Kept here rather than read back off
/// the transform because the transform holds heel and heading multiplied
/// together, and unpicking a quaternion every frame to learn a number this
/// system wrote itself is work for nothing.
#[derive(Component, Default)]
pub struct Boat {
    way: f32,
    heel: f32,
}

pub struct BoatPlugin;

impl Plugin for BoatPlugin {
    fn build(&self, app: &mut App) {
        // Steering before floating, so ground gained or lost by this frame's
        // movement is under the hull the same frame rather than the next.
        app.add_systems(OnEnter(AppState::InWorld), launch)
            .add_systems(
                Update,
                (steer, float).chain().run_if(in_state(AppState::InWorld)),
            );
    }
}

/// Puts the boat in the world at the point the world is entered, pointing the
/// way the opening view looks.
///
/// The view names where the player enters the world, so the boat goes there
/// rather than anywhere of its own choosing. Entry is the world's spawn
/// point — open water the layout keeps just off the first island's coast —
/// so the boat starts afloat with land dead ahead; a `--focus` can still put
/// it down inland, aground until the movement keys drive it back to the sea.
/// The meshes hang off the boat as children rather than on it: a mesh carries
/// one material, and the hull and the spar are two colours. Their geometry is
/// already in the boat's own frame — the modeller places the mast on the deck,
/// not the game — so the children sit at the identity and the only transform
/// anything writes is the boat's own.
fn launch(
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    assets: Res<AssetServer>,
    view: Res<View>,
) {
    // The file's own materials are ignored, and the meshes are pulled out of it
    // one at a time rather than the whole scene being spawned. glTF materials
    // are PBR — a roughness, a metalness, a specular response — and the look
    // here is a small fixed palette under `matte`, so a hull lit the way the
    // file asked for would be the one surface in the world with a highlight on
    // it. Leaving the colours in Rust also keeps them beside the ground's,
    // which is the comparison that matters when either is picked.
    let hull_material = materials.add(matte(HULL_COLOR));
    let spar_material = materials.add(matte(SPAR_COLOR));

    commands.spawn((
        Name::new("Boat"),
        Boat::default(),
        DespawnOnExit(AppState::InWorld),
        // A rotation of `yaw` about the vertical takes -Z to the camera's own
        // forward, so the boat starts pointing away from the viewer.
        Transform::from_xyz(view.focus.x, 0.0, view.focus.z)
            .with_rotation(Quat::from_rotation_y(view.yaw)),
        // Carried by the parent because the children inherit it: without one
        // here there is nothing for their own visibility to be computed
        // against, and a boat whose meshes are on entities of their own would
        // never be drawn.
        Visibility::default(),
        children![
            (
                Name::new("Hull"),
                Mesh3d(assets.load(mesh_in_model(HULL_MESH))),
                MeshMaterial3d(hull_material),
            ),
            (
                Name::new("Spar"),
                Mesh3d(assets.load(mesh_in_model(SPAR_MESH))),
                MeshMaterial3d(spar_material),
            )
        ],
    ));

    // Said out loud for the same reason a run without a seed says which world
    // it picked: a placeholder nobody can find is indistinguishable from one
    // that never spawned, and `--focus` takes exactly these two numbers.
    info!("boat launched at {}, {}", view.focus.x, view.focus.z);
}

/// Keeps the boat on the surface it is over — [`Ground::surface`], the same
/// rule the other players' markers ride.
///
/// The waterline is the hull's origin, so a boat that has run aground is
/// half-buried in the hillside; that is what aground looks like, and steering
/// is what will keep it off.
fn float(ground: Option<Res<Ground>>, mut boats: Query<&mut Transform, With<Boat>>) {
    for mut transform in &mut boats {
        let at = transform.translation;
        let Some(surface) = ground.as_ref().and_then(|g| g.surface(at.x, at.z)) else {
            continue;
        };
        transform.translation.y = surface;
    }
}

/// How far the bottom stands above the depth the hull is held at, in metres,
/// taken at the worst-placed point of the keel — negative for as long as there
/// is water enough under all of it, zero where the hull is about to be stopped.
/// Not the keel's own penetration, which is this plus the gap between [`DRAFT`]
/// and [`GROUNDING_DRAFT`]: the rule wants one number that rises as the ground
/// does, and nothing ever reads it but its sign and its ordering against
/// itself, both of which the offset leaves alone.
///
/// This is the whole of collision. The ground the client has is a height field
/// on a two-metre lattice, and the boat is a keel line above it, so "is there
/// water enough here" is a handful of lookups rather than anything to do with
/// intersecting the hull's triangles: [`Ground::height`] answers on exactly the
/// facets the mesh was built from, which is what makes the ground the boat is
/// stopped by the ground the player can see.
///
/// A probe over a chunk that has not arrived says nothing rather than
/// objecting, the same choice [`float`] makes: ground the client has not been
/// sent is not ground it may invent. Outrunning the stream would take a stalled
/// server, and if land does turn up under the hull, backing off still works —
/// see [`steer`] for why.
fn grounding(ground: Option<&Ground>, transform: &Transform) -> f32 {
    let Some(ground) = ground else {
        return f32::NEG_INFINITY;
    };

    let keel = HEEL_STATION - FOREFOOT_STATION;
    (0..KEEL_PROBES)
        .filter_map(|i| {
            let station = FOREFOOT_STATION + keel * i as f32 / (KEEL_PROBES - 1) as f32;
            let at = transform.transform_point(Vec3::new(0.0, 0.0, station));
            Some(ground.height(at.x, at.z)? + GROUNDING_DRAFT)
        })
        .fold(f32::NEG_INFINITY, f32::max)
}

/// Drives the boat in its own frame, the way a boat is driven: forward and
/// back run the hull along its heading, and the steering keys are the helm,
/// bringing the bow round for as long as they're held. The view plays no part
/// — turning the camera changes what the keys look like on screen, never what
/// they do — which is what makes a long sail a held key rather than a chase
/// between the camera's yaw and the boat's.
///
/// The throttle is eased rather than instant: the keys name a target speed
/// and the hull's way relaxes towards it on the [`WAY_RESPONSE`] curve,
/// stepped exactly for however long the frame was, so the ramp is the same
/// shape at any frame rate. That covers both ends of a sail — way gathered
/// over seconds when the key goes down, and carried into a glide when it
/// comes up — from one constant, with [`WAY_STOPPED`] closing the tail the
/// exponential would otherwise never finish.
///
/// The helm answers even with no way on, which no rudder would; a boat that
/// can't point where it's told while stationary is annoying before it is
/// realistic. It answers aground as well, and for a better reason than that:
/// a turn refused alongside an advance is a hull wedged bow-first against a
/// shore with nothing left that would free it, so the bow may always come
/// round even where the hull may not go.
///
/// Land is what the hull may not go through, and the frame's advance is
/// offered to [`grounding`] before it is taken. It is allowed if the pose it
/// would reach floats — or, failing that, if it is aground no *deeper* than
/// the pose already held. That second half is not an escape hatch for a boat
/// that has got itself ashore; it is the only rule that both frees one and
/// can't be played. It is also narrower than it reads. A hull that is floating
/// can only ever be allowed a pose that floats — `here` at or under zero makes
/// the second clause imply the first — so a boat under way never reaches dry
/// ground at all: it halts still afloat, with at most the fifth of a metre
/// between [`DRAFT`] and [`GROUNDING_DRAFT`] in the mud, and backing off from
/// there is the *first* clause doing the work, the water astern being water.
/// What the second clause is for is the pose the boat did not sail into — a
/// `--focus` that puts it inland, and ground arriving under a hull already
/// sitting there. Out of those every way down to the sea is downhill, so it
/// goes; and every way further in is uphill, so nosing the bow over a beach to
/// unlock the island — which "aground already, let it through" would hand a
/// player on the first frame — is refused like any other climb. Which is also
/// why the comparison carries no tolerance: a hair of slack is a hair of climb
/// every frame, and a hair a frame is a metre a second up a hillside.
///
/// Only the pose at the end of the advance is judged; the path swept getting
/// there is covered by the probes of the frame before, which holds for as long
/// as a frame's advance stays under the probe spacing. At [`SPEED`] that is
/// seventeen centimetres at sixty frames a second, and two and a half metres
/// at the quarter second Bevy clamps a stalled frame to — so the sweep is only
/// ever missed on a frame that was already a visible break in the picture.
///
/// Turning at speed also heels the hull: the target lean is helm times way —
/// sharpness times speed, so a hard turn at full way carries the whole of
/// [`HEEL_AT_FULL_TURN`], a gentle one at half way a quarter of it, and a bow
/// swung round at rest none at all — and the shown heel relaxes towards it on
/// the [`HEEL_RESPONSE`] curve, which is both the roll into the turn and the
/// straightening out of it. Signed way keeps the geometry honest going
/// astern: the same helm turns about a centre on the other side, so the heel
/// flips with it. The roll is applied about the boat's own forward axis, and
/// the keel lies along that axis, so heeling moves nothing [`grounding`]
/// probes or [`steer`] advances along — it is wholly a thing the eye gets.
///
/// Both poses are judged with the rotation the helm has just applied, so a
/// turn only ever changes where the advance goes, never whether it is allowed.
/// Being stopped takes the way off, which is what running aground does; the
/// ease then makes coming off again the few seconds it should be. What the
/// hull is stopped *by* is the keel line and nothing else — the stem rakes out
/// over the forefoot, so a bow can overhang a cliff face by half a metre
/// before anything objects, which from a camera forty metres up is nothing.
fn steer(
    keys: Res<ButtonInput<KeyCode>>,
    bindings: Res<KeyBindings>,
    time: Res<Time>,
    ground: Option<Res<Ground>>,
    mut boats: Query<(&mut Transform, &mut Boat)>,
) {
    // Direction first, speed second, so opposed keys cancel outright rather
    // than the faster gear winning by the difference.
    let mut drive = 0.0;
    if bindings.held(&keys, Action::MoveForward, KeyCode::ArrowUp) {
        drive += 1.0;
    }
    if bindings.held(&keys, Action::MoveBack, KeyCode::ArrowDown) {
        drive -= 1.0;
    }
    let speed = if drive > 0.0 { SPEED } else { ASTERN_SPEED };

    // Port is a positive turn about the vertical, the same way round as the
    // camera's own Q.
    let mut helm = 0.0;
    if bindings.held(&keys, Action::SteerLeft, KeyCode::ArrowLeft) {
        helm += 1.0;
    }
    if bindings.held(&keys, Action::SteerRight, KeyCode::ArrowRight) {
        helm -= 1.0;
    }

    let target = drive * speed;
    // A response named in seconds is a rate of its reciprocal.
    let t = eased(1.0 / WAY_RESPONSE, time.delta_secs());
    let heel_t = eased(1.0 / HEEL_RESPONSE, time.delta_secs());
    let ground = ground.as_deref();

    for (mut transform, mut boat) in &mut boats {
        if helm != 0.0 {
            transform.rotate_y(helm * TURN_RATE * time.delta_secs());
        }
        // Written only while something is happening, so an idle boat holds
        // still without being marked changed every frame.
        if target != 0.0 || boat.way != 0.0 {
            let way = boat.way + (target - boat.way) * t;
            let way = if target == 0.0 && way.abs() < WAY_STOPPED {
                0.0
            } else {
                way
            };

            let advance = transform.forward() * way * time.delta_secs();
            let here = grounding(ground, &transform);
            let there = grounding(
                ground,
                &Transform {
                    translation: transform.translation + advance,
                    ..*transform
                },
            );

            if there <= 0.0 || there <= here {
                boat.way = way;
                transform.translation += advance;
            } else {
                boat.way = 0.0;
            }
        }

        // Heel last, against the way this frame settled on, so running
        // aground starts the straightening the same frame it takes the way
        // off. Port helm is a positive turn and an outward lean is to
        // starboard, which about the forward axis is a negative roll — hence
        // the sign. The transform holds heading-then-heel, and the helm above
        // multiplies heading on from the left, so rolling on from the right
        // reaches the heel factor alone and the guard keeps an idle boat's
        // rotation unwritten.
        let target_heel = -HEEL_AT_FULL_TURN * helm * boat.way / SPEED;
        if boat.heel != target_heel {
            let heel = boat.heel + (target_heel - boat.heel) * heel_t;
            let heel = if (target_heel - heel).abs() < HEEL_SETTLED {
                target_heel
            } else {
                heel
            };
            transform.rotation *= Quat::from_rotation_z(heel - boat.heel);
            boat.heel = heel;
        }
    }
}

/// Where in [`MODEL`] to find one of its meshes.
///
/// `primitive: 0` because each object in the master carries one material and so
/// exports as a mesh of a single primitive; an object split across two
/// materials would arrive as two, and would want spawning as two children.
fn mesh_in_model(mesh: usize) -> AssetPath<'static> {
    GltfAssetLabel::Primitive { mesh, primitive: 0 }.from_asset(MODEL)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::state::app::StatesPlugin;
    use bevy::time::{TimePlugin, TimeUpdateStrategy};
    use protocol::ground::FACET_METRES;

    use super::*;
    use crate::testing::{elapsed, hold, rebind, run_frames, test_ground, TEST_ISLAND_REACH};

    /// How long every test frame lasts. Headless frames take next to no real
    /// time, which the old instant throttle never noticed — but the eased one
    /// is a curve *in seconds*, so the clock is stepped by a fixed sixty-a-
    /// second frame and the tests get the ramp a player would.
    const FRAME: Duration = Duration::from_millis(16);

    /// Frames enough for the ease to be indistinguishable from settled —
    /// over eight time constants, a remainder of a few parts in ten thousand.
    const SETTLED: usize = 800;

    /// A headless app with the boat systems running, already in a match.
    fn test_app() -> App {
        let mut app = App::new();
        // `AssetPlugin` because the boat is spawned out of a file now, and
        // `TaskPoolPlugin` because that is where it finds the thread to read it
        // on. Nothing here waits for the load — these tests are about where the
        // hull is and what it does, not what it looks like — but `launch` asks
        // the asset server for its meshes, and without one there is no boat.
        app.add_plugins((
            TaskPoolPlugin::default(),
            AssetPlugin::default(),
            TimePlugin,
            StatesPlugin,
            BoatPlugin,
        ))
        .insert_resource(TimeUpdateStrategy::ManualDuration(FRAME))
        .init_state::<AppState>()
        .init_resource::<View>()
        .init_resource::<KeyBindings>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_asset::<Mesh>()
        .init_resource::<Assets<StandardMaterial>>();
        app.update();
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::InWorld);
        app.update();
        app
    }

    fn boat(app: &mut App) -> Transform {
        *app.world_mut()
            .query_filtered::<&Transform, With<Boat>>()
            .single(app.world())
            .expect("a match should have a boat in it")
    }

    /// The bow's bearing, in the same terms as a camera yaw.
    fn heading_yaw(app: &mut App) -> f32 {
        let forward = boat(app).forward();
        f32::atan2(-forward.x, -forward.z)
    }

    /// Radians per second the bow comes round at while `key` is held. A rate
    /// rather than an angle — proportionality to how long the key was held is
    /// what makes a turn the same on any machine.
    fn turn_rate(key: KeyCode) -> f32 {
        let mut app = test_app();
        let start_yaw = heading_yaw(&mut app);
        let before = elapsed(&app);
        hold(&mut app, key);
        run_frames(&mut app, 20);
        let seconds = elapsed(&app) - before;
        assert!(seconds > 0.0, "no time passed while the key was held");
        (heading_yaw(&mut app) - start_yaw) / seconds
    }

    /// The model as it sits on disk, read straight out of the `.glb` rather
    /// than through Bevy's loader.
    ///
    /// A glTF binary is a JSON chunk describing the file and a binary chunk
    /// holding the numbers, and the tests below want both. Going through the
    /// asset server instead would mean standing up a render app and waiting on
    /// a load, to learn things the file states plainly.
    fn model() -> (serde_json::Value, Vec<u8>) {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/boat.glb");
        let file = std::fs::read(path).expect("assets/boat.glb — run assets-src/boat.glb/build.sh");
        assert_eq!(&file[..4], b"glTF", "not a glTF binary");

        let (mut at, mut chunks) = (12, Vec::new());
        while at < file.len() {
            let length = u32::from_le_bytes(file[at..at + 4].try_into().unwrap()) as usize;
            chunks.push(file[at + 8..at + 8 + length].to_vec());
            at += 8 + length;
        }
        let json = serde_json::from_slice(&chunks[0]).expect("the glTF's JSON chunk");
        (json, chunks[1].clone())
    }

    /// One mesh of the model, as the triangles it is made of — `attribute`
    /// being `POSITION` for where its corners are or `NORMAL` for where they
    /// face. Read through the accessors' own view of the buffer, so an
    /// exporter that changes how it packs the numbers changes nothing here.
    fn triangles(index: usize, attribute: &str) -> Vec<[Vec3; 3]> {
        let (json, buffer) = model();
        let primitive = &json["meshes"][index]["primitives"][0];

        let read = |accessor: &serde_json::Value, stride: usize| -> Vec<u8> {
            let view = &json["bufferViews"][accessor["bufferView"].as_u64().unwrap() as usize];
            let start = view["byteOffset"].as_u64().unwrap_or(0) as usize
                + accessor["byteOffset"].as_u64().unwrap_or(0) as usize;
            let count = accessor["count"].as_u64().unwrap() as usize;
            buffer[start..start + count * stride].to_vec()
        };

        let wanted = primitive["attributes"][attribute].as_u64().unwrap() as usize;
        let values: Vec<Vec3> = read(&json["accessors"][wanted], 12)
            .chunks_exact(12)
            .map(|v| {
                Vec3::new(
                    f32::from_le_bytes(v[0..4].try_into().unwrap()),
                    f32::from_le_bytes(v[4..8].try_into().unwrap()),
                    f32::from_le_bytes(v[8..12].try_into().unwrap()),
                )
            })
            .collect();

        let indices = &json["accessors"][primitive["indices"].as_u64().unwrap() as usize];
        // 5123 is glTF's code for an unsigned short, which is what an exporter
        // reaches for on a mesh this small.
        assert_eq!(indices["componentType"], 5123, "indices are not u16");
        read(indices, 2)
            .chunks_exact(6)
            .map(|t| {
                let at = |b: &[u8]| values[u16::from_le_bytes(b.try_into().unwrap()) as usize];
                [at(&t[0..2]), at(&t[2..4]), at(&t[4..6])]
            })
            .collect()
    }

    #[test]
    fn the_model_holds_a_hull_and_a_spar_in_that_order() {
        // The one thing about the file the game cannot see for itself. It asks
        // for its meshes by number, and glTF numbers them in whatever order the
        // objects sat in the master — so an afternoon in Blender that leaves the
        // spar first would paint the hull in bare-spar cream and stand a
        // seven-metre plank of timber where the mast should be, with nothing
        // failing anywhere to say so.
        let (json, _) = model();
        assert_eq!(json["meshes"][HULL_MESH]["name"], "hull");
        assert_eq!(json["meshes"][SPAR_MESH]["name"], "spar");
    }

    #[test]
    fn the_model_is_the_hull_the_keel_is_probed_along() {
        // What `grounding` assumes about a shape it never looks at: the keel
        // runs at DRAFT below the waterline, from the forefoot aft to the heel,
        // and the hull is that long. Remodel the boat deeper and every probe
        // would be reading the water above its own keel — the hull would sail
        // through the shallows it should be stopped by, and nothing but this
        // would notice.
        let corners: Vec<Vec3> = triangles(HULL_MESH, "POSITION")
            .into_iter()
            .flatten()
            .collect();
        let lowest = corners.iter().map(|c| c.y).fold(f32::MAX, f32::min);
        let (bow, transom) = corners
            .iter()
            .fold((f32::MAX, f32::MIN), |(f, a), c| (f.min(c.z), a.max(c.z)));

        assert!(
            (lowest + DRAFT).abs() < 1e-4,
            "the model's keel is {lowest} below the waterline, not {}",
            -DRAFT
        );
        assert!(
            (bow + LENGTH * 0.5).abs() < 1e-4 && (transom - LENGTH * 0.5).abs() < 1e-4,
            "the model runs {bow}..{transom}, not a {LENGTH}m hull about amidships"
        );

        // The keel itself, not just the depth: the probes are spread between
        // these two stations, and each one has to be over hull rather than over
        // the water ahead of a forefoot that has crept aft.
        let keel: Vec<&Vec3> = corners
            .iter()
            .filter(|c| (c.y + DRAFT).abs() < 1e-4)
            .collect();
        let forefoot = keel.iter().map(|c| c.z).fold(f32::MAX, f32::min);
        let heel = keel.iter().map(|c| c.z).fold(f32::MIN, f32::max);
        assert!(
            (forefoot - FOREFOOT_STATION).abs() < 1e-4 && (heel - HEEL_STATION).abs() < 1e-4,
            "the keel runs {forefoot}..{heel}, not {FOREFOOT_STATION}..{HEEL_STATION}"
        );
    }

    #[test]
    fn the_model_is_flat_shaded() {
        // The look, as a condition on the file. Everything the game draws is a
        // flat tone per facet, and a mesh left smooth in Blender exports with
        // its normals averaged across the faces each vertex meets — which
        // arrives as a hull with gradients running over it, the one thing this
        // palette cannot absorb. It is a checkbox in a modelling program and
        // reads as a subtly wrong-looking boat rather than as a mistake, so it
        // is worth a test rather than an eye.
        for mesh in [HULL_MESH, SPAR_MESH] {
            let faces = triangles(mesh, "POSITION");
            let normals = triangles(mesh, "NORMAL");

            for (face, normal) in faces.iter().zip(normals) {
                let flat = (face[1] - face[0]).cross(face[2] - face[0]).normalize();
                for corner in normal {
                    assert!(
                        corner.dot(flat) > 0.999,
                        "a corner of mesh {mesh} faces {corner:?} on a facet lying {flat:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn every_face_looks_outwards() {
        // Winding is invisible until something is drawn — a face wound the
        // wrong way round is simply culled, so what is seen through the hole is
        // the inside of the far side of the shape, lit as though it faced away
        // from the sun. On a mast that is a mast whose lit side is the shaded
        // one and whose top has gone; a change small enough to look at without
        // noticing, and this file caught it once already.
        //
        // Checked against a point inside: a closed convex-ish shell has every
        // face pointing away from its own middle. Both meshes, because the one
        // that was wound inwards was the spar.
        for mesh in [HULL_MESH, SPAR_MESH] {
            let faces = triangles(mesh, "POSITION");
            let corners: Vec<Vec3> = faces.iter().flatten().copied().collect();
            let middle = corners.iter().sum::<Vec3>() / corners.len() as f32;

            for face in faces {
                let outward = (face[0] + face[1] + face[2]) / 3.0 - middle;
                let normal = (face[1] - face[0]).cross(face[2] - face[0]).normalize();
                assert!(
                    normal.dot(outward) > 0.0,
                    "a face of mesh {mesh} at {outward:?} from the middle points \
                     {normal:?}, which is inwards"
                );
            }
        }
    }

    #[test]
    fn the_boat_points_its_bow_the_way_it_faces() {
        // What steering will be written against: -Z is the bow, so a boat
        // turned to a heading moves along its own forward.
        let mut app = test_app();
        let heading = boat(&mut app).forward();
        let yaw = app.world().resource::<View>().yaw;

        let expected = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
        assert!(
            (*heading - expected).length() < 1e-5,
            "a boat at yaw {yaw} faces {heading:?}, not {expected:?}"
        );
    }

    #[test]
    fn the_forward_key_drives_the_boat_the_way_the_bow_points() {
        let mut app = test_app();
        let before = boat(&mut app);
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 20);
        let moved = boat(&mut app).translation - before.translation;

        // Forward means the boat's own forward — no helm held, so the whole
        // of the movement is dead ahead.
        assert!(moved.length() > 0.0, "the boat never moved");
        assert!(
            moved.normalize().dot(*before.forward()) > 0.999,
            "the boat went {moved:?} rather than along its heading"
        );
        // And along the surface, not through it — height is `float`'s alone.
        assert_eq!(moved.y, 0.0);
    }

    /// Metres per second the boat settles to while `key` is held, signed by
    /// whether it went ahead or astern — measured after the way is gathered,
    /// so it is the speed made good and not some point on the ramp.
    fn speed_made(key: KeyCode) -> f32 {
        let mut app = test_app();
        hold(&mut app, key);
        run_frames(&mut app, SETTLED);

        let before = boat(&mut app);
        let start = elapsed(&app);
        run_frames(&mut app, 60);
        let seconds = elapsed(&app) - start;
        assert!(seconds > 0.0, "no time passed while the key was held");

        let moved = boat(&mut app).translation - before.translation;
        moved.dot(*before.forward()) / seconds
    }

    #[test]
    fn ahead_and_astern_each_make_their_own_speed() {
        let ahead = speed_made(KeyCode::ArrowUp);
        assert!(
            (ahead - SPEED).abs() < SPEED * 0.01,
            "the boat made {ahead} m/s ahead, not {SPEED}"
        );

        // Backing off a beach is the whole use of astern, so it is slower and
        // it is backwards — along the heading reversed, not a turn.
        let astern = speed_made(KeyCode::ArrowDown);
        assert!(
            (astern + ASTERN_SPEED).abs() < ASTERN_SPEED * 0.01,
            "the boat made {astern} m/s astern, not -{ASTERN_SPEED}"
        );
    }

    #[test]
    fn the_boat_gathers_way_rather_than_leaping_to_speed() {
        // The first half second of a standing start: under way at once, but
        // nowhere near full speed — the ramp is the point of the ease.
        let mut app = test_app();
        let before = boat(&mut app).translation;
        let start = elapsed(&app);
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 30);
        let seconds = elapsed(&app) - start;

        let made = (boat(&mut app).translation - before).length() / seconds;
        assert!(made > 0.0, "the boat never began to move");
        assert!(
            made < SPEED * 0.5,
            "{made} m/s inside the first half second is a leap, not gathered way"
        );
    }

    #[test]
    fn the_boat_carries_its_way_into_a_glide_and_then_stops() {
        let mut app = test_app();
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, SETTLED);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release_all();

        // Off the key at full speed: a glide of a few metres, not a dead stop.
        let going = boat(&mut app).translation;
        run_frames(&mut app, 30);
        let glide = (boat(&mut app).translation - going).length();
        assert!(
            glide > 1.0,
            "the boat stopped dead the moment the key came up"
        );

        // And the glide ends: the way snaps to stopped, and a boat at rest
        // holds the same spot exactly, frame after frame.
        run_frames(&mut app, SETTLED);
        let at_rest = boat(&mut app).translation;
        run_frames(&mut app, 10);
        assert_eq!(
            boat(&mut app).translation,
            at_rest,
            "the boat is still creeping"
        );
    }

    #[test]
    fn the_helm_brings_the_bow_round_at_the_turn_rate() {
        // Port is a positive turn about the vertical, the same way round as
        // the camera's own Q; starboard its mirror.
        let port = turn_rate(KeyCode::ArrowLeft);
        let starboard = turn_rate(KeyCode::ArrowRight);

        let tolerance = TURN_RATE * 0.01;
        assert!(
            (port - TURN_RATE).abs() < tolerance,
            "the bow came round at {port} rad/s to port, not {TURN_RATE}"
        );
        assert!(
            (starboard + TURN_RATE).abs() < tolerance,
            "the bow came round at {starboard} rad/s to starboard, not -{TURN_RATE}"
        );
    }

    #[test]
    fn the_helm_answers_with_no_way_on() {
        // Turning without moving: the bow swings, the hull stays put.
        let mut app = test_app();
        let before = boat(&mut app);
        hold(&mut app, KeyCode::ArrowLeft);
        run_frames(&mut app, 20);
        let after = boat(&mut app);

        assert_eq!(after.translation, before.translation);
        assert_ne!(after.rotation, before.rotation, "the bow never swung");
    }

    /// The hull's heel, in radians, positive with the masthead to port — what
    /// is left of the pose once the heading's yaw is taken back off it. Read
    /// from the transform rather than the component, because the lean the
    /// player sees is the one the transform holds.
    fn heel_shown(app: &mut App) -> f32 {
        let transform = boat(app);
        let forward = transform.forward();
        let yaw = f32::atan2(-forward.x, -forward.z);
        let roll = Quat::from_rotation_y(yaw).inverse() * transform.rotation;
        let (axis, angle) = roll.to_axis_angle();
        angle * axis.z
    }

    #[test]
    fn a_turn_at_speed_heels_the_hull_outwards() {
        let mut app = test_app();
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, SETTLED);

        // Port helm at full way: the whole of the heel, and to starboard —
        // a keeled hull leans out of a corner, not into it like a bicycle.
        hold(&mut app, KeyCode::ArrowLeft);
        run_frames(&mut app, SETTLED);
        let heel = heel_shown(&mut app);
        assert!(
            (heel + HEEL_AT_FULL_TURN).abs() < HEEL_AT_FULL_TURN * 0.05,
            "a full-speed port turn heels {heel} rad, not -{HEEL_AT_FULL_TURN}"
        );

        // The lean is the eye's alone: the bow still points along the
        // surface, so nothing the movement is built on has tilted with it.
        assert!(boat(&mut app).forward().y.abs() < 1e-6);

        // Starboard is the mirror.
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::ArrowLeft);
        hold(&mut app, KeyCode::ArrowRight);
        run_frames(&mut app, SETTLED);
        let heel = heel_shown(&mut app);
        assert!(
            (heel - HEEL_AT_FULL_TURN).abs() < HEEL_AT_FULL_TURN * 0.05,
            "a full-speed starboard turn heels {heel} rad, not {HEEL_AT_FULL_TURN}"
        );
    }

    #[test]
    fn the_heel_rolls_on_rather_than_snapping() {
        // A few frames into a full-speed turn: leaning already, but nowhere
        // near the whole heel — the roll is eased the way the way is.
        let mut app = test_app();
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, SETTLED);
        hold(&mut app, KeyCode::ArrowLeft);
        run_frames(&mut app, 6);

        let heel = heel_shown(&mut app).abs();
        assert!(heel > 0.0, "the hull never began to lean");
        assert!(
            heel < HEEL_AT_FULL_TURN * 0.5,
            "{heel} rad inside the first tenth of a second is a snap, not a roll"
        );
    }

    #[test]
    fn the_helm_alone_heels_nothing() {
        // The lean is sharpness times speed, and a bow swung round at rest
        // has no speed: the boat pivots bolt upright.
        let mut app = test_app();
        hold(&mut app, KeyCode::ArrowLeft);
        run_frames(&mut app, SETTLED);
        assert_eq!(heel_shown(&mut app), 0.0);
    }

    #[test]
    fn the_hull_comes_level_when_the_turn_ends() {
        let mut app = test_app();
        hold(&mut app, KeyCode::ArrowUp);
        hold(&mut app, KeyCode::ArrowLeft);
        run_frames(&mut app, SETTLED);
        assert_ne!(heel_shown(&mut app), 0.0, "the turn never heeled the hull");

        // Helm amidships, way still on: the heel runs back down its own
        // curve and *ends* — exactly level, not forever creeping towards it.
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::ArrowLeft);
        run_frames(&mut app, SETTLED);
        let level = heel_shown(&mut app);
        assert!(
            level.abs() < 1e-6,
            "the hull is still heeled {level} rad with the helm amidships"
        );
    }

    #[test]
    fn opposed_keys_hold_the_boat_still() {
        // All four at once: ahead cancels astern outright — not by the faster
        // gear's margin — and port cancels starboard.
        let mut app = test_app();
        let before = boat(&mut app);
        for key in [
            KeyCode::ArrowUp,
            KeyCode::ArrowDown,
            KeyCode::ArrowLeft,
            KeyCode::ArrowRight,
        ] {
            hold(&mut app, key);
        }
        run_frames(&mut app, 20);
        let after = boat(&mut app);
        assert_eq!(after.translation, before.translation);
        assert_eq!(after.rotation, before.rotation);
    }

    #[test]
    fn a_rebound_key_steers_and_the_key_it_replaced_stops() {
        let mut app = test_app();
        rebind(&mut app, Action::MoveForward, KeyCode::KeyJ);

        let before = boat(&mut app).translation;
        hold(&mut app, KeyCode::KeyJ);
        run_frames(&mut app, 20);
        assert_ne!(
            boat(&mut app).translation,
            before,
            "the newly bound key did not steer"
        );

        // J was nobody's key, so nothing was traded for it and W is now bound
        // to nothing at all. Holding it has to leave the boat where it lies.
        let mut app = test_app();
        rebind(&mut app, Action::MoveForward, KeyCode::KeyJ);
        let before = boat(&mut app).translation;
        hold(&mut app, KeyCode::KeyW);
        run_frames(&mut app, 20);
        assert_eq!(
            boat(&mut app).translation,
            before,
            "W still steers after being rebound away"
        );
    }

    #[test]
    fn the_arrow_keys_steer_whatever_the_bindings_say() {
        let mut app = test_app();
        // Hand every movement action to keys nowhere near the arrows.
        rebind(&mut app, Action::MoveForward, KeyCode::KeyI);
        rebind(&mut app, Action::MoveBack, KeyCode::KeyK);
        rebind(&mut app, Action::SteerLeft, KeyCode::KeyJ);
        rebind(&mut app, Action::SteerRight, KeyCode::KeyL);

        let before = boat(&mut app).translation;
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 20);
        assert_ne!(
            boat(&mut app).translation,
            before,
            "the arrow keys stopped steering once the letters moved"
        );
    }

    #[test]
    fn a_match_launches_the_boat_where_the_world_is_entered() {
        let mut app = test_app();
        app.insert_resource(View {
            focus: Vec3::new(98.0, 0.0, -317.0),
            ..default()
        });
        // Back to the menu and in again: entering is what launches a boat, and
        // the one from the last world went with it.
        for state in [AppState::MainMenu, AppState::InWorld] {
            app.world_mut()
                .resource_mut::<NextState<AppState>>()
                .set(state);
            app.update();
        }

        let at = boat(&mut app).translation;
        assert_eq!(Vec2::new(at.x, at.z), Vec2::new(98.0, -317.0));
    }

    #[test]
    fn the_boat_rides_the_surface_it_is_over() {
        let mut app = island_app();

        /// Where the boat comes to rest when it is put down at a spot.
        fn put_down(app: &mut App, spot: Vec2) -> f32 {
            let mut transform = app
                .world_mut()
                .query_filtered::<&mut Transform, With<Boat>>()
                .single_mut(app.world_mut())
                .expect("a match should have a boat in it");
            transform.translation.x = spot.x;
            transform.translation.z = spot.y;
            app.update();
            boat(app).translation.y
        }

        /// What the ground the boat is riding says about a spot.
        fn ground(app: &App, spot: Vec2) -> f32 {
            app.world()
                .resource::<Ground>()
                .height(spot.x, spot.y)
                .expect("the test ground has arrived")
        }

        // Dry land: the highest ground on the island, where the answer is
        // furthest from the waterline.
        let mut peak = (Vec2::ZERO, f32::MIN);
        for iz in 0..24 {
            for ix in 0..24 {
                let spot = Vec2::new(ix as f32 / 23.0 * 2.0 - 1.0, iz as f32 / 23.0 * 2.0 - 1.0)
                    * TEST_ISLAND_REACH;
                let height = ground(&app, spot);
                if height > peak.1 {
                    peak = (spot, height);
                }
            }
        }
        assert!(peak.1 > 0.0, "the island is entirely under water");
        assert_eq!(
            put_down(&mut app, peak.0),
            peak.1,
            "the boat is not sitting on the ground it is over"
        );

        // And open water past the coast: the waterline exactly, however deep
        // the seabed under it.
        let offshore = Vec2::new(TEST_ISLAND_REACH * 1.5, 0.0);
        assert!(
            ground(&app, offshore) < 0.0,
            "the point picked to be open water is dry land"
        );
        assert_eq!(
            put_down(&mut app, offshore),
            0.0,
            "the boat is not floating at the waterline"
        );
    }

    /// The same app with a hand of ground already delivered — the island the
    /// collision tests run aground on, and the surface the float test rides.
    /// The tests that take a bare [`test_app`] instead are the other case worth
    /// having: a client that has been sent nothing must still be able to move.
    fn island_app() -> App {
        let mut app = test_app();
        app.insert_resource(test_ground());
        app
    }

    /// Puts the boat down at a spot, pointing a way — a `--focus` in little.
    fn place(app: &mut App, at: Vec2, facing: Vec2) {
        let mut transform = app
            .world_mut()
            .query_filtered::<&mut Transform, With<Boat>>()
            .single_mut(app.world_mut())
            .expect("a match should have a boat in it");
        transform.translation = Vec3::new(at.x, 0.0, at.y);
        transform.rotation = Quat::from_rotation_y(f32::atan2(-facing.x, -facing.y));
    }

    /// How far the boat is from the middle of the test island, which is round:
    /// this is its distance off the coast plus [`TEST_ISLAND_REACH`].
    fn from_the_island(app: &mut App) -> f32 {
        let at = boat(app).translation;
        Vec2::new(at.x, at.z).length()
    }

    /// What [`grounding`] makes of the pose the boat is lying in: how far the
    /// bottom there stands above the depth the hull is held at, so positive is
    /// aground and more positive is further in.
    fn bite(app: &mut App) -> f32 {
        let transform = boat(app);
        grounding(Some(app.world().resource::<Ground>()), &transform)
    }

    #[test]
    fn the_keel_is_probed_as_closely_as_the_ground_is_sampled() {
        // What a handful of points along the keel does buy: no facet of the
        // height field fits between two probes, so ground rising across a facet
        // is read on the way up. Not the same as seeing everything the field can
        // draw — a crest narrower than a facet is read off its flanks and
        // missed, which no spacing at this scale fixes; [`KEEL_PROBES`] carries
        // the argument for wearing that rather than probing the keel to death.
        let spacing = (HEEL_STATION - FOREFOOT_STATION) / (KEEL_PROBES - 1) as f32;
        assert!(
            spacing <= FACET_METRES,
            "{spacing} m between probes leaves room for a {FACET_METRES} m facet to hide in"
        );
    }

    #[test]
    fn a_boat_driven_at_a_coast_grounds_short_of_it() {
        let mut app = island_app();
        place(
            &mut app,
            Vec2::new(TEST_ISLAND_REACH + 60.0, 0.0),
            Vec2::new(-1.0, 0.0),
        );

        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, SETTLED);

        // Held off the island rather than stopped out in the open: the test
        // island's rim is near vertical, so its coast is where its reach says.
        let reached = from_the_island(&mut app);
        assert!(
            reached > TEST_ISLAND_REACH,
            "the boat is {reached} m out, which is inside a coast at {TEST_ISLAND_REACH} m"
        );
        assert!(
            reached < TEST_ISLAND_REACH + 10.0,
            "the boat stopped {reached} m out, nowhere near the coast it was driven at"
        );

        // And stopped is stopped, not grinding: the way came off, so the hull
        // holds exactly the same spot with the key still down.
        let aground = boat(&mut app).translation;
        run_frames(&mut app, 10);
        assert_eq!(
            boat(&mut app).translation,
            aground,
            "the boat is still creeping ashore"
        );
    }

    #[test]
    fn astern_backs_a_grounded_boat_off() {
        let mut app = island_app();
        place(
            &mut app,
            Vec2::new(TEST_ISLAND_REACH + 60.0, 0.0),
            Vec2::new(-1.0, 0.0),
        );
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, SETTLED);
        let aground = from_the_island(&mut app);
        // Phase one has to have been *stopped* for phase two to say anything:
        // a run that sailed clean over the island would back off it exactly as
        // far, and the test would pass with no collision in the build at all.
        // So the same check the coast test makes, made again here.
        assert!(
            aground > TEST_ISLAND_REACH && aground < TEST_ISLAND_REACH + 10.0,
            "the boat is {aground} m out, which is not held at a coast at {TEST_ISLAND_REACH} m"
        );

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release_all();
        hold(&mut app, KeyCode::ArrowDown);
        run_frames(&mut app, SETTLED);

        // Backing off a beach is what astern is for, and it needs no rule of
        // its own: away from the ground is the deeper water the hull came in
        // over, and deeper is always allowed.
        let backed = from_the_island(&mut app);
        assert!(
            backed > aground + LENGTH,
            "the boat came off {} m, less than its own length",
            backed - aground
        );
    }

    #[test]
    fn open_water_is_sailed_at_full_speed() {
        // Nothing under the keel, nothing in the way: the ground the boat has
        // been sent must cost it no speed at all where there is water enough.
        let mut app = island_app();
        place(
            &mut app,
            Vec2::new(TEST_ISLAND_REACH + 100.0, 0.0),
            Vec2::new(1.0, 0.0),
        );
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, SETTLED);

        let before = boat(&mut app).translation;
        let start = elapsed(&app);
        run_frames(&mut app, 60);
        let seconds = elapsed(&app) - start;

        let made = (boat(&mut app).translation - before).length() / seconds;
        assert!(
            (made - SPEED).abs() < SPEED * 0.01,
            "the boat made {made} m/s over open water, not {SPEED}"
        );
    }

    #[test]
    fn a_boat_put_down_inland_drives_back_to_the_sea() {
        // What `--focus` on an island leaves behind, and the case that says
        // the ground holds a boat without ever trapping one.
        let mut app = island_app();
        let ashore = Vec2::new(TEST_ISLAND_REACH * 0.5, 0.0);
        place(&mut app, ashore, Vec2::new(1.0, 0.0));
        assert!(
            bite(&mut app) > 0.0,
            "the spot picked to be dry land has water over it"
        );

        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, SETTLED);

        assert_eq!(
            boat(&mut app).translation.y,
            0.0,
            "the boat never made it back to the waterline"
        );
        let afloat = from_the_island(&mut app);
        assert!(
            afloat > TEST_ISLAND_REACH,
            "the boat is still {afloat} m from the middle, inside the coast"
        );
    }

    #[test]
    fn a_grounded_boat_is_never_driven_further_aground() {
        // The pose a rule of "aground already, let it through" would hand the
        // whole island to: bow over the beach, and then whatever the player
        // likes. Driven straight at it, and with no helm — the helm was held
        // here once, on the thought that a hull swept round every heading
        // tries the rule from more angles than one heading does. It tries
        // nothing at all. The turning circle at full way is five metres, so
        // the bow comes round and the boat sails off the shore under any rule
        // whatever, including both of the ones this test exists to catch; what
        // it was watching was a boat that had floated away. A heading held is
        // also what makes the two bites comparable, `grounding` being a
        // reading of the ground under a particular pose and not a property of
        // the spot. Coming round while aground has a test of its own.
        let mut app = island_app();
        place(
            &mut app,
            Vec2::new(TEST_ISLAND_REACH - 1.0, 0.0),
            Vec2::new(-1.0, 0.0),
        );
        let before = bite(&mut app);
        let out = from_the_island(&mut app);
        assert!(before > 0.0, "the boat was meant to start aground");

        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, SETTLED);

        let after = bite(&mut app);
        assert!(
            after <= before,
            "the boat worked its way {} m further into the ground",
            after - before
        );
        // And the same thing said in the terms a player sees it in: bow at the
        // island, key down for eight hundred frames, and not a metre of the
        // island gained.
        let ended = from_the_island(&mut app);
        assert!(
            ended >= out,
            "the boat made {} m towards the middle of the island",
            out - ended
        );
    }

    #[test]
    fn the_helm_answers_while_aground() {
        // A refused turn on top of a refused advance is a hull wedged against
        // a shore for good, so the bow comes round whatever is under it.
        let mut app = island_app();
        place(
            &mut app,
            Vec2::new(TEST_ISLAND_REACH - 1.0, 0.0),
            Vec2::new(-1.0, 0.0),
        );
        let before = boat(&mut app).rotation;

        hold(&mut app, KeyCode::ArrowUp);
        hold(&mut app, KeyCode::ArrowLeft);
        run_frames(&mut app, 20);

        assert_ne!(
            boat(&mut app).rotation,
            before,
            "a boat the ground has stopped cannot come round"
        );
    }
}
