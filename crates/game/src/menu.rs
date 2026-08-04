//! Main menu, the map setup dialog and the controls screen.

use bevy::input::keyboard::KeyboardInput;
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::text::FontSize;

use crate::bindings::{is_bindable, typed_label, Action, KeyBindings};
use crate::terrain::MapConfig;
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
            // Shared with the camera, which registers them too — see
            // `MapCameraPlugin`.
            .init_resource::<KeyBindings>()
            .init_resource::<Rebinding>()
            .add_systems(OnEnter(AppState::MainMenu), spawn_main_menu)
            .add_systems(OnEnter(AppState::NewMap), spawn_new_map_dialog)
            .add_systems(OnEnter(AppState::Settings), spawn_settings)
            // Leaving the screen mid-capture would otherwise come back to it
            // still waiting for a key.
            .add_systems(OnExit(AppState::Settings), cancel_rebinding)
            .add_systems(
                Update,
                (
                    highlight_buttons,
                    main_menu_actions.run_if(in_state(AppState::MainMenu)),
                    (dialog_actions, type_seed, refresh_dialog).run_if(in_state(AppState::NewMap)),
                    // Ordered so that arming a row and reading the key meant for
                    // it can't land in the same frame. `settings_keys` carries
                    // no run condition of its own — see its comment.
                    (
                        settings_actions.run_if(in_state(AppState::Settings)),
                        settings_keys,
                        refresh_settings.run_if(in_state(AppState::Settings)),
                    )
                        .chain(),
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

/// The action whose new key the controls screen is waiting for, if any. Only
/// one row can be armed at a time — the next key pressed has to mean one thing.
#[derive(Resource, Default)]
struct Rebinding(Option<Action>);

#[derive(Component, Clone, Copy, PartialEq)]
enum MenuButton {
    NewMap,
    Settings,
    Exit,
    ChooseSize(u32),
    RandomSeed,
    Start,
    /// Arms this action's row, so the next key pressed becomes its key.
    Rebind(Action),
    ResetKeys,
    Back,
}

/// Marks the seed readout so it can be refreshed as the player types.
#[derive(Component)]
struct SeedText;

/// Marks the key readout on an action's row, so it can be refreshed when the
/// binding changes or the row starts waiting for a key.
#[derive(Component)]
struct KeyText(Action);

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
            spawn_button(screen, MenuButton::Settings, "Controls", 240.0);
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
            MenuButton::Settings => next.set(AppState::Settings),
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
                *config = MapConfig::square(settings.size, settings.seed_value());
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
// Controls
// ---------------------------------------------------------------------------

fn spawn_settings(mut commands: Commands, bindings: Res<KeyBindings>) {
    commands
        .spawn((
            Name::new("Controls screen"),
            DespawnOnExit(AppState::Settings),
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
                        row_gap: Val::Px(8.0),
                        ..default()
                    },
                    BackgroundColor(PANEL),
                    BorderColor::all(PANEL_EDGE),
                ))
                .with_children(|panel| {
                    panel.spawn((
                        Text::new("Controls"),
                        TextFont {
                            font_size: FontSize::Px(34.0),
                            ..default()
                        },
                        TextColor(TEXT),
                        Node {
                            margin: UiRect::bottom(Val::Px(6.0)),
                            ..default()
                        },
                    ));
                    label(panel, "click a key, then press the one you want");

                    panel
                        .spawn(Node {
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(6.0),
                            margin: UiRect::vertical(Val::Px(16.0)),
                            ..default()
                        })
                        .with_children(|rows| {
                            for action in Action::ALL {
                                spawn_key_row(rows, action, &bindings.name(action));
                            }
                        });

                    // Plain punctuation only: the default font has no dash of
                    // any kind and draws a missing glyph as an empty box.
                    label(panel, "the arrow keys always pan, and escape always");
                    label(panel, "leaves; neither can be reassigned");

                    panel
                        .spawn(Node {
                            column_gap: Val::Px(8.0),
                            margin: UiRect::top(Val::Px(24.0)),
                            ..default()
                        })
                        .with_children(|row| {
                            spawn_button(row, MenuButton::ResetKeys, "Defaults", 130.0);
                            spawn_button(row, MenuButton::Back, "Back", 130.0);
                        });
                });
        });
}

/// One action and the key it sits on, as a name on the left and a button on the
/// right that arms the row when clicked.
fn spawn_key_row(parent: &mut ChildSpawnerCommands, action: Action, key_name: &str) {
    parent
        .spawn(Node {
            width: Val::Px(400.0),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            ..default()
        })
        .with_children(|row| {
            row.spawn((
                Text::new(action.label()),
                TextFont {
                    font_size: FontSize::Px(18.0),
                    ..default()
                },
                TextColor(TEXT),
            ));
            row.spawn(button(MenuButton::Rebind(action), 170.0))
                .with_children(|button| {
                    button.spawn((KeyText(action), button_label(key_name)));
                });
        });
}

fn settings_actions(
    buttons: Query<(&Interaction, &MenuButton), Changed<Interaction>>,
    mut rebinding: ResMut<Rebinding>,
    mut bindings: ResMut<KeyBindings>,
    mut next: ResMut<NextState<AppState>>,
) {
    for (interaction, button) in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match button {
            MenuButton::Rebind(action) => rebinding.0 = Some(*action),
            MenuButton::ResetKeys => {
                *bindings = KeyBindings::default();
                rebinding.0 = None;
            }
            MenuButton::Back => next.set(AppState::MainMenu),
            _ => {}
        }
    }
}

/// What the keyboard does on the controls screen: fills in an armed row, or —
/// with nothing armed — leaves, the same as Back.
///
/// Runs on every screen rather than only this one so that its cursor into the
/// keypress stream always advances. Left to lag, it would deliver whatever was
/// pressed on the way here the instant the screen opened.
fn settings_keys(
    state: Res<State<AppState>>,
    mut presses: MessageReader<KeyboardInput>,
    mut rebinding: ResMut<Rebinding>,
    mut bindings: ResMut<KeyBindings>,
    mut next: ResMut<NextState<AppState>>,
) {
    let on_screen = *state.get() == AppState::Settings;

    for press in presses.read() {
        // A key held down repeats; the first press is the one that counts.
        if !on_screen || press.state != ButtonState::Pressed || press.repeat {
            continue;
        }

        let Some(action) = rebinding.0 else {
            if press.key_code == KeyCode::Escape {
                next.set(AppState::MainMenu);
            }
            continue;
        };

        match press.key_code {
            // Escape backs out of the capture rather than becoming the new key.
            // Somebody who armed a row by accident needs a way out, and it has
            // to be a key that can never itself have been reassigned.
            KeyCode::Escape => rebinding.0 = None,
            // A reserved key leaves the row armed and waiting, which reads as
            // "not that one" without having to say so.
            key if !is_bindable(key) => {}
            key => {
                // The label comes from this very keypress, so it is what this
                // keyboard types rather than what a US one would have.
                bindings.bind(action, key, typed_label(&press.logical_key));
                rebinding.0 = None;
            }
        }
    }
}

/// Keeps the rows in step with the bindings behind them.
fn refresh_settings(
    bindings: Res<KeyBindings>,
    rebinding: Res<Rebinding>,
    mut keys: Query<(&KeyText, &mut Text)>,
    mut buttons: Query<(&MenuButton, &mut BackgroundColor, &Interaction)>,
) {
    if !bindings.is_changed() && !rebinding.is_changed() {
        return;
    }

    for (key, mut text) in &mut keys {
        text.0 = if rebinding.0 == Some(key.0) {
            "press a key".to_string()
        } else {
            bindings.name(key.0)
        };
    }

    // The armed row stays lit, so it's clear which key is about to change.
    for (button, mut color, interaction) in &mut buttons {
        if let MenuButton::Rebind(action) = button {
            if *interaction == Interaction::None {
                *color = BackgroundColor(if rebinding.0 == Some(*action) {
                    BUTTON_ON
                } else {
                    BUTTON
                });
            }
        }
    }
}

fn cancel_rebinding(mut rebinding: ResMut<Rebinding>) {
    rebinding.0 = None;
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

/// A menu button without its label, so that callers who need to mark the label
/// — as the controls screen does, to rewrite it later — can spawn their own.
fn button(action: MenuButton, width: f32) -> impl Bundle {
    (
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
    )
}

fn button_label(text: &str) -> impl Bundle {
    (
        Text::new(text),
        TextFont {
            font_size: FontSize::Px(19.0),
            ..default()
        },
        TextColor(TEXT),
        TextLayout::justify(Justify::Center),
    )
}

fn spawn_button(parent: &mut ChildSpawnerCommands, action: MenuButton, text: &str, width: f32) {
    parent.spawn(button(action, width)).with_children(|button| {
        button.spawn(button_label(text));
    });
}

fn highlight_buttons(
    settings: Res<NewMapSettings>,
    rebinding: Res<Rebinding>,
    mut buttons: Query<(&Interaction, &MenuButton, &mut BackgroundColor), Changed<Interaction>>,
) {
    for (interaction, button, mut color) in &mut buttons {
        let idle = match button {
            MenuButton::ChooseSize(size) if *size == settings.size => BUTTON_ON,
            MenuButton::Rebind(action) if rebinding.0 == Some(*action) => BUTTON_ON,
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
    use crate::terrain::CHUNK_TILES;
    use bevy::input::keyboard::Key;
    use bevy::state::app::StatesPlugin;

    /// A headless app running the menu systems, with no renderer attached.
    fn test_app(state: AppState) -> App {
        let mut app = App::new();
        app.add_plugins((StatesPlugin, MenuPlugin))
            .insert_state(state)
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<MapConfig>()
            .add_message::<AppExit>()
            .add_message::<KeyboardInput>();
        app.update();
        app
    }

    /// Sends the keypress the controls screen reads: a real one carries both
    /// the position pressed and what that position typed, and the screen wants
    /// each for a different purpose.
    fn type_key(app: &mut App, key: KeyCode, typed: &str) {
        app.world_mut().write_message(KeyboardInput {
            key_code: key,
            logical_key: Key::Character(typed.into()),
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
        app.update();
    }

    fn bindings(app: &App) -> &KeyBindings {
        app.world().resource::<KeyBindings>()
    }

    fn waiting_on(app: &App) -> Option<Action> {
        app.world().resource::<Rebinding>().0
    }

    /// Moves between screens the way clicking would, so `OnEnter` and `OnExit`
    /// run.
    fn go_to(app: &mut App, state: AppState) {
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(state);
        app.update();
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
    fn controls_opens_the_settings_screen() {
        let mut app = test_app(AppState::MainMenu);
        click(&mut app, MenuButton::Settings);
        assert_eq!(state(&app), AppState::Settings);
    }

    #[test]
    fn a_row_waits_for_a_key_and_then_takes_it() {
        let mut app = test_app(AppState::Settings);

        click(&mut app, MenuButton::Rebind(Action::PanForward));
        assert_eq!(waiting_on(&app), Some(Action::PanForward));

        type_key(&mut app, KeyCode::KeyJ, "j");
        assert_eq!(bindings(&app).key(Action::PanForward), KeyCode::KeyJ);
        assert_eq!(waiting_on(&app), None, "the row is still waiting");
    }

    #[test]
    fn a_key_is_named_by_what_it_typed_not_where_it_sits() {
        let mut app = test_app(AppState::Settings);

        // A Dvorak keyboard pressing the key marked "," reports the position
        // where a US keyboard keeps W. The row has to say what the keycap says.
        click(&mut app, MenuButton::Rebind(Action::TurnLeft));
        type_key(&mut app, KeyCode::KeyW, ",");

        assert_eq!(bindings(&app).key(Action::TurnLeft), KeyCode::KeyW);
        assert_eq!(bindings(&app).name(Action::TurnLeft), ",");
    }

    /// What an action's row currently reads on the right-hand button.
    fn row_text(app: &mut App, action: Action) -> String {
        app.world_mut()
            .query::<(&KeyText, &Text)>()
            .iter(app.world())
            .find(|(key, _)| key.0 == action)
            .map(|(_, text)| text.0.clone())
            .expect("no row for the action")
    }

    #[test]
    fn an_armed_row_says_it_is_waiting_and_then_shows_the_new_key() {
        let mut app = test_app(AppState::MainMenu);
        go_to(&mut app, AppState::Settings);
        assert_eq!(row_text(&mut app, Action::PanLeft), "A");

        click(&mut app, MenuButton::Rebind(Action::PanLeft));
        assert_eq!(row_text(&mut app, Action::PanLeft), "press a key");

        type_key(&mut app, KeyCode::KeyH, "h");
        app.update();
        assert_eq!(row_text(&mut app, Action::PanLeft), "H");
    }

    #[test]
    fn escape_abandons_a_capture_and_changes_nothing() {
        let mut app = test_app(AppState::Settings);
        let before = bindings(&app).clone();

        click(&mut app, MenuButton::Rebind(Action::PanBack));
        type_key(&mut app, KeyCode::Escape, "\u{1b}");

        assert_eq!(waiting_on(&app), None);
        assert_eq!(bindings(&app), &before);
        // And having cancelled, we're still on the screen rather than back out.
        assert_eq!(state(&app), AppState::Settings);
    }

    #[test]
    fn a_reserved_key_is_refused_and_the_row_keeps_waiting() {
        let mut app = test_app(AppState::Settings);

        click(&mut app, MenuButton::Rebind(Action::PanBack));
        type_key(&mut app, KeyCode::ArrowUp, "");

        assert_eq!(bindings(&app).key(Action::PanBack), KeyCode::KeyS);
        assert_eq!(
            waiting_on(&app),
            Some(Action::PanBack),
            "a refused key should leave the row armed"
        );

        // And a real key still lands afterwards.
        type_key(&mut app, KeyCode::KeyN, "n");
        assert_eq!(bindings(&app).key(Action::PanBack), KeyCode::KeyN);
    }

    #[test]
    fn taking_a_key_another_action_had_trades_the_two() {
        let mut app = test_app(AppState::Settings);

        click(&mut app, MenuButton::Rebind(Action::PanForward));
        type_key(&mut app, KeyCode::KeyE, "e");

        assert_eq!(bindings(&app).key(Action::PanForward), KeyCode::KeyE);
        assert_eq!(
            bindings(&app).key(Action::TurnRight),
            KeyCode::KeyW,
            "turning right should have taken the key panning gave up"
        );
    }

    #[test]
    fn defaults_puts_every_key_back() {
        let mut app = test_app(AppState::Settings);

        click(&mut app, MenuButton::Rebind(Action::PanLeft));
        type_key(&mut app, KeyCode::KeyZ, "z");
        click(&mut app, MenuButton::ResetKeys);

        assert_eq!(bindings(&app), &KeyBindings::default());
        assert_eq!(waiting_on(&app), None);
    }

    #[test]
    fn escape_leaves_the_controls_screen_when_no_row_is_waiting() {
        let mut app = test_app(AppState::Settings);
        type_key(&mut app, KeyCode::Escape, "\u{1b}");
        app.update();
        assert_eq!(state(&app), AppState::MainMenu);
    }

    #[test]
    fn leaving_the_screen_forgets_a_waiting_row() {
        let mut app = test_app(AppState::Settings);

        click(&mut app, MenuButton::Rebind(Action::TurnRight));
        click(&mut app, MenuButton::Back);
        assert_eq!(state(&app), AppState::MainMenu);

        go_to(&mut app, AppState::Settings);
        assert_eq!(
            waiting_on(&app),
            None,
            "the screen came back still waiting for a key"
        );
    }

    #[test]
    fn keys_pressed_before_the_screen_opened_are_not_taken() {
        // Escape leaves a match, so the key that gets you to the menu is one
        // that was pressed moments before the controls screen can open. None of
        // that backlog may count as an answer to "press a key".
        let mut app = test_app(AppState::InWorld);
        type_key(&mut app, KeyCode::KeyJ, "j");

        go_to(&mut app, AppState::Settings);
        click(&mut app, MenuButton::Rebind(Action::PanForward));

        assert_eq!(waiting_on(&app), Some(Action::PanForward));
        assert_eq!(bindings(&app).key(Action::PanForward), KeyCode::KeyW);
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
            assert_eq!(
                size % CHUNK_TILES,
                0,
                "{size} m is not a whole number of chunks"
            );
        }
        let mut sorted = sizes.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sizes, sorted);
    }
}
