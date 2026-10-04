//! The menus: the screens the game is entered, left and configured through.
//!
//! One module per screen, and [`kit`] for the furniture they are all built
//! out of. What stays here is the plugin that stands them up, the vocabulary
//! of buttons they answer to ([`MenuButton`]), and the one line of text three
//! of them share ([`Status`]).
//!
//! The screens under Options are each reached two ways — from the main menu,
//! and from the pause menu over a world that must not be taken down to get to
//! them. See [`ladder`], which is where that doubling lives.

use bevy::prelude::*;

use crate::bindings::{Action, KeyBindings};
use crate::camera::View;
use crate::net::{Dialing, Hosting, Online};
use crate::settings::{self, DisplaySettings, OnTrial, Resolution, Wanted};
use crate::{AppState, Helm};

mod controls;
mod display;
mod join;
mod kit;
mod ladder;
mod main_menu;
mod new_world;
mod set_sail;

use controls::{
    cancel_rebinding, refresh_settings, settings_actions, settings_keys, spawn_paused_settings,
    spawn_settings, Rebinding,
};
use display::{
    close_display, display_actions, open_display, refresh_display, refresh_picker, run_trial,
    spawn_display, spawn_paused_display, Picking,
};
use join::{join_actions, join_keys, read_address, spawn_join_dialog, JoinSettings};
use kit::{scroll_the_panel, Highlight};
use ladder::{
    back_to_options, helm_keys, options_actions, options_keys, over_a_world, pause_actions,
    spawn_options, spawn_pause_menu, spawn_paused_options,
};
use main_menu::{main_menu_actions, spawn_main_menu};
use new_world::{
    dialog_actions, open_world, read_field, refresh_dialog, settle_seed, share_label,
    spawn_new_world_dialog, Field, NewWorldSettings,
};
use set_sail::{read_the_harbour, set_sail_actions, show_set_sail, Harbour};

fn highlight_buttons(
    rebinding: Res<Rebinding>,
    picking: Res<Picking>,
    wanted: Res<Wanted>,
    mut buttons: Query<
        (&Interaction, &MenuButton, &Highlight, &mut BackgroundColor),
        Changed<Interaction>,
    >,
) {
    for (interaction, button, ink, mut color) in &mut buttons {
        let idle = match button {
            MenuButton::Rebind(action) if rebinding.0 == Some(*action) => ink.armed,
            // The two things an open list has to say: that it is open, and
            // which of its rungs is the one already taken. Both are the same
            // colour a waiting key row is, which is the menu's one word for
            // *this is the live one*.
            MenuButton::OpenResolutions if picking.0 => ink.armed,
            MenuButton::PickResolution(rung) if wanted.0.resolution == *rung => ink.armed,
            _ => ink.idle,
        };

        *color = BackgroundColor(match interaction {
            Interaction::Pressed => ink.press,
            Interaction::Hovered => ink.hover,
            Interaction::None => idle,
        });
    }
}

pub struct MenuPlugin;

impl Plugin for MenuPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NewWorldSettings>()
            .init_resource::<JoinSettings>()
            .init_resource::<Harbour>()
            .init_resource::<Status>()
            // Shared with the camera, which registers them too — see
            // `MapCameraPlugin`.
            .init_resource::<KeyBindings>()
            .init_resource::<Rebinding>()
            // Shared the same way with the plugin that keeps them and the one
            // that applies them to the sun — see [`crate::settings`]. The
            // display screen edits what is *wanted* and Apply is what makes
            // that the settings; nothing here touches the window.
            .init_resource::<DisplaySettings>()
            .init_resource::<Wanted>()
            .init_resource::<OnTrial>()
            .init_resource::<Picking>()
            // The text fields are Bevy's, and tell what they hold by event.
            .add_observer(read_field)
            .add_observer(settle_seed)
            .add_observer(read_address)
            .add_systems(OnEnter(AppState::MainMenu), spawn_main_menu)
            .add_systems(OnEnter(AppState::Options), spawn_options)
            // Set from the machine before the screen is built out of it.
            .add_systems(
                OnEnter(AppState::Display),
                (open_display, spawn_display).chain(),
            )
            .add_systems(OnEnter(AppState::Controls), spawn_settings)
            // The pause menu and the screens behind it, all standing over a
            // world that is still running — see [`Helm`].
            .add_systems(OnEnter(Helm::Paused), spawn_pause_menu)
            .add_systems(OnEnter(Helm::Options), spawn_paused_options)
            .add_systems(
                OnEnter(Helm::Display),
                (open_display, spawn_paused_display).chain(),
            )
            .add_systems(OnEnter(Helm::Controls), spawn_paused_settings)
            // Cleared before the screen is built, so that a screen only ever
            // shows what this visit to it has had to say.
            //
            // The set-sail screen is not built here, unlike the two below it:
            // entering it only reads the worlds directory, and `show_set_sail`
            // builds the screen out of what was read — and again every time it
            // changes, which is how a row becomes a question.
            .add_systems(
                OnEnter(AppState::SetSail),
                (clear_status, read_the_harbour).chain(),
            )
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
            .add_systems(OnExit(AppState::SetSail), stop_dialing)
            .add_systems(OnExit(AppState::NewWorld), stop_dialing)
            .add_systems(OnExit(AppState::JoinWorld), stop_dialing)
            // Leaving the screen mid-capture would otherwise come back to it
            // still waiting for a key. Both ways to the same screen.
            .add_systems(OnExit(AppState::Controls), cancel_rebinding)
            .add_systems(OnExit(Helm::Controls), cancel_rebinding)
            // And leaving the display screen settles what it had in the air:
            // a trial nobody stopped is stood by, a list still down is shut.
            .add_systems(OnExit(AppState::Display), close_display)
            .add_systems(OnExit(Helm::Display), close_display)
            .add_systems(
                Update,
                (
                    highlight_buttons,
                    settle_dialing.run_if(resource_exists::<Dialing>),
                    main_menu_actions.run_if(in_state(AppState::MainMenu)),
                    // Chained so that a press and the screen it changes land
                    // in the same frame: a row asked about must not be a row
                    // that spends a frame looking as though nothing happened.
                    (set_sail_actions, show_set_sail)
                        .chain()
                        .run_if(in_state(AppState::SetSail)),
                    (dialog_actions, open_world, refresh_dialog)
                        .run_if(in_state(AppState::NewWorld)),
                    // `join_keys` carries no run condition of its own, for the
                    // reason `settings_keys` below carries none.
                    (
                        join_actions.run_if(in_state(AppState::JoinWorld)),
                        join_keys,
                    )
                        .chain(),
                    // Both dialogs have a line for how a dial is going, so
                    // whichever is on screen owns the only ones that exist.
                    refresh_status,
                    // Ordered so that arming a row and reading the key meant for
                    // it can't land in the same frame. `settings_keys` carries
                    // no run condition of its own — see its comment. The rest
                    // run on the controls screen wherever it was opened from.
                    (
                        settings_actions
                            .run_if(in_state(AppState::Controls).or_else(in_state(Helm::Controls))),
                        settings_keys,
                        refresh_settings
                            .run_if(in_state(AppState::Controls).or_else(in_state(Helm::Controls))),
                    )
                        .chain(),
                    // The controls screen is also the one that can be taller
                    // than the window — see [`scrolling_panel`].
                    scroll_the_panel
                        .run_if(in_state(AppState::Controls).or_else(in_state(Helm::Controls))),
                    // The two screens above the controls, each on both of the
                    // states it can be reached through.
                    // Chained the whole way down, so that everything a press
                    // sets off lands on the frame it was pressed rather than
                    // the frame after: Escape shuts the list, a press opens
                    // it, a trial that has run out is put back, and only then
                    // is the screen written out of what all of that left
                    // behind. `options_keys` is in the chain rather than
                    // beside it for exactly that reason — a list Escape shut
                    // has to be gone before anything goes looking for it.
                    (
                        options_actions
                            .run_if(in_state(AppState::Options).or_else(in_state(Helm::Options))),
                        // Escape on any of the three screens, which all mean
                        // the same thing by it: one step back. The controls
                        // screen is the exception and hears its own — see
                        // [`settings_keys`].
                        options_keys,
                        (display_actions, refresh_picker, run_trial, refresh_display)
                            .chain()
                            .run_if(in_state(AppState::Display).or_else(in_state(Helm::Display))),
                    )
                        .chain(),
                    pause_actions.run_if(in_state(Helm::Paused)),
                    // Wants the state itself, so it can only run where there
                    // is one — which is to say, inside a world.
                    helm_keys.run_if(in_state(AppState::InWorld)),
                ),
            );
    }
}

/// How a dial is going, for the screen that started it: empty when there is
/// nothing to say, otherwise a line of prose the player reads and acts on.
#[derive(Resource, Default)]
struct Status(String);

#[derive(Component, Clone, Copy, PartialEq, Debug)]
pub(crate) enum MenuButton {
    /// Opens the kept-worlds screen, which is the only way to a world of one's
    /// own — see [`spawn_main_menu`].
    SetSail,
    /// Opens the new-world dialog, from the set-sail screen.
    NewWorld,
    JoinWorld,
    /// Opens the options screen, from the main menu or the pause menu.
    Options,
    /// And the two screens it is the way to.
    Display,
    Controls,
    Exit,
    /// Puts the keyboard in this field of the new-world dialog.
    Edit(Field),
    RandomSeed,
    /// Turns sharing the world about to be started on and off.
    ToggleShare,
    Start,
    /// Reopens the kept world at this row of the [`Harbour`].
    OpenKept(usize),
    /// Asks about this row: whether the world on it is to be thrown away. The
    /// row becomes the question rather than growing a second one beside it.
    AskDiscard(usize),
    /// Answers it: this world goes, for good.
    Discard(usize),
    /// And answers it the other way. Leaving the screen answers it too — a
    /// question walked away from is not a yes.
    KeepIt,
    /// Turns sharing the kept world about to be reopened on and off.
    ToggleKeptShare,
    /// Dials the address on the join screen.
    Connect,
    /// Arms this action's row, so the next key pressed becomes its key.
    Rebind(Action),
    ResetKeys,
    /// Fills the screen, or gives it back.
    ToggleFullscreen,
    /// Drops the list of resolutions, or takes it back up.
    OpenResolutions,
    /// Takes the rung on this line of the list — see [`settings::LADDER`].
    PickResolution(Resolution),
    /// Shuts the list without taking anything from it. Carried by the sheet
    /// spread behind the open list, so that a click anywhere else on the
    /// screen is a click on this.
    ShutResolutions,
    /// Makes what the screen is set to what the machine does — and afterwards
    /// stands by it, which is the same button because during a trial there is
    /// nothing else it could mean. See [`settings::OnTrial`].
    ApplyDisplay,
    Back,
    /// Puts the player back at the helm of the world behind the pause menu.
    Resume,
    /// Gives that world up. Named for what it costs rather than "Back", which
    /// on every other screen means one step and here means the whole world.
    LeaveWorld,
}

impl MenuButton {
    /// The bare word this button answers to in a `click` line — see
    /// [`crate::control`].
    ///
    /// Matched on `self` rather than listed beside the enum, which is the
    /// whole point: a button added to a screen cannot be added without being
    /// named, because the compiler asks. [`Action::name`] is the same shape
    /// for the same reason. The six that carry something are named without
    /// it; what comes after is [`MenuButton::parse`]'s business.
    ///
    /// One word, hyphenated where it needs to be, because a line is split on
    /// spaces and a name with one in it could not be typed.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::SetSail => "set-sail",
            Self::NewWorld => "new-world",
            Self::JoinWorld => "join-world",
            Self::Options => "options",
            Self::Display => "display",
            Self::Controls => "controls",
            Self::Exit => "exit",
            Self::Edit(_) => "edit",
            Self::RandomSeed => "random-seed",
            Self::ToggleShare => "share",
            Self::Start => "start",
            Self::OpenKept(_) => "open-kept",
            Self::AskDiscard(_) => "ask-discard",
            Self::Discard(_) => "discard",
            Self::KeepIt => "keep-it",
            Self::ToggleKeptShare => "kept-share",
            Self::Connect => "connect",
            Self::Rebind(_) => "rebind",
            Self::ResetKeys => "reset-keys",
            Self::ToggleFullscreen => "fullscreen",
            Self::OpenResolutions => "resolutions",
            Self::PickResolution(_) => "resolution",
            Self::ShutResolutions => "shut-resolutions",
            Self::ApplyDisplay => "apply",
            Self::Back => "back",
            Self::Resume => "resume",
            Self::LeaveWorld => "leave-world",
        }
    }

    /// One of every button, in the order a missed name is offered them:
    /// roughly the order a player meets them, screen by screen down from the
    /// main menu.
    ///
    /// The arguments here are placeholders and nothing reads them — what this
    /// list is for is [`MenuButton::name`], which does not look. It is the
    /// grammar's index rather than the grammar, and a test holds the two to
    /// agreement.
    const EVERY: [Self; 27] = [
        Self::SetSail,
        Self::NewWorld,
        Self::JoinWorld,
        Self::Options,
        Self::Display,
        Self::Controls,
        Self::Exit,
        Self::Edit(Field::Name),
        Self::RandomSeed,
        Self::ToggleShare,
        Self::Start,
        Self::OpenKept(0),
        Self::AskDiscard(0),
        Self::Discard(0),
        Self::KeepIt,
        Self::ToggleKeptShare,
        Self::Connect,
        Self::Rebind(Action::MoveForward),
        Self::ResetKeys,
        Self::ToggleFullscreen,
        Self::OpenResolutions,
        Self::PickResolution(Resolution::Native),
        Self::ShutResolutions,
        Self::ApplyDisplay,
        Self::Back,
        Self::Resume,
        Self::LeaveWorld,
    ];

    /// Every name there is, for the line a missed one is answered with — read
    /// off [`MenuButton::name`] so the offer and the grammar cannot disagree.
    pub(crate) fn names() -> Vec<&'static str> {
        Self::EVERY.iter().map(|button| button.name()).collect()
    }

    /// What the six buttons that carry something want said after their name.
    pub(crate) const WANTS: &'static str = "`open-kept`, `ask-discard` and `discard` want a row \
                                            number, `rebind` a control, `resolution` a rung or \
                                            `native`, `edit` a field: `name` or `seed`";

    /// The button a `click` line names, or `None` for a line that names none.
    ///
    /// One word and, for the six that carry something, one more.
    pub(crate) fn parse(words: &[&str]) -> Option<Self> {
        Some(match words {
            ["set-sail"] => Self::SetSail,
            ["new-world"] => Self::NewWorld,
            ["join-world"] => Self::JoinWorld,
            ["options"] => Self::Options,
            ["display"] => Self::Display,
            ["controls"] => Self::Controls,
            ["exit"] => Self::Exit,
            ["edit", field] => Self::Edit(
                [Field::Name, Field::Seed]
                    .into_iter()
                    .find(|candidate| candidate.name() == *field)?,
            ),
            ["random-seed"] => Self::RandomSeed,
            ["share"] => Self::ToggleShare,
            ["start"] => Self::Start,
            ["open-kept", row] => Self::OpenKept(row.parse().ok()?),
            ["ask-discard", row] => Self::AskDiscard(row.parse().ok()?),
            ["discard", row] => Self::Discard(row.parse().ok()?),
            ["keep-it"] => Self::KeepIt,
            ["kept-share"] => Self::ToggleKeptShare,
            ["connect"] => Self::Connect,
            ["rebind", control] => Self::Rebind(
                Action::ALL
                    .into_iter()
                    .find(|action| action.name() == *control)?,
            ),
            ["reset-keys"] => Self::ResetKeys,
            ["fullscreen"] => Self::ToggleFullscreen,
            ["resolutions"] => Self::OpenResolutions,
            ["resolution", "native"] => Self::PickResolution(Resolution::Native),
            // The rung by its *rows* and not by the label the button wears.
            // `label` exists to be read on a screen — it is display text, and
            // a parser that went through it would break the day it read
            // "1080p60" or was translated, silently and with nothing to catch
            // it. What is being asked here is whether the ladder has a rung
            // that tall, which is a question about the value.
            ["resolution", rung] => {
                let wanted = Resolution::Rows(rung.parse().ok()?);
                settings::LADDER
                    .into_iter()
                    .find(|rung| *rung == wanted)
                    .map(Self::PickResolution)?
            }
            ["shut-resolutions"] => Self::ShutResolutions,
            ["apply"] => Self::ApplyDisplay,
            ["back"] => Self::Back,
            ["resume"] => Self::Resume,
            ["leave-world"] => Self::LeaveWorld,
            _ => return None,
        })
    }
}

/// Marks the line a screen reports a dial on.
#[derive(Component)]
struct StatusText;

/// Watches the dial the screen started, and enters the world when it lands.
///
/// This is the whole of what opening and joining have in common, which is all
/// of it: by the time a welcome has arrived, a world of one's own and somebody
/// else's are the same thing — a point to stand at and something to look at,
/// both of them the server's to say.
fn settle_dialing(
    mut commands: Commands,
    dialing: Res<Dialing>,
    mut status: ResMut<Status>,
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

    // On the spawn the server named, facing what it said to face. The view is
    // brought there rather than trusted: it carries over from wherever it last
    // was, which for a world chosen from within a match is a point in a world
    // that no longer exists.
    view.enter(session.connection.spawn, session.connection.facing);

    // The logbook first, while the session is still whole: what the chart
    // reads on its way into the world, for a world this machine remembers.
    if let Some(logbook) = crate::logbook::for_session(&session) {
        commands.insert_resource(logbook);
    }
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

#[cfg(test)]
mod tests;
