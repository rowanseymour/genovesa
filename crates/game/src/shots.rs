//! A picture of the view, on a key.
//!
//! F12 writes what the window is showing to a PNG — whatever that is: the
//! world, the chart unrolled over it, a menu. The pictures land in
//! `screenshots/` beside the worlds and logbooks — see [`server::data_dir`] —
//! numbered in the order they were taken, and each one's path goes to the log
//! once the file is written. Written rather than asked for, and said once:
//! a picture takes a few frames to reach its file and the writing can fail,
//! so a line said on the way would be a success announced before it was one.
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
//!
//! Every picture either mouth asks for is written by [`save_stamped`], which
//! puts where the camera stood into the file as well as on the screen. The
//! overlay has always said it — see [`crate::debug`], whose last line reads
//! the world and the view back in the words that put them there — but only
//! when it is switched on, and only by spending the corner of the picture on
//! it. A shot worth arguing about is rarely the one somebody remembered to
//! turn the readout on for, so the words go in the file too, where they cost
//! the picture nothing and cannot be cropped off.
//!
//! A `tEXt` chunk keyed [`STAMP`], because that is the metadata PNG actually
//! has: EXIF is a JPEG habit that only reached PNG in 2017, and it has no
//! tag for a seed anyway, so a stamp in it would be a string smuggled through
//! `UserComment`. A named text chunk says what it is.

use std::fs;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};

use crate::debug::Stamp;

/// What the stamp is filed under in the file. Read it back with any reader of
/// PNG text chunks — the chunk is the format's own, not this game's.
pub const STAMP: &str = "genovesa";

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
fn shot_key(
    keys: Res<ButtonInput<KeyCode>>,
    stamp: Stamp,
    mut taken: Local<u32>,
    mut commands: Commands,
) {
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
        .observe(save_stamped(path, stamp.text()));
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

/// Writes the picture, with what [`Stamp::text`] said about where it was
/// taken from written into it.
///
/// Bevy's own `save_to_disk` goes through `image`, which encodes a PNG but
/// exposes none of its text chunks, so the encoder underneath is reached
/// directly. What is discarded on the way is the alpha channel, for the reason
/// Bevy discards it: with HDR on it carries brightness rather than opacity,
/// and a picture kept as RGBA would come out wrong.
///
/// A stamp of `None` — a picture of a menu screen — writes an ordinary PNG.
/// Nothing is lost by it: there was no place to name.
pub(crate) fn save_stamped(
    path: PathBuf,
    stamp: Option<String>,
) -> impl FnMut(On<ScreenshotCaptured>) {
    move |captured| {
        if let Err(why) = write_stamped(&path, &captured.image, stamp.as_deref()) {
            error!("no picture: {} not written: {why}", path.display());
        } else {
            info!("shot {}", path.display());
        }
    }
}

/// The writing itself, split out so a test can call it with an image it made
/// rather than one a GPU handed back.
fn write_stamped(path: &Path, image: &Image, stamp: Option<&str>) -> Result<(), String> {
    let picture = image
        .clone()
        .try_into_dynamic()
        .map_err(|why| format!("the screen cannot be understood: {why}"))?
        .to_rgb8();
    let file = fs::File::create(path).map_err(|why| why.to_string())?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), picture.width(), picture.height());
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    if let Some(stamp) = stamp {
        encoder
            .add_text_chunk(STAMP.to_string(), stamp.to_string())
            .map_err(|why| why.to_string())?;
    }
    encoder
        .write_header()
        .and_then(|mut writer| {
            writer.write_image_data(&picture)?;
            // Finished rather than dropped, which is the difference between a
            // half-written picture being reported and being swallowed: the
            // last of the pixels are still in the `BufWriter` here, and both
            // the closing chunk and that flush can fail. Dropping discards
            // either error — `png`'s own `Drop` writes the end chunk with the
            // result thrown away — and this call is what a full disk has to
            // travel through to reach the log, and to stop `control`'s `shot`
            // answering a driver with a path to a truncated file.
            writer.finish()
        })
        .map_err(|why| why.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::debug::Toggles;

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

    /// A two-by-one picture, which is enough to be a PNG.
    fn a_picture() -> Image {
        Image::new(
            bevy::render::render_resource::Extent3d {
                width: 2,
                height: 1,
                depth_or_array_layers: 1,
            },
            bevy::render::render_resource::TextureDimension::D2,
            vec![255, 0, 0, 255, 0, 0, 255, 255],
            bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
            bevy::asset::RenderAssetUsages::default(),
        )
    }

    /// What a reader gets back out — the whole point of the exercise, and the
    /// half a stamp gathered but never written would fail silently at.
    fn read_back(path: &Path) -> (png::OutputInfo, Option<String>) {
        let file = fs::File::open(path).expect("the picture written");
        let decoder = png::Decoder::new(std::io::BufReader::new(file));
        let mut reader = decoder.read_info().expect("a PNG");
        let mut pixels = vec![0; reader.output_buffer_size().expect("a bounded picture")];
        let info = reader.next_frame(&mut pixels).expect("the pixels");
        let stamp = reader
            .info()
            .uncompressed_latin1_text
            .iter()
            .find(|chunk| chunk.keyword == STAMP)
            .map(|chunk| chunk.text.clone());
        (info, stamp)
    }

    #[test]
    fn a_picture_carries_the_words_that_would_stand_you_here_again() {
        let path = shot_dir("stamped").join("shot-001.png");
        let said = "seed 4242 / goto 480 -1200 / yaw 90 / zoom 240";
        write_stamped(&path, &a_picture(), Some(said)).expect("the picture written");

        let (info, stamp) = read_back(&path);
        assert_eq!(stamp.as_deref(), Some(said));
        // And it is still a picture: a stamp that cost the pixels would be a
        // worse trade than no stamp at all.
        assert_eq!((info.width, info.height), (2, 1));
    }

    #[test]
    fn a_picture_of_nowhere_is_an_ordinary_png() {
        let path = shot_dir("unstamped").join("shot-001.png");
        write_stamped(&path, &a_picture(), None).expect("the picture written");

        let (info, stamp) = read_back(&path);
        assert_eq!(stamp, None);
        assert_eq!((info.width, info.height), (2, 1));
    }

    #[test]
    fn the_key_asks_for_one_picture_per_press() {
        crate::testing::quarantine_data_dir();
        let mut app = App::new();
        app.init_resource::<ButtonInput<KeyCode>>()
            // What a picture is stamped with, which the key reads whether or
            // not the overlay drawing the same words is up.
            .init_resource::<Toggles>()
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
