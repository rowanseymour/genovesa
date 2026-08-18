//! Being asked to stop by the machine rather than by the player.
//!
//! A window closed, or Exit pressed, ends the app the tidy way: the world's
//! resources are dropped, and with them the server this process may have been
//! hosting, which hangs up on anybody else in that world and writes it down.
//! A signal — a `kill`, a terminal closing on a game started from one — is the
//! same intention arriving by another road, and left to itself it stops the
//! process where it stands: a hosted world loses whatever has happened since
//! its last periodic save, and the guests in it are left to notice a socket
//! that stopped answering.
//!
//! So the ask is turned into an ordinary [`AppExit`] and everything downstream
//! of that is the path the Exit button already takes. The listening itself
//! belongs to the server crate — it is the side that knows what a world stands
//! to lose — and this is only what a client does about it.
//!
//! Not a substitute for the drop being right: what this buys is that the drop
//! happens at all.

use bevy::prelude::*;

pub struct StoppingPlugin;

impl Plugin for StoppingPlugin {
    fn build(&self, app: &mut App) {
        server::signals::catch();
        app.add_systems(Update, quit_when_asked);
    }
}

/// Turns the ask into the app's own way of ending.
///
/// Every frame rather than once, and no state of its own: the ask does not
/// wear off, so a frame that has already written one writes another, and the
/// app exits on the first of them. A run that is never asked pays an atomic
/// load a frame.
fn quit_when_asked(mut exit: MessageWriter<AppExit>) {
    if server::signals::asked_to_stop() {
        exit.write(AppExit::Success);
    }
}

#[cfg(test)]
mod tests {
    use std::net::{SocketAddr, TcpListener};

    use server::{Server, WorldConfig};

    use super::*;
    use crate::net::Hosting;

    #[test]
    fn being_asked_to_stop_ends_the_app() {
        let mut app = App::new();
        app.add_message::<AppExit>().add_plugins(StoppingPlugin);

        app.update();
        assert!(
            app.world().resource::<Messages<AppExit>>().is_empty(),
            "the app gave up without being asked"
        );

        // Standing in for the signal, which cannot be raised at one test
        // without stopping the whole test binary. This is the only test in
        // this crate that asks, the latch being one per process.
        server::signals::ask();
        app.update();
        assert!(
            !app.world().resource::<Messages<AppExit>>().is_empty(),
            "the app was asked to stop and carried on"
        );
    }

    #[test]
    fn an_app_that_goes_away_takes_its_world_with_it() {
        // The other half of the bargain, and the half worth pinning: quitting
        // is only worth turning a signal into if the quit itself puts the
        // world away. Bevy's runner owns the app and drops it when the loop
        // ends, so what a stopped run costs is exactly what dropping an app
        // holding a [`Hosting`] costs — which is this.
        let dir = std::env::temp_dir()
            .join("genovesa-stopping-tests")
            .join(format!("{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp space");
        let path = dir.join("one.world");

        let host = Server::bind(("127.0.0.1", 0), WorldConfig { seed: 7 })
            .expect("bind")
            .keeping_at(path.clone())
            .expect("keeping")
            .spawn()
            .expect("spawn");
        let addr: SocketAddr = host.addr();

        let mut app = App::new();
        app.add_message::<AppExit>().insert_resource(Hosting(host));
        app.update();

        // What the world was worth on paper before the app went. A world is
        // written down at birth as well as at death, so an age on its own
        // proves nothing — it is the age *moving* that is the closing save.
        let born = age_of(&path);
        std::thread::sleep(std::time::Duration::from_millis(50));

        drop(app);

        // The port is free, so nothing is still listening on a world nobody
        // is in...
        TcpListener::bind(addr).expect("the world outlived the app holding it");
        // ...and the world was written down on the way out rather than left
        // at whatever the last save happened to have caught, which is the
        // whole of what stopping properly is worth.
        let kept = age_of(&path);
        assert!(
            kept > born,
            "the closing save never happened: the file still says {born}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// How old the world in this file says it is, in seconds of its own life.
    fn age_of(path: &std::path::Path) -> f32 {
        std::fs::read_to_string(path)
            .expect("a kept world has a file")
            .lines()
            .find_map(|line| line.strip_prefix("age "))
            .expect("a file says how old its world is")
            .parse()
            .expect("an age is a number")
    }
}
