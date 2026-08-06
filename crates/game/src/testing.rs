//! What the app-level tests are built out of.
//!
//! A headless `App` is driven by hand — nothing pumps its frame loop — so a
//! test that is waiting on something has to run the frames itself, and a test
//! that presses a key has to clear it the way the real input plugin would.
//! Every module testing a system needs some of this, and each of them had a
//! copy of the piece it needed.

use std::thread;
use std::time::{Duration, Instant};

use bevy::prelude::*;

use protocol::ground::{
    quantize, ChunkPayload, Surface, Tone, CHUNK_METRES, FACET_METRES, FACET_TRIS, FACET_VERTS,
    OCEAN_DEPTH,
};

use crate::bindings::{Action, KeyBindings};
use crate::terrain::Ground;

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
