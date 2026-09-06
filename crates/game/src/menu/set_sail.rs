//! The kept worlds, and the way back into one.
//!
//! A row per world this machine is keeping, newest first, each with a way to
//! throw it away — and the question that asks first. The row *becomes* the
//! question rather than growing a second one beside it, so there is never a
//! screen with two things to read on one line, and a question walked away
//! from is not a yes.
//!
//! A row is the world's name — the one the player gave it when it was made,
//! see [`world_title`] — over how far into its days it is and when it was
//! last sailed, which is what puts the rows in order — see [`world_detail`].

use std::time::SystemTime;

use bevy::prelude::*;

use crate::net::{self, Dialing, Reach};
use crate::AppState;
use protocol::{DAY_SECONDS, DEFAULT_PORT};
use server::{kept_worlds, KeptWorld};

use super::kit::{
    button, button_label, cartouche_rule, heading, label, panel, screen, spawn_button, status_line,
    Palette, ON_PAPER, PANEL_PADDING,
};
use super::{share_label, MenuButton, Status};

/// The kept worlds the set-sail screen is offering, read off the worlds
/// directory each time the screen opens, and whether the one chosen will be
/// shared. The rows hold indexes into this list — see
/// [`MenuButton::OpenKept`].
///
/// The screen is built from this and nothing else, and built again whenever it
/// changes — see [`show_set_sail`] — which is what lets a row stop being a
/// world and become a question.
#[derive(Resource, Default)]
pub(super) struct Harbour {
    pub(super) worlds: Vec<server::KeptWorld>,
    pub(super) share: bool,
    /// The row that has been asked about, if any: its Discard has been pressed
    /// and the row is now the question, waiting to be answered. One at a time,
    /// because "yes" has to mean one world.
    pub(super) asked: Option<usize>,
}

impl Harbour {
    /// Whether this machine is keeping as many worlds as it will — which is
    /// what closes the way to a new one. `>=` rather than `==`: a directory
    /// somebody has copied worlds into can hold more than the cap, and it is
    /// full then too.
    fn is_full(&self) -> bool {
        self.worlds.len() >= MOST_KEPT_WORLDS
    }
}

/// How many worlds this machine will keep at once.
///
/// A real limit rather than a limit on the rows: the screen used to offer the
/// newest eight and summarise the rest as older, which meant a world that
/// could be neither returned to nor thrown away — a dead end is worse than a
/// limit. So every kept world is on the screen, and with five of them the way
/// to a sixth is closed until one is discarded, which is one press away on the
/// same screen.
///
/// Five because a world is a place to live in rather than a slot to fill, and
/// somebody keeping more than five is really keeping a list they no longer
/// read. The cap is this screen's, not the server's: a dedicated host may keep
/// a directory of as many worlds as it likes, and a directory that already
/// holds more than five is shown whole here rather than truncated.
pub(super) const MOST_KEPT_WORLDS: usize = 5;

/// A kept world's row: the world's own button, the press beside it, and the
/// gap between them.
pub(super) const KEPT_WIDTH: f32 = 340.0;

pub(super) const DISCARD_WIDTH: f32 = 118.0;

pub(super) const ANSWER_GAP: f32 = 6.0;

/// Each of the two answers that replace the Discard button when a row is asked
/// about. Half of what they stand in for, less the gap and the pixel a border
/// takes on either side of each of them, so the pair ends exactly where the one
/// button ended.
pub(super) const ANSWER_WIDTH: f32 = (DISCARD_WIDTH - ANSWER_GAP - 2.0) / 2.0;

/// How wide a row is, whichever face it is wearing.
///
/// Set on the row rather than left to the sum of what is in it, so that a row
/// becoming a question cannot make the panel a different width — and so that
/// the two answers land where the button they replaced was, whatever the
/// arithmetic of borders comes to. Two pixels over the widths inside it, one
/// for each button's border.
pub(super) const KEPT_ROW_WIDTH: f32 = KEPT_WIDTH + ANSWER_GAP + DISCARD_WIDTH + 4.0;

/// Marks the set-sail screen, so it can be taken down and built again — which
/// is how a row becomes a question. See [`show_set_sail`].
#[derive(Component)]
pub(super) struct SetSailScreen;

/// Reads the harbour off the worlds directory.
///
/// Fresh on every visit to the screen: worlds are files, and files can have
/// been copied in, deleted, or sailed from another install since the last
/// look. Any question left standing from last time goes with it — see
/// [`MenuButton::KeepIt`].
pub(super) fn read_the_harbour(mut harbour: ResMut<Harbour>) {
    harbour.worlds = net::worlds_dir()
        .map(|dir| kept_worlds(&dir))
        .unwrap_or_default();
    harbour.asked = None;
}

/// Builds the set-sail screen, and builds it again whenever the harbour
/// changes.
///
/// A row asked about is not the same row with a different label on it: the
/// world's own button is gone, replaced by the question and the two answers,
/// so that the one press that opens a world cannot be the one press that was
/// aimed at throwing it away. Rebuilding the list is the honest way to say
/// that, and the list is a handful of buttons — it costs a frame's worth of
/// spawning, on a screen where nothing is moving.
pub(super) fn show_set_sail(
    mut commands: Commands,
    harbour: Res<Harbour>,
    status: Res<Status>,
    standing: Query<Entity, With<SetSailScreen>>,
) {
    if !harbour.is_changed() && !standing.is_empty() {
        return;
    }
    for screen in &standing {
        commands.entity(screen).despawn();
    }

    let ink = ON_PAPER;
    commands
        .spawn((
            Name::new("Set sail dialog"),
            SetSailScreen,
            DespawnOnExit(AppState::SetSail),
            screen(&ink),
        ))
        .with_children(|screen| {
            screen
                .spawn(panel(&ink, 10.0, PANEL_PADDING))
                .with_children(|panel| {
                    cartouche_rule(panel, &ink);
                    heading(panel, &ink, "Set Sail", 20.0);
                    if harbour.worlds.is_empty() {
                        label(panel, &ink, "no world sailed from here yet");
                    } else {
                        label(panel, &ink, "return to a world this machine keeps");
                    }

                    // Every one of them, however many there are: a world on
                    // this machine that the screen does not offer is a world
                    // nobody can reach — see [`MOST_KEPT_WORLDS`].
                    for (row, world) in harbour.worlds.iter().enumerate() {
                        if harbour.asked == Some(row) {
                            spawn_question(panel, &ink, row);
                        } else {
                            spawn_kept_row(panel, &ink, row, world);
                        }
                    }
                    // Said as a state rather than as a count: a directory
                    // somebody has copied a sixth world into is full too, and
                    // a line naming five while six are listed would be a line
                    // arguing with the rows above it. What to do about it is
                    // the answer to the press — see [`set_sail_actions`].
                    if harbour.is_full() {
                        label(panel, &ink, "no room for another world");
                    }

                    // The same switch the new-world dialog carries, meaning
                    // the same thing: who can reach the world about to be
                    // served.
                    panel
                        .spawn(Node {
                            margin: UiRect::top(Val::Px(20.0)),
                            ..default()
                        })
                        .with_children(|row| {
                            spawn_button(
                                row,
                                &ink,
                                MenuButton::ToggleKeptShare,
                                share_label(harbour.share),
                                160.0,
                            );
                        });
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
                            spawn_button(row, &ink, MenuButton::NewWorld, "New World", 160.0);
                        });
                });
        });
}

/// One kept world: the press that returns to it, and beside it the press that
/// asks about throwing it away.
pub(super) fn spawn_kept_row(
    parent: &mut ChildSpawnerCommands,
    ink: &Palette,
    row: usize,
    world: &KeptWorld,
) {
    parent.spawn(kept_row()).with_children(|line| {
        // Two lines on the one button, the second smaller and dimmer: the
        // name is what is being chosen, the rest is what tells this row from
        // the one under it. One text with a span rather than two texts, so
        // the button lays out one label like every other button does.
        line.spawn(button(ink, MenuButton::OpenKept(row), KEPT_WIDTH))
            .with_children(|button| {
                button
                    .spawn(button_label(ink, &world_title(world)))
                    .with_children(|title| {
                        title.spawn((
                            TextSpan::new(format!("\n{}", world_detail(world))),
                            TextFont {
                                font_size: FontSize::Px(15.0),
                                ..default()
                            },
                            TextColor(ink.dim),
                        ));
                    });
            });
        spawn_button(
            line,
            ink,
            MenuButton::AskDiscard(row),
            "Discard",
            DISCARD_WIDTH,
        );
    });
}

/// And the same row once it has been asked about. The question stands where
/// the world's name stood, which is what says *this* world: there is nothing
/// else on the row to mean.
pub(super) fn spawn_question(parent: &mut ChildSpawnerCommands, ink: &Palette, row: usize) {
    parent.spawn(kept_row()).with_children(|line| {
        line.spawn((
            Text::new("Discard this world for good?"),
            TextFont {
                font_size: FontSize::Px(17.0),
                ..default()
            },
            TextColor(ink.text),
        ));
        line.spawn(Node {
            column_gap: Val::Px(ANSWER_GAP),
            ..default()
        })
        .with_children(|answers| {
            spawn_button(answers, ink, MenuButton::Discard(row), "Yes", ANSWER_WIDTH);
            spawn_button(answers, ink, MenuButton::KeepIt, "No", ANSWER_WIDTH);
        });
    });
}

/// The line a kept world is laid out along, whichever of its two faces it is
/// wearing: what it opens with on the left, what it is answered with on the
/// right, and the row's own width holding the two apart.
pub(super) fn kept_row() -> Node {
    Node {
        width: Val::Px(KEPT_ROW_WIDTH),
        align_items: AlignItems::Center,
        justify_content: JustifyContent::SpaceBetween,
        column_gap: Val::Px(ANSWER_GAP),
        ..default()
    }
}

/// What a kept world's row is headed by: the name the player gave it. Every
/// world made from the menu has one, so a world without is one that came
/// from somewhere else — a dedicated server's file copied in — and is said
/// to be nameless rather than shown as a blank.
pub(super) fn world_title(world: &KeptWorld) -> String {
    if world.name.is_empty() {
        "a world without a name".to_string()
    } else {
        world.name.clone()
    }
}

/// The line under the title: how far into its days the world is, and when it
/// was last sailed — which, on a machine keeping several, is what says which
/// of them is the latest.
pub(super) fn world_detail(world: &KeptWorld) -> String {
    let day = (world.age / DAY_SECONDS) as u32 + 1;
    format!("Day {day} — {}", sailed_when(world.kept))
}

/// A last-sailed moment as prose, as fine as it takes to put the rows in
/// order and no finer: minutes within the hour, hours within the day, then
/// days, weeks and months. It used to stop at "today", and three worlds all
/// sailed today said nothing about which was the one left an hour ago.
pub(super) fn sailed_when(kept: SystemTime) -> String {
    // A file stamped in the future is a clock that moved, not a world that
    // has not been played yet — and reads as one just sailed.
    let since = kept.elapsed().unwrap_or_default().as_secs();
    const HOUR: u64 = 60 * 60;
    const DAY: u64 = 24 * HOUR;
    let (count, unit) = match since {
        s if s < 60 => return "sailed just now".to_string(),
        s if s < HOUR => (s / 60, "minute"),
        s if s < DAY => (s / HOUR, "hour"),
        s if s < 2 * DAY => return "sailed yesterday".to_string(),
        s if s < 7 * DAY => (s / DAY, "day"),
        s if s < 30 * DAY => (s / (7 * DAY), "week"),
        s => (s / (30 * DAY), "month"),
    };
    let ago = match (count, unit) {
        (1, "hour") => "an hour".to_string(),
        (1, unit) => format!("a {unit}"),
        (count, unit) => format!("{count} {unit}s"),
    };
    format!("sailed {ago} ago")
}

pub(super) fn set_sail_actions(
    mut commands: Commands,
    buttons: Query<(&Interaction, &MenuButton), Changed<Interaction>>,
    dialing: Option<Res<Dialing>>,
    mut harbour: ResMut<Harbour>,
    mut status: ResMut<Status>,
    mut next: ResMut<NextState<AppState>>,
) {
    for (interaction, button) in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match button {
            MenuButton::Back => next.set(AppState::MainMenu),
            // The one door the cap closes, and the only place it is enforced:
            // a world is started from the dialog behind this press, and this
            // is the press that opens it.
            MenuButton::NewWorld if harbour.is_full() => {
                status.0 = "discard a world to make room for another".to_string();
            }
            MenuButton::NewWorld => next.set(AppState::NewWorld),
            MenuButton::ToggleKeptShare => harbour.share = !harbour.share,
            MenuButton::AskDiscard(row) => harbour.asked = Some(*row),
            MenuButton::KeepIt => harbour.asked = None,
            MenuButton::Discard(row) => discard_kept(&mut harbour, *row, &mut status),
            // Already ringing: asking for a second world would only fail on
            // the lock the first one is taking.
            MenuButton::OpenKept(_) if dialing.is_some() => {}
            MenuButton::OpenKept(row) => {
                let Some(world) = harbour.worlds.get(*row) else {
                    continue;
                };
                status.0 = "reopening the world...".to_string();
                commands.insert_resource(Dialing::reopening(
                    world.path.clone(),
                    if harbour.share {
                        Reach::Shared
                    } else {
                        Reach::Alone
                    },
                ));
            }
            _ => {}
        }
    }
}

/// Throws a kept world away, having been asked twice.
///
/// Both halves of what this machine holds of the place go: the world's own
/// file, which is its history, and this machine's logbook for it, which is the
/// player's name in it. Leaving the book would leave papers for somewhere that
/// no longer exists.
///
/// A world being hosted refuses — see [`server::discard`] — and that refusal is
/// worth reporting rather than swallowing: it is the one case where a player
/// presses yes and nothing happens.
pub(super) fn discard_kept(harbour: &mut Harbour, row: usize, status: &mut Status) {
    harbour.asked = None;
    let Some((path, id)) = harbour
        .worlds
        .get(row)
        .map(|world| (world.path.clone(), world.id))
    else {
        return;
    };

    match server::discard(&path) {
        Ok(()) => {
            crate::logbook::forget(id);
            harbour.worlds.remove(row);
        }
        Err(problem) => status.0 = format!("the world could not be discarded: {problem}"),
    }
}
