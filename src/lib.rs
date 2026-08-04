//! Terrain generation and the app that walks around in it.
//!
//! The crate is a library so that more than one binary can share it: `kassiter`
//! is the game, and `mapgen` renders maps in plan without opening a window.

pub mod bindings;
pub mod camera;
pub mod capture;
pub mod cli;
pub mod menu;
pub mod noise;
pub mod plan;
pub mod terrain;

use bevy::prelude::*;

/// Colour of the sky above the horizon. The camera's distance fog fades to the
/// same colour, so the two meet seamlessly.
pub const SKY: Color = Color::srgb(0.63, 0.80, 0.93);

/// Distance from the eye at which aerial haze starts to take the ground over,
/// in metres.
pub const HAZE_START: f32 = 320.0;
/// Distance at which the haze has fully replaced the ground with [`SKY`]. This
/// is the edge of what the camera can see at all, whatever it is pointed at, so
/// anything the picture depends on has to reach at least this far — the sun's
/// shadows included.
pub const HAZE_END: f32 = 900.0;

/// Size of the window the game is played in, in pixels. Captured shots are
/// sized by `--resolution` instead, having no window to take it from.
pub const WINDOW: UVec2 = UVec2::new(1280, 720);

/// Top-level screen the app is on.
#[derive(States, Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum AppState {
    #[default]
    MainMenu,
    /// Choosing map size and seed.
    NewMap,
    /// Choosing which key does what.
    Settings,
    InWorld,
}
