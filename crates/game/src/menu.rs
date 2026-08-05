//! Main menu, the new-world dialog, the join screen and the controls screen.

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::text::FontSize;

use protocol::DEFAULT_PORT;

use crate::bindings::{is_bindable, typed_label, Action, KeyBindings};
use crate::camera::View;
use crate::net::{Dialing, Hosting, Online};
use crate::terrain::{Archipelago, WorldConfig};
use crate::AppState;

/// Longest seed the user can type. Keeps it inside a u32.
const MAX_SEED_DIGITS: usize = 9;

/// Longest address the join screen will take. Room for a fully qualified name
/// and a port, well past anything anybody types.
const MAX_ADDRESS: usize = 60;

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
        app.init_resource::<NewWorldSettings>()
            .init_resource::<JoinSettings>()
            .init_resource::<Status>()
            // Shared with the camera, which registers them too — see
            // `MapCameraPlugin`.
            .init_resource::<KeyBindings>()
            .init_resource::<Rebinding>()
            .add_systems(OnEnter(AppState::MainMenu), spawn_main_menu)
            .add_systems(OnEnter(AppState::Settings), spawn_settings)
            // Cleared before the screen is built, so that a screen only ever
            // shows what this visit to it has had to say.
            .add_systems(
                OnEnter(AppState::NewWorld),
                (clear_status, spawn_new_world_dialog).chain(),
            )
            .add_systems(
                OnEnter(AppState::JoinWorld),
                (clear_status, spawn_join_dialog).chain(),
            )
            // Leaving a screen abandons whatever it had in the air: the dial
            // lands on a dropped receiver, and the session it was carrying —
            // with the hosted world behind it, if there was one — is dropped
            // with it. Left running, it would arrive in the middle of some
            // other screen and drag the player into a world they had already
            // walked away from.
            .add_systems(OnExit(AppState::NewWorld), stop_dialing)
            .add_systems(OnExit(AppState::JoinWorld), stop_dialing)
            // Leaving the screen mid-capture would otherwise come back to it
            // still waiting for a key.
            .add_systems(OnExit(AppState::Settings), cancel_rebinding)
            .add_systems(
                Update,
                (
                    highlight_buttons,
                    settle_dialing.run_if(resource_exists::<Dialing>),
                    main_menu_actions.run_if(in_state(AppState::MainMenu)),
                    (dialog_actions, share_world, type_seed, refresh_dialog)
                        .run_if(in_state(AppState::NewWorld)),
                    // `join_keys` carries no run condition of its own, for the
                    // reason `settings_keys` below carries none.
                    (
                        join_actions.run_if(in_state(AppState::JoinWorld)),
                        join_keys,
                    )
                        .chain(),
                    refresh_join.run_if(in_state(AppState::JoinWorld)),
                    // Both dialogs have a line for how a dial is going, so
                    // whichever is on screen owns the only ones that exist.
                    refresh_status,
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

/// What the new-world dialog is currently showing: which world to enter, and
/// whether anyone else may come. The seed is the whole of what a world *is*,
/// so it is nearly the whole of the dialog too.
#[derive(Resource)]
struct NewWorldSettings {
    /// Held as text so the field can be edited a digit at a time, including
    /// being temporarily empty.
    seed: String,
    /// Whether to host the world rather than keep it to ourselves. A shared
    /// world is a served one, joined over the loopback like any other — see
    /// [`Dialing::hosting`] — so turning this on is the whole difference
    /// between playing alone and being somebody's server.
    share: bool,
}

impl Default for NewWorldSettings {
    fn default() -> Self {
        Self {
            seed: WorldConfig::default().seed.to_string(),
            share: false,
        }
    }
}

impl NewWorldSettings {
    fn seed_value(&self) -> u32 {
        self.seed.parse().unwrap_or(0)
    }
}

/// What the join screen is currently showing.
#[derive(Resource)]
struct JoinSettings {
    /// A server as `host` or `host:port`, exactly as `--join` takes it.
    address: String,
}

impl Default for JoinSettings {
    fn default() -> Self {
        // The machine you are sitting at, which is both the likeliest thing to
        // be hosting while a world is being set up and a sensible thing to
        // have to edit rather than type from nothing.
        Self {
            address: "localhost".to_string(),
        }
    }
}

/// How a dial is going, for the screen that started it: empty when there is
/// nothing to say, otherwise a line of prose the player reads and acts on.
#[derive(Resource, Default)]
struct Status(String);

/// The action whose new key the controls screen is waiting for, if any. Only
/// one row can be armed at a time — the next key pressed has to mean one thing.
#[derive(Resource, Default)]
struct Rebinding(Option<Action>);

#[derive(Component, Clone, Copy, PartialEq)]
enum MenuButton {
    NewWorld,
    JoinWorld,
    Settings,
    Exit,
    RandomSeed,
    /// Turns sharing the world about to be started on and off.
    ToggleShare,
    Start,
    /// Dials the address on the join screen.
    Connect,
    /// Arms this action's row, so the next key pressed becomes its key.
    Rebind(Action),
    ResetKeys,
    Back,
}

/// Marks the seed readout so it can be refreshed as the player types.
#[derive(Component)]
struct SeedText;

/// Marks the sharing button's label, so it can say which way it is set.
#[derive(Component)]
struct ShareText;

/// Marks the address readout on the join screen.
#[derive(Component)]
struct AddressText;

/// Marks the line a screen reports a dial on.
#[derive(Component)]
struct StatusText;

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
                Text::new("GENOVESA"),
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
                Text::new("an ocean of islands in flat colours"),
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

            spawn_button(screen, MenuButton::NewWorld, "New World", 240.0);
            spawn_button(screen, MenuButton::JoinWorld, "Join World", 240.0);
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
            MenuButton::NewWorld => next.set(AppState::NewWorld),
            MenuButton::JoinWorld => next.set(AppState::JoinWorld),
            MenuButton::Settings => next.set(AppState::Settings),
            MenuButton::Exit => {
                exit.write(AppExit::Success);
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// New world dialog
// ---------------------------------------------------------------------------

fn spawn_new_world_dialog(mut commands: Commands, settings: Res<NewWorldSettings>) {
    commands
        .spawn((
            Name::new("New world dialog"),
            DespawnOnExit(AppState::NewWorld),
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
                        Text::new("New World"),
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

                    // Sharing. A world of one's own needs no server at all, so
                    // this is the switch between playing alone and hosting.
                    // No label of its own, unlike the seed above: a switch that
                    // says which way it is set has already said what it is.
                    panel
                        .spawn(Node {
                            margin: UiRect::top(Val::Px(20.0)),
                            ..default()
                        })
                        .with_children(|row| {
                            row.spawn(button(MenuButton::ToggleShare, 160.0))
                                .with_children(|button| {
                                    button.spawn((
                                        ShareText,
                                        button_label(share_label(settings.share)),
                                    ));
                                });
                        });
                    // Phrased as what the switch does rather than as what is
                    // happening, since it is read in both positions.
                    label(
                        panel,
                        &format!("sharing hosts the world on port {DEFAULT_PORT}"),
                    );

                    status_line(panel);
                    panel
                        .spawn(Node {
                            column_gap: Val::Px(8.0),
                            margin: UiRect::top(Val::Px(8.0)),
                            ..default()
                        })
                        .with_children(|row| {
                            spawn_button(row, MenuButton::Back, "Back", 130.0);
                            spawn_button(row, MenuButton::Start, "Start", 130.0);
                        });
                });
        });
}

/// What the sharing button reads. A switch has to say which way it is set,
/// not what pressing it would do.
fn share_label(share: bool) -> &'static str {
    if share {
        "Share: on"
    } else {
        "Share: off"
    }
}

/// The dialog's buttons, and starting a world of one's own. Starting a shared
/// one is [`share_world`], which is a different enough thing to be a different
/// system: a world nobody else can reach is entered here and now, and a shared
/// one is not entered until a server has answered for it.
fn dialog_actions(
    buttons: Query<(&Interaction, &MenuButton), Changed<Interaction>>,
    mut settings: ResMut<NewWorldSettings>,
    mut config: ResMut<WorldConfig>,
    mut view: ResMut<View>,
    mut next: ResMut<NextState<AppState>>,
) {
    for (interaction, button) in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match button {
            MenuButton::RandomSeed => settings.seed = random_seed().to_string(),
            MenuButton::ToggleShare => settings.share = !settings.share,
            MenuButton::Back => next.set(AppState::MainMenu),
            MenuButton::Start if settings.share => {}
            MenuButton::Start => {
                *config = WorldConfig {
                    seed: settings.seed_value(),
                };
                // Start the match on land. The world is an endless ocean and
                // the view carries over from wherever it last was — which on a
                // fresh run is the origin, open water on essentially every
                // seed, and on a new world chosen from within a match is a
                // point in a world that no longer exists. Either way the
                // player would be dropped on a blank blue plane with no way of
                // knowing which way to sail.
                //
                // Measured from the view's *current* focus rather than from
                // the origin, so that coming back to the dialog and starting
                // the same seed again lands where the player was rather than
                // hauling them back across the ocean. Layout only, so it
                // generates nothing and costs the frame a few hash mixes.
                let world = Archipelago::new(&config);
                let here = Vec2::new(view.focus.x, view.focus.z);
                if let Some(island) = world.nearest_island(here) {
                    let centre = island.centre();
                    view.focus = Vec3::new(centre.x, 0.0, centre.y);
                }
                next.set(AppState::InWorld);
            }
            _ => {}
        }
    }
}

/// Starting a world with the sharing switch on, which means hosting it.
///
/// The way in is then the way into anybody else's: dial it and wait.
/// [`settle_dialing`] takes the seed and the spawn point from the welcome,
/// exactly as a run started with `--join` does, so nothing about the world is
/// settled here — not even the seed this very machine is about to serve.
fn share_world(
    mut commands: Commands,
    buttons: Query<(&Interaction, &MenuButton), Changed<Interaction>>,
    dialing: Option<Res<Dialing>>,
    settings: Res<NewWorldSettings>,
    mut status: ResMut<Status>,
) {
    // A server already being started. Asking for a second would only fail on
    // the port the first one is holding.
    if !settings.share || dialing.is_some() {
        return;
    }

    for (interaction, button) in &buttons {
        if *interaction == Interaction::Pressed && *button == MenuButton::Start {
            status.0 = "opening the world...".to_string();
            commands.insert_resource(Dialing::hosting(
                WorldConfig {
                    seed: settings.seed_value(),
                },
                DEFAULT_PORT,
            ));
            return;
        }
    }
}

/// Digit-by-digit editing of the seed field.
fn type_seed(keys: Res<ButtonInput<KeyCode>>, mut settings: ResMut<NewWorldSettings>) {
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
    settings: Res<NewWorldSettings>,
    mut seed_text: Query<&mut Text, (With<SeedText>, Without<ShareText>)>,
    mut share_text: Query<&mut Text, With<ShareText>>,
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
    for mut text in &mut share_text {
        text.0 = share_label(settings.share).to_string();
    }
}

// ---------------------------------------------------------------------------
// Join screen
// ---------------------------------------------------------------------------

fn spawn_join_dialog(mut commands: Commands, settings: Res<JoinSettings>) {
    commands
        .spawn((
            Name::new("Join world dialog"),
            DespawnOnExit(AppState::JoinWorld),
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
                        Text::new("Join World"),
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

                    label(panel, "Server");
                    panel.spawn((
                        AddressText,
                        Text::new(settings.address.clone()),
                        TextFont {
                            font_size: FontSize::Px(26.0),
                            ..default()
                        },
                        TextColor(TEXT),
                    ));
                    label(panel, "type an address, backspace to edit");
                    label(panel, &format!("a bare name joins on port {DEFAULT_PORT}"));

                    status_line(panel);
                    panel
                        .spawn(Node {
                            column_gap: Val::Px(8.0),
                            margin: UiRect::top(Val::Px(8.0)),
                            ..default()
                        })
                        .with_children(|row| {
                            spawn_button(row, MenuButton::Back, "Back", 130.0);
                            spawn_button(row, MenuButton::Connect, "Join", 130.0);
                        });
                });
        });
}

fn join_actions(
    mut commands: Commands,
    buttons: Query<(&Interaction, &MenuButton), Changed<Interaction>>,
    dialing: Option<Res<Dialing>>,
    settings: Res<JoinSettings>,
    mut status: ResMut<Status>,
    mut next: ResMut<NextState<AppState>>,
) {
    for (interaction, button) in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match button {
            MenuButton::Back => next.set(AppState::MainMenu),
            // Already ringing. A second dial would leave the first to arrive
            // in nobody's hands.
            MenuButton::Connect if dialing.is_some() => {}
            MenuButton::Connect => dial(&mut commands, &mut status, &settings.address),
            _ => {}
        }
    }
}

/// Typing an address, and the two keys the screen answers to: enter joins,
/// escape leaves.
///
/// Runs on every screen rather than only this one, for the reason
/// [`settings_keys`] does: a reader left to lag would deliver whatever was
/// pressed on the way here the instant the screen opened — and on this screen
/// that would be typed into the address.
fn join_keys(
    mut commands: Commands,
    state: Res<State<AppState>>,
    mut presses: MessageReader<KeyboardInput>,
    dialing: Option<Res<Dialing>>,
    mut settings: ResMut<JoinSettings>,
    mut status: ResMut<Status>,
    mut next: ResMut<NextState<AppState>>,
) {
    let on_screen = *state.get() == AppState::JoinWorld;

    for press in presses.read() {
        // A held key repeats, which is what a text field wants: holding
        // backspace should clear the address rather than one character of it.
        if !on_screen || press.state != ButtonState::Pressed {
            continue;
        }

        match press.key_code {
            KeyCode::Escape => next.set(AppState::MainMenu),
            KeyCode::Backspace => {
                settings.address.pop();
            }
            KeyCode::Enter | KeyCode::NumpadEnter => {
                if dialing.is_none() {
                    dial(&mut commands, &mut status, &settings.address);
                }
            }
            // Whatever this keyboard types, rather than what a US one would
            // have typed at the same position — the same reason the controls
            // screen names keys by [`typed_label`].
            _ => {
                if let Key::Character(typed) = &press.logical_key {
                    for character in typed.chars().filter(|c| is_address_character(*c)) {
                        if settings.address.len() < MAX_ADDRESS {
                            settings.address.push(character);
                        }
                    }
                }
            }
        }
    }
}

/// Whether a character belongs in an address. Names, numbers and ports, and
/// the brackets an IPv6 address wears when it carries one — everything else a
/// keyboard can produce would only make a name that cannot resolve.
fn is_address_character(c: char) -> bool {
    c.is_ascii_alphanumeric() || ".:-_[]".contains(c)
}

/// Starts dialling, and says so. Shared by the button and the enter key, which
/// have to mean exactly the same thing.
fn dial(commands: &mut Commands, status: &mut Status, address: &str) {
    status.0 = format!("joining {address}...");
    commands.insert_resource(Dialing::to(address));
}

/// Keeps the address readout in step with what has been typed.
fn refresh_join(settings: Res<JoinSettings>, mut address: Query<&mut Text, With<AddressText>>) {
    if !settings.is_changed() {
        return;
    }
    for mut text in &mut address {
        // An empty field would render as nothing at all, and a field that
        // looks like no field is one nobody can tell they are typing into.
        text.0 = if settings.address.is_empty() {
            "_".to_string()
        } else {
            settings.address.clone()
        };
    }
}

// ---------------------------------------------------------------------------
// Getting into a served world
// ---------------------------------------------------------------------------

/// Watches the dial the screen started, and enters the world when it lands.
///
/// This is the whole of what hosting and joining have in common, which is
/// nearly all of it: by the time a welcome has arrived, a world of one's own
/// and somebody else's are the same thing — a seed to generate and a point to
/// stand at, both of them the server's to say.
fn settle_dialing(
    mut commands: Commands,
    dialing: Res<Dialing>,
    mut status: ResMut<Status>,
    mut config: ResMut<WorldConfig>,
    mut view: ResMut<View>,
    mut next: ResMut<NextState<AppState>>,
) {
    let Some(outcome) = dialing.outcome() else {
        return;
    };
    // Answered, one way or the other. Taken off the app either way: an outcome
    // is only given up once, so a dial that has landed has nothing left to say.
    commands.remove_resource::<Dialing>();

    let session = match outcome {
        // Which leaves the player on the screen they started from, with the
        // address or the port still in front of them to correct.
        Err(problem) => {
            status.0 = problem;
            return;
        }
        Ok(session) => session,
    };

    *config = WorldConfig {
        seed: session.connection.seed,
    };
    // Where the server puts arrivals down, which on a hosted world is the
    // island a lone run of the same seed would have opened on. No island snap
    // of our own: everybody in a session has to enter it in the same place.
    let spawn = session.connection.spawn;
    view.focus = Vec3::new(spawn.x, 0.0, spawn.y);

    if let Some(host) = session.hosting {
        commands.insert_resource(Hosting(host));
    }
    commands.insert_resource(Online::new(session.connection));
    next.set(AppState::InWorld);
}

/// Abandons a dial still in the air. See where this is registered for why.
fn stop_dialing(mut commands: Commands) {
    commands.remove_resource::<Dialing>();
}

fn clear_status(mut status: ResMut<Status>) {
    status.0.clear();
}

/// Keeps the reporting line in step with what there is to report.
fn refresh_status(status: Res<Status>, mut lines: Query<&mut Text, With<StatusText>>) {
    if !status.is_changed() {
        return;
    }
    for mut text in &mut lines {
        text.0 = status.0.clone();
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

/// The line a dialog reports a dial on. Spawned empty and left that way until
/// there is something to say, but spawned all the same: a line that appeared
/// only when it had text would push the buttons under it down the moment the
/// player pressed one.
fn status_line(parent: &mut ChildSpawnerCommands) {
    parent.spawn((
        StatusText,
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(15.0),
            ..default()
        },
        TextColor(TEXT),
        Node {
            margin: UiRect::top(Val::Px(8.0)),
            // Held open so an empty line still takes its room, for the reason
            // above.
            height: Val::Px(18.0),
            ..default()
        },
    ));
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
    rebinding: Res<Rebinding>,
    mut buttons: Query<(&Interaction, &MenuButton, &mut BackgroundColor), Changed<Interaction>>,
) {
    for (interaction, button, mut color) in &mut buttons {
        let idle = match button {
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
    use std::net::TcpListener;
    use std::thread;
    use std::time::{Duration, Instant};

    use bevy::input::keyboard::Key;
    use bevy::state::app::StatesPlugin;

    use super::*;
    use crate::net::fake_server;

    /// A host that accepts a connection and then says nothing — a dial that
    /// stays in the air for as long as the test needs it to. Its listener is
    /// handed back so the test decides when it stops existing.
    fn silent_server() -> (TcpListener, String) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("addr").to_string();
        (listener, address)
    }

    /// Runs frames until the condition holds. Dials cross real sockets and a
    /// thread, so a frame or two of patience is legitimate — five seconds of
    /// it is a failure.
    fn run_until(app: &mut App, what: &str, mut done: impl FnMut(&App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            app.update();
            if done(app) {
                return;
            }
            thread::sleep(Duration::from_millis(2));
        }
        panic!("timed out waiting until {what}");
    }

    /// A headless app running the menu systems, with no renderer attached.
    fn test_app(state: AppState) -> App {
        let mut app = App::new();
        app.add_plugins((StatesPlugin, MenuPlugin))
            .insert_state(state)
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<WorldConfig>()
            // Normally the camera plugin's, but the new-world dialog moves the
            // focus onto land when a world starts — see `dialog_actions`.
            .init_resource::<View>()
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
        click_once(app, button);
        app.update();
    }

    /// A click and the one frame it takes to be seen, with none of the frames
    /// in which something the click started could land. What the button *did*
    /// is visible; what may come of it later is not.
    fn click_once(app: &mut App, button: MenuButton) {
        app.world_mut().spawn((button, Interaction::Pressed));
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
    fn new_world_opens_the_setup_dialog() {
        let mut app = test_app(AppState::MainMenu);
        click(&mut app, MenuButton::NewWorld);
        assert_eq!(state(&app), AppState::NewWorld);
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
        let mut app = test_app(AppState::NewWorld);
        click(&mut app, MenuButton::Back);
        assert_eq!(state(&app), AppState::MainMenu);
    }

    #[test]
    fn start_applies_the_chosen_seed() {
        let mut app = test_app(AppState::NewWorld);

        app.world_mut().resource_mut::<NewWorldSettings>().seed = "77".to_string();
        click(&mut app, MenuButton::Start);

        let config = app.world().resource::<WorldConfig>();
        assert_eq!(config.seed, 77);
        assert_eq!(state(&app), AppState::InWorld);
    }

    #[test]
    fn start_puts_the_view_on_land() {
        // The origin is open ocean on essentially every seed, so entering a
        // world from the dialog has to move the view onto the nearest island —
        // otherwise the match opens on a blank blue plane.
        let mut app = test_app(AppState::NewWorld);

        app.world_mut().resource_mut::<NewWorldSettings>().seed = "77".to_string();
        click(&mut app, MenuButton::Start);

        let focus = app.world().resource::<View>().focus;
        let island = Archipelago::new(&WorldConfig { seed: 77 })
            .nearest_island(Vec2::ZERO)
            .expect("seed 77 should have an island near the origin");

        assert_ne!(focus, Vec3::ZERO, "the match still starts on water");
        let out = (Vec2::new(focus.x, focus.z) - island.centre()).abs() - island.extent() * 0.5;
        assert!(
            out.max_element() <= 0.0,
            "{focus:?} is outside the nearest island's frame"
        );
    }

    #[test]
    fn join_world_opens_the_join_screen() {
        let mut app = test_app(AppState::MainMenu);
        click(&mut app, MenuButton::JoinWorld);
        assert_eq!(state(&app), AppState::JoinWorld);
    }

    #[test]
    fn sharing_is_off_until_it_is_asked_for() {
        let mut app = test_app(AppState::NewWorld);
        assert!(!app.world().resource::<NewWorldSettings>().share);

        click(&mut app, MenuButton::ToggleShare);
        assert!(app.world().resource::<NewWorldSettings>().share);
        click(&mut app, MenuButton::ToggleShare);
        assert!(!app.world().resource::<NewWorldSettings>().share);
    }

    #[test]
    fn an_unshared_world_needs_no_server_at_all() {
        // The point of the switch: a world of one's own is entered outright,
        // with nothing dialled and nothing listening.
        let mut app = test_app(AppState::NewWorld);
        click(&mut app, MenuButton::Start);

        assert_eq!(state(&app), AppState::InWorld);
        assert!(!app.world().contains_resource::<Dialing>());
        assert!(!app.world().contains_resource::<Online>());
    }

    #[test]
    fn a_shared_world_waits_on_the_server_it_starts() {
        // Started, not entered: a shared world is a served one, so the player
        // stays on the dialog until the welcome comes back — which is where
        // `settle_dialing` takes over, tested below against a server this test
        // file can name.
        //
        // Looked at after a single frame, before anything can have come of the
        // dial. What the well-known port does when it is asked for is not this
        // test's business — and on the machine a test runs on it may well
        // already be somebody's world.
        let mut app = test_app(AppState::NewWorld);
        click(&mut app, MenuButton::ToggleShare);
        click_once(&mut app, MenuButton::Start);

        assert_eq!(state(&app), AppState::NewWorld);
        assert!(
            app.world().contains_resource::<Dialing>(),
            "sharing a world started no server"
        );
        // And the world is left entirely to the welcome, seed included.
        assert_eq!(
            app.world().resource::<WorldConfig>().seed,
            WorldConfig::default().seed
        );
    }

    #[test]
    fn a_dial_that_lands_enters_the_served_world() {
        let (address, _socket) = fake_server(77, Vec2::new(100.0, -200.0));
        let mut app = test_app(AppState::JoinWorld);
        app.world_mut().resource_mut::<JoinSettings>().address = address;
        click(&mut app, MenuButton::Connect);

        run_until(&mut app, "the world is entered", |app| {
            *app.world().resource::<State<AppState>>().get() == AppState::InWorld
        });

        // The world is the server's, whole: its seed, and its idea of where we
        // are standing in it.
        assert_eq!(app.world().resource::<WorldConfig>().seed, 77);
        assert_eq!(
            app.world().resource::<View>().focus,
            Vec3::new(100.0, 0.0, -200.0)
        );
        assert!(app.world().contains_resource::<Online>());
        assert!(
            !app.world().contains_resource::<Dialing>(),
            "a dial that landed is still on the app"
        );
    }

    #[test]
    fn a_dial_that_fails_says_so_and_stays_put() {
        let mut app = test_app(AppState::JoinWorld);
        // Port 1, where nothing has ever listened.
        app.world_mut().resource_mut::<JoinSettings>().address = "127.0.0.1:1".to_string();
        click(&mut app, MenuButton::Connect);

        run_until(&mut app, "the failure is reported", |app| {
            !app.world().contains_resource::<Dialing>()
        });
        assert_eq!(state(&app), AppState::JoinWorld);
        assert!(
            app.world().resource::<Status>().0.contains("127.0.0.1:1"),
            "the screen does not say what went wrong: {:?}",
            app.world().resource::<Status>().0
        );
    }

    #[test]
    fn leaving_a_screen_abandons_the_dial_it_started() {
        // Otherwise a server started here, and given up on, would go on
        // holding the port for the life of the process — and a late welcome
        // would drag the player into a world they had walked away from.
        //
        // Dialled at a host that accepts and then says nothing, so the dial is
        // still in the air when the player gives up on it: one that had
        // already landed would be testing a different moment.
        let (silent, address) = silent_server();
        let mut app = test_app(AppState::JoinWorld);
        app.world_mut().resource_mut::<JoinSettings>().address = address;
        click(&mut app, MenuButton::Connect);
        assert!(app.world().contains_resource::<Dialing>());

        click(&mut app, MenuButton::Back);
        assert_eq!(state(&app), AppState::MainMenu);
        assert!(!app.world().contains_resource::<Dialing>());

        // And it stays abandoned, however the far end comes to life.
        drop(silent);
        for _ in 0..20 {
            app.update();
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(state(&app), AppState::MainMenu);
        assert!(!app.world().contains_resource::<Online>());
    }

    #[test]
    fn typing_edits_the_address() {
        let mut app = test_app(AppState::JoinWorld);
        app.world_mut().resource_mut::<JoinSettings>().address = String::new();

        for (key, typed) in [
            (KeyCode::KeyA, "a"),
            (KeyCode::Period, "."),
            (KeyCode::Digit1, "1"),
            (KeyCode::Semicolon, ":"),
            (KeyCode::Digit8, "8"),
        ] {
            type_key(&mut app, key, typed);
        }
        assert_eq!(app.world().resource::<JoinSettings>().address, "a.1:8");

        type_key(&mut app, KeyCode::Backspace, "\u{8}");
        assert_eq!(app.world().resource::<JoinSettings>().address, "a.1:");
    }

    #[test]
    fn the_address_field_takes_only_what_could_be_an_address() {
        let mut app = test_app(AppState::JoinWorld);
        app.world_mut().resource_mut::<JoinSettings>().address = String::new();

        // A space, and a character no host name has ever contained.
        type_key(&mut app, KeyCode::Space, " ");
        type_key(&mut app, KeyCode::Slash, "/");
        assert_eq!(app.world().resource::<JoinSettings>().address, "");

        for _ in 0..MAX_ADDRESS + 5 {
            type_key(&mut app, KeyCode::KeyX, "x");
        }
        assert_eq!(
            app.world().resource::<JoinSettings>().address.len(),
            MAX_ADDRESS
        );
    }

    #[test]
    fn enter_joins_and_escape_leaves_the_join_screen() {
        let mut app = test_app(AppState::JoinWorld);
        app.world_mut().resource_mut::<JoinSettings>().address = "127.0.0.1:1".to_string();

        type_key(&mut app, KeyCode::Enter, "\r");
        assert!(
            app.world().contains_resource::<Dialing>(),
            "enter did not join"
        );

        let mut app = test_app(AppState::JoinWorld);
        type_key(&mut app, KeyCode::Escape, "\u{1b}");
        app.update();
        assert_eq!(state(&app), AppState::MainMenu);
    }

    #[test]
    fn keys_pressed_before_the_join_screen_opened_are_not_typed_into_it() {
        // The same backlog the controls screen has to ignore: escape leaves a
        // match, and whatever else was pressed on the way to the menu must not
        // land in the address.
        let mut app = test_app(AppState::InWorld);
        type_key(&mut app, KeyCode::KeyJ, "j");

        go_to(&mut app, AppState::JoinWorld);
        assert_eq!(
            app.world().resource::<JoinSettings>().address,
            JoinSettings::default().address
        );
    }

    #[test]
    fn typing_edits_the_seed() {
        let mut app = test_app(AppState::NewWorld);
        app.world_mut().resource_mut::<NewWorldSettings>().seed = String::new();

        press_key(&mut app, KeyCode::Digit4);
        press_key(&mut app, KeyCode::Digit2);
        assert_eq!(app.world().resource::<NewWorldSettings>().seed, "42");

        press_key(&mut app, KeyCode::Backspace);
        assert_eq!(app.world().resource::<NewWorldSettings>().seed, "4");
    }

    #[test]
    fn seed_field_is_length_capped() {
        let mut app = test_app(AppState::NewWorld);
        app.world_mut().resource_mut::<NewWorldSettings>().seed = String::new();

        for _ in 0..MAX_SEED_DIGITS + 5 {
            press_key(&mut app, KeyCode::Digit9);
        }

        let seed = &app.world().resource::<NewWorldSettings>().seed;
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
        let settings = NewWorldSettings {
            seed: String::new(),
            ..default()
        };
        assert_eq!(settings.seed_value(), 0);
    }

    #[test]
    fn seed_field_parses_digits() {
        let settings = NewWorldSettings {
            seed: "123456".to_string(),
            ..default()
        };
        assert_eq!(settings.seed_value(), 123_456);
    }

    #[test]
    fn random_seeds_fit_the_field() {
        let seed = random_seed();
        assert!(seed.to_string().len() <= MAX_SEED_DIGITS);
    }

    #[test]
    fn defaults_match_the_world_config() {
        let settings = NewWorldSettings::default();
        assert_eq!(settings.seed_value(), WorldConfig::default().seed);
    }
}
