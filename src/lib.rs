//! Terrain generation and the app that walks around in it.
//!
//! The crate is a library so that more than one binary can share it: `kassiter`
//! is the game, and `mapgen` renders maps in plan without opening a window.

pub mod camera;
pub mod menu;
pub mod noise;
pub mod plan;
pub mod screenshot;
pub mod terrain;

use bevy::prelude::*;

/// Colour of the sky above the horizon. The camera's distance fog fades to the
/// same colour, so the two meet seamlessly.
pub const SKY: Color = Color::srgb(0.63, 0.80, 0.93);

/// Top-level screen the app is on.
#[derive(States, Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum AppState {
    #[default]
    MainMenu,
    /// Choosing map size and seed.
    NewMap,
    InWorld,
}

impl AppState {
    /// `KASSITER_STATE=newmap|inworld` skips straight to a screen, so that
    /// working on one doesn't mean clicking through the others every run.
    pub fn from_env() -> Self {
        match std::env::var("KASSITER_STATE").as_deref() {
            Ok("newmap") => Self::NewMap,
            Ok("inworld") => Self::InWorld,
            _ => Self::MainMenu,
        }
    }
}
