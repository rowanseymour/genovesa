//! What the app-level tests are built out of.
//!
//! A headless `App` is driven by hand — nothing pumps its frame loop — so a
//! test that is waiting on something has to run the frames itself. Both the
//! menu's tests and the net module's do, and had a copy of the loop apiece.

use std::thread;
use std::time::{Duration, Instant};

use bevy::app::App;

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
