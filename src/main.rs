use bevy::prelude::*;

use kassiter::camera::MapCameraPlugin;
use kassiter::menu::MenuPlugin;
use kassiter::screenshot::ScreenshotPlugin;
use kassiter::terrain::{MapConfig, TerrainPlugin};
use kassiter::{AppState, SKY};

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
    .add_plugins((TerrainPlugin, MapCameraPlugin, MenuPlugin, ScreenshotPlugin));

    app.run();
}
