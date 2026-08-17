//! Main menu, the new-world dialog, the join screen and the controls screen.

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::text::{FontSize, FontSource, FontStyle};

use std::time::SystemTime;

use protocol::{DAY_SECONDS, DEFAULT_PORT};

use crate::bindings::{is_bindable, typed_label, Action, KeyBindings};
use crate::camera::View;
use crate::chart::{INK, INK_DIM, PAPER};
use crate::net::{self, Dialing, Hosting, Online, Reach};
use crate::{AppState, Helm};
use server::{kept_worlds, random_seed, KeptWorld, WorldConfig, MAX_SEED};

/// Longest seed the user can type — read off [`MAX_SEED`], so the field can
/// always hold a seed the game itself picked.
const MAX_SEED_DIGITS: usize = MAX_SEED.ilog10() as usize + 1;

/// Longest address the join screen will take. Room for a fully qualified name
/// and a port, well past anything anybody types.
const MAX_ADDRESS: usize = 60;

/// The word on the front of the box, the line under it, and how big each is
/// drawn.
const TITLE: &str = "GENOVESA";
const TITLE_SIZE: f32 = 56.0;
const SUBTITLE: &str = "the sea is charted no further";
const SUBTITLE_SIZE: f32 = 19.0;
/// How far the rules either side of the title run out.
const TITLE_RULE: f32 = 64.0;

/// How big a dialog's own title is drawn — well under [`TITLE_SIZE`], since it
/// names a screen rather than the game.
const HEADING_SIZE: f32 = 34.0;

/// The inks a menu is drawn with.
///
/// There are two sets and there have to be, because there are two places a
/// menu goes up. Outside a world it stands on [`crate::backdrop`]'s sheet and
/// is drawn *on paper*, as part of the chart; inside one it stands over the
/// world itself, where paper would be a chart laid on the sea. They are not
/// one thing lit two ways — a button drawn in ink and a button drawn as a
/// panel are two different objects — so the whole set travels together and no
/// screen mixes them.
#[derive(Clone, Copy)]
struct Palette {
    text: Color,
    dim: Color,
    /// Every edge the menu draws, panels and buttons alike. One colour for all
    /// of them, so a button can never come out brighter than the frame it sits
    /// in.
    edge: Color,
    panel: Color,
    /// What the full-screen node behind the menu is washed with. Nothing on
    /// paper — the sheet is already the colour it should be, and a wash over
    /// it would only make it dirty paper.
    behind: Color,
    /// Whether a panel is a *cartouche*: the double rule with the paper
    /// showing between the two lines that an engraver letters a chart's title
    /// inside. It is the whole difference between a box drawn on the paper and
    /// a box laid over it.
    cartouche: bool,
    /// What a button does under the pointer, and what one that is switched on
    /// or waiting for a key looks like.
    button: Highlight,
}

/// On the sheet.
///
/// A button is drawn rather than filled: paper has no lights to turn on, so
/// the only thing that can happen to a shape on it is more ink, and pressing
/// one is a wash of the reading ink. Armed is the one thing in another colour,
/// the same red the chart marks the reader's own position in, which is how the
/// eye finds it.
const ON_PAPER: Palette = Palette {
    text: INK,
    dim: INK_DIM,
    edge: INK,
    panel: PAPER,
    behind: Color::NONE,
    cartouche: true,
    button: Highlight {
        idle: Color::NONE,
        hover: Color::srgba(0.24, 0.17, 0.11, 0.13),
        press: Color::srgba(0.24, 0.17, 0.11, 0.26),
        armed: Color::srgba(0.55, 0.16, 0.12, 0.22),
    },
};

/// Over the world: the pause menu, and the controls screen reached from it.
const OVER_THE_WORLD: Palette = Palette {
    text: Color::srgb(0.88, 0.87, 0.80),
    dim: Color::srgb(0.60, 0.60, 0.55),
    edge: Color::srgb(0.70, 0.69, 0.62),
    panel: Color::srgba(0.09, 0.11, 0.10, 0.94),
    behind: Color::srgba(0.05, 0.07, 0.09, 0.72),
    cartouche: false,
    button: Highlight {
        idle: Color::srgb(0.17, 0.20, 0.16),
        hover: Color::srgb(0.26, 0.31, 0.22),
        press: Color::srgb(0.35, 0.42, 0.28),
        armed: Color::srgb(0.44, 0.40, 0.18),
    },
};

/// What a button is filled with in each of its states.
///
/// Carried on the button entity rather than looked up when one is hovered,
/// because [`highlight_buttons`] sees every button in the app and cannot tell
/// from one which of the two sheets it was drawn on.
#[derive(Component, Clone, Copy)]
struct Highlight {
    idle: Color,
    hover: Color,
    press: Color,
    /// A button switched on, or a key row waiting for a key.
    armed: Color,
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
            .add_systems(OnEnter(AppState::MainMenu), spawn_main_menu)
            .add_systems(OnEnter(AppState::Settings), spawn_settings)
            // The pause menu and the controls screen behind it, both standing
            // over a world that is still running — see [`Helm`].
            .add_systems(OnEnter(Helm::Paused), spawn_pause_menu)
            .add_systems(OnEnter(Helm::Controls), spawn_paused_settings)
            // Cleared before the screen is built, so that a screen only ever
            // shows what this visit to it has had to say.
            .add_systems(
                OnEnter(AppState::SetSail),
                (clear_status, spawn_set_sail).chain(),
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
            .add_systems(OnExit(AppState::Settings), cancel_rebinding)
            .add_systems(OnExit(Helm::Controls), cancel_rebinding)
            .add_systems(
                Update,
                (
                    highlight_buttons,
                    settle_dialing.run_if(resource_exists::<Dialing>),
                    main_menu_actions.run_if(in_state(AppState::MainMenu)),
                    (set_sail_actions, refresh_set_sail).run_if(in_state(AppState::SetSail)),
                    (dialog_actions, open_world, refresh_dialog)
                        .run_if(in_state(AppState::NewWorld)),
                    // `type_seed` carries no run condition of its own, for
                    // the reason `join_keys` and `settings_keys` carry none.
                    type_seed,
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
                    // no run condition of its own — see its comment. The rest
                    // run on the controls screen wherever it was opened from.
                    (
                        settings_actions
                            .run_if(in_state(AppState::Settings).or_else(in_state(Helm::Controls))),
                        settings_keys,
                        refresh_settings
                            .run_if(in_state(AppState::Settings).or_else(in_state(Helm::Controls))),
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

/// The kept worlds the set-sail screen is offering, read off the worlds
/// directory each time the screen opens, and whether the one chosen will be
/// shared. The rows hold indexes into this list — see
/// [`MenuButton::OpenKept`].
#[derive(Resource, Default)]
struct Harbour {
    worlds: Vec<server::KeptWorld>,
    share: bool,
}

/// The action whose new key the controls screen is waiting for, if any. Only
/// one row can be armed at a time — the next key pressed has to mean one thing.
#[derive(Resource, Default)]
struct Rebinding(Option<Action>);

#[derive(Component, Clone, Copy, PartialEq)]
enum MenuButton {
    /// Opens the kept-worlds screen. Only offered once there is a world to
    /// return to.
    SetSail,
    NewWorld,
    JoinWorld,
    Settings,
    Exit,
    RandomSeed,
    /// Turns sharing the world about to be started on and off.
    ToggleShare,
    Start,
    /// Reopens the kept world at this row of the [`Harbour`].
    OpenKept(usize),
    /// Turns sharing the kept world about to be reopened on and off.
    ToggleKeptShare,
    /// Dials the address on the join screen.
    Connect,
    /// Arms this action's row, so the next key pressed becomes its key.
    Rebind(Action),
    ResetKeys,
    Back,
    /// Puts the player back at the helm of the world behind the pause menu.
    Resume,
    /// Gives that world up. Named for what it costs rather than "Back", which
    /// on every other screen means one step and here means the whole world.
    LeaveWorld,
}

/// Marks the seed readout so it can be refreshed as the player types.
#[derive(Component)]
struct SeedText;

/// Marks the sharing button's label, so it can say which way it is set.
#[derive(Component)]
struct ShareText;

/// The set-sail screen's own sharing label — a component of its own so the
/// two screens' refreshes cannot write each other's switches.
#[derive(Component)]
struct KeptShareText;

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
    // Whether there is anywhere to set sail *to*: a machine with no kept
    // worlds gets the menu it always had, and the button appears the first
    // time there is a world to return to. Asked of the directory on each
    // opening of this screen, which is exactly as often as the answer can
    // have changed.
    let kept_any = net::worlds_dir().is_some_and(|dir| !server::kept_worlds(&dir).is_empty());

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

                    if kept_any {
                        spawn_button(panel, &ink, MenuButton::SetSail, "Set Sail", 240.0);
                    }
                    spawn_button(panel, &ink, MenuButton::NewWorld, "New World", 240.0);
                    spawn_button(panel, &ink, MenuButton::JoinWorld, "Join World", 240.0);
                    spawn_button(panel, &ink, MenuButton::Settings, "Controls", 240.0);
                    spawn_button(panel, &ink, MenuButton::Exit, "Exit", 240.0);
                });
        });
}

/// The title, engraved and ruled: tracked-out serif capitals with a hairline
/// running out to either side of them, which is how a chart of the period puts
/// its own name at the top.
fn spawn_title(parent: &mut ChildSpawnerCommands, ink: &Palette) {
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
fn spawn_subtitle(parent: &mut ChildSpawnerCommands, ink: &Palette) {
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
fn title_font() -> TextFont {
    TextFont {
        font: FontSource::Serif,
        font_size: FontSize::Px(TITLE_SIZE),
        ..default()
    }
}

fn subtitle_font() -> TextFont {
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
fn spawn_hairline(parent: &mut ChildSpawnerCommands, ink: Color) {
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
fn spaced(word: &str) -> String {
    let mut spaced = String::with_capacity(word.len() * 2);
    for letter in word.chars() {
        if !spaced.is_empty() {
            spaced.push(' ');
        }
        spaced.push(letter);
    }
    spaced
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
            MenuButton::SetSail => next.set(AppState::SetSail),
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
// Set sail: the kept worlds
// ---------------------------------------------------------------------------

/// Rows the screen offers before it starts summarising. Eight is more worlds
/// than most machines will ever keep; the summary line below the rows is
/// what says the rest are not lost, only older.
const MOST_KEPT_ROWS: usize = 8;

fn spawn_set_sail(mut commands: Commands, mut harbour: ResMut<Harbour>) {
    // Read fresh on every visit to the screen: worlds are files, and files
    // can have been copied in, deleted, or sailed from another install since
    // the last look.
    harbour.worlds = net::worlds_dir()
        .map(|dir| kept_worlds(&dir))
        .unwrap_or_default();

    let ink = ON_PAPER;
    commands
        .spawn((
            Name::new("Set sail dialog"),
            DespawnOnExit(AppState::SetSail),
            screen(&ink),
        ))
        .with_children(|screen| {
            screen
                .spawn(panel(&ink, 10.0, PANEL_PADDING))
                .with_children(|panel| {
                    cartouche_rule(panel, &ink);
                    heading(panel, &ink, "Set Sail", 20.0);
                    label(panel, &ink, "return to a world this machine keeps");

                    for (row, world) in harbour.worlds.iter().take(MOST_KEPT_ROWS).enumerate() {
                        spawn_button(
                            panel,
                            &ink,
                            MenuButton::OpenKept(row),
                            &world_label(world),
                            360.0,
                        );
                    }
                    if harbour.worlds.len() > MOST_KEPT_ROWS {
                        label(
                            panel,
                            &ink,
                            &format!(
                                "and {} more, sailed longer ago",
                                harbour.worlds.len() - MOST_KEPT_ROWS
                            ),
                        );
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
                            row.spawn(button(&ink, MenuButton::ToggleKeptShare, 160.0))
                                .with_children(|button| {
                                    button.spawn((
                                        KeptShareText,
                                        button_label(&ink, share_label(harbour.share)),
                                    ));
                                });
                        });
                    label(
                        panel,
                        &ink,
                        &format!("sharing hosts the world on port {DEFAULT_PORT}"),
                    );

                    status_line(panel, &ink);
                    spawn_button(panel, &ink, MenuButton::Back, "Back", 130.0);
                });
        });
}

/// What a kept world's row reads. The place's own story where it has one —
/// its name, once worlds have names — and otherwise how far into its days it
/// is, which is the one fact the file can offer that describes the world
/// rather than the machinery. "Sailed when" is what tells two rows apart on
/// a machine that keeps several.
fn world_label(world: &KeptWorld) -> String {
    let day = (world.age / DAY_SECONDS) as u32 + 1;
    let sailed = sailed_when(world.kept);
    if world.name.is_empty() {
        format!("Day {day} — {sailed}")
    } else {
        format!("{} — {sailed}", world.name)
    }
}

/// A last-sailed moment as prose. Coarse on purpose: "sailed today" is what
/// a person checks a list by, and an hour count would just be a smaller
/// number to ignore.
fn sailed_when(kept: SystemTime) -> String {
    match kept.elapsed() {
        Ok(since) => match since.as_secs() / 86_400 {
            0 => "sailed today".to_string(),
            1 => "sailed yesterday".to_string(),
            days => format!("sailed {days} days ago"),
        },
        // A file stamped in the future is a clock that moved, not a world
        // that has not been played yet.
        Err(_) => "sailed today".to_string(),
    }
}

fn set_sail_actions(
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
            MenuButton::ToggleKeptShare => harbour.share = !harbour.share,
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

/// Keeps the sharing switch's label in step with the setting behind it.
fn refresh_set_sail(harbour: Res<Harbour>, mut share_text: Query<&mut Text, With<KeptShareText>>) {
    if !harbour.is_changed() {
        return;
    }
    for mut text in &mut share_text {
        text.0 = share_label(harbour.share).to_string();
    }
}

// ---------------------------------------------------------------------------
// New world dialog
// ---------------------------------------------------------------------------

fn spawn_new_world_dialog(mut commands: Commands, settings: Res<NewWorldSettings>) {
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

                    label(panel, &ink, "Seed");
                    panel.spawn((
                        SeedText,
                        Text::new(settings.seed.clone()),
                        TextFont {
                            font_size: FontSize::Px(26.0),
                            ..default()
                        },
                        TextColor(ink.text),
                    ));
                    panel.spawn((
                        Text::new("type digits, backspace to edit"),
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

                    status_line(panel, &ink);
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
fn share_label(share: bool) -> &'static str {
    if share {
        "Share: on"
    } else {
        "Share: off"
    }
}

/// The dialog's buttons that only touch the settings. Starting a world is
/// [`open_world`], which has to wait for a server and so cannot be a button
/// handler that decides anything on the spot.
fn dialog_actions(
    buttons: Query<(&Interaction, &MenuButton), Changed<Interaction>>,
    mut settings: ResMut<NewWorldSettings>,
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
/// [`settle_dialing`] takes the spawn from the welcome exactly as a run
/// started with `--join` does. Nothing about the world is settled here, not
/// even by the machine that is about to serve it.
fn open_world(
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
            status.0 = "opening the world...".to_string();
            commands.insert_resource(Dialing::opening(
                WorldConfig {
                    seed: settings.seed_value(),
                },
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

/// Digit-by-digit editing of the seed field.
///
/// Asks what the keyboard *typed* rather than which positions were pressed,
/// exactly as the address field does — so the game's two text fields are
/// edited by one mechanism instead of two. It also spares this a table of
/// every key that produces a digit: the main row and the number pad both
/// simply type one, and so does whatever a layout puts them on.
///
/// Runs on every screen rather than only this one, for the reason
/// [`join_keys`] does: a reader left to lag would deliver whatever was
/// pressed on the way here the instant the dialog opened — and on this
/// screen that would land in the seed.
fn type_seed(
    state: Res<State<AppState>>,
    mut presses: MessageReader<KeyboardInput>,
    mut settings: ResMut<NewWorldSettings>,
) {
    let on_screen = *state.get() == AppState::NewWorld;

    for press in presses.read() {
        // A held key repeats, which is what a text field wants: holding
        // backspace should clear the seed rather than one digit of it.
        if !on_screen || press.state != ButtonState::Pressed {
            continue;
        }

        match press.key_code {
            KeyCode::Backspace => {
                settings.seed.pop();
            }
            _ => {
                if let Key::Character(typed) = &press.logical_key {
                    for digit in typed.chars().filter(char::is_ascii_digit) {
                        if settings.seed.len() < MAX_SEED_DIGITS {
                            settings.seed.push(digit);
                        }
                    }
                }
            }
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

                    status_line(panel, &ink);
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

// ---------------------------------------------------------------------------
// Controls
// ---------------------------------------------------------------------------

/// The controls screen as reached from the main menu.
fn spawn_settings(commands: Commands, bindings: Res<KeyBindings>) {
    spawn_controls(
        commands,
        &ON_PAPER,
        &bindings,
        DespawnOnExit(AppState::Settings),
    );
}

/// The same screen as reached from the pause menu, differing only in living
/// and dying with [`Helm::Controls`] instead — so that opening it does not
/// leave the world, which is the whole reason the pause menu exists.
fn spawn_paused_settings(commands: Commands, bindings: Res<KeyBindings>) {
    spawn_controls(
        commands,
        &OVER_THE_WORLD,
        &bindings,
        DespawnOnExit(Helm::Controls),
    );
}

/// Builds the controls screen, cleared up by whichever state opened it.
fn spawn_controls(
    mut commands: Commands,
    ink: &Palette,
    bindings: &KeyBindings,
    until: impl Bundle,
) {
    commands
        .spawn((Name::new("Controls screen"), until, screen(ink)))
        .with_children(|screen| {
            screen
                .spawn(panel(ink, 6.0, CONTROLS_PADDING))
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
fn spawn_key_row(parent: &mut ChildSpawnerCommands, ink: &Palette, action: Action, key_name: &str) {
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
                KEY_ROW_PADDING,
            ))
            .with_children(|button| {
                button.spawn((KeyText(action), button_label(ink, key_name)));
            });
        });
}

/// Whether the controls screen currently open is the one over a paused world
/// rather than the one over the main menu. The two are the same screen — see
/// [`spawn_controls`] — and this is the only thing that tells them apart.
fn over_a_world(helm: &Option<Res<State<Helm>>>) -> bool {
    helm.as_ref().is_some_and(|h| *h.get() == Helm::Controls)
}

/// Shuts the controls screen, returning to whichever screen opened it.
fn close_controls(
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

fn settings_actions(
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
            MenuButton::Back => close_controls(over_a_world, &mut next_app, &mut next_helm),
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
fn settings_keys(
    state: Res<State<AppState>>,
    helm: Option<Res<State<Helm>>>,
    mut presses: MessageReader<KeyboardInput>,
    mut rebinding: ResMut<Rebinding>,
    mut bindings: ResMut<KeyBindings>,
    mut next_app: ResMut<NextState<AppState>>,
    mut next_helm: ResMut<NextState<Helm>>,
) {
    let over_a_world = over_a_world(&helm);
    let on_screen = *state.get() == AppState::Settings || over_a_world;

    for press in presses.read() {
        // A key held down repeats; the first press is the one that counts.
        if !on_screen || press.state != ButtonState::Pressed || press.repeat {
            continue;
        }

        let Some(action) = rebinding.0 else {
            if press.key_code == KeyCode::Escape {
                close_controls(over_a_world, &mut next_app, &mut next_helm);
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

fn cancel_rebinding(mut rebinding: ResMut<Rebinding>) {
    rebinding.0 = None;
}

// ---------------------------------------------------------------------------
// In world
// ---------------------------------------------------------------------------

/// The pause menu, over the world rather than instead of it.
///
/// A panel like the dialogs, on the dimmed [`screen`] they all stand on, so
/// the water carries on moving behind it and it is plain that the world is
/// still there. Leaving is the last of the three and says which world it is
/// leaving, because it is the one button here that cannot be taken back.
fn spawn_pause_menu(mut commands: Commands, hosting: Option<Res<Hosting>>) {
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
                            spawn_button(rows, &ink, MenuButton::Settings, "Controls", 200.0);
                            spawn_button(rows, &ink, MenuButton::LeaveWorld, "Leave World", 200.0);
                        });
                });
        });
}

fn pause_actions(
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
            MenuButton::Settings => next_helm.set(Helm::Controls),
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
fn helm_keys(
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
        // Not ours. On the controls screen Escape may mean "not that key"
        // rather than "back", and only `settings_keys` knows which; at the
        // console it means "close the console"; on the chart it may mean
        // "stop writing this island's name". Each screen hears its own key,
        // because only it knows what the key means while it is up.
        Helm::Controls | Helm::Console | Helm::Chart => {}
    }
}

// ---------------------------------------------------------------------------
// Shared widgets
// ---------------------------------------------------------------------------

/// Full-screen, centred column that every menu screen is built inside.
fn screen(ink: &Palette) -> impl Bundle {
    (
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            row_gap: Val::Px(12.0),
            // On paper the room a menu has is the *paper*, not the window:
            // the sheet's edge is ruled just inside it, and a cartouche laid
            // across that rule reads as a chart with a hole cut in it. The
            // same margin the sheet's own furniture stands off by.
            padding: UiRect::all(Val::Px(if ink.cartouche {
                crate::chart::PAPER_MARGIN
            } else {
                0.0
            })),
            ..default()
        },
        BackgroundColor(ink.behind),
    )
}

/// The bordered box a menu is built inside.
///
/// `row_gap` is the space between the rows stacked in it, and `pad` the room
/// inside its rule. Both are the controls screen's doing: it packs a list of
/// nine key rows and has to fit them between the sheet's own edges, where a
/// dialog of a few fields has all the paper it wants.
fn panel(ink: &Palette, row_gap: f32, pad: f32) -> impl Bundle {
    (
        Node {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            padding: UiRect::axes(Val::Px(32.0), Val::Px(pad)),
            border: UiRect::all(Val::Px(2.0)),
            row_gap: Val::Px(row_gap),
            ..default()
        },
        BackgroundColor(ink.panel),
        BorderColor::all(ink.edge),
    )
}

/// How much room a panel leaves inside its rule, above and below.
const PANEL_PADDING: f32 = 32.0;

/// And how much the controls screen leaves, which is as little as nine key
/// rows and two lines of prose can be got into a sheet in.
const CONTROLS_PADDING: f32 = 14.0;

/// How far inside a cartouche's outer rule the second one runs, in pixels.
const CARTOUCHE_INSET: f32 = 6.0;

/// The second rule that makes a panel a cartouche.
///
/// Taken out of the flow and pinned inside the panel's padding, so it costs
/// the contents no room and does not have to know how much there are of them.
/// A double rule with the paper showing between the two is what an engraver
/// puts a chart's title inside, and it is the whole difference between a box
/// drawn on paper and a box laid over it.
fn cartouche_rule(parent: &mut ChildSpawnerCommands, ink: &Palette) {
    if !ink.cartouche {
        return;
    }
    parent.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(CARTOUCHE_INSET),
            top: Val::Px(CARTOUCHE_INSET),
            right: Val::Px(CARTOUCHE_INSET),
            bottom: Val::Px(CARTOUCHE_INSET),
            border: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        BorderColor::all(ink.edge),
    ));
}

/// A dialog's title, and how much room it keeps between itself and what
/// follows — which is a whole line's worth on the dialogs that open with a
/// field, and almost nothing on the controls screen, where the line under it
/// is part of the same thought.
fn heading(parent: &mut ChildSpawnerCommands, ink: &Palette, text: &str, below: f32) {
    parent.spawn((
        Text::new(text),
        TextFont {
            font_size: FontSize::Px(HEADING_SIZE),
            ..default()
        },
        TextColor(ink.text),
        Node {
            margin: UiRect::bottom(Val::Px(below)),
            ..default()
        },
    ));
}

/// The line a dialog reports a dial on. Spawned empty and left that way until
/// there is something to say, but spawned all the same: a line that appeared
/// only when it had text would push the buttons under it down the moment the
/// player pressed one.
fn status_line(parent: &mut ChildSpawnerCommands, ink: &Palette) {
    parent.spawn((
        StatusText,
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(15.0),
            ..default()
        },
        TextColor(ink.text),
        Node {
            margin: UiRect::top(Val::Px(8.0)),
            // Held open so an empty line still takes its room, for the reason
            // above.
            height: Val::Px(18.0),
            ..default()
        },
    ));
}

fn label(parent: &mut ChildSpawnerCommands, ink: &Palette, text: &str) {
    parent.spawn((
        Text::new(text),
        TextFont {
            font_size: FontSize::Px(15.0),
            ..default()
        },
        TextColor(ink.dim),
    ));
}

/// A menu button without its label, so that callers who need to mark the label
/// — as the controls screen does, to rewrite it later — can spawn their own.
fn button(ink: &Palette, action: MenuButton, width: f32) -> impl Bundle {
    padded_button(ink, action, width, 12.0)
}

/// How much shorter a key row's button is than a menu's.
///
/// The controls screen is the one screen whose height follows from how many
/// things there are to bind, and it has to fit in the window at every count it
/// is ever going to have. Taking four pixels off each row's button buys back a
/// row and a half, and a key row is a wide target that loses nothing by not
/// being a tall one as well.
const KEY_ROW_PADDING: f32 = 8.0;

fn padded_button(ink: &Palette, action: MenuButton, width: f32, pad: f32) -> impl Bundle {
    (
        Button,
        action,
        ink.button,
        Node {
            width: Val::Px(width),
            padding: UiRect::axes(Val::Px(12.0), Val::Px(pad)),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            border: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        BackgroundColor(ink.button.idle),
        BorderColor::all(ink.edge),
    )
}

fn button_label(ink: &Palette, text: &str) -> impl Bundle {
    (
        Text::new(text),
        TextFont {
            font_size: FontSize::Px(19.0),
            ..default()
        },
        TextColor(ink.text),
        TextLayout::justify(Justify::Center),
    )
}

fn spawn_button(
    parent: &mut ChildSpawnerCommands,
    ink: &Palette,
    action: MenuButton,
    text: &str,
    width: f32,
) {
    parent
        .spawn(button(ink, action, width))
        .with_children(|button| {
            button.spawn(button_label(ink, text));
        });
}

fn highlight_buttons(
    rebinding: Res<Rebinding>,
    mut buttons: Query<
        (&Interaction, &MenuButton, &Highlight, &mut BackgroundColor),
        Changed<Interaction>,
    >,
) {
    for (interaction, button, ink, mut color) in &mut buttons {
        let idle = match button {
            MenuButton::Rebind(action) if rebinding.0 == Some(*action) => ink.armed,
            _ => ink.idle,
        };

        *color = BackgroundColor(match interaction {
            Interaction::Pressed => ink.press,
            Interaction::Hovered => ink.hover,
            Interaction::None => idle,
        });
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    use bevy::input::keyboard::Key;
    use bevy::state::app::StatesPlugin;

    use super::*;
    use crate::net::fake_server;
    use crate::testing::run_until;

    /// A host that accepts a connection and then says nothing — a dial that
    /// stays in the air for as long as the test needs it to. Its listener is
    /// handed back so the test decides when it stops existing.
    fn silent_server() -> (TcpListener, String) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("addr").to_string();
        (listener, address)
    }

    /// A headless app running the menu systems, with no renderer attached.
    fn test_app(state: AppState) -> App {
        // The Start button keeps the world it opens, and a test's world must
        // not land among the player's real ones.
        crate::testing::quarantine_data_dir();
        let mut app = App::new();
        app.add_plugins((StatesPlugin, MenuPlugin))
            .insert_state(state)
            .add_sub_state::<Helm>()
            .init_resource::<ButtonInput<KeyCode>>()
            // Normally the camera plugin's, but entering a world moves the
            // view onto the served spawn — see `settle_dialing`.
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

    /// What the player is doing in the world, or `None` when there is no world
    /// to be doing it in — which is itself worth asserting, since a pause menu
    /// that outlived its world would be the bug this all exists to prevent.
    fn helm(app: &App) -> Option<Helm> {
        app.world().get_resource::<State<Helm>>().map(|h| *h.get())
    }

    /// A match with the pause menu already up.
    fn paused_app() -> App {
        let mut app = test_app(AppState::InWorld);
        app.world_mut()
            .resource_mut::<NextState<Helm>>()
            .set(Helm::Paused);
        app.update();
        app
    }

    /// A world actually being served, so that a test of what the pause menu
    /// says about hosting is looking at a real [`Hosting`]. Cheap: the port is
    /// the kernel's to pick and nothing ever dials it.
    ///
    /// `bind` is what decides whether the world counts as shared — the
    /// loopback is a world of one's own, anything wider is one others could be
    /// in. An ephemeral port either way, so two test runs cannot collide the
    /// way binding the real shared port would.
    fn fake_host(bind: &str) -> server::Host {
        server::Server::bind(bind, WorldConfig::default())
            .expect("bind")
            .spawn()
            .expect("spawn")
    }

    /// How many screens of a given name are standing.
    fn named(app: &mut App, name: &str) -> usize {
        app.world_mut()
            .query::<&Name>()
            .iter(app.world())
            .filter(|n| n.as_str() == name)
            .count()
    }

    /// Everything the pause menu currently says, run together.
    fn pause_text(app: &mut App) -> String {
        app.world_mut()
            .query::<&Text>()
            .iter(app.world())
            .map(|t| t.0.clone())
            .collect::<Vec<_>>()
            .join(" ")
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
    fn entering_a_served_world_afoot_stands_the_player_on_the_spawn() {
        // The path a player actually takes into a world: the welcome moves
        // the view onto the served spawn *and then* enters. This fake
        // server seats nobody at any helm and tells of no boats, so what
        // entry owes is a walker standing exactly where the server said —
        // the hulls are the server's to tell, not entry's to invent.
        let (address, _socket) = fake_server(Vec2::new(100.0, -200.0), Vec2::new(100.0, -400.0));
        let mut app = test_app(AppState::JoinWorld);
        // Time for the reporting the net plugin brings with it; the menu's
        // own systems never ask what o'clock it is. Assets because a boat
        // telling would arrive as meshes.
        app.add_plugins((
            TaskPoolPlugin::default(),
            AssetPlugin::default(),
            bevy::time::TimePlugin,
            crate::net::NetPlugin,
        ))
        .init_asset::<Mesh>()
        .init_resource::<Assets<StandardMaterial>>();

        app.world_mut().resource_mut::<JoinSettings>().address = address;
        click(&mut app, MenuButton::Connect);
        run_until(&mut app, "the world is entered", |app| {
            *app.world().resource::<State<AppState>>().get() == AppState::InWorld
        });
        app.update();

        let at = app
            .world_mut()
            .query_filtered::<&Transform, With<crate::player::Player>>()
            .single(app.world())
            .expect("entering a served world afoot should stand a walker up")
            .translation;
        assert_eq!(Vec2::new(at.x, at.z), Vec2::new(100.0, -200.0));
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
    fn even_a_world_of_ones_own_is_served() {
        // The ground comes from a server, so there is no such thing here as a
        // world without one — the sharing switch decides who can reach it and
        // nothing else. Keeping a world therefore starts a server and waits
        // for it, exactly as sharing one does.
        let mut app = test_app(AppState::NewWorld);
        assert!(
            !app.world().resource::<NewWorldSettings>().share,
            "this test is about the switch being off"
        );
        click_once(&mut app, MenuButton::Start);

        assert_eq!(state(&app), AppState::NewWorld, "entered without a world");
        assert!(
            app.world().contains_resource::<Dialing>(),
            "keeping a world started no server"
        );
    }

    #[test]
    fn a_shared_world_waits_on_the_server_it_starts() {
        // Started, not entered: the player stays on the dialog until the
        // welcome comes back — which is where `settle_dialing` takes over,
        // tested below against a server this test file can name.
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
    }

    #[test]
    fn a_dial_that_lands_enters_the_served_world() {
        let (address, _socket) = fake_server(Vec2::new(100.0, -200.0), Vec2::new(100.0, -400.0));
        let mut app = test_app(AppState::JoinWorld);
        // Somewhere the menu's own drifting sea might have left the view. A
        // match must open where the server said, not where the menu was
        // looking.
        app.world_mut().resource_mut::<View>().focus = Vec3::new(4_000.0, 0.0, -2_500.0);
        app.world_mut().resource_mut::<JoinSettings>().address = address;
        click(&mut app, MenuButton::Connect);

        run_until(&mut app, "the world is entered", |app| {
            *app.world().resource::<State<AppState>>().get() == AppState::InWorld
        });

        // Where we are standing in the world is the server's to say, and so is
        // which way to look: the view opens with the first land dead ahead
        // rather than wherever the bearing happened to be.
        let view = *app.world().resource::<View>();
        assert_eq!(view.focus, Vec3::new(100.0, 0.0, -200.0));
        let ahead = Vec2::new(-view.yaw.sin(), -view.yaw.cos());
        assert!(
            ahead.dot(Vec2::new(0.0, -1.0)) > 0.999,
            "the match opens looking {ahead}, not at the land it was pointed at"
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

        type_key(&mut app, KeyCode::Digit4, "4");
        // The number pad types the same digit from a different position,
        // which is the whole reason the field reads what was typed.
        type_key(&mut app, KeyCode::Numpad2, "2");
        assert_eq!(app.world().resource::<NewWorldSettings>().seed, "42");

        type_key(&mut app, KeyCode::Backspace, "\u{8}");
        assert_eq!(app.world().resource::<NewWorldSettings>().seed, "4");
    }

    #[test]
    fn the_seed_field_takes_only_digits() {
        let mut app = test_app(AppState::NewWorld);
        app.world_mut().resource_mut::<NewWorldSettings>().seed = String::new();

        // A letter, a symbol and a space: a seed is a number, and anything
        // else would only fail to parse back out of the field.
        type_key(&mut app, KeyCode::KeyA, "a");
        type_key(&mut app, KeyCode::Period, ".");
        type_key(&mut app, KeyCode::Space, " ");
        assert_eq!(app.world().resource::<NewWorldSettings>().seed, "");

        type_key(&mut app, KeyCode::Digit7, "7");
        assert_eq!(app.world().resource::<NewWorldSettings>().seed, "7");
    }

    #[test]
    fn keys_pressed_before_the_new_world_dialog_opened_are_not_typed_into_it() {
        // The same backlog the join and controls screens have to ignore: the
        // reader runs on every screen so its cursor keeps up, which means it
        // has to refuse everything pressed before this screen was the one on
        // it.
        // Held against the seed the dialog already had rather than against a
        // fresh default, which would be a different world every time it was
        // asked for — that being the point of `NewWorldSettings::default`.
        let mut app = test_app(AppState::InWorld);
        let before = app.world().resource::<NewWorldSettings>().seed.clone();
        type_key(&mut app, KeyCode::Digit9, "9");

        go_to(&mut app, AppState::NewWorld);
        assert_eq!(
            app.world().resource::<NewWorldSettings>().seed,
            before,
            "a digit pressed on the way here landed in the seed"
        );
    }

    #[test]
    fn seed_field_is_length_capped() {
        let mut app = test_app(AppState::NewWorld);
        app.world_mut().resource_mut::<NewWorldSettings>().seed = String::new();

        for _ in 0..MAX_SEED_DIGITS + 5 {
            type_key(&mut app, KeyCode::Digit9, "9");
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

        click(&mut app, MenuButton::Rebind(Action::MoveForward));
        assert_eq!(waiting_on(&app), Some(Action::MoveForward));

        type_key(&mut app, KeyCode::KeyJ, "j");
        assert_eq!(bindings(&app).key(Action::MoveForward), KeyCode::KeyJ);
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
        assert_eq!(row_text(&mut app, Action::SteerLeft), "A");

        click(&mut app, MenuButton::Rebind(Action::SteerLeft));
        assert_eq!(row_text(&mut app, Action::SteerLeft), "press a key");

        type_key(&mut app, KeyCode::KeyH, "h");
        app.update();
        assert_eq!(row_text(&mut app, Action::SteerLeft), "H");
    }

    #[test]
    fn escape_abandons_a_capture_and_changes_nothing() {
        let mut app = test_app(AppState::Settings);
        let before = bindings(&app).clone();

        click(&mut app, MenuButton::Rebind(Action::MoveBack));
        type_key(&mut app, KeyCode::Escape, "\u{1b}");

        assert_eq!(waiting_on(&app), None);
        assert_eq!(bindings(&app), &before);
        // And having cancelled, we're still on the screen rather than back out.
        assert_eq!(state(&app), AppState::Settings);
    }

    #[test]
    fn a_reserved_key_is_refused_and_the_row_keeps_waiting() {
        let mut app = test_app(AppState::Settings);

        click(&mut app, MenuButton::Rebind(Action::MoveBack));
        type_key(&mut app, KeyCode::ArrowUp, "");

        assert_eq!(bindings(&app).key(Action::MoveBack), KeyCode::KeyS);
        assert_eq!(
            waiting_on(&app),
            Some(Action::MoveBack),
            "a refused key should leave the row armed"
        );

        // And a real key still lands afterwards.
        type_key(&mut app, KeyCode::KeyN, "n");
        assert_eq!(bindings(&app).key(Action::MoveBack), KeyCode::KeyN);
    }

    #[test]
    fn taking_a_key_another_action_had_trades_the_two() {
        let mut app = test_app(AppState::Settings);

        click(&mut app, MenuButton::Rebind(Action::MoveForward));
        type_key(&mut app, KeyCode::KeyE, "e");

        assert_eq!(bindings(&app).key(Action::MoveForward), KeyCode::KeyE);
        assert_eq!(
            bindings(&app).key(Action::TurnRight),
            KeyCode::KeyW,
            "turning right should have taken the key panning gave up"
        );
    }

    #[test]
    fn defaults_puts_every_key_back() {
        let mut app = test_app(AppState::Settings);

        click(&mut app, MenuButton::Rebind(Action::SteerLeft));
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
        click(&mut app, MenuButton::Rebind(Action::MoveForward));

        assert_eq!(waiting_on(&app), Some(Action::MoveForward));
        assert_eq!(bindings(&app).key(Action::MoveForward), KeyCode::KeyW);
    }

    #[test]
    fn escape_pauses_the_match_rather_than_leaving_it() {
        let mut app = test_app(AppState::InWorld);
        press_key(&mut app, KeyCode::Escape);
        app.update();
        assert_eq!(helm(&app), Some(Helm::Paused));
        // The whole point: the world is still there to go back to.
        assert_eq!(state(&app), AppState::InWorld);
    }

    #[test]
    fn escape_again_returns_to_the_helm() {
        let mut app = test_app(AppState::InWorld);
        press_key(&mut app, KeyCode::Escape);
        app.update();
        press_key(&mut app, KeyCode::Escape);
        app.update();
        assert_eq!(helm(&app), Some(Helm::Sailing));
        assert_eq!(state(&app), AppState::InWorld);
    }

    #[test]
    fn resume_returns_to_the_helm() {
        let mut app = paused_app();
        click(&mut app, MenuButton::Resume);
        assert_eq!(helm(&app), Some(Helm::Sailing));
        assert_eq!(state(&app), AppState::InWorld);
    }

    /// The one press that gives the world up — and the only one, which is what
    /// the pause menu is for.
    #[test]
    fn leaving_the_world_is_a_button_of_its_own() {
        let mut app = paused_app();
        click(&mut app, MenuButton::LeaveWorld);
        assert_eq!(state(&app), AppState::MainMenu);
        // Gone with the world it belonged to.
        assert_eq!(helm(&app), None);
    }

    #[test]
    fn pausing_puts_a_menu_up_and_resuming_takes_it_down() {
        let mut app = paused_app();
        assert_eq!(named(&mut app, "Pause menu"), 1);
        click(&mut app, MenuButton::Resume);
        assert_eq!(named(&mut app, "Pause menu"), 0);
    }

    #[test]
    fn controls_open_over_the_paused_world_and_come_back_to_it() {
        let mut app = paused_app();
        click(&mut app, MenuButton::Settings);
        assert_eq!(helm(&app), Some(Helm::Controls));
        assert_eq!(state(&app), AppState::InWorld);
        assert_eq!(named(&mut app, "Controls screen"), 1);

        click(&mut app, MenuButton::Back);
        assert_eq!(helm(&app), Some(Helm::Paused));
        assert_eq!(named(&mut app, "Controls screen"), 0);
    }

    #[test]
    fn escape_backs_out_of_the_paused_controls_to_the_pause_menu() {
        let mut app = paused_app();
        click(&mut app, MenuButton::Settings);
        type_key(&mut app, KeyCode::Escape, "");
        // The frame the transition lands on.
        app.update();
        assert_eq!(helm(&app), Some(Helm::Paused));
        assert_eq!(state(&app), AppState::InWorld);
    }

    /// Escape means "not that key" while a row is armed, wherever the screen
    /// was opened from — so it must not also step back to the pause menu.
    #[test]
    fn escape_on_the_paused_controls_cancels_an_armed_row_first() {
        let mut app = paused_app();
        click(&mut app, MenuButton::Settings);
        click(&mut app, MenuButton::Rebind(Action::MoveForward));
        assert_eq!(waiting_on(&app), Some(Action::MoveForward));

        type_key(&mut app, KeyCode::Escape, "");
        // A frame in which a step back would have landed, had one been taken.
        app.update();
        assert_eq!(waiting_on(&app), None);
        assert_eq!(helm(&app), Some(Helm::Controls));
    }

    #[test]
    fn keys_rebound_from_the_pause_menu_take() {
        let mut app = paused_app();
        click(&mut app, MenuButton::Settings);
        click(&mut app, MenuButton::Rebind(Action::MoveForward));
        type_key(&mut app, KeyCode::KeyT, "t");
        assert_eq!(bindings(&app).key(Action::MoveForward), KeyCode::KeyT);
    }

    /// Leaving a shared world shuts it on whoever else is in it, and that is
    /// worth a word before the button that does it.
    #[test]
    fn the_pause_menu_warns_before_closing_a_shared_world() {
        let mut app = test_app(AppState::InWorld);
        app.insert_resource(Hosting(fake_host("0.0.0.0:0")));
        press_key(&mut app, KeyCode::Escape);
        app.update();
        assert!(pause_text(&mut app).contains("leaving closes it on them"));
    }

    /// But a world of one's own is served too — over the loopback — so the
    /// warning must not go to somebody sailing alone, who has nobody to
    /// strand.
    #[test]
    fn a_world_of_ones_own_gets_no_warning_though_it_is_hosted_too() {
        let mut app = test_app(AppState::InWorld);
        app.insert_resource(Hosting(fake_host("127.0.0.1:0")));
        press_key(&mut app, KeyCode::Escape);
        app.update();
        assert!(!pause_text(&mut app).contains("leaving closes it on them"));
    }

    #[test]
    fn random_seeds_fit_the_field() {
        let seed = random_seed();
        assert!(seed.to_string().len() <= MAX_SEED_DIGITS);
    }

    #[test]
    fn the_dialog_opens_on_a_world_nobody_chose() {
        let settings = NewWorldSettings::default();
        assert!(settings.seed.len() <= MAX_SEED_DIGITS);
        // And a different one each time the game is started, rather than one
        // island every player who pressed start ever saw.
        assert_ne!(settings.seed, NewWorldSettings::default().seed);
    }
}
