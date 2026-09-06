//! Naming a world about to be made, choosing its seed, and whether to share
//! it.
//!
//! A seed is the whole of what a world *is*, and a name is what the player
//! will know it by on the screen that lists them — see [`NewWorldSettings`].
//! Two fields means one of them has the keyboard: clicking a field takes it,
//! and the one that has it wears the caret. Both are edited by what the
//! keyboard typed rather than by which positions were pressed, so the number
//! pad and whatever a layout puts the letters on both simply work.
//!
//! [`share_label`] is the one thing here the kept-worlds screen borrows: both
//! screens offer the same choice about who may reach the world, and they say
//! it in the same words.

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;

use crate::net::{Dialing, Reach};
use crate::AppState;
use protocol::{DEFAULT_PORT, NAME_LETTERS};
use server::{random_seed, MAX_SEED};

use super::kit::{
    button, button_label, cartouche_rule, heading, label, panel, screen, spawn_button, status_line,
    Palette, ON_PAPER, PANEL_PADDING,
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
    /// Which field the keyboard is in.
    pub(super) editing: Field,
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
            editing: Field::default(),
        }
    }
}

impl NewWorldSettings {
    fn seed_value(&self) -> u32 {
        self.seed.parse().unwrap_or(0)
    }

    fn field(&mut self, field: Field) -> &mut String {
        match field {
            Field::Name => &mut self.name,
            Field::Seed => &mut self.seed,
        }
    }

    /// What a field reads on the screen: what is in it, and a caret on the
    /// one the keyboard is in — the whole of how a player can tell which
    /// field their next key lands in. An empty seed reads as the zero it will
    /// be read as; an empty name reads as nothing, which is what it is.
    fn field_text(&self, field: Field) -> String {
        let text = match field {
            Field::Name => self.name.clone(),
            Field::Seed if self.seed.is_empty() && self.editing != field => "0".to_string(),
            Field::Seed => self.seed.clone(),
        };
        if self.editing == field {
            format!("{text}_")
        } else {
            text
        }
    }
}

/// Marks a field's readout so it can be refreshed as the player types.
#[derive(Component)]
pub(super) struct FieldText(Field);

/// How wide the fields are: the dialog's widest thing, and wide enough that
/// the longest name the field takes sits in it with the caret.
const FIELD_WIDTH: f32 = 340.0;

/// A field: a button, so that clicking it is how the keyboard gets there,
/// wearing its own text.
fn spawn_field(
    parent: &mut ChildSpawnerCommands,
    ink: &Palette,
    settings: &NewWorldSettings,
    field: Field,
) {
    parent
        .spawn(button(ink, MenuButton::Edit(field), FIELD_WIDTH))
        .with_children(|button| {
            button.spawn((
                FieldText(field),
                button_label(ink, &settings.field_text(field)),
            ));
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
    mut settings: ResMut<NewWorldSettings>,
    mut next: ResMut<NextState<AppState>>,
) {
    for (interaction, button) in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match button {
            MenuButton::Edit(field) => settings.editing = *field,
            MenuButton::RandomSeed => settings.seed = random_seed().to_string(),
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

/// Key-by-key editing of whichever field the keyboard is in.
///
/// Asks what the keyboard *typed* rather than which positions were pressed,
/// exactly as the address field does — so the game's text fields are edited
/// by one mechanism instead of two. It also spares this a table of every key
/// that produces a digit: the main row and the number pad both simply type
/// one, and so does whatever a layout puts them on.
///
/// The seed takes digits and the name takes anything typed, to the length
/// the file and the row that shows it have room for. A space is not a
/// character to the keyboard — it is a key of its own — and a name with a
/// space in it is most names.
///
/// Runs on every screen rather than only this one, for the reason
/// [`super::join::join_keys`] does: a reader left to lag would deliver whatever was
/// pressed on the way here the instant the dialog opened — and on this
/// screen that would land in a field.
pub(super) fn type_into_field(
    state: Res<State<AppState>>,
    mut presses: MessageReader<KeyboardInput>,
    mut settings: ResMut<NewWorldSettings>,
) {
    let on_screen = *state.get() == AppState::NewWorld;

    for press in presses.read() {
        // A held key repeats, which is what a text field wants: holding
        // backspace should clear the field rather than one letter of it.
        if !on_screen || press.state != ButtonState::Pressed {
            continue;
        }

        let editing = settings.editing;
        let field = settings.field(editing);
        match (press.key_code, &press.logical_key) {
            (KeyCode::Backspace, _) => {
                field.pop();
            }
            (_, Key::Space) if editing == Field::Name && field.chars().count() < NAME_LETTERS => {
                field.push(' ');
            }
            (_, Key::Character(typed)) => {
                for letter in typed.chars() {
                    let fits = match editing {
                        Field::Name => !letter.is_control() && field.chars().count() < NAME_LETTERS,
                        Field::Seed => letter.is_ascii_digit() && field.len() < MAX_SEED_DIGITS,
                    };
                    if fits {
                        field.push(letter);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Keeps the dialog's readouts in step with the settings behind them.
pub(super) fn refresh_dialog(
    settings: Res<NewWorldSettings>,
    mut fields: Query<(&FieldText, &mut Text), Without<ShareText>>,
    mut share_text: Query<&mut Text, With<ShareText>>,
) {
    if !settings.is_changed() {
        return;
    }

    for (field, mut text) in &mut fields {
        text.0 = settings.field_text(field.0);
    }
    for mut text in &mut share_text {
        text.0 = share_label(settings.share).to_string();
    }
}
