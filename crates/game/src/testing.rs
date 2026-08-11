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
use crate::{AppState, Helm};

/// How long every test frame lasts in a [`world_app`]. Headless frames take
/// next to no real time, which nothing eased can live with — the eases are
/// curves *in seconds* — so the clock is stepped by a fixed sixty-a-second
/// frame and the tests get the ramp a player would.
pub const FRAME: Duration = Duration::from_millis(16);

/// A headless app already in a match, with the boat and player systems
/// running — the world as the movement tests know it, shared here because
/// the boat's tests and the player's want exactly the same one.
///
/// `AssetPlugin` because the boat is spawned out of a file, and
/// `TaskPoolPlugin` because that is where it finds the thread to read it on.
/// Nothing here waits for the load — these tests are about where things are
/// and what they do, not what they look like — but `launch` asks the asset
/// server for its meshes, and without one there is no boat.
pub fn world_app() -> App {
    let mut app = App::new();
    app.add_plugins((
        TaskPoolPlugin::default(),
        AssetPlugin::default(),
        TimePlugin,
        StatesPlugin,
        BoatPlugin,
        PlayerPlugin,
    ))
    .insert_resource(TimeUpdateStrategy::ManualDuration(FRAME))
    .init_state::<AppState>()
    .add_sub_state::<Helm>()
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
/// slope steep enough to be a problem.
pub fn test_ground() -> Ground {
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
                    quantize(test_island_height(corner))
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
                    // The test island is a smooth dome with nothing to
                    // enclose a basin, so there is no lake on it to draw.
                    water: None,
                    // Nor anything the palm rule would call a beach.
                    palms: Vec::new(),
                });
            ground.deliver(chunk, payload);
        }
    }
    ground
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

// --- Models -------------------------------------------------------------------

/// A model under `assets/`, as its glTF JSON and its binary chunk.
///
/// Read straight out of the `.glb` rather than through Bevy's loader: what the
/// tests want to know is what the file *says*, and going through the asset
/// server would mean standing up a render app and waiting on a load to learn
/// it.
pub fn model(name: &str) -> (serde_json::Value, Vec<u8>) {
    let path = format!("{}/../../assets/{name}", env!("CARGO_MANIFEST_DIR"));
    let file = std::fs::read(&path).unwrap_or_else(|_| panic!("{path} — run assets-src/export.sh"));
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
/// `POSITION` for where its corners are or `NORMAL` for where they face.
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
