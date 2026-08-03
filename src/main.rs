mod camera;
mod menu;
mod noise;
mod screenshot;
mod terrain;

use bevy::prelude::*;

use camera::MapCameraPlugin;
use menu::MenuPlugin;
use terrain::{MapConfig, TerrainPlugin};

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
    fn from_env() -> Self {
        match std::env::var("KASSITER_STATE").as_deref() {
            Ok("newmap") => Self::NewMap,
            Ok("inworld") => Self::InWorld,
            _ => Self::MainMenu,
        }
    }
}

fn main() {
    let mut app = App::new();

    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "Kassiter".into(),
            resolution: (1280, 720).into(),
            ..default()
        }),
        ..default()
    }))
    .insert_state(AppState::from_env())
    .insert_resource(ClearColor(SKY))
    // Sky fill. Deliberately strong relative to the sun — this look wants
    // shadows that read as a second flat tone, not as darkness.
    .insert_resource(GlobalAmbientLight {
        color: Color::srgb(0.82, 0.89, 1.0),
        brightness: 1_400.0,
        ..default()
    })
    .insert_resource(MapConfig::from_env())
    .add_plugins((
        TerrainPlugin,
        MapCameraPlugin,
        MenuPlugin,
        screenshot::ScreenshotPlugin,
    ));

    app.run();
}
