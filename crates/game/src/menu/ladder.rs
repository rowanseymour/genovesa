//! The pause menu, the options screen under it, and the way back down.
//!
//! Every screen from Options down is doubled — one [`AppState`] and one
//! [`Helm`] beside it — and that doubling is the point rather than an
//! oversight. The same screen reached from the main menu has no world behind
//! it; reached from the pause menu it must not take one down, and leaving
//! `AppState::InWorld` is exactly what would. One builder each, and the only
//! difference is which screen Back returns to.
//!
//! [`over_a_world`] is the one thing that tells the two sides apart, and it
//! is asked of the state rather than remembered, so nothing has to be kept in
//! step with which door was used.

use bevy::prelude::*;

use crate::net::Hosting;
use crate::{AppState, Helm};

use super::display::Picking;
use super::kit::{
    cartouche_rule, heading, label, panel, screen, spawn_button, Palette, ON_PAPER, OVER_THE_WORLD,
    PANEL_PADDING,
};
use super::MenuButton;

/// The options screen: the way to the two below it, and nothing else.
///
/// A screen that only points at other screens has to earn the press it costs,
/// and this one does by what it keeps *out* of the pause menu. Both of the
/// screens under it are long — ten key rows, or a list of resolutions — and
/// hanging either off the pause menu directly would put a wall of settings one
/// press from the helm.
pub(super) fn spawn_options(commands: Commands) {
    build_options(commands, &ON_PAPER, DespawnOnExit(AppState::Options));
}

/// The same screen over a paused world, differing only in living and dying
/// with [`Helm::Options`] instead — so that opening it does not leave the
/// world, which is the whole reason the pause menu exists.
pub(super) fn spawn_paused_options(commands: Commands) {
    build_options(commands, &OVER_THE_WORLD, DespawnOnExit(Helm::Options));
}

pub(super) fn build_options(mut commands: Commands, ink: &Palette, until: impl Bundle) {
    commands
        .spawn((Name::new("Options screen"), until, screen(ink)))
        .with_children(|screen| {
            screen
                .spawn(panel(ink, 10.0, PANEL_PADDING))
                .with_children(|panel| {
                    cartouche_rule(panel, ink);
                    heading(panel, ink, "Options", 6.0);

                    panel
                        .spawn(Node {
                            flex_direction: FlexDirection::Column,
                            align_items: AlignItems::Center,
                            row_gap: Val::Px(8.0),
                            margin: UiRect::top(Val::Px(16.0)),
                            ..default()
                        })
                        .with_children(|rows| {
                            spawn_button(rows, ink, MenuButton::Display, "Display", 200.0);
                            spawn_button(rows, ink, MenuButton::Controls, "Controls", 200.0);
                            spawn_button(rows, ink, MenuButton::Back, "Back", 200.0);
                        });
                });
        });
}

/// Whether the screen currently open is the one over a paused world rather
/// than the one over the main menu. Each of the three screens under Options is
/// built once and stood up on either side — see [`build_options`] — and this is
/// the only thing that tells the two apart.
pub(super) fn over_a_world(helm: &Option<Res<State<Helm>>>) -> bool {
    helm.as_ref()
        .is_some_and(|h| matches!(h.get(), Helm::Options | Helm::Display | Helm::Controls))
}

/// Shuts the options screen, returning to whichever screen opened it.
pub(super) fn close_options(
    over_a_world: bool,
    next_app: &mut NextState<AppState>,
    next_helm: &mut NextState<Helm>,
) {
    if over_a_world {
        next_helm.set(Helm::Paused);
    } else {
        next_app.set(AppState::MainMenu);
    }
}

/// And shuts either screen under it, which both go back to Options rather than
/// all the way out.
pub(super) fn back_to_options(
    over_a_world: bool,
    next_app: &mut NextState<AppState>,
    next_helm: &mut NextState<Helm>,
) {
    if over_a_world {
        next_helm.set(Helm::Options);
    } else {
        next_app.set(AppState::Options);
    }
}

pub(super) fn options_actions(
    buttons: Query<(&Interaction, &MenuButton), Changed<Interaction>>,
    helm: Option<Res<State<Helm>>>,
    mut next_app: ResMut<NextState<AppState>>,
    mut next_helm: ResMut<NextState<Helm>>,
) {
    let over_a_world = over_a_world(&helm);

    for (interaction, button) in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match button {
            MenuButton::Display if over_a_world => next_helm.set(Helm::Display),
            MenuButton::Display => next_app.set(AppState::Display),
            MenuButton::Controls if over_a_world => next_helm.set(Helm::Controls),
            MenuButton::Controls => next_app.set(AppState::Controls),
            MenuButton::Back => close_options(over_a_world, &mut next_app, &mut next_helm),
            _ => {}
        }
    }
}

/// Escape on the options screen and on the display screen: one step back,
/// which is what Back does and what it means everywhere else in the menus —
/// except that on the display screen a dropped list of resolutions is a step
/// of its own, and is shut before the screen is.
///
/// The controls screen is not here, and that is the whole reason this is a
/// system of its own rather than an arm of [`helm_keys`]: there, Escape may
/// mean "not that key" instead, and only [`settings_keys`] knows which.
pub(super) fn options_keys(
    keys: Res<ButtonInput<KeyCode>>,
    state: Res<State<AppState>>,
    helm: Option<Res<State<Helm>>>,
    mut picking: ResMut<Picking>,
    mut next_app: ResMut<NextState<AppState>>,
    mut next_helm: ResMut<NextState<Helm>>,
) {
    if !keys.just_pressed(KeyCode::Escape) {
        return;
    }
    let over_a_world = over_a_world(&helm);
    let on = |screen: AppState, paused: Helm| {
        if over_a_world {
            helm.as_ref().is_some_and(|h| *h.get() == paused)
        } else {
            *state.get() == screen
        }
    };

    if on(AppState::Options, Helm::Options) {
        close_options(over_a_world, &mut next_app, &mut next_helm);
    } else if on(AppState::Display, Helm::Display) {
        // A dropped list answers the first Escape and the screen the second,
        // the way a waiting key row does on the controls screen: what was
        // opened last is what is shut first.
        if picking.0 {
            picking.0 = false;
        } else {
            back_to_options(over_a_world, &mut next_app, &mut next_helm);
        }
    }
}

/// The pause menu, over the world rather than instead of it.
///
/// A panel like the dialogs, on the dimmed [`screen`] they all stand on, so
/// the water carries on moving behind it and it is plain that the world is
/// still there. Leaving is the last of the three and says which world it is
/// leaving, because it is the one button here that cannot be taken back.
pub(super) fn spawn_pause_menu(mut commands: Commands, hosting: Option<Res<Hosting>>) {
    // Only a *shared* world is worth a word of warning. Every world is served,
    // a solo one over the loopback — see [`Hosting::shared`] — so saying it
    // whenever there is a host would tell a player sailing alone that they are
    // about to strand somebody.
    let shared = hosting.is_some_and(|hosting| hosting.shared());
    let ink = OVER_THE_WORLD;
    commands
        .spawn((
            Name::new("Pause menu"),
            DespawnOnExit(Helm::Paused),
            screen(&ink),
        ))
        .with_children(|screen| {
            screen
                .spawn(panel(&ink, 10.0, PANEL_PADDING))
                .with_children(|panel| {
                    heading(panel, &ink, "Paused", 6.0);
                    if shared {
                        label(panel, &ink, "others can be sailing in this world, and");
                        label(panel, &ink, "leaving closes it on them");
                    }

                    panel
                        .spawn(Node {
                            flex_direction: FlexDirection::Column,
                            align_items: AlignItems::Center,
                            row_gap: Val::Px(8.0),
                            margin: UiRect::top(Val::Px(20.0)),
                            ..default()
                        })
                        .with_children(|rows| {
                            spawn_button(rows, &ink, MenuButton::Resume, "Resume", 200.0);
                            spawn_button(rows, &ink, MenuButton::Options, "Options", 200.0);
                            spawn_button(rows, &ink, MenuButton::LeaveWorld, "Leave World", 200.0);
                        });
                });
        });
}

pub(super) fn pause_actions(
    buttons: Query<(&Interaction, &MenuButton), Changed<Interaction>>,
    mut next_app: ResMut<NextState<AppState>>,
    mut next_helm: ResMut<NextState<Helm>>,
) {
    for (interaction, button) in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match button {
            MenuButton::Resume => next_helm.set(Helm::Sailing),
            MenuButton::Options => next_helm.set(Helm::Options),
            // The only way out. Leaving `InWorld` is what takes the world
            // down — see [`Helm`] — so this is the one press that does.
            MenuButton::LeaveWorld => next_app.set(AppState::MainMenu),
            _ => {}
        }
    }
}

/// Escape inside a world: into the pause menu from the helm, and back out of
/// it again.
///
/// One step back, never all the way out. Leaving a world is a button now, and
/// that is the point — it used to be this key, and a world is far too
/// expensive to lose to a mispress.
pub(super) fn helm_keys(
    keys: Res<ButtonInput<KeyCode>>,
    helm: Res<State<Helm>>,
    mut next: ResMut<NextState<Helm>>,
) {
    if !keys.just_pressed(KeyCode::Escape) {
        return;
    }
    match helm.get() {
        Helm::Sailing => next.set(Helm::Paused),
        Helm::Paused => next.set(Helm::Sailing),
        // Not ours. On the options and display screens Escape means one step
        // back and `options_keys` takes it; on the controls screen it may mean
        // "not that key" instead, and only `settings_keys` knows which; at the
        // console it means "close the console"; on the chart it may mean
        // "stop writing this island's name". Each screen hears its own key,
        // because only it knows what the key means while it is up.
        Helm::Options | Helm::Display | Helm::Controls | Helm::Console | Helm::Chart => {}
    }
}
