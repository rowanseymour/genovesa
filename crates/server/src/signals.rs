//! Being asked to stop, as against being cut off in the middle of a sentence.
//!
//! A world is written down on the way out: the closing save happens inside the
//! drop that ends the session. A process killed where it stands does none of
//! that and loses everything since the last periodic save — see
//! `KEEP_INTERVAL`. That is the ordinary way a dedicated server stops, so the
//! signals meaning *stop* are caught and turned into the ordinary end of a
//! session: the same drop, the same closing save, the same hanging up.
//!
//! Caught here: `SIGINT`, what Ctrl-C sends; `SIGTERM`, what an init system
//! and a bare `kill` send; and `SIGHUP`, the terminal that started it going
//! away. Asked a second time the process goes at once, on the grounds that a
//! stop already taking too long is not a reason to have to reach for `kill -9`.
//!
//! What is deliberately *not* caught is `SIGKILL`, which cannot be, and that is
//! the whole reason the periodic save exists as well as this. This narrows the
//! window; it does not close it.
//!
//! Somewhere without these signals catches nothing, [`asked_to_stop`] never
//! becomes true, and the ways a world is left on purpose are untouched.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

/// How often [`wait_to_be_asked`] looks up.
///
/// Polled rather than parked because waking a thread is not something a signal
/// handler may safely do — it takes locks a handler can arrive in the middle
/// of — and a tenth of a second is nothing against the seconds an init system
/// gives a process to stop.
const HEARTBEAT: Duration = Duration::from_millis(100);

/// Set by the handler and never unset: having been asked to stop is not
/// something that stops being true.
static ASKED: AtomicBool = AtomicBool::new(false);

/// Whether this process has been asked to stop.
pub fn asked_to_stop() -> bool {
    ASKED.load(Ordering::Relaxed)
}

/// Asks this process to stop, exactly as a signal does — for the tests that
/// stand in for one, there being no way to raise a real signal at a single
/// test without stopping the whole test binary.
pub fn ask() {
    ASKED.store(true, Ordering::Relaxed);
}

/// Blocks until somebody asks this process to stop. What a process whose whole
/// job is to keep a world up does with its main thread.
pub fn wait_to_be_asked() {
    while !asked_to_stop() {
        thread::sleep(HEARTBEAT);
    }
}

/// Starts listening for the signals that mean stop. Called once, by a binary
/// that has somewhere sensible to be when one arrives.
#[cfg(unix)]
pub fn catch() {
    // Through a pointer rather than straight to an integer, which is what a
    // function item will not cast to on its own.
    let handler = heard as *const () as libc::sighandler_t;
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        // SAFETY: `heard` does only what a signal handler may do — one atomic
        // store, and `_exit` if it has been here before. Nothing it touches
        // can be locked by the thread it interrupts.
        unsafe { libc::signal(signal, handler) };
    }
}

#[cfg(not(unix))]
pub fn catch() {}

/// The handler itself, which does as little as a handler should: it says that
/// the ask happened, and leaves the acting on it to a thread that is allowed
/// to take a lock.
#[cfg(unix)]
extern "C" fn heard(signal: libc::c_int) {
    if ASKED.swap(true, Ordering::Relaxed) {
        // Asked twice, so whatever the first ask started is not getting there
        // — a save on a wedged disk, a client's thread that will not let go.
        // Going now costs the world its closing save, which is what the
        // person pressing Ctrl-C the second time is asking for.
        //
        // `_exit` and not `exit`: the latter runs handlers registered by
        // whatever else is in the process, and none of them were written to be
        // arrived at sideways from a signal.
        unsafe { libc::_exit(128 + signal) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn being_asked_is_not_something_that_wears_off() {
        // Deliberately the only test here that touches the latch: it is one
        // per process, so a test that could unset it would be a test the
        // others race against.
        assert!(!asked_to_stop(), "something asked before the test did");
        ask();
        assert!(asked_to_stop());
        // And a wait that is already over returns rather than hanging.
        wait_to_be_asked();
    }
}
