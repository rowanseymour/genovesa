//! A picture of the view, on a key.
//!
//! F12 writes what the window is showing to a PNG — whatever that is: the
//! world, the chart unrolled over it, a menu. The pictures land in
//! `screenshots/` beside the worlds and logbooks — see [`server::data_dir`] —
//! numbered in the order they were taken, and each one's path goes to the log
//! as it is asked for.
//!
//! The key is hard-wired rather than a [`crate::bindings::Action`], the way
//! the console's backquote is: photographing the screen is a control of the
//! machine, not of the world, and it is listed with the reserved keys so that
//! no binding can come to share it — see `crate::bindings::RESERVED`. On a Mac
//! keyboard whose top row plays media keys, it is pressed with Fn held.
//!
//! The debug socket's `shot` — see [`crate::control`] — stays the way a
//! *driven* run takes pictures: it waits for the ground to arrive and answers
//! with the path once the file is written, which a hand on a key needs none
//! of. A hand can see whether the view is worth keeping.

use std::fs;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};

pub struct ShotsPlugin;

impl Plugin for ShotsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, shot_key);
    }
}

/// Asks for the picture on the frame F12 goes down. No screen has a claim on
/// the key — it types no character for the console or a name to take, and a
/// menu is as photographable as the world — so unlike `chart::chart_key` this
/// listens the same whatever is up.
fn shot_key(keys: Res<ButtonInput<KeyCode>>, mut taken: Local<u32>, mut commands: Commands) {
    if !keys.just_pressed(KeyCode::F12) {
        return;
    }
    let Some(dir) = server::data_dir().map(|dir| dir.join("screenshots")) else {
        warn!("no picture: this machine has nowhere to keep one");
        return;
    };
    if let Err(why) = fs::create_dir_all(&dir) {
        warn!("no picture: cannot make {}: {why}", dir.display());
        return;
    }
    let (path, number) = next_place(&dir, *taken);
    *taken = number;
    commands
        .spawn(Screenshot::primary_window())
        .observe(save_to_disk(path.clone()));
    info!("shot {}", path.display());
}

/// Where the next picture goes: one past the highest number standing in the
/// directory, or past the highest this run has already handed out if that is
/// further — a picture takes a few frames to reach its file, and two presses
/// inside that window must not be sent to the same name.
fn next_place(dir: &Path, taken: u32) -> (PathBuf, u32) {
    let standing = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| numbered(&entry.file_name()))
        .max()
        .unwrap_or(0);
    let number = standing.max(taken) + 1;
    (dir.join(format!("shot-{number:03}.png")), number)
}

/// The number in a name this module wrote, if the name is one of its own.
fn numbered(name: &std::ffi::OsStr) -> Option<u32> {
    name.to_str()?
        .strip_prefix("shot-")?
        .strip_suffix(".png")?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of this test's own to number pictures in.
    fn shot_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("genovesa-shots-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a directory to test in");
        dir
    }

    #[test]
    fn numbering_carries_on_from_what_is_standing() {
        let dir = shot_dir("standing");
        for name in ["shot-001.png", "shot-007.png"] {
            fs::write(dir.join(name), b"").expect("a picture already kept");
        }
        // Files that are not this module's are not in the sequence.
        fs::write(dir.join("chart-100.png"), b"").expect("a stray file");

        assert_eq!(next_place(&dir, 0), (dir.join("shot-008.png"), 8));
    }

    #[test]
    fn numbering_never_reuses_a_name_already_handed_out() {
        // An empty directory, as it stands while a just-asked-for picture is
        // still crossing the render world on its way to a file.
        let dir = shot_dir("handed-out");
        assert_eq!(next_place(&dir, 3), (dir.join("shot-004.png"), 4));
    }

    #[test]
    fn the_key_asks_for_one_picture_per_press() {
        crate::testing::quarantine_data_dir();
        let mut app = App::new();
        app.init_resource::<ButtonInput<KeyCode>>()
            .add_systems(Update, shot_key);

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F12);
        app.update();

        let asked = |app: &mut App| {
            app.world_mut()
                .query::<&Screenshot>()
                .iter(app.world())
                .count()
        };
        assert_eq!(asked(&mut app), 1);

        // Held is not pressed again.
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .clear_just_pressed(KeyCode::F12);
        app.update();
        assert_eq!(asked(&mut app), 1);
    }
}
