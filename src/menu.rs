//! Main menu and the map setup dialog.

use bevy::prelude::*;
use bevy::text::FontSize;

use crate::terrain::{MapConfig, CHUNK_TILES};
use crate::AppState;

/// Map sizes offered in the setup dialog, in metres per side — each a whole
/// number of the generator's chunks, which is the unit maps are measured in.
/// The generator itself takes any X by Z chunks; the dialog offers squares
/// until it grows a proper size control.
pub const SIZE_PRESETS: [(&str, u32); 3] = [("Small", 768), ("Medium", 1024), ("Large", 1536)];

/// Longest seed the user can type. Keeps it inside a u32.
const MAX_SEED_DIGITS: usize = 9;

const PANEL: Color = Color::srgba(0.09, 0.11, 0.10, 0.94);
const PANEL_EDGE: Color = Color::srgb(0.42, 0.40, 0.28);
const BUTTON: Color = Color::srgb(0.17, 0.20, 0.16);
const BUTTON_HOVER: Color = Color::srgb(0.26, 0.31, 0.22);
const BUTTON_PRESS: Color = Color::srgb(0.35, 0.42, 0.28);
const BUTTON_ON: Color = Color::srgb(0.44, 0.40, 0.18);
const TEXT: Color = Color::srgb(0.88, 0.87, 0.80);
const TEXT_DIM: Color = Color::srgb(0.60, 0.60, 0.55);

pub struct MenuPlugin;

impl Plugin for MenuPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NewMapSettings>()
            .add_systems(OnEnter(AppState::MainMenu), spawn_main_menu)
            .add_systems(OnEnter(AppState::NewMap), spawn_new_map_dialog)
            .add_systems(
                Update,
                (
                    highlight_buttons,
                    main_menu_actions.run_if(in_state(AppState::MainMenu)),
                    (dialog_actions, type_seed, refresh_dialog)
                        .run_if(in_state(AppState::NewMap)),
                    leave_world.run_if(in_state(AppState::InWorld)),
                ),
            );
    }
}

/// What the setup dialog is currently showing. Copied into [`MapConfig`] when
/// a new map is generated.
#[derive(Resource)]
struct NewMapSettings {
    size: u32,
    /// Held as text so the field can be edited a digit at a time, including
    /// being temporarily empty.
    seed: String,
}

impl Default for NewMapSettings {
    fn default() -> Self {
        let defaults = MapConfig::default();
        Self {
            // The dialog offers square maps, so one axis stands for both.
            size: defaults.tiles().x,
            seed: defaults.seed.to_string(),
        }
    }
}

impl NewMapSettings {
    fn seed_value(&self) -> u32 {
        self.seed.parse().unwrap_or(0)
    }
}

#[derive(Component, Clone, Copy, PartialEq)]
enum MenuButton {
    NewMap,
    Exit,
    ChooseSize(u32),
    RandomSeed,
    Start,
    Back,
}

/// Marks the seed readout so it can be refreshed as the player types.
#[derive(Component)]
struct SeedText;

// ---------------------------------------------------------------------------
// Main menu
// ---------------------------------------------------------------------------

fn spawn_main_menu(mut commands: Commands) {
    commands
        .spawn((
            Name::new("Main menu"),
            DespawnOnExit(AppState::MainMenu),
            screen(),
        ))
        .with_children(|screen| {
            screen.spawn((
                Text::new("KASSITER"),
                TextFont {
                    font_size: FontSize::Px(64.0),
                    ..default()
                },
                TextColor(TEXT),
                Node {
                    margin: UiRect::bottom(Val::Px(8.0)),
                    ..default()
                },
            ));
            screen.spawn((
                Text::new("islands in flat colours"),
                TextFont {
                    font_size: FontSize::Px(18.0),
                    ..default()
                },
                TextColor(TEXT_DIM),
                Node {
                    margin: UiRect::bottom(Val::Px(48.0)),
                    ..default()
                },
            ));

            spawn_button(screen, MenuButton::NewMap, "New Map", 240.0);
            spawn_button(screen, MenuButton::Exit, "Exit", 240.0);
        });
}

fn main_menu_actions(
    buttons: Query<(&Interaction, &MenuButton), Changed<Interaction>>,
    mut next: ResMut<NextState<AppState>>,
    mut exit: MessageWriter<AppExit>,
) {
    for (interaction, button) in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match button {
            MenuButton::NewMap => next.set(AppState::NewMap),
            MenuButton::Exit => {
                exit.write(AppExit::Success);
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// New map dialog
// ---------------------------------------------------------------------------

fn spawn_new_map_dialog(mut commands: Commands, settings: Res<NewMapSettings>) {
    commands
        .spawn((
            Name::new("New map dialog"),
            DespawnOnExit(AppState::NewMap),
            screen(),
        ))
        .with_children(|screen| {
            screen
                .spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::Center,
                        padding: UiRect::all(Val::Px(32.0)),
                        border: UiRect::all(Val::Px(2.0)),
                        row_gap: Val::Px(10.0),
                        ..default()
                    },
                    BackgroundColor(PANEL),
                    BorderColor::all(PANEL_EDGE),
                ))
                .with_children(|panel| {
                    panel.spawn((
                        Text::new("New Map"),
                        TextFont {
                            font_size: FontSize::Px(34.0),
                            ..default()
                        },
                        TextColor(TEXT),
                        Node {
                            margin: UiRect::bottom(Val::Px(20.0)),
                            ..default()
                        },
                    ));

                    label(panel, "Map size");
                    panel
                        .spawn(Node {
                            column_gap: Val::Px(8.0),
                            margin: UiRect::bottom(Val::Px(16.0)),
                            ..default()
                        })
                        .with_children(|row| {
                            for (name, size) in SIZE_PRESETS {
                                spawn_button(
                                    row,
                                    MenuButton::ChooseSize(size),
                                    &format!("{name}\n{size} m"),
                                    120.0,
                                );
                            }
                        });

                    label(panel, "Seed");
                    panel.spawn((
                        SeedText,
                        Text::new(settings.seed.clone()),
                        TextFont {
                            font_size: FontSize::Px(26.0),
                            ..default()
                        },
                        TextColor(TEXT),
                    ));
                    panel.spawn((
                        Text::new("type digits, backspace to edit"),
                        TextFont {
                            font_size: FontSize::Px(13.0),
                            ..default()
                        },
                        TextColor(TEXT_DIM),
                    ));
                    spawn_button(panel, MenuButton::RandomSeed, "Random", 160.0);

                    panel
                        .spawn(Node {
                            column_gap: Val::Px(8.0),
                            margin: UiRect::top(Val::Px(24.0)),
                            ..default()
                        })
                        .with_children(|row| {
                            spawn_button(row, MenuButton::Back, "Back", 130.0);
                            spawn_button(row, MenuButton::Start, "Start", 130.0);
                        });
                });
        });
}

fn dialog_actions(
    buttons: Query<(&Interaction, &MenuButton), Changed<Interaction>>,
    mut settings: ResMut<NewMapSettings>,
    mut config: ResMut<MapConfig>,
    mut next: ResMut<NextState<AppState>>,
) {
    for (interaction, button) in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match button {
            MenuButton::ChooseSize(size) => settings.size = *size,
            MenuButton::RandomSeed => settings.seed = random_seed().to_string(),
            MenuButton::Back => next.set(AppState::MainMenu),
            MenuButton::Start => {
                config.chunks = UVec2::splat(settings.size / CHUNK_TILES);
                config.seed = settings.seed_value();
                next.set(AppState::InWorld);
            }
            _ => {}
        }
    }
}

/// Digit-by-digit editing of the seed field.
fn type_seed(keys: Res<ButtonInput<KeyCode>>, mut settings: ResMut<NewMapSettings>) {
    for key in keys.get_just_pressed() {
        let digit = match key {
            KeyCode::Digit0 | KeyCode::Numpad0 => '0',
            KeyCode::Digit1 | KeyCode::Numpad1 => '1',
            KeyCode::Digit2 | KeyCode::Numpad2 => '2',
            KeyCode::Digit3 | KeyCode::Numpad3 => '3',
            KeyCode::Digit4 | KeyCode::Numpad4 => '4',
            KeyCode::Digit5 | KeyCode::Numpad5 => '5',
            KeyCode::Digit6 | KeyCode::Numpad6 => '6',
            KeyCode::Digit7 | KeyCode::Numpad7 => '7',
            KeyCode::Digit8 | KeyCode::Numpad8 => '8',
            KeyCode::Digit9 | KeyCode::Numpad9 => '9',
            KeyCode::Backspace => {
                settings.seed.pop();
                continue;
            }
            _ => continue,
        };

        if settings.seed.len() < MAX_SEED_DIGITS {
            settings.seed.push(digit);
        }
    }
}

/// Keeps the dialog's readouts in step with the settings behind them.
fn refresh_dialog(
    settings: Res<NewMapSettings>,
    mut seed_text: Query<&mut Text, With<SeedText>>,
    mut buttons: Query<(&MenuButton, &mut BackgroundColor, &Interaction)>,
) {
    if !settings.is_changed() {
        return;
    }

    for mut text in &mut seed_text {
        // An empty field would render as nothing at all, so show the zero it
        // will be read as.
        text.0 = if settings.seed.is_empty() {
            "0".to_string()
        } else {
            settings.seed.clone()
        };
    }

    // The chosen size stays lit so it's clear which preset is active.
    for (button, mut color, interaction) in &mut buttons {
        if let MenuButton::ChooseSize(size) = button {
            if *interaction == Interaction::None {
                *color = BackgroundColor(if *size == settings.size {
                    BUTTON_ON
                } else {
                    BUTTON
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// In world
// ---------------------------------------------------------------------------

fn leave_world(keys: Res<ButtonInput<KeyCode>>, mut next: ResMut<NextState<AppState>>) {
    if keys.just_pressed(KeyCode::Escape) {
        next.set(AppState::MainMenu);
    }
}

// ---------------------------------------------------------------------------
// Shared widgets
// ---------------------------------------------------------------------------

/// Full-screen, centred column that every menu screen is built inside.
fn screen() -> impl Bundle {
    (
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            row_gap: Val::Px(12.0),
            ..default()
        },
        BackgroundColor(Color::srgba(0.05, 0.07, 0.09, 0.72)),
    )
}

fn label(parent: &mut ChildSpawnerCommands, text: &str) {
    parent.spawn((
        Text::new(text),
        TextFont {
            font_size: FontSize::Px(15.0),
            ..default()
        },
        TextColor(TEXT_DIM),
    ));
}

fn spawn_button(parent: &mut ChildSpawnerCommands, action: MenuButton, text: &str, width: f32) {
    parent
        .spawn((
            Button,
            action,
            Node {
                width: Val::Px(width),
                padding: UiRect::axes(Val::Px(12.0), Val::Px(12.0)),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BackgroundColor(BUTTON),
            BorderColor::all(PANEL_EDGE),
        ))
        .with_children(|button| {
            button.spawn((
                Text::new(text),
                TextFont {
                    font_size: FontSize::Px(19.0),
                    ..default()
                },
                TextColor(TEXT),
                TextLayout::justify(Justify::Center),
            ));
        });
}

fn highlight_buttons(
    settings: Res<NewMapSettings>,
    mut buttons: Query<(&Interaction, &MenuButton, &mut BackgroundColor), Changed<Interaction>>,
) {
    for (interaction, button, mut color) in &mut buttons {
        let idle = match button {
            MenuButton::ChooseSize(size) if *size == settings.size => BUTTON_ON,
            _ => BUTTON,
        };

        *color = BackgroundColor(match interaction {
            Interaction::Pressed => BUTTON_PRESS,
            Interaction::Hovered => BUTTON_HOVER,
            Interaction::None => idle,
        });
    }
}

/// Seeds off the clock. Good enough for "give me a different map".
fn random_seed() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() ^ d.as_secs() as u32)
        .unwrap_or(0);
    nanos % 10u32.pow(MAX_SEED_DIGITS as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::state::app::StatesPlugin;

    /// A headless app running the menu systems, with no renderer attached.
    fn test_app(state: AppState) -> App {
        let mut app = App::new();
        app.add_plugins((StatesPlugin, MenuPlugin))
            .insert_state(state)
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<MapConfig>()
            .add_message::<AppExit>();
        app.update();
        app
    }

    /// Simulates a click by spawning a pressed button; `Changed<Interaction>`
    /// fires for a freshly inserted component.
    fn click(app: &mut App, button: MenuButton) {
        app.world_mut().spawn((button, Interaction::Pressed));
        app.update();
        app.update();
    }

    /// Taps a key for exactly one frame. Releasing afterwards matters: a key
    /// still held down never counts as just-pressed again.
    fn press_key(app: &mut App, key: KeyCode) {
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(key);
        app.update();

        let mut input = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        input.release(key);
        input.clear();
    }

    fn state(app: &App) -> AppState {
        *app.world().resource::<State<AppState>>().get()
    }

    #[test]
    fn new_map_opens_the_setup_dialog() {
        let mut app = test_app(AppState::MainMenu);
        click(&mut app, MenuButton::NewMap);
        assert_eq!(state(&app), AppState::NewMap);
    }

    #[test]
    fn exit_requests_shutdown() {
        let mut app = test_app(AppState::MainMenu);
        click(&mut app, MenuButton::Exit);

        let exits = app.world().resource::<Messages<AppExit>>();
        assert!(!exits.is_empty(), "no AppExit was sent");
    }

    #[test]
    fn back_returns_to_the_main_menu() {
        let mut app = test_app(AppState::NewMap);
        click(&mut app, MenuButton::Back);
        assert_eq!(state(&app), AppState::MainMenu);
    }

    #[test]
    fn start_applies_the_chosen_size_and_seed() {
        let mut app = test_app(AppState::NewMap);

        click(&mut app, MenuButton::ChooseSize(256));
        press_key(&mut app, KeyCode::Digit7);
        press_key(&mut app, KeyCode::Digit7);
        app.world_mut().resource_mut::<NewMapSettings>().seed = "77".to_string();
        click(&mut app, MenuButton::Start);

        let config = app.world().resource::<MapConfig>();
        assert_eq!(config.chunks, UVec2::splat(2));
        assert_eq!(config.seed, 77);
        assert_eq!(state(&app), AppState::InWorld);
    }

    #[test]
    fn typing_edits_the_seed() {
        let mut app = test_app(AppState::NewMap);
        app.world_mut().resource_mut::<NewMapSettings>().seed = String::new();

        press_key(&mut app, KeyCode::Digit4);
        press_key(&mut app, KeyCode::Digit2);
        assert_eq!(app.world().resource::<NewMapSettings>().seed, "42");

        press_key(&mut app, KeyCode::Backspace);
        assert_eq!(app.world().resource::<NewMapSettings>().seed, "4");
    }

    #[test]
    fn seed_field_is_length_capped() {
        let mut app = test_app(AppState::NewMap);
        app.world_mut().resource_mut::<NewMapSettings>().seed = String::new();

        for _ in 0..MAX_SEED_DIGITS + 5 {
            press_key(&mut app, KeyCode::Digit9);
        }

        let seed = &app.world().resource::<NewMapSettings>().seed;
        assert_eq!(seed.len(), MAX_SEED_DIGITS);
        // Whatever the player types has to survive the trip into a u32.
        assert!(seed.parse::<u32>().is_ok(), "{seed} does not fit a u32");
    }

    #[test]
    fn escape_leaves_the_match() {
        let mut app = test_app(AppState::InWorld);
        press_key(&mut app, KeyCode::Escape);
        app.update();
        assert_eq!(state(&app), AppState::MainMenu);
    }

    #[test]
    fn seed_field_reads_as_zero_when_empty() {
        let settings = NewMapSettings {
            size: 96,
            seed: String::new(),
        };
        assert_eq!(settings.seed_value(), 0);
    }

    #[test]
    fn seed_field_parses_digits() {
        let settings = NewMapSettings {
            size: 96,
            seed: "123456".to_string(),
        };
        assert_eq!(settings.seed_value(), 123_456);
    }

    #[test]
    fn random_seeds_fit_the_field() {
        let seed = random_seed();
        assert!(seed.to_string().len() <= MAX_SEED_DIGITS);
    }

    #[test]
    fn defaults_match_the_map_config() {
        let settings = NewMapSettings::default();
        let config = MapConfig::default();
        assert_eq!(settings.size, config.tiles().x);
        assert_eq!(settings.seed_value(), config.seed);
    }

    #[test]
    fn size_presets_are_distinct_ordered_whole_chunks() {
        let sizes: Vec<u32> = SIZE_PRESETS.iter().map(|(_, size)| *size).collect();
        for size in &sizes {
            assert_eq!(size % CHUNK_TILES, 0, "{size} m is not a whole number of chunks");
        }
        let mut sorted = sizes.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sizes, sorted);
    }
}
