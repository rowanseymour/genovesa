//! Naming a world about to be made, choosing its seed, and whether to share
//! it.
//!
//! A seed is the whole of what a world *is*, and a name is what the player
//! will know it by on the screen that lists them — see [`NewWorldSettings`].
//! Two fields means one of them has the keyboard: clicking a field takes it,
//! and the one that has it wears the caret.
//!
//! [`share_label`] is the one thing here the kept-worlds screen borrows: both
//! screens offer the same choice about who may reach the world, and they say
//! it in the same words.

use bevy::input_focus::{AutoFocus, FocusCause, FocusLost, InputFocus};
use bevy::prelude::*;
use bevy::text::{EditableText, TextEdit, TextEditChange};

use crate::net::{Dialing, Reach};
use crate::AppState;
use protocol::{DEFAULT_PORT, NAME_LETTERS};
use server::{random_seed, MAX_SEED};

use super::kit::{
    button, button_label, cartouche_rule, heading, label, panel, screen, spawn_button, status_line,
    text_field, Palette, ON_PAPER, PANEL_PADDING,
};
use super::{MenuButton, Status};

/// Longest seed the user can type — read off [`MAX_SEED`], so the field can
/// always hold a seed the game itself picked.
pub(super) const MAX_SEED_DIGITS: usize = MAX_SEED.ilog10() as usize + 1;

/// The dialog's two text fields, and which of them the keyboard is in.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Field {
    /// First, because it is the one the dialog will not start without.
    #[default]
    Name,
    Seed,
}

impl Field {
    /// The word a `click edit` line names this field by.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Seed => "seed",
        }
    }
}

/// What the new-world dialog is currently showing: what to call the world,
/// which world it is, and whether anyone else may come.
#[derive(Resource)]
pub(super) struct NewWorldSettings {
    /// What the world will be called on the screen that lists them. Empty
    /// until typed, and the dialog does not start without one: a name is the
    /// whole of what tells one kept world from the next, and a world nobody
    /// named would be a row nobody could pick out.
    pub(super) name: String,
    /// Held as text so the field can be edited a digit at a time, including
    /// being temporarily empty.
    pub(super) seed: String,
    /// Whether to host the world rather than keep it to ourselves — the whole
    /// of what [`Reach`] decides, and nothing about the session either way.
    pub(super) share: bool,
}

impl Default for NewWorldSettings {
    fn default() -> Self {
        Self {
            name: String::new(),
            // A world nobody has been to, so that opening the dialog and
            // pressing start is a new island rather than the one every other
            // player who did the same thing got. Drawn once, when the game
            // starts, rather than each time the dialog opens: a seed typed in
            // and then navigated away from is one the player chose, and
            // rolling over it would be the dialog forgetting. The dice button
            // is there for another.
            seed: random_seed().to_string(),
            share: false,
        }
    }
}

impl NewWorldSettings {
    fn seed_value(&self) -> u32 {
        self.seed.parse().unwrap_or(0)
    }
}

/// Marks which of the dialog's text fields an `EditableText` is.
#[derive(Component)]
pub(super) struct FieldText(pub(super) Field);

/// How wide the fields are: the dialog's widest thing, and wide enough that
/// the longest name the field takes sits in it with the caret.
const FIELD_WIDTH: f32 = 340.0;

/// A field, inside a button so that the whole box takes the click that puts
/// the keyboard there, and not only the letters. The name field has the
/// keyboard to begin with: it is the one the dialog will not start without.
///
/// The seed takes digits and the name anything printable, to the length the
/// file and the row that shows it have room for.
fn spawn_field(
    parent: &mut ChildSpawnerCommands,
    ink: &Palette,
    settings: &NewWorldSettings,
    field: Field,
) {
    parent
        .spawn(button(ink, MenuButton::Edit(field), FIELD_WIDTH))
        .with_children(|button| {
            let mut text = match field {
                Field::Name => {
                    button.spawn(text_field(ink, &settings.name, 19.0, NAME_LETTERS, |c| {
                        !c.is_control()
                    }))
                }
                Field::Seed => button.spawn(text_field(
                    ink,
                    &settings.seed,
                    19.0,
                    MAX_SEED_DIGITS,
                    |c| c.is_ascii_digit(),
                )),
            };
            text.insert(FieldText(field));
            if field == Field::Name {
                text.insert(AutoFocus);
            }
        });
}

/// Marks the sharing button's label, so it can say which way it is set.
#[derive(Component)]
pub(super) struct ShareText;

pub(super) fn spawn_new_world_dialog(
    mut commands: Commands,
    settings: Res<NewWorldSettings>,
    status: Res<Status>,
) {
    let ink = ON_PAPER;
    commands
        .spawn((
            Name::new("New world dialog"),
            DespawnOnExit(AppState::NewWorld),
            screen(&ink),
        ))
        .with_children(|screen| {
            screen
                .spawn(panel(&ink, 10.0, PANEL_PADDING))
                .with_children(|panel| {
                    cartouche_rule(panel, &ink);
                    heading(panel, &ink, "New World", 20.0);

                    label(panel, &ink, "Name");
                    spawn_field(panel, &ink, &settings, Field::Name);
                    label(panel, &ink, "Seed");
                    spawn_field(panel, &ink, &settings, Field::Seed);
                    panel.spawn((
                        Text::new("click a field and type; backspace to edit"),
                        TextFont {
                            font_size: FontSize::Px(13.0),
                            ..default()
                        },
                        TextColor(ink.dim),
                    ));
                    spawn_button(panel, &ink, MenuButton::RandomSeed, "Random", 160.0);

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
                            row.spawn(button(&ink, MenuButton::ToggleShare, 160.0))
                                .with_children(|button| {
                                    button.spawn((
                                        ShareText,
                                        button_label(&ink, share_label(settings.share)),
                                    ));
                                });
                        });
                    // Phrased as what the switch does rather than as what is
                    // happening, since it is read in both positions.
                    label(
                        panel,
                        &ink,
                        &format!("sharing hosts the world on port {DEFAULT_PORT}"),
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
                            spawn_button(row, &ink, MenuButton::Start, "Start", 130.0);
                        });
                });
        });
}

/// What the sharing button reads. A switch has to say which way it is set,
/// not what pressing it would do.
pub(super) fn share_label(share: bool) -> &'static str {
    if share {
        "Share: on"
    } else {
        "Share: off"
    }
}

/// The dialog's buttons that only touch the settings. Starting a world is
/// [`open_world`], which has to wait for a server and so cannot be a button
/// handler that decides anything on the spot.
pub(super) fn dialog_actions(
    buttons: Query<(&Interaction, &MenuButton), Changed<Interaction>>,
    mut fields: Query<(Entity, &FieldText, &mut EditableText)>,
    mut focus: ResMut<InputFocus>,
    mut settings: ResMut<NewWorldSettings>,
    mut next: ResMut<NextState<AppState>>,
) {
    for (interaction, button) in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match button {
            MenuButton::Edit(field) => {
                if let Some((entity, _, _)) = fields.iter().find(|(_, f, _)| f.0 == *field) {
                    focus.set(entity, FocusCause::Pressed);
                }
            }
            // Typed in over what was there, so it lands the way a typed seed
            // does and is read back by [`read_field`] like one.
            MenuButton::RandomSeed => {
                for (_, field, mut text) in &mut fields {
                    if field.0 == Field::Seed {
                        text.queue_edit(TextEdit::SelectAll);
                        text.queue_edit(TextEdit::Insert(random_seed().to_string().into()));
                    }
                }
            }
            MenuButton::ToggleShare => settings.share = !settings.share,
            // One step back is the screen this one opens from, which is where
            // the worlds are — not the front of the game.
            MenuButton::Back => next.set(AppState::SetSail),
            _ => {}
        }
    }
}

/// Starting a world, kept or shared.
///
/// One system for both, because there is only one thing to do. The ground
/// comes from a server, so a world of one's own is a server too — see
/// [`Reach`], which is the whole of what the sharing switch decides. The way
/// in is then the way into anybody else's: dial it and wait, and
/// [`super::settle_dialing`] takes the spawn from the welcome exactly as a run
/// started with `--join` does. Nothing about the world is settled here, not
/// even by the machine that is about to serve it.
pub(super) fn open_world(
    mut commands: Commands,
    buttons: Query<(&Interaction, &MenuButton), Changed<Interaction>>,
    dialing: Option<Res<Dialing>>,
    settings: Res<NewWorldSettings>,
    mut status: ResMut<Status>,
) {
    // A world already being opened. Asking for a second would only fail on
    // the port the first one is holding.
    if dialing.is_some() {
        return;
    }

    for (interaction, button) in &buttons {
        if *interaction == Interaction::Pressed && *button == MenuButton::Start {
            // The one thing the dialog insists on. Said on the status line
            // rather than by refusing silently: a press that does nothing is
            // a press the player repeats.
            let name = settings.name.trim();
            if name.is_empty() {
                status.0 = "the world needs a name".to_string();
                return;
            }
            status.0 = "opening the world...".to_string();
            commands.insert_resource(Dialing::opening(
                settings.seed_value(),
                name.to_string(),
                if settings.share {
                    Reach::Shared
                } else {
                    Reach::Alone
                },
                // A world opened from the menu opens at the hour worlds
                // open at; choosing another is a thing the command line can
                // ask for and this screen has no room to.
                server::OPENING,
                // And it is kept: the menu's worlds are worlds to live in,
                // and the set-sail screen is where they are returned to.
                true,
            ));
            return;
        }
    }
}

/// Takes what a field now holds into the settings, which are what the
/// dialog starts a world from and what it reopens showing.
pub(super) fn read_field(
    change: On<TextEditChange>,
    fields: Query<(&FieldText, &EditableText)>,
    mut settings: ResMut<NewWorldSettings>,
) {
    let Ok((field, text)) = fields.get(change.event_target()) else {
        return;
    };
    let value = text.value().to_string();
    let held = match field.0 {
        Field::Name => &mut settings.name,
        Field::Seed => &mut settings.seed,
    };
    if *held != value {
        *held = value;
    }
}

/// An emptied seed, once the keyboard leaves it, reads as the zero it will be
/// read as.
pub(super) fn settle_seed(lost: On<FocusLost>, mut fields: Query<(&FieldText, &mut EditableText)>) {
    if let Ok((field, mut text)) = fields.get_mut(lost.event_target()) {
        if field.0 == Field::Seed && text.value().to_string().is_empty() {
            text.queue_edit(TextEdit::Insert("0".into()));
        }
    }
}

/// Keeps the sharing button in step with the setting behind it.
pub(super) fn refresh_dialog(
    settings: Res<NewWorldSettings>,
    mut share_text: Query<&mut Text, With<ShareText>>,
) {
    if !settings.is_changed() {
        return;
    }

    for mut text in &mut share_text {
        text.0 = share_label(settings.share).to_string();
    }
}
