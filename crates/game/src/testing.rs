//! What the app-level tests are built out of.
//!
//! A headless `App` is driven by hand — nothing pumps its frame loop — so a
//! test that is waiting on something has to run the frames itself, and a test
//! that presses a key has to clear it the way the real input plugin would.
//! Every module testing a system needs some of this, and each of them had a
//! copy of the piece it needed.
//!
//! The model readers below are here for the same reason. Three modules draw
//! glTF files now and all of them hold theirs to the same handful of
//! conditions, so the reader that checks them is written once — a second copy
//! would be a second opinion about what the format says.

use std::thread;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use bevy::time::{TimePlugin, TimeUpdateStrategy};

use protocol::ground::{
    quantize, ChunkPayload, Surface, Tone, CHUNK_METRES, FACET_METRES, FACET_TRIS, FACET_VERTS,
    OCEAN_DEPTH,
};

use crate::bindings::{Action, KeyBindings};
use crate::boat::BoatPlugin;
use crate::camera::View;
use crate::player::PlayerPlugin;
use crate::terrain::Ground;
use crate::wake::WakePlugin;
use crate::{AppState, Helm};

/// How long every test frame lasts in a [`world_app`]. Headless frames take
/// next to no real time, which nothing eased can live with — the eases are
/// curves *in seconds* — so the clock is stepped by a fixed sixty-a-second
/// frame and the tests get the ramp a player would.
pub const FRAME: Duration = Duration::from_millis(16);

/// Points `GENOVESA_DATA` at a directory of this test process's own, once,
/// so nothing a test keeps — a world file, a logbook — lands among the
/// player's real ones. Called by every test helper whose app could reach
/// the data directory; idempotent, and the directory is shared by the whole
/// process, exactly as the real one would be.
pub fn quarantine_data_dir() {
    use std::sync::OnceLock;
    static QUARANTINE: OnceLock<std::path::PathBuf> = OnceLock::new();
    let dir = QUARANTINE.get_or_init(|| {
        std::env::temp_dir().join(format!("genovesa-test-data-{}", std::process::id()))
    });
    std::env::set_var("GENOVESA_DATA", dir);
}

/// A headless app already in a match, with the boat and player systems
/// running — the world as the movement tests know it, shared here because
/// the boat's tests and the player's want exactly the same one.
///
/// `AssetPlugin` because the boat is spawned out of a file, and
/// `TaskPoolPlugin` because that is where it finds the thread to read it on.
/// Nothing here waits for the load — these tests are about where things are
/// and what they do, not what they look like — but `launch` asks the asset
/// server for its meshes, and without one there is no boat.
///
/// `AnimationPlugin` for the same reason one step further on: the player's
/// figure is a rigged model, so the clips it is walked by are assets and the
/// graph they hang in is another. The clips never load here — there is no
/// render app to read a glTF with — and the figure's own tests stand a
/// stand-in clip up in their place.
pub fn world_app() -> App {
    let mut app = world_app_ashore_of_entry();
    enter_world(&mut app);
    app
}

/// The same app, stopped one step short of entering the world — for the
/// tests that have to put something in place first, the way a real run
/// inserts the logbook before the match opens. Finish with [`enter_world`].
pub fn world_app_ashore_of_entry() -> App {
    let mut app = App::new();
    app.add_plugins((
        TaskPoolPlugin::default(),
        AssetPlugin::default(),
        TimePlugin,
        StatesPlugin,
        bevy::animation::AnimationPlugin,
        BoatPlugin,
        // Nothing here draws a sea for it to be painted on, so all the wake
        // does in these tests is keep its track — which is what a test of a
        // boat's wake wants to look at, and what every other test here wants
        // running over the hulls it sails without ever noticing it.
        WakePlugin,
        PlayerPlugin,
    ))
    .insert_resource(TimeUpdateStrategy::ManualDuration(FRAME))
    .init_state::<AppState>()
    .add_sub_state::<Helm>()
    .init_resource::<View>()
    .init_resource::<KeyBindings>()
    .init_resource::<ButtonInput<KeyCode>>()
    .init_asset::<Mesh>()
    // What a spawned model arrives as, which `DefaultPlugins` would have
    // registered: the figure asks for one, and an asset server handed a type
    // it has never heard of panics rather than declining.
    .init_asset::<bevy::world_serialization::WorldAsset>()
    .init_resource::<Assets<StandardMaterial>>();
    app.update();
    app
}

/// Crosses into the world — see [`world_app_ashore_of_entry`].
pub fn enter_world(app: &mut App) {
    app.world_mut()
        .resource_mut::<NextState<AppState>>()
        .set(AppState::InWorld);
    app.update();
}

/// How long a test waits before calling something a failure rather than a
/// slow machine. Only ever paid in full by a test that was going to fail
/// anyway, so it can afford to be generous.
const PATIENCE: Duration = Duration::from_secs(5);

/// Runs frames until the condition holds.
///
/// The waiting is legitimate and the deadline is what keeps it honest: what
/// these tests are waiting on crosses a real socket and a thread, so a frame
/// or two is ordinary and five seconds is a hang. `what` is the condition in
/// words, so a timeout says which one never came true rather than only that
/// one didn't.
pub fn run_until(app: &mut App, what: &str, mut done: impl FnMut(&mut App) -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        app.update();
        if done(app) {
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
    panic!("timed out waiting until {what}");
}

/// Holds a key down. It stays down until something releases it, which is what
/// a test of a held control wants.
pub fn hold(app: &mut App, key: KeyCode) {
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(key);
}

/// Runs frames with whatever keys are down. Clears the just-pressed flags
/// between them the way the real input plugin does, so a key held here reads
/// as held rather than as pressed afresh every frame.
pub fn run_frames(app: &mut App, count: usize) {
    for _ in 0..count {
        app.update();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .clear();
    }
}

/// Seconds of clock the app has run for. Frames take however long they take
/// in a headless run, so anything driven by `delta_secs` has to be measured
/// against the time that actually passed rather than a frame count.
pub fn elapsed(app: &App) -> f32 {
    app.world().resource::<Time>().elapsed_secs()
}

/// Puts an action on a key, as the controls screen does.
pub fn rebind(app: &mut App, action: Action, key: KeyCode) {
    app.world_mut()
        .resource_mut::<KeyBindings>()
        .bind(action, key, None);
}

/// Sets the wind the frozen test sea blows. A [`world_app`] never settles the
/// conditions — no forecast, no easing — so this holds until the test says
/// otherwise, and without it every test runs under the assumed day's wind,
/// under which the default boat lies *in irons*: a driving test that forgets
/// to set a wind is testing a boat that never moves, so assert way or
/// movement, never only where the hull ended up. The other direction bites
/// too: tests that compare heights against `SeaConditions::default().swell`
/// must not call this — a different wind is a different sea.
pub fn set_wind(app: &mut App, wind: Vec2) {
    app.insert_resource(crate::sea::SeaConditions::blowing(wind));
}

/// How far the test island reaches from the origin, in metres — the radius at
/// which its ground has fallen all the way to the ocean floor.
pub const TEST_ISLAND_REACH: f32 = 200.0;

/// How high it stands at the origin.
const TEST_ISLAND_PEAK: f32 = 120.0;

/// How its profile falls away. Well under one, so the island is a broad top
/// ending in a near-vertical rim: the shallow cone it would otherwise be has
/// nothing steep enough on it to make the camera's own clearance clamp fire,
/// and that clamp is one of the things these tests are for.
const TEST_ISLAND_PITCH: f32 = 0.35;

/// A patch of world already delivered, exactly as a server would have sent it:
/// a steep island at the origin reaching [`TEST_ISLAND_REACH`], with open
/// water round it.
///
/// Anything riding the ground asks [`Ground::surface`], which answers `None`
/// until the chunk under the point has arrived — so a test of the boat
/// floating, the camera grounding itself or a marker standing up needs ground
/// that has actually turned up, not merely been asked for.
///
/// Made here rather than fetched from a real world because a client cannot
/// generate one: it is handed chunks, and this is a hand of chunks. What the
/// tests need of it is height to stand on, a waterline to float at, and a
/// slope steep enough to be a problem. What they need of the *other* island —
/// [`test_shore`] — is a coast gentle enough to stand on, which this one, being
/// a wall at the waterline, has nowhere.
pub fn test_ground() -> Ground {
    hand_of_chunks(test_island_height)
}

/// The test island's height field: a broad top falling to the ocean floor at
/// [`TEST_ISLAND_REACH`], and flat floor beyond.
fn test_island_height(at: Vec2) -> f32 {
    let out = at.length() / TEST_ISLAND_REACH;
    if out >= 1.0 {
        return -OCEAN_DEPTH;
    }
    (TEST_ISLAND_PEAK + OCEAN_DEPTH) * (1.0 - out).powf(TEST_ISLAND_PITCH) - OCEAN_DEPTH
}

/// How steeply the shore island's apron rises, in metres of height per metre
/// inland: gentle enough that a walker crosses it, and steep enough that the
/// water a hull needs under it and the water a walker will wade are a stride
/// apart rather than several — which is what lets a boat lie within boarding
/// reach of where its crew stepped ashore.
const SHORE_PITCH: f32 = 0.5;

/// How high the apron climbs, in metres, before the bluff behind it takes over.
/// Public because the tests of what a walker may climb are written against the
/// two levels of the island: below this is the ground they cross, above it the
/// ground that turns them back.
pub const SHORE_BLUFF_FOOT: f32 = 8.0;

/// How far from the middle the shore island's apron crosses the waterline, in
/// metres — well inside [`TEST_ISLAND_REACH`], the apron starting from the
/// ocean floor out there. Public because a boat is put down off the coast by
/// its distance from *this*, the reach being a long way out to sea on this
/// island.
pub const SHORE_WATERLINE: f32 = TEST_ISLAND_REACH - OCEAN_DEPTH / SHORE_PITCH;

/// How steeply the bluff climbs — several times anything a walker will take on,
/// so a test of the limit is not a test of where exactly the limit is set.
const SHORE_BLUFF_PITCH: f32 = 2.0;

/// Where the bluff gives out into a level top, so that the island is a shape
/// rather than a spike. Public for the same reason as [`SHORE_BLUFF_FOOT`].
pub const SHORE_PEAK: f32 = 60.0;

/// How far from the middle that level top begins, in metres: inside this the
/// ground stands at [`SHORE_PEAK`] and is flat, and at it the bluff drops away.
/// Public so a test of what a walker will step *down* can put one at the brink.
pub const SHORE_TOP: f32 = TEST_ISLAND_REACH
    - (SHORE_BLUFF_FOOT + OCEAN_DEPTH) / SHORE_PITCH
    - (SHORE_PEAK - SHORE_BLUFF_FOOT) / SHORE_BLUFF_PITCH;

/// The other patch of delivered world: an island of the same reach as
/// [`test_ground`]'s, shaped like a coast rather than like a cliff — a gently
/// shelving apron a boat can nose up to and a walker can land on and cross,
/// and a bluff behind it far steeper than a walker will climb.
///
/// This is what the walking tests want, and neither half of it is decoration.
/// The apron is what lets a landing happen at all and what a walker wades off;
/// the bluff is the wall they are turned back by, and the two meet at
/// [`SHORE_BLUFF_FOOT`] so a test can say which side of it somebody ended up
/// on.
pub fn test_shore() -> Ground {
    hand_of_chunks(shore_island_height)
}

/// The shore island's height field: ocean floor out beyond
/// [`TEST_ISLAND_REACH`], then an apron at [`SHORE_PITCH`] up through the
/// waterline, a bluff at [`SHORE_BLUFF_PITCH`] from [`SHORE_BLUFF_FOOT`], and a
/// level top at [`SHORE_PEAK`].
fn shore_island_height(at: Vec2) -> f32 {
    let inland = TEST_ISLAND_REACH - at.length();
    if inland <= 0.0 {
        return -OCEAN_DEPTH;
    }
    let foot = (SHORE_BLUFF_FOOT + OCEAN_DEPTH) / SHORE_PITCH;
    if inland <= foot {
        inland * SHORE_PITCH - OCEAN_DEPTH
    } else {
        (SHORE_BLUFF_FOOT + (inland - foot) * SHORE_BLUFF_PITCH).min(SHORE_PEAK)
    }
}

/// A height field turned into the chunks a server would have sent of it: the
/// island and a ring of open water round it, delivered as answers.
fn hand_of_chunks(height: impl Fn(Vec2) -> f32) -> Ground {
    let mut ground = Ground::default();

    // Enough chunks to hold the island and a ring of open water around it, so
    // that a test walking off the coast finds sea rather than the edge of what
    // has arrived.
    let reach = (TEST_ISLAND_REACH / CHUNK_METRES).ceil() as i32 + 2;
    for cz in -reach..=reach {
        for cx in -reach..=reach {
            let chunk = IVec2::new(cx, cz);
            let base = chunk.as_vec2() * CHUNK_METRES;
            let heights: Vec<u16> = (0..FACET_VERTS * FACET_VERTS)
                .map(|i| {
                    let corner = base
                        + Vec2::new((i % FACET_VERTS) as f32, (i / FACET_VERTS) as f32)
                            * FACET_METRES;
                    quantize(height(corner))
                })
                .collect();

            // Flat floor is what open water *is* — see `Archipelago::
            // chunk_payload`, which answers exactly this way.
            let payload = heights
                .iter()
                .any(|h| *h != quantize(-OCEAN_DEPTH))
                .then(|| ChunkPayload {
                    heights,
                    surfaces: vec![Surface::plain(Tone::Grass); FACET_TRIS],
                    // Both test islands are smooth shapes with nothing to
                    // enclose a basin, so there is no lake on either to draw.
                    water: None,
                    // Nor anything the palm rule would call a beach.
                    plants: Vec::new(),
                });
            ground.deliver(chunk, payload);
        }
    }
    ground
}

// --- Models -------------------------------------------------------------------

/// A model under `assets/`, as its glTF JSON and its binary chunk.
///
/// Read straight out of the `.glb` rather than through Bevy's loader: what the
/// tests want to know is what the file *says*, and going through the asset
/// server would mean standing up a render app and waiting on a load to learn
/// it.
pub fn model(name: &str) -> (serde_json::Value, Vec<u8>) {
    let path = format!("{}/../../assets/{name}", env!("CARGO_MANIFEST_DIR"));
    let file =
        std::fs::read(&path).unwrap_or_else(|_| panic!("{path} — run assets-src/models/export.sh"));
    assert_eq!(&file[..4], b"glTF", "{name} is not a glTF binary");

    let (mut at, mut chunks) = (12, Vec::new());
    while at < file.len() {
        let length = u32::from_le_bytes(file[at..at + 4].try_into().unwrap()) as usize;
        chunks.push(file[at + 8..at + 8 + length].to_vec());
        at += 8 + length;
    }
    let json = serde_json::from_slice(&chunks[0]).expect("the glTF's JSON chunk");
    (json, chunks[1].clone())
}

/// One mesh of a model, as the triangles it is made of — `attribute` being
/// `POSITION` for where its corners are, `NORMAL` for where they face, or
/// `COLOR_0` for what colour they are painted, whose fourth component is an
/// opacity nothing here has any use for and is dropped.
///
/// Read through the accessors' own view of the buffer, so an exporter that
/// changes how it packs the numbers changes nothing here.
pub fn triangles(name: &str, index: usize, attribute: &str) -> Vec<[Vec3; 3]> {
    let (json, buffer) = model(name);
    let primitive = &json["meshes"][index]["primitives"][0];

    let read = |accessor: &serde_json::Value, stride: usize| -> Vec<u8> {
        let view = &json["bufferViews"][accessor["bufferView"].as_u64().unwrap() as usize];
        let start = view["byteOffset"].as_u64().unwrap_or(0) as usize
            + accessor["byteOffset"].as_u64().unwrap_or(0) as usize;
        let count = accessor["count"].as_u64().unwrap() as usize;
        buffer[start..start + count * stride].to_vec()
    };

    let wanted = primitive["attributes"][attribute].as_u64().unwrap() as usize;
    let stride = match json["accessors"][wanted]["type"].as_str() {
        Some("VEC3") => 12,
        Some("VEC4") => 16,
        other => panic!("{attribute} of {name} is a {other:?}, which is not a vector"),
    };
    let values: Vec<Vec3> = read(&json["accessors"][wanted], stride)
        .chunks_exact(stride)
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
    // reaches for on meshes this small.
    assert_eq!(indices["componentType"], 5123, "indices are not u16");
    read(indices, 2)
        .chunks_exact(6)
        .map(|t| {
            let at = |b: &[u8]| values[u16::from_le_bytes(b.try_into().unwrap()) as usize];
            [at(&t[0..2]), at(&t[2..4]), at(&t[4..6])]
        })
        .collect()
}

/// How each vertex of a mesh is shared out between the bones that carry it —
/// four weights apiece, which is what glTF gives every vertex of a skin
/// whether it uses them or not.
///
/// Only a rigged model has these. What reads them cares about one thing: that
/// no vertex is shared at all, every one of them riding a single bone at full
/// weight. That is what keeps a facet a facet while the model moves, and it
/// is a weight-painting decision in Blender that nothing else would catch.
pub fn skin_weights(name: &str, index: usize) -> Vec<[f32; 4]> {
    let (json, buffer) = model(name);
    let primitive = &json["meshes"][index]["primitives"][0];
    let accessor = &json["accessors"][primitive["attributes"]["WEIGHTS_0"]
        .as_u64()
        .expect("the mesh is skinned") as usize];
    assert_eq!(
        accessor["componentType"], 5126,
        "the weights are not plain floats"
    );

    let view = &json["bufferViews"][accessor["bufferView"].as_u64().unwrap() as usize];
    let start = view["byteOffset"].as_u64().unwrap_or(0) as usize
        + accessor["byteOffset"].as_u64().unwrap_or(0) as usize;
    let count = accessor["count"].as_u64().unwrap() as usize;
    buffer[start..start + count * 16]
        .chunks_exact(16)
        .map(|v| {
            let at = |i: usize| f32::from_le_bytes(v[i * 4..i * 4 + 4].try_into().unwrap());
            [at(0), at(1), at(2), at(3)]
        })
        .collect()
}

/// Holds a model to everything the game needs of one it did not make.
///
/// `meshes` pairs each position in the file with the name the object has in
/// the master, and all three conditions are checked against every one of
/// them:
///
/// - **The order.** The game asks for its meshes by number, and glTF numbers
///   them in whatever order the exporter wrote them — so an afternoon in
///   Blender can silently swap a hull for a spar, painting one in the
///   other's colour with nothing failing to say so.
/// - **The winding.** A face wound the wrong way round is simply culled, so
///   what is seen through the hole is the inside of the far side of the
///   shape, lit as though it faced away from the sun. Invisible until
///   something is drawn, and this has caught it once already — a spar.
/// - **The shading.** Everything the game draws is a flat tone per facet,
///   and a mesh left smooth in Blender exports with its normals averaged
///   across the faces each vertex meets, which arrives as gradients running
///   over the shape. It is a checkbox in a modelling program and reads as a
///   subtly wrong-looking model rather than as a mistake.
///
/// One reader for every model, because the conditions are the same for a
/// hull, a palm and a whale — a second copy would be a second opinion about
/// what the look is.
pub fn assert_model_draws(file: &str, meshes: &[(usize, &str)]) {
    let (json, _) = model(file);
    for (index, name) in meshes {
        assert_eq!(
            json["meshes"][*index]["name"], *name,
            "mesh {index} of {file} is not the {name}"
        );
        let faces = triangles(file, *index, "POSITION");
        assert!(
            winds_outwards(&faces),
            "the {name} of {file} is wound inside-out"
        );
        assert!(
            is_flat_shaded(&faces, &triangles(file, *index, "NORMAL")),
            "the {name} of {file} is smooth-shaded"
        );
    }
}

/// Whether a model brings its own colours.
///
/// Every model painted this way is drawn with a *white* material, so a mesh
/// that lost its colour attribute — a master saved without one, or an export
/// that dropped it — would arrive as a white animal rather than as an
/// obviously broken one. That is a failure worth a test of its own, because
/// nothing else in the game would report it.
pub fn assert_model_is_painted(file: &str, mesh: usize) {
    let (json, _) = model(file);
    let attributes = &json["meshes"][mesh]["primitives"][0]["attributes"];
    assert!(
        !attributes["COLOR_0"].is_null(),
        "{file} carries no colours — see its NOTES.md, and the export settings \
         that pass them through"
    );
}

/// The creature a model file is named for: `models/whale.glb` is a whale.
///
/// The one-mesh models are all named this way — the master names the object
/// for the animal it is — so a test can ask that the file contains what its
/// name claims without repeating either.
pub fn creature_named_by(file: &str) -> &str {
    file.strip_prefix("models/")
        .and_then(|name| name.strip_suffix(".glb"))
        .expect("a glTF binary under assets/models/")
}

/// What the meshes in a file are called, in the order the file holds them.
pub fn mesh_names(file: &str) -> Vec<String> {
    named(file, "meshes")
}

/// What the animation clips in a file are called, likewise — the game asks
/// for a clip by its position and holds it to its name here, exactly as it
/// does with meshes.
pub fn clip_names(file: &str) -> Vec<String> {
    named(file, "animations")
}

fn named(file: &str, kind: &str) -> Vec<String> {
    model(file).0[kind]
        .as_array()
        .unwrap_or_else(|| panic!("{file} has no {kind}"))
        .iter()
        .map(|thing| {
            thing["name"]
                .as_str()
                .unwrap_or_else(|| panic!("an unnamed thing among {file}'s {kind}"))
                .to_owned()
        })
        .collect()
}

/// How far one mesh reaches along an axis — `0` for X and across a wingspan,
/// `1` for Y and how tall a thing stands, `2` for Z and nose to tail.
///
/// Every model test asks this of something, and each of them used to fold its
/// own min and max, in four spellings of the same thing. What they are all
/// guarding against is one mistake: a remodel that came through in
/// centimetres, or with the exporter's axes wrong, which draws a
/// hundred-metre whale.
pub fn extent(file: &str, mesh: usize, axis: usize) -> (f32, f32) {
    triangles(file, mesh, "POSITION")
        .into_iter()
        .flatten()
        .fold((f32::MAX, f32::MIN), |(low, high), corner| {
            (low.min(corner[axis]), high.max(corner[axis]))
        })
}

/// The same, as one number: how much of that axis the mesh occupies.
pub fn span(file: &str, mesh: usize, axis: usize) -> f32 {
    let (low, high) = extent(file, mesh, axis);
    high - low
}

/// The rule every rigged model here lives by: each vertex carried by exactly
/// one bone, at full weight.
///
/// Weight-paint a shoulder smoothly in Blender and the facets round off as
/// the model moves — gradients across faces, in a look built out of flat
/// tones that has none. It is a modelling decision nothing at runtime would
/// catch, and one of the rules `assets-src/models/NOTES.md` calls out, so it
/// is asserted from one place rather than copied per rigged model.
pub fn assert_rigid_skin(file: &str) {
    let names = mesh_names(file);
    for (mesh, name) in names.iter().enumerate() {
        for weights in skin_weights(file, mesh) {
            let carrying = weights.iter().filter(|w| **w > 0.0).count();
            assert_eq!(
                carrying, 1,
                "a vertex of {file}'s {name} is shared between bones: {weights:?}"
            );
            assert!(
                weights.iter().any(|w| (*w - 1.0).abs() < 1e-3),
                "a vertex of {file}'s {name} is carried at {weights:?}"
            );
        }
    }
}

/// Whether every face of a mesh is wound to look outwards, by the volume the
/// winding implies.
///
/// A closed shell's faces sum to its own volume through the divergence
/// theorem, positive when they face out and negative when they all face in.
/// Written this way rather than by comparing each face against its own
/// middle, because a mesh may be several separate solids and the middle of
/// seven fronds is not inside any of them.
pub fn winds_outwards(faces: &[[Vec3; 3]]) -> bool {
    let volume: f32 = faces.iter().map(|f| f[0].dot(f[1].cross(f[2])) / 6.0).sum();
    volume > 0.0
}

/// Whether every corner's normal agrees with the facet it belongs to — which
/// is what makes a mesh read as flat tones rather than as a curved surface.
pub fn is_flat_shaded(faces: &[[Vec3; 3]], normals: &[[Vec3; 3]]) -> bool {
    faces.iter().zip(normals).all(|(face, normal)| {
        let flat = (face[1] - face[0]).cross(face[2] - face[0]).normalize();
        normal.iter().all(|corner| corner.dot(flat) > 0.999)
    })
}
