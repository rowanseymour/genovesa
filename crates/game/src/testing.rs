//! What the app-level tests are built out of.
//!
//! A headless `App` is driven by hand — nothing pumps its frame loop — so a
//! test that is waiting on something has to run the frames itself, and a test
//! that presses a key has to clear it the way the real input plugin would.
//! Every module testing a system needs some of this, and each of them had a
//! copy of the piece it needed.

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use bevy::prelude::*;

use crate::bindings::{Action, KeyBindings};
use crate::terrain::{Archipelago, IslandSpec, WorldConfig};

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

/// A world with its biggest island near the origin already generated, and
/// that island's spec.
///
/// Anything riding the ground asks [`crate::terrain::WorldTerrain::surface`],
/// which answers `None` until the island under the point exists — so a test
/// of the boat floating, the camera grounding itself or a marker standing up
/// needs terrain that has actually been generated, not merely laid out. The
/// biggest island, because a test that wants somewhere to put things down
/// wants room to put them.
pub fn test_world() -> (Arc<Archipelago>, IslandSpec) {
    let world = Arc::new(Archipelago::new(&WorldConfig { seed: 1 }));
    let spec = world
        .islands_within(Vec2::splat(-6_000.0), Vec2::splat(6_000.0))
        .into_iter()
        .max_by_key(|s| s.chunks.x * s.chunks.y)
        .expect("a world should have an island within a few kilometres");
    world.island(spec);
    (world, spec)
}
