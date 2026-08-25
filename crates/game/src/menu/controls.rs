//! Choosing which key does what.
//!
//! One row per [`Action`], each naming its key by what that key *typed* when
//! it was chosen — so a rebound or foreign keyboard is offered its own key
//! rather than a US one. A row is armed by clicking it and takes the next key
//! pressed; only one may be armed at a time, since the next key has to mean
//! one thing.
//!
//! The one screen that can be taller than the window, and so the one that
//! scrolls — see [`super::kit::scrolling_panel`]. Reached from the main menu
//! and from the pause menu, and built once for both.

use bevy::input::keyboard::KeyboardInput;
use bevy::input::ButtonState;
use bevy::prelude::*;

use crate::bindings::{is_bindable, typed_label, Action, KeyBindings};
use crate::{AppState, Helm};

use super::kit::{
    button_label, cartouche_rule, heading, label, padded_button, screen, scrolling_panel,
    spawn_button, Highlight, Palette, CONTROLS_PADDING, ON_PAPER, OVER_THE_WORLD,
    ROW_BUTTON_PADDING,
};
use super::{back_to_options, over_a_world, MenuButton};

/// The action whose new key the controls screen is waiting for, if any. Only
/// one row can be armed at a time — the next key pressed has to mean one thing.
#[derive(Resource, Default)]
pub(super) struct Rebinding(pub(super) Option<Action>);

/// Marks the key readout on an action's row, so it can be refreshed when the
/// binding changes or the row starts waiting for a key.
#[derive(Component)]
pub(super) struct KeyText(pub(super) Action);

/// The controls screen as reached from the options screen.
pub(super) fn spawn_settings(commands: Commands, bindings: Res<KeyBindings>) {
    spawn_controls(
        commands,
        &ON_PAPER,
        &bindings,
        DespawnOnExit(AppState::Controls),
    );
}

/// The same screen as reached from the pause menu — see
/// [`spawn_paused_options`].
pub(super) fn spawn_paused_settings(commands: Commands, bindings: Res<KeyBindings>) {
    spawn_controls(
        commands,
        &OVER_THE_WORLD,
        &bindings,
        DespawnOnExit(Helm::Controls),
    );
}

/// Builds the controls screen, cleared up by whichever state opened it.
pub(super) fn spawn_controls(
    mut commands: Commands,
    ink: &Palette,
    bindings: &KeyBindings,
    until: impl Bundle,
) {
    commands
        .spawn((Name::new("Controls screen"), until, screen(ink)))
        .with_children(|screen| {
            screen
                .spawn(scrolling_panel(ink, 6.0, CONTROLS_PADDING))
                .with_children(|panel| {
                    cartouche_rule(panel, ink);
                    heading(panel, ink, "Controls", 6.0);
                    label(panel, ink, "click a key, then press the one you want");

                    panel
                        .spawn(Node {
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(4.0),
                            margin: UiRect::vertical(Val::Px(8.0)),
                            ..default()
                        })
                        .with_children(|rows| {
                            for action in Action::ALL {
                                spawn_key_row(rows, ink, action, &bindings.name(action));
                            }
                        });

                    // Plain punctuation only: the default font has no dash of
                    // any kind and draws a missing glyph as an empty box.
                    label(panel, ink, "the arrow keys always move, and escape always");
                    label(panel, ink, "goes back; neither can be reassigned");

                    panel
                        .spawn(Node {
                            column_gap: Val::Px(8.0),
                            margin: UiRect::top(Val::Px(10.0)),
                            ..default()
                        })
                        .with_children(|row| {
                            spawn_button(row, ink, MenuButton::ResetKeys, "Defaults", 130.0);
                            spawn_button(row, ink, MenuButton::Back, "Back", 130.0);
                        });
                });
        });
}

/// One action and the key it sits on, as a name on the left and a button on the
/// right that arms the row when clicked.
pub(super) fn spawn_key_row(
    parent: &mut ChildSpawnerCommands,
    ink: &Palette,
    action: Action,
    key_name: &str,
) {
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
                TextColor(ink.text),
            ));
            row.spawn(padded_button(
                ink,
                MenuButton::Rebind(action),
                170.0,
                ROW_BUTTON_PADDING,
            ))
            .with_children(|button| {
                button.spawn((KeyText(action), button_label(ink, key_name)));
            });
        });
}

pub(super) fn settings_actions(
    buttons: Query<(&Interaction, &MenuButton), Changed<Interaction>>,
    helm: Option<Res<State<Helm>>>,
    mut rebinding: ResMut<Rebinding>,
    mut bindings: ResMut<KeyBindings>,
    mut next_app: ResMut<NextState<AppState>>,
    mut next_helm: ResMut<NextState<Helm>>,
) {
    let over_a_world = over_a_world(&helm);

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
            MenuButton::Back => back_to_options(over_a_world, &mut next_app, &mut next_helm),
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
///
/// Escape on this screen belongs to this system wherever the screen was opened
/// from, because only here is it known whether a row is armed — in which case
/// the key means "not that one" and the screen stays put. `helm_keys` steps
/// aside in [`Helm::Controls`] for exactly that reason.
pub(super) fn settings_keys(
    state: Res<State<AppState>>,
    helm: Option<Res<State<Helm>>>,
    mut presses: MessageReader<KeyboardInput>,
    mut rebinding: ResMut<Rebinding>,
    mut bindings: ResMut<KeyBindings>,
    mut next_app: ResMut<NextState<AppState>>,
    mut next_helm: ResMut<NextState<Helm>>,
) {
    let over_a_world = helm.as_ref().is_some_and(|h| *h.get() == Helm::Controls);
    let on_screen = *state.get() == AppState::Controls || over_a_world;

    for press in presses.read() {
        // A key held down repeats; the first press is the one that counts.
        if !on_screen || press.state != ButtonState::Pressed || press.repeat {
            continue;
        }

        let Some(action) = rebinding.0 else {
            if press.key_code == KeyCode::Escape {
                back_to_options(over_a_world, &mut next_app, &mut next_helm);
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
pub(super) fn refresh_settings(
    bindings: Res<KeyBindings>,
    rebinding: Res<Rebinding>,
    mut keys: Query<(&KeyText, &mut Text)>,
    mut buttons: Query<(&MenuButton, &Highlight, &mut BackgroundColor, &Interaction)>,
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
    for (button, ink, mut color, interaction) in &mut buttons {
        if let MenuButton::Rebind(action) = button {
            if *interaction == Interaction::None {
                *color = BackgroundColor(if rebinding.0 == Some(*action) {
                    ink.armed
                } else {
                    ink.idle
                });
            }
        }
    }
}

pub(super) fn cancel_rebinding(mut rebinding: ResMut<Rebinding>) {
    rebinding.0 = None;
}
