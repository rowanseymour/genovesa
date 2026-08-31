//! Naming a server to play in.
//!
//! One field and one button. The address is taken as `host` or `host:port`,
//! and a bare host gets [`DEFAULT_PORT`] — see [`crate::net`], which is where
//! that rule lives, since it is the dialling that has to know it.
//!
//! The field takes what could be part of an address and nothing else, which
//! is a narrower rule than "printable" and a wider one than a hostname: it
//! has to admit an IPv6 literal in brackets and a port after a colon, and it
//! must not admit a space.

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;

use crate::net::Dialing;
use crate::AppState;
use protocol::DEFAULT_PORT;

use super::kit::{
    cartouche_rule, heading, label, panel, screen, spawn_button, status_line, ON_PAPER,
    PANEL_PADDING,
};
use super::{MenuButton, Status};

/// Longest address the join screen will take. Room for a fully qualified name
/// and a port, well past anything anybody types.
pub(super) const MAX_ADDRESS: usize = 60;

/// What the join screen is currently showing.
#[derive(Resource)]
pub(super) struct JoinSettings {
    /// A server as `host` or `host:port`, exactly as `--join` takes it.
    pub(super) address: String,
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

/// Marks the address readout on the join screen.
#[derive(Component)]
pub(super) struct AddressText;

pub(super) fn spawn_join_dialog(
    mut commands: Commands,
    settings: Res<JoinSettings>,
    status: Res<Status>,
) {
    let ink = ON_PAPER;
    commands
        .spawn((
            Name::new("Join world dialog"),
            DespawnOnExit(AppState::JoinWorld),
            screen(&ink),
        ))
        .with_children(|screen| {
            screen
                .spawn(panel(&ink, 10.0, PANEL_PADDING))
                .with_children(|panel| {
                    cartouche_rule(panel, &ink);
                    heading(panel, &ink, "Join World", 20.0);

                    label(panel, &ink, "Server");
                    panel.spawn((
                        AddressText,
                        Text::new(settings.address.clone()),
                        TextFont {
                            font_size: FontSize::Px(26.0),
                            ..default()
                        },
                        TextColor(ink.text),
                    ));
                    label(panel, &ink, "type an address, backspace to edit");
                    label(
                        panel,
                        &ink,
                        &format!("a bare name joins on port {DEFAULT_PORT}"),
                    );

                    status_line(panel, &ink, &status.0);
                    panel
                        .spawn(Node {
                            column_gap: Val::Px(8.0),
                            margin: UiRect::top(Val::Px(8.0)),
                            ..default()
                        })
                        .with_children(|row| {
                            spawn_button(row, &ink, MenuButton::Back, "Back", 130.0);
                            spawn_button(row, &ink, MenuButton::Connect, "Join", 130.0);
                        });
                });
        });
}

pub(super) fn join_actions(
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
/// [`super::controls::settings_keys`] does: a reader left to lag would deliver
/// whatever was pressed on the way here the instant the screen opened — and on
/// this screen that would be typed into the address.
pub(super) fn join_keys(
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
pub(super) fn is_address_character(c: char) -> bool {
    c.is_ascii_alphanumeric() || ".:-_[]".contains(c)
}

/// Starts dialling, and says so. Shared by the button and the enter key, which
/// have to mean exactly the same thing.
pub(super) fn dial(commands: &mut Commands, status: &mut Status, address: &str) {
    status.0 = format!("joining {address}...");
    commands.insert_resource(Dialing::to(address));
}

/// Keeps the address readout in step with what has been typed.
pub(super) fn refresh_join(
    settings: Res<JoinSettings>,
    mut address: Query<&mut Text, With<AddressText>>,
) {
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
