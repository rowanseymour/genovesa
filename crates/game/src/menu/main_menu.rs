//! The screen the game opens on: the title, and the four ways out of it.
//!
//! The title is set in a serif from the machine's own fonts rather than the
//! face compiled into the engine — see [`title_font`] — and letter-spaced by
//! [`spaced`], since a title is read as a shape rather than as a word.
//!
//! Nothing here opens a world. Every way in from this screen is a way to
//! another screen, because opening one takes a seed or an address and this
//! has room for neither.

use bevy::prelude::*;
use bevy::text::{FontSize, FontSource, FontStyle};

use crate::AppState;

use super::kit::{cartouche_rule, panel, screen, spawn_button, Palette, ON_PAPER, PANEL_PADDING};
use super::MenuButton;

/// The word on the front of the box, the line under it, and how big each is
/// drawn.
pub(super) const TITLE: &str = "GENOVESA";

pub(super) const TITLE_SIZE: f32 = 56.0;

pub(super) const SUBTITLE: &str = "the sea is charted no further";

pub(super) const SUBTITLE_SIZE: f32 = 19.0;

/// How far the rules either side of the title run out.
pub(super) const TITLE_RULE: f32 = 64.0;

/// The front screen.
///
/// One way to a world of one's own, not two: "Set Sail" and "New World" both
/// read as *start playing*, and a player made to tell them apart before they
/// have seen either is being asked about the machinery. So setting sail is the
/// whole of it — the worlds this machine keeps and the way to a fresh one are
/// one screen, because they answer one question.
pub(super) fn spawn_main_menu(mut commands: Commands) {
    // The whole menu goes inside the cartouche, which is where an engraved
    // chart carries its title and everything said about it.
    let ink = ON_PAPER;
    commands
        .spawn((
            Name::new("Main menu"),
            DespawnOnExit(AppState::MainMenu),
            screen(&ink),
        ))
        .with_children(|screen| {
            screen
                .spawn(panel(&ink, 12.0, PANEL_PADDING))
                .with_children(|panel| {
                    cartouche_rule(panel, &ink);
                    spawn_title(panel, &ink);
                    spawn_subtitle(panel, &ink);

                    spawn_button(panel, &ink, MenuButton::SetSail, "Set Sail", 240.0);
                    spawn_button(panel, &ink, MenuButton::JoinWorld, "Join World", 240.0);
                    spawn_button(panel, &ink, MenuButton::Options, "Options", 240.0);
                    spawn_button(panel, &ink, MenuButton::Exit, "Exit", 240.0);
                });
        });
}

/// The title, engraved and ruled: tracked-out serif capitals with a hairline
/// running out to either side of them, which is how a chart of the period puts
/// its own name at the top.
pub(super) fn spawn_title(parent: &mut ChildSpawnerCommands, ink: &Palette) {
    let text = ink.text;
    parent
        .spawn(Node {
            align_items: AlignItems::Center,
            column_gap: Val::Px(24.0),
            margin: UiRect::bottom(Val::Px(10.0)),
            ..default()
        })
        .with_children(|row| {
            spawn_hairline(row, text);
            row.spawn((Text::new(spaced(TITLE)), title_font(), TextColor(text)));
            spawn_hairline(row, text);
        });
}

/// The line under the title, in the hand a chart names its waters in.
///
/// Drawn in the same ink as the title rather than the dim grey the panels use
/// for their asides: this one is read against open water, which the menu only
/// dims rather than covers, and a grey that sits well on a panel disappears
/// against it.
pub(super) fn spawn_subtitle(parent: &mut ChildSpawnerCommands, ink: &Palette) {
    parent.spawn((
        Text::new(SUBTITLE),
        subtitle_font(),
        TextColor(ink.text),
        Node {
            margin: UiRect::bottom(Val::Px(48.0)),
            ..default()
        },
    ));
}

/// The face both lines are set in: resolved from the system's font database
/// rather than shipped, so the menu gains a serif without the game gaining an
/// assets directory. The exact face is the machine's to choose; every one of
/// them has the strokes the engraving is drawn from.
pub(super) fn title_font() -> TextFont {
    TextFont {
        font: FontSource::Serif,
        font_size: FontSize::Px(TITLE_SIZE),
        ..default()
    }
}

pub(super) fn subtitle_font() -> TextFont {
    TextFont {
        style: FontStyle::Italic,
        font_size: FontSize::Px(SUBTITLE_SIZE),
        ..title_font()
    }
}

/// One of the rules the title sits between.
///
/// Full ink rather than [`Palette::edge`], which every border on screen is
/// drawn in. It belongs to the title rather than to the furniture, and a rule
/// that runs out of a letter has to be the same weight as the letter.
pub(super) fn spawn_hairline(parent: &mut ChildSpawnerCommands, ink: Color) {
    parent.spawn((
        Node {
            width: Val::Px(TITLE_RULE),
            height: Val::Px(1.0),
            ..default()
        },
        BackgroundColor(ink),
    ));
}

/// The letters of a word with a space between each. Tracking of the kind
/// engraved capitals are set with is not something a text style can ask for, so
/// it is spelled into the string instead.
pub(super) fn spaced(word: &str) -> String {
    let mut spaced = String::with_capacity(word.len() * 2);
    for letter in word.chars() {
        if !spaced.is_empty() {
            spaced.push(' ');
        }
        spaced.push(letter);
    }
    spaced
}

pub(super) fn main_menu_actions(
    buttons: Query<(&Interaction, &MenuButton), Changed<Interaction>>,
    mut next: ResMut<NextState<AppState>>,
    mut exit: MessageWriter<AppExit>,
) {
    for (interaction, button) in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match button {
            MenuButton::SetSail => next.set(AppState::SetSail),
            MenuButton::JoinWorld => next.set(AppState::JoinWorld),
            MenuButton::Options => next.set(AppState::Options),
            MenuButton::Exit => {
                exit.write(AppExit::Success);
            }
            _ => {}
        }
    }
}
