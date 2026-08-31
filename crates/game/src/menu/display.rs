//! Choosing how the game is drawn: how much screen it takes, and how many
//! pixels it draws into.
//!
//! Two rows, and they are two because they are what somebody whose machine
//! cannot keep up reaches for, in the order they reach for them. Nothing here
//! touches the window: the screen edits what is [`Wanted`] and Apply is what
//! makes that the settings — see [`crate::settings`], which owns the trial a
//! change is put on and the putting back that ends one nobody stood by.
//!
//! Reached from the main menu and from the pause menu, and built once for
//! both — see [`super::over_a_world`].

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::window::{Monitor, PrimaryMonitor, PrimaryWindow};

use crate::settings::{self, DisplaySettings, OnTrial, Trial, Wanted};
use crate::{AppState, Helm};

use super::kit::{
    button, button_label, cartouche_rule, heading, label, padded_button, panel, screen,
    spawn_button, Palette, ON_PAPER, OVER_THE_WORLD, PANEL_PADDING, ROW_BUTTON_PADDING,
};
use super::{back_to_options, over_a_world, MenuButton};

/// Marks a display row's readout, so it can say which way its setting is set.
/// The button it belongs to is the one that changes that setting, so the two
/// are named by the same list.
#[derive(Component, Clone, Copy, PartialEq)]
pub(super) enum DisplayText {
    Fullscreen,
    Resolution,
    /// Not a setting but a word about them: what the display will really do
    /// with the resolution being asked of it, and whether it has been asked
    /// yet at all — see [`settings::available`].
    Caveat,
    /// The Apply button's own label, which counts a trial down.
    Apply,
}

/// Whether the list of resolutions is down.
///
/// A flag rather than an entity to go looking for, the same shape as
/// [`super::controls::Rebinding`] and for the same reason: there is one such
/// list in the whole game, and what a system wants to know about it is never
/// which one.
#[derive(Resource, Default)]
pub(super) struct Picking(pub(super) bool);

/// Marks the display screen's root, which is what an open list's backing sheet
/// is hung from — it has to cover the screen, so it cannot hang from the row.
#[derive(Component)]
pub(super) struct DisplayScreen;

/// Marks the cell the list of resolutions drops out of.
#[derive(Component)]
pub(super) struct Resolutions;

/// The display screen as reached from the main menu.
pub(super) fn spawn_display(commands: Commands, wanted: Res<Wanted>) {
    build_display(
        commands,
        &ON_PAPER,
        &wanted.0,
        DespawnOnExit(AppState::Display),
    );
}

/// And as reached from the pause menu — see [`super::ladder::spawn_paused_options`].
pub(super) fn spawn_paused_display(commands: Commands, wanted: Res<Wanted>) {
    build_display(
        commands,
        &OVER_THE_WORLD,
        &wanted.0,
        DespawnOnExit(Helm::Display),
    );
}

/// The screen opens set to whatever the machine is already doing, and never
/// opens with the list of resolutions already down.
pub(super) fn open_display(
    settings: Res<DisplaySettings>,
    mut wanted: ResMut<Wanted>,
    mut picking: ResMut<Picking>,
) {
    wanted.0 = *settings;
    picking.0 = false;
}

/// Leaving stands by a change still on trial.
///
/// Not because a trial is a formality but because of what it is for: it
/// catches a display the player cannot see, and a player who has found Back
/// and pressed it has shown that they can. What they had set but never applied
/// goes the other way and is simply dropped — [`open_display`] builds the
/// screen out of the machine again next time.
pub(super) fn close_display(mut on_trial: ResMut<OnTrial>, mut picking: ResMut<Picking>) {
    on_trial.0 = None;
    picking.0 = false;
}

/// Two rows, and they are two because they are what somebody whose machine
/// cannot keep up reaches for, in the order they reach for them: fill the
/// screen, draw fewer pixels.
///
/// Neither does anything on its own — the screen edits [`Wanted`]
/// and Apply is what reaches the window, for the reason that resource gives —
/// so it is built out of what is *wanted* now rather than empty for
/// [`refresh_display`] to fill in, and is right on the frame it appears rather
/// than one after.
pub(super) fn build_display(
    mut commands: Commands,
    ink: &Palette,
    wanted: &DisplaySettings,
    until: impl Bundle,
) {
    commands
        .spawn((
            Name::new("Display screen"),
            DisplayScreen,
            until,
            screen(ink),
        ))
        .with_children(|screen| {
            screen
                .spawn(panel(ink, 6.0, PANEL_PADDING))
                .with_children(|panel| {
                    cartouche_rule(panel, ink);
                    heading(panel, ink, "Display", 6.0);

                    panel
                        .spawn(Node {
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(6.0),
                            margin: UiRect::vertical(Val::Px(12.0)),
                            ..default()
                        })
                        .with_children(|rows| {
                            spawn_switch_row(
                                rows,
                                ink,
                                "Fullscreen",
                                MenuButton::ToggleFullscreen,
                                DisplayText::Fullscreen,
                                wanted.fullscreen,
                            );
                            spawn_resolution_row(rows, ink, wanted);
                        });

                    // Plain punctuation only: the default font has no dash of
                    // any kind and draws a missing glyph as an empty box.
                    label(panel, ink, "fewer pixels is less work for the machine; the");
                    label(
                        panel,
                        ink,
                        "window or the screen itself changes size to suit",
                    );
                    // Held open whether or not there is anything to put in it,
                    // so that a resolution the display cannot do, or a change
                    // waiting to be applied, does not shove the buttons down
                    // the moment it has something to say.
                    panel.spawn((
                        DisplayText::Caveat,
                        Text::new(String::new()),
                        TextFont {
                            font_size: FontSize::Px(15.0),
                            ..default()
                        },
                        TextColor(ink.dim),
                        Node {
                            height: Val::Px(18.0),
                            margin: UiRect::top(Val::Px(6.0)),
                            ..default()
                        },
                    ));

                    panel
                        .spawn(Node {
                            margin: UiRect::top(Val::Px(10.0)),
                            column_gap: Val::Px(10.0),
                            ..default()
                        })
                        .with_children(|row| {
                            row.spawn(button(ink, MenuButton::ApplyDisplay, 130.0))
                                .with_children(|button| {
                                    // Its inks travel with it, the way the
                                    // list's cell carries them: the system
                                    // that dims this label sees both sheets at
                                    // once and cannot tell from a label which
                                    // of them it was drawn on.
                                    button.spawn((
                                        DisplayText::Apply,
                                        *ink,
                                        button_label(ink, "Apply"),
                                    ));
                                });
                            spawn_button(row, ink, MenuButton::Back, "Back", 130.0);
                        });
                });
        });
}

/// How wide the control on the right of a setting's row is. One width for all
/// of them, so the rows read as a column rather than as three separate
/// questions — and it is the list's width too, which drops out of one of them.
pub(super) const CONTROL_WIDTH: f32 = 170.0;

/// One setting and how it is set, as a name on the left and something on the
/// right that changes it — the same shape as a key row, because it is the same
/// question asked about something other than a key.
pub(super) fn setting_row<'a>(
    parent: &'a mut ChildSpawnerCommands,
    ink: &Palette,
    name: &str,
) -> EntityCommands<'a> {
    let mut row = parent.spawn(Node {
        width: Val::Px(400.0),
        align_items: AlignItems::Center,
        justify_content: JustifyContent::SpaceBetween,
        ..default()
    });
    row.with_children(|row| {
        row.spawn((
            Text::new(name.to_string()),
            TextFont {
                font_size: FontSize::Px(18.0),
                ..default()
            },
            TextColor(ink.text),
        ));
    });
    row
}

/// A setting with two ways to be, which a button can carry on its own: it says
/// which way it is set, and pressing it says the other.
pub(super) fn spawn_switch_row(
    parent: &mut ChildSpawnerCommands,
    ink: &Palette,
    name: &str,
    action: MenuButton,
    mark: DisplayText,
    on: bool,
) {
    setting_row(parent, ink, name).with_children(|row| {
        row.spawn(padded_button(
            ink,
            action,
            CONTROL_WIDTH,
            ROW_BUTTON_PADDING,
        ))
        .with_children(|button| {
            button.spawn((mark, button_label(ink, switch_label(on))));
        });
    });
}

/// The resolution's row, which is a list rather than a switch.
///
/// A setting with two ways to be can be a button that shows one of them; a
/// setting with five cannot. The button that used to step through them applied
/// four settings to reach the fifth, and in fullscreen each of those steps is a
/// real display mode switch — the screen goes black and the monitor resyncs.
/// So the list drops down instead and every rung is one press from any other.
pub(super) fn spawn_resolution_row(
    parent: &mut ChildSpawnerCommands,
    ink: &Palette,
    wanted: &DisplaySettings,
) {
    setting_row(parent, ink, "Resolution").with_children(|row| {
        // The list hangs off a cell of its own rather than off the button,
        // because it is positioned against what it drops from and has to be
        // able to cover it. The cell carries the inks too: the list is built
        // long after this, by a system that sees both sheets at once and
        // cannot tell from a row which of them it was drawn on.
        row.spawn((Resolutions, *ink, Node::default()))
            .with_children(|cell| {
                cell.spawn(padded_button(
                    ink,
                    MenuButton::OpenResolutions,
                    CONTROL_WIDTH,
                    ROW_BUTTON_PADDING,
                ))
                .with_children(|button| {
                    button.spawn((
                        DisplayText::Resolution,
                        button_label(ink, &wanted.resolution.label()),
                    ));
                });
            });
    });
}

/// What a switch reads. It has to say which way it is set, not what pressing it
/// would do.
pub(super) fn switch_label(on: bool) -> &'static str {
    if on {
        "On"
    } else {
        "Off"
    }
}

/// Marks everything an open list puts on the screen, so that shutting it is
/// one query rather than two entities to remember.
#[derive(Component)]
pub(super) struct Dropped;

/// The sheet behind an open list draws over the screen; the list draws over
/// the sheet. Both are lifted out of the panel they belong to rather than
/// ordered within it, since one of them has to cover the whole window.
///
/// The same two numbers the debug readout and the console take, and they are
/// free to be: the console is a sibling of the pause menu and so is never up
/// with this screen, and the readout has nothing to fear from a sheet that
/// draws nothing.
pub(super) const BEHIND_THE_LIST: i32 = 1;

pub(super) const THE_LIST: i32 = 2;

/// Puts the list of resolutions down, and takes it up again.
///
/// Spawned and despawned rather than kept and hidden: a list that is always
/// there is a list that has to be held in step with what is picked whether
/// anybody is looking at it or not. Commands queued here are applied before
/// the frame is laid out, so it appears on the frame it was asked for rather
/// than the one after — which is also why the rung already taken is coloured
/// here rather than left to [`super::highlight_buttons`], whose `Changed<Interaction>`
/// cannot fire until the frame after these commands land.
///
/// Nothing but the list going up or down is worth watching: what is *wanted*
/// cannot change underneath an open list. Every control that writes it either
/// shuts the list in the same press or sits behind the catcher, and a trial
/// can neither begin nor run while the list is down.
pub(super) fn refresh_picker(
    mut commands: Commands,
    picking: Res<Picking>,
    wanted: Res<Wanted>,
    cells: Query<(Entity, &Palette), With<Resolutions>>,
    screens: Query<Entity, With<DisplayScreen>>,
    dropped: Query<Entity, With<Dropped>>,
) {
    if !picking.is_changed() {
        return;
    }
    for entity in &dropped {
        commands.entity(entity).despawn();
    }
    if !picking.0 {
        return;
    }
    let (Ok((cell, ink)), Ok(screen)) = (cells.single(), screens.single()) else {
        return;
    };

    // Everything the list does not catch. It is the whole window rather than
    // the panel, so that a click anywhere at all shuts the list — and it is
    // hung from the screen for the same reason, the panel being too small to
    // do the job from inside a row.
    commands.entity(screen).with_children(|screen| {
        screen.spawn((
            Dropped,
            Button,
            MenuButton::ShutResolutions,
            GlobalZIndex(BEHIND_THE_LIST),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
        ));
    });

    commands.entity(cell).with_children(|cell| {
        cell.spawn((
            Dropped,
            GlobalZIndex(THE_LIST),
            Node {
                position_type: PositionType::Absolute,
                // Straight under the button it came out of, and the same
                // width, so the list reads as the button opened out.
                top: Val::Percent(100.0),
                left: Val::Px(0.0),
                width: Val::Px(CONTROL_WIDTH),
                flex_direction: FlexDirection::Column,
                border: UiRect::all(Val::Px(1.0)),
                ..default()
            },
            BackgroundColor(ink.panel),
            BorderColor::all(ink.edge),
        ))
        .with_children(|list| {
            for rung in settings::LADDER {
                let taken = rung == wanted.0.resolution;
                list.spawn(padded_button(
                    ink,
                    MenuButton::PickResolution(rung),
                    CONTROL_WIDTH,
                    ROW_BUTTON_PADDING,
                ))
                // Over the idle fill `padded_button` builds in, there being no
                // room for a second `BackgroundColor` alongside it.
                .insert(BackgroundColor(if taken {
                    ink.button.armed
                } else {
                    ink.button.idle
                }))
                .with_children(|item| {
                    item.spawn(button_label(ink, &rung.label()));
                });
            }
        });
    });
}

/// What the machine is doing, what the screen is set to, and what has come of
/// the difference so far.
///
/// Three resources that are one thought: none of them means anything without
/// the other two, and the two systems that act on this screen — as against the
/// ones that only read it back out — want all three.
#[derive(SystemParam)]
pub(super) struct Editing<'w> {
    settings: ResMut<'w, DisplaySettings>,
    wanted: ResMut<'w, Wanted>,
    on_trial: ResMut<'w, OnTrial>,
}

pub(super) fn display_actions(
    buttons: Query<(&Interaction, &MenuButton), Changed<Interaction>>,
    helm: Option<Res<State<Helm>>>,
    mut editing: Editing,
    mut picking: ResMut<Picking>,
    mut next_app: ResMut<NextState<AppState>>,
    mut next_helm: ResMut<NextState<Helm>>,
) {
    let over_a_world = over_a_world(&helm);

    for (interaction, button) in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match button {
            // A trial is one question, and the three rows are not it. There is
            // nowhere on the screen for a fresh edit to show while one runs —
            // the caveat line is the trial's and the button reads Keep — and
            // `run_trial` puts back what is *wanted* along with the settings,
            // so an edit taken now would be swallowed and then quietly undone.
            // Better to answer nothing until the change on the table is
            // settled, which is one press away either way.
            MenuButton::ToggleFullscreen
            | MenuButton::OpenResolutions
            | MenuButton::PickResolution(_)
                if editing.on_trial.0.is_some() => {}
            MenuButton::ToggleFullscreen => {
                editing.wanted.0.fullscreen = !editing.wanted.0.fullscreen
            }
            // Opens, and only opens: the sheet behind an open list covers the
            // button it dropped from, so the press that shuts it again is a
            // press on that rather than a second one on this.
            MenuButton::OpenResolutions => picking.0 = true,
            MenuButton::PickResolution(rung) => {
                editing.wanted.0.resolution = *rung;
                picking.0 = false;
            }
            MenuButton::ShutResolutions => picking.0 = false,
            // The one button, and which of the two things it means follows
            // from whether there is a trial running: during one there is
            // nothing else it could mean, and afterwards nothing else it
            // could be for.
            MenuButton::ApplyDisplay if editing.on_trial.0.is_some() => editing.on_trial.0 = None,
            MenuButton::ApplyDisplay if editing.wanted.0 != *editing.settings => {
                editing.on_trial.0 = Some(Trial::new(*editing.settings));
                *editing.settings = editing.wanted.0;
            }
            MenuButton::Back => back_to_options(over_a_world, &mut next_app, &mut next_helm),
            _ => {}
        }
    }
}

/// Counts a trial down and puts the settings back when it runs out — see
/// [`OnTrial`]. What is *wanted* goes back with them, so the screen the player
/// is left looking at says what the machine is really doing rather than what
/// it briefly was.
pub(super) fn run_trial(time: Res<Time>, mut editing: Editing) {
    let Some(trial) = &mut editing.on_trial.0 else {
        return;
    };
    let Some(was) = trial.ran_out(time.delta()) else {
        return;
    };
    *editing.settings = was;
    editing.wanted.0 = was;
    editing.on_trial.0 = None;
}

/// Keeps the rows in step with what is wanted, says what the display will
/// really do with it, and tells the Apply button which of its two jobs it is
/// currently doing.
///
/// Every line is compared before it is written rather than guarded by
/// `is_changed`, because what a line should say depends on more than the
/// settings: the monitors turn up a frame or two into the run, and a screen
/// opened before they did would otherwise keep a caveat it can no longer
/// justify. Writing only what differs is what keeps that from re-laying out
/// the panel every frame.
pub(super) fn refresh_display(
    settings: Res<DisplaySettings>,
    wanted: Res<Wanted>,
    on_trial: Res<OnTrial>,
    monitors: Query<(Entity, &Monitor, Has<PrimaryMonitor>)>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut readouts: Query<(&DisplayText, &mut Text, &mut TextColor, Option<&Palette>)>,
) {
    // The screen this window is on, which on two monitors is not the primary
    // one and is the only one the caveat can honestly be about — see
    // [`settings::showing_on`].
    let monitor =
        settings::showing_on(windows.single().ok(), monitors.iter()).map(|(_, screen)| screen);
    let unapplied = wanted.0 != *settings;

    for (which, mut text, mut colour, ink) in &mut readouts {
        let saying = match which {
            DisplayText::Fullscreen => switch_label(wanted.0.fullscreen).to_string(),
            DisplayText::Resolution => wanted.0.resolution.label(),
            // Three states and a word for each, because a control that says
            // which way it is set is the rule the switches on this screen
            // already follow. Counting down in the button rather than beside
            // it, too: the one control about to act on its own is also the one
            // saying how long there is to stop it.
            DisplayText::Apply => match (&on_trial.0, unapplied) {
                (Some(trial), _) => format!("Keep ({})", trial.seconds_left()),
                (None, true) => "Apply".to_string(),
                (None, false) => "Applied".to_string(),
            },
            // The line under the rows says what the display will really do,
            // and a trial takes it over because that is the one thing on the
            // screen with a clock on it. It does not also carry *not applied
            // yet* — the button says that, and the caveat a picked rung earns
            // is worth more before it is applied than after.
            DisplayText::Caveat if on_trial.0.is_some() => {
                "put back on its own unless you keep it".to_string()
            }
            // Only worth a word when the display has no such mode, and only
            // then about the screen it would have filled: windowed, a size is
            // a size and every display can do it.
            DisplayText::Caveat
                if wanted.0.fullscreen && !settings::available(wanted.0.resolution, monitor) =>
            {
                format!(
                    "this screen has no {} mode, so filling it draws every pixel",
                    wanted.0.resolution.label()
                )
            }
            DisplayText::Caveat => String::new(),
        };
        if text.0 != saying {
            text.0 = saying;
        }
        // Only the Apply button carries its inks, and only it changes colour:
        // a button with nothing to apply and nothing to keep is drawn dim, so
        // that it never looks like a press worth making.
        if let Some(ink) = ink {
            let wanted_ink = if unapplied || on_trial.0.is_some() {
                ink.text
            } else {
                ink.dim
            };
            if colour.0 != wanted_ink {
                colour.0 = wanted_ink;
            }
        }
    }
}
