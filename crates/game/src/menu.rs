//! Main menu, the new-world dialog, the join screen and the options screens.

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::text::{FontSize, FontSource, FontStyle};
use bevy::window::{Monitor, PrimaryMonitor, PrimaryWindow};

use std::time::SystemTime;

use protocol::{DAY_SECONDS, DEFAULT_PORT};

use crate::bindings::{is_bindable, typed_label, Action, KeyBindings};
use crate::camera::View;
use crate::chart::{INK, INK_DIM, PAPER};
use crate::net::{self, Dialing, Hosting, Online, Reach};
use crate::settings::{self, DisplaySettings};
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
            // Shared the same way with the plugin that keeps them and the one
            // that applies them to the sun — see [`crate::settings`]. The
            // display screen reads and writes them; nothing here applies them.
            .init_resource::<DisplaySettings>()
            .add_systems(OnEnter(AppState::MainMenu), spawn_main_menu)
            .add_systems(OnEnter(AppState::Options), spawn_options)
            .add_systems(OnEnter(AppState::Display), spawn_display)
            .add_systems(OnEnter(AppState::Controls), spawn_settings)
            // The pause menu and the screens behind it, all standing over a
            // world that is still running — see [`Helm`].
            .add_systems(OnEnter(Helm::Paused), spawn_pause_menu)
            .add_systems(OnEnter(Helm::Options), spawn_paused_options)
            .add_systems(OnEnter(Helm::Display), spawn_paused_display)
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
                            .run_if(in_state(AppState::Controls).or_else(in_state(Helm::Controls))),
                        settings_keys,
                        refresh_settings
                            .run_if(in_state(AppState::Controls).or_else(in_state(Helm::Controls))),
                    )
                        .chain(),
                    // The two screens above the controls, each on both of the
                    // states it can be reached through.
                    options_actions
                        .run_if(in_state(AppState::Options).or_else(in_state(Helm::Options))),
                    (display_actions, refresh_display)
                        .chain()
                        .run_if(in_state(AppState::Display).or_else(in_state(Helm::Display))),
                    // Escape on any of the three, which all mean the same thing
                    // by it: one step back. The controls screen is the
                    // exception and hears its own — see [`settings_keys`].
                    options_keys,
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
///
/// The screen is built from this and nothing else, and built again whenever it
/// changes — see [`show_set_sail`] — which is what lets a row stop being a
/// world and become a question.
#[derive(Resource, Default)]
struct Harbour {
    worlds: Vec<server::KeptWorld>,
    share: bool,
    /// The row that has been asked about, if any: its Discard has been pressed
    /// and the row is now the question, waiting to be answered. One at a time,
    /// because "yes" has to mean one world.
    asked: Option<usize>,
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

/// The action whose new key the controls screen is waiting for, if any. Only
/// one row can be armed at a time — the next key pressed has to mean one thing.
#[derive(Resource, Default)]
struct Rebinding(Option<Action>);

#[derive(Component, Clone, Copy, PartialEq)]
enum MenuButton {
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
    /// Steps down the ladder of resolutions and round to the top again — see
    /// [`settings::Resolution`].
    CycleResolution,
    /// Stops the sun casting, or sets it casting again.
    ToggleShadows,
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

/// Marks a display row's readout, so it can say which way its setting is set.
/// The button it belongs to is the one that changes that setting, so the two
/// are named by the same list.
#[derive(Component, Clone, Copy, PartialEq)]
enum DisplayText {
    Fullscreen,
    Resolution,
    Shadows,
    /// Not a setting but a word about one: what the display will actually do
    /// with the resolution asked of it — see [`settings::available`].
    Caveat,
}

// ---------------------------------------------------------------------------
// Main menu
// ---------------------------------------------------------------------------

/// The front screen.
///
/// One way to a world of one's own, not two: "Set Sail" and "New World" both
/// read as *start playing*, and a player made to tell them apart before they
/// have seen either is being asked about the machinery. So setting sail is the
/// whole of it — the worlds this machine keeps and the way to a fresh one are
/// one screen, because they answer one question.
fn spawn_main_menu(mut commands: Commands) {
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
            MenuButton::JoinWorld => next.set(AppState::JoinWorld),
            MenuButton::Options => next.set(AppState::Options),
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
const MOST_KEPT_WORLDS: usize = 5;

/// A kept world's row: the world's own button, the press beside it, and the
/// gap between them.
const KEPT_WIDTH: f32 = 340.0;
const DISCARD_WIDTH: f32 = 118.0;
const ANSWER_GAP: f32 = 6.0;

/// Each of the two answers that replace the Discard button when a row is asked
/// about. Half of what they stand in for, less the gap and the pixel a border
/// takes on either side of each of them, so the pair ends exactly where the one
/// button ended.
const ANSWER_WIDTH: f32 = (DISCARD_WIDTH - ANSWER_GAP - 2.0) / 2.0;

/// How wide a row is, whichever face it is wearing.
///
/// Set on the row rather than left to the sum of what is in it, so that a row
/// becoming a question cannot make the panel a different width — and so that
/// the two answers land where the button they replaced was, whatever the
/// arithmetic of borders comes to. Two pixels over the widths inside it, one
/// for each button's border.
const KEPT_ROW_WIDTH: f32 = KEPT_WIDTH + ANSWER_GAP + DISCARD_WIDTH + 4.0;

/// Marks the set-sail screen, so it can be taken down and built again — which
/// is how a row becomes a question. See [`show_set_sail`].
#[derive(Component)]
struct SetSailScreen;

/// Reads the harbour off the worlds directory.
///
/// Fresh on every visit to the screen: worlds are files, and files can have
/// been copied in, deleted, or sailed from another install since the last
/// look. Any question left standing from last time goes with it — see
/// [`MenuButton::KeepIt`].
fn read_the_harbour(mut harbour: ResMut<Harbour>) {
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
fn show_set_sail(
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
fn spawn_kept_row(parent: &mut ChildSpawnerCommands, ink: &Palette, row: usize, world: &KeptWorld) {
    parent.spawn(kept_row()).with_children(|line| {
        spawn_button(
            line,
            ink,
            MenuButton::OpenKept(row),
            &world_label(world),
            KEPT_WIDTH,
        );
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
fn spawn_question(parent: &mut ChildSpawnerCommands, ink: &Palette, row: usize) {
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
fn kept_row() -> Node {
    Node {
        width: Val::Px(KEPT_ROW_WIDTH),
        align_items: AlignItems::Center,
        justify_content: JustifyContent::SpaceBetween,
        column_gap: Val::Px(ANSWER_GAP),
        ..default()
    }
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
fn discard_kept(harbour: &mut Harbour, row: usize, status: &mut Status) {
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

// ---------------------------------------------------------------------------
// New world dialog
// ---------------------------------------------------------------------------

fn spawn_new_world_dialog(
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

fn spawn_join_dialog(mut commands: Commands, settings: Res<JoinSettings>, status: Res<Status>) {
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
// Options
// ---------------------------------------------------------------------------

/// The options screen: the way to the two below it, and nothing else.
///
/// A screen that only points at other screens has to earn the press it costs,
/// and this one does by what it keeps *out* of the pause menu. Both of the
/// screens under it are long — ten key rows, or a list of resolutions — and
/// hanging either off the pause menu directly would put a wall of settings one
/// press from the helm.
fn spawn_options(commands: Commands) {
    build_options(commands, &ON_PAPER, DespawnOnExit(AppState::Options));
}

/// The same screen over a paused world, differing only in living and dying
/// with [`Helm::Options`] instead — so that opening it does not leave the
/// world, which is the whole reason the pause menu exists.
fn spawn_paused_options(commands: Commands) {
    build_options(commands, &OVER_THE_WORLD, DespawnOnExit(Helm::Options));
}

fn build_options(mut commands: Commands, ink: &Palette, until: impl Bundle) {
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
fn over_a_world(helm: &Option<Res<State<Helm>>>) -> bool {
    helm.as_ref()
        .is_some_and(|h| matches!(h.get(), Helm::Options | Helm::Display | Helm::Controls))
}

/// Shuts the options screen, returning to whichever screen opened it.
fn close_options(
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
fn back_to_options(
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

fn options_actions(
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
/// which is what Back does and what it means everywhere else in the menus.
///
/// The controls screen is not here, and that is the whole reason this is a
/// system of its own rather than an arm of [`helm_keys`]: there, Escape may
/// mean "not that key" instead, and only [`settings_keys`] knows which.
fn options_keys(
    keys: Res<ButtonInput<KeyCode>>,
    state: Res<State<AppState>>,
    helm: Option<Res<State<Helm>>>,
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
        back_to_options(over_a_world, &mut next_app, &mut next_helm);
    }
}

// ---------------------------------------------------------------------------
// Display
// ---------------------------------------------------------------------------

/// The display screen as reached from the main menu.
fn spawn_display(commands: Commands, settings: Res<DisplaySettings>) {
    build_display(
        commands,
        &ON_PAPER,
        &settings,
        DespawnOnExit(AppState::Display),
    );
}

/// And as reached from the pause menu — see [`spawn_paused_options`].
fn spawn_paused_display(commands: Commands, settings: Res<DisplaySettings>) {
    build_display(
        commands,
        &OVER_THE_WORLD,
        &settings,
        DespawnOnExit(Helm::Display),
    );
}

/// Three rows, and they are three because they are what somebody whose machine
/// cannot keep up reaches for, in the order they reach for them: fill the
/// screen, draw fewer pixels, stop casting shadows.
///
/// Each is built with what the setting is *now* rather than empty for
/// [`refresh_display`] to fill in, so the screen is right on the frame it
/// appears rather than one after.
fn build_display(
    mut commands: Commands,
    ink: &Palette,
    settings: &DisplaySettings,
    until: impl Bundle,
) {
    commands
        .spawn((Name::new("Display screen"), until, screen(ink)))
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
                            spawn_setting_row(
                                rows,
                                ink,
                                "Fullscreen",
                                MenuButton::ToggleFullscreen,
                                DisplayText::Fullscreen,
                                switch_label(settings.fullscreen),
                            );
                            spawn_setting_row(
                                rows,
                                ink,
                                "Resolution",
                                MenuButton::CycleResolution,
                                DisplayText::Resolution,
                                &settings.resolution.label(),
                            );
                            spawn_setting_row(
                                rows,
                                ink,
                                "Shadows",
                                MenuButton::ToggleShadows,
                                DisplayText::Shadows,
                                switch_label(settings.shadows),
                            );
                        });

                    // Plain punctuation only: the default font has no dash of
                    // any kind and draws a missing glyph as an empty box.
                    label(panel, ink, "fewer pixels is less work for the machine; the");
                    label(
                        panel,
                        ink,
                        "window or the screen itself changes size to suit",
                    );
                    // Held open whether or not there is a caveat to put in it,
                    // so a resolution the display cannot do does not shove the
                    // buttons down the moment it is picked.
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
                            ..default()
                        })
                        .with_children(|row| {
                            spawn_button(row, ink, MenuButton::Back, "Back", 130.0);
                        });
                });
        });
}

/// One setting and how it is set, as a name on the left and a button on the
/// right that changes it — the same shape as a key row, because it is the same
/// question asked about something other than a key.
fn spawn_setting_row(
    parent: &mut ChildSpawnerCommands,
    ink: &Palette,
    name: &str,
    action: MenuButton,
    mark: DisplayText,
    value: &str,
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
                Text::new(name.to_string()),
                TextFont {
                    font_size: FontSize::Px(18.0),
                    ..default()
                },
                TextColor(ink.text),
            ));
            row.spawn(padded_button(ink, action, 170.0, KEY_ROW_PADDING))
                .with_children(|button| {
                    button.spawn((mark, button_label(ink, value)));
                });
        });
}

/// What a switch reads. It has to say which way it is set, not what pressing it
/// would do.
fn switch_label(on: bool) -> &'static str {
    if on {
        "On"
    } else {
        "Off"
    }
}

fn display_actions(
    buttons: Query<(&Interaction, &MenuButton), Changed<Interaction>>,
    helm: Option<Res<State<Helm>>>,
    mut settings: ResMut<DisplaySettings>,
    mut next_app: ResMut<NextState<AppState>>,
    mut next_helm: ResMut<NextState<Helm>>,
) {
    let over_a_world = over_a_world(&helm);

    for (interaction, button) in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match button {
            MenuButton::ToggleFullscreen => settings.fullscreen = !settings.fullscreen,
            MenuButton::CycleResolution => settings.resolution = settings.resolution.next(),
            MenuButton::ToggleShadows => settings.shadows = !settings.shadows,
            MenuButton::Back => back_to_options(over_a_world, &mut next_app, &mut next_helm),
            _ => {}
        }
    }
}

/// Keeps the rows in step with the settings behind them, and says what the
/// display will really do with the resolution being asked of it.
///
/// Every line is compared before it is written rather than guarded by
/// `is_changed` on the settings, because the caveat depends on something else
/// as well: the monitors turn up a frame or two into the run, and a screen
/// opened before they did would otherwise keep a caveat it can no longer
/// justify. Writing only what differs is what keeps that from re-laying out the
/// panel every frame.
fn refresh_display(
    settings: Res<DisplaySettings>,
    monitors: Query<(Entity, &Monitor, Has<PrimaryMonitor>)>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut readouts: Query<(&DisplayText, &mut Text)>,
) {
    // The screen this window is on, which on two monitors is not the primary
    // one and is the only one the caveat can honestly be about — see
    // [`settings::showing_on`].
    let monitor =
        settings::showing_on(windows.single().ok(), monitors.iter()).map(|(_, screen)| screen);
    for (which, mut text) in &mut readouts {
        let saying = match which {
            DisplayText::Fullscreen => switch_label(settings.fullscreen).to_string(),
            DisplayText::Resolution => settings.resolution.label(),
            DisplayText::Shadows => switch_label(settings.shadows).to_string(),
            // Only worth a word when the display has no such mode, and only
            // then about the screen it would have filled: windowed, a size is
            // a size and every display can do it.
            DisplayText::Caveat
                if !settings.fullscreen || settings::available(settings.resolution, monitor) =>
            {
                String::new()
            }
            DisplayText::Caveat => format!(
                "this screen has no {} mode, so filling it draws every pixel",
                settings.resolution.label()
            ),
        };
        if text.0 != saying {
            text.0 = saying;
        }
    }
}

// ---------------------------------------------------------------------------
// Controls
// ---------------------------------------------------------------------------

/// The controls screen as reached from the options screen.
fn spawn_settings(commands: Commands, bindings: Res<KeyBindings>) {
    spawn_controls(
        commands,
        &ON_PAPER,
        &bindings,
        DespawnOnExit(AppState::Controls),
    );
}

/// The same screen as reached from the pause menu — see
/// [`spawn_paused_options`].
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
fn settings_keys(
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
                            spawn_button(rows, &ink, MenuButton::Options, "Options", 200.0);
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
        // Not ours. On the options and display screens Escape means one step
        // back and `options_keys` takes it; on the controls screen it may mean
        // "not that key" instead, and only `settings_keys` knows which; at the
        // console it means "close the console"; on the chart it may mean
        // "stop writing this island's name". Each screen hears its own key,
        // because only it knows what the key means while it is up.
        Helm::Options | Helm::Display | Helm::Controls | Helm::Console | Helm::Chart => {}
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

/// The line a dialog reports a dial on. Spawned even when there is nothing to
/// say — a line that appeared only when it had text would push the buttons
/// under it down the moment the player pressed one — and spawned with whatever
/// there is to say, since the set-sail screen can be built again mid-dial and
/// must not come back having forgotten it.
fn status_line(parent: &mut ChildSpawnerCommands, ink: &Palette, saying: &str) {
    parent.spawn((
        StatusText,
        Text::new(saying.to_string()),
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

    /// A keypress as the controls screen reads it: a real one carries both the
    /// position pressed and what that position typed, and the screen wants
    /// each for a different purpose.
    fn a_press(key: KeyCode, typed: &str) -> KeyboardInput {
        KeyboardInput {
            key_code: key,
            logical_key: Key::Character(typed.into()),
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        }
    }

    /// Sends one, down the channel [`settings_keys`] reads.
    fn type_key(app: &mut App, key: KeyCode, typed: &str) {
        app.world_mut().write_message(a_press(key, typed));
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
    ///
    /// Taken away again before the frame the transition lands on, and that
    /// matters rather than being tidiness. A button left lying in the world
    /// still reads as *freshly changed* to any system that has not run since it
    /// was spawned — and a click that changes screens is exactly that, the
    /// arriving screen's systems having sat out every frame until now. A Back
    /// pressed on one screen would be read a second time by the screen it
    /// returned to, which then went back again. Real buttons cannot do it: a
    /// screen takes its own down on the way out, `DespawnOnExit`, during the
    /// very transition this frame is here to let land. This is what gives the
    /// stand-ins the same manners.
    fn click(app: &mut App, button: MenuButton) {
        let pressed = click_once(app, button);
        app.world_mut().entity_mut(pressed).despawn();
        app.update();
    }

    /// A click and the one frame it takes to be seen, with none of the frames
    /// in which something the click started could land. What the button *did*
    /// is visible; what may come of it later is not. The button is handed back
    /// so a caller that runs on can clear it up — see [`click`].
    fn click_once(app: &mut App, button: MenuButton) -> Entity {
        let pressed = app.world_mut().spawn((button, Interaction::Pressed)).id();
        app.update();
        pressed
    }

    /// Taps a key for exactly one frame, down the channel [`options_keys`]
    /// reads. Releasing afterwards matters: a key still held down never counts
    /// as just-pressed again.
    fn press_key(app: &mut App, key: KeyCode) {
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(key);
        app.update();

        let mut input = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        input.release(key);
        input.clear();
    }

    /// One key, hit the way a keyboard hits it: both channels at once. A real
    /// press is a message *and* a button held down for a frame, and the menus
    /// read it both ways — [`options_keys`] off the button, [`settings_keys`]
    /// off the message. A test that writes only the channel the system it is
    /// about happens to read can never catch the two of them answering the
    /// same press.
    fn hit_key(app: &mut App, key: KeyCode, typed: &str) {
        app.world_mut().write_message(a_press(key, typed));
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

    /// Everything the screen currently says, run together.
    fn screen_text(app: &mut App) -> String {
        app.world_mut()
            .query::<&Text>()
            .iter(app.world())
            .map(|t| t.0.clone())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn setting_sail_opens_the_worlds() {
        let mut app = test_app(AppState::MainMenu);
        click(&mut app, MenuButton::SetSail);
        assert_eq!(state(&app), AppState::SetSail);
    }

    #[test]
    fn new_world_opens_the_setup_dialog() {
        // From the worlds screen, which is where the choice between returning
        // to one and starting one is made. An empty harbour, so this is about
        // the press and not about the cap.
        let mut app = harbour_of(Vec::new());
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

    /// One step back, not all the way out: the dialog is opened from the
    /// worlds screen, so that is where Back belongs.
    #[test]
    fn back_out_of_the_new_world_dialog_returns_to_the_worlds() {
        let mut app = test_app(AppState::NewWorld);
        click(&mut app, MenuButton::Back);
        assert_eq!(state(&app), AppState::SetSail);
    }

    #[test]
    fn back_out_of_the_worlds_returns_to_the_main_menu() {
        let mut app = test_app(AppState::SetSail);
        click(&mut app, MenuButton::Back);
        assert_eq!(state(&app), AppState::MainMenu);
    }

    /// A world for the screen to offer, with a real file behind it so that
    /// discarding one has something to delete. Not a world anything could be
    /// sailed in — what a discard needs is a name to take the lock on and
    /// files to remove, and it never reads a byte of what is in them.
    fn a_kept_world(id: u64) -> KeptWorld {
        let dir = net::worlds_dir().expect("the quarantined data dir");
        std::fs::create_dir_all(&dir).expect("the worlds directory");
        let world = KeptWorld {
            path: dir.join(format!("{}.world", protocol::WorldId(id))),
            id: protocol::WorldId(id),
            name: format!("Test Water {id:x}"),
            age: 0.0,
            kept: SystemTime::now(),
        };
        std::fs::write(&world.path, "a world, as far as this test is concerned").expect("write");
        world
    }

    /// The worlds screen offering exactly the worlds a test names, rather than
    /// whatever this machine happens to keep. Set after entering the screen,
    /// which is what reads the directory — and a changed harbour is what
    /// builds the screen again, so the list on screen is this one.
    fn harbour_of(worlds: Vec<KeptWorld>) -> App {
        let mut app = test_app(AppState::SetSail);
        app.world_mut().resource_mut::<Harbour>().worlds = worlds;
        app.update();
        app
    }

    fn asked(app: &App) -> Option<usize> {
        app.world().resource::<Harbour>().asked
    }

    #[test]
    fn a_kept_world_is_offered_with_a_way_to_throw_it_away() {
        let mut app = harbour_of(vec![a_kept_world(0x51)]);
        let text = screen_text(&mut app);
        assert!(
            text.contains("Test Water 51"),
            "the world is not offered: {text}"
        );
        assert!(text.contains("Discard"), "no way to throw it away: {text}");
    }

    #[test]
    fn discarding_asks_first_and_takes_no_for_an_answer() {
        let world = a_kept_world(0x52);
        let path = world.path.clone();
        let mut app = harbour_of(vec![world]);

        click(&mut app, MenuButton::AskDiscard(0));
        assert_eq!(asked(&app), Some(0));
        assert!(
            screen_text(&mut app).contains("for good?"),
            "the row asked nothing"
        );
        assert!(path.exists(), "the world went before anybody said yes");

        click(&mut app, MenuButton::KeepIt);
        assert_eq!(asked(&app), None);
        assert!(path.exists(), "the world went on a no");
        assert_eq!(app.world().resource::<Harbour>().worlds.len(), 1);
        assert!(
            !screen_text(&mut app).contains("for good?"),
            "the question is still standing"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn yes_throws_the_world_away() {
        let world = a_kept_world(0x53);
        let path = world.path.clone();
        let mut app = harbour_of(vec![world]);

        click(&mut app, MenuButton::AskDiscard(0));
        click(&mut app, MenuButton::Discard(0));

        assert!(!path.exists(), "the world's file outlived the discard");
        assert!(
            app.world().resource::<Harbour>().worlds.is_empty(),
            "the row outlived the world"
        );
        assert_eq!(asked(&app), None);
    }

    /// Leaving the screen answers the question the safe way — and coming back
    /// must not find it still standing over a world that was never chosen.
    #[test]
    fn a_question_walked_away_from_is_not_a_yes() {
        let world = a_kept_world(0x54);
        let path = world.path.clone();
        let mut app = harbour_of(vec![world]);

        click(&mut app, MenuButton::AskDiscard(0));
        click(&mut app, MenuButton::Back);
        assert_eq!(state(&app), AppState::MainMenu);

        go_to(&mut app, AppState::SetSail);
        assert_eq!(asked(&app), None, "the screen came back still asking");
        assert!(path.exists(), "the world went while nobody was looking");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_empty_harbour_says_so() {
        let mut app = harbour_of(Vec::new());
        assert!(screen_text(&mut app).contains("no world sailed from here yet"));
    }

    /// The cap is a closed door, not a hidden row: a full machine says so and
    /// refuses the press, and every world it is keeping is still on the screen
    /// to be returned to or thrown away.
    #[test]
    fn a_full_harbour_refuses_another_world() {
        let worlds: Vec<KeptWorld> = (0..MOST_KEPT_WORLDS)
            .map(|n| a_kept_world(0x60 + n as u64))
            .collect();
        let paths: Vec<_> = worlds.iter().map(|world| world.path.clone()).collect();
        let mut app = harbour_of(worlds);

        click(&mut app, MenuButton::NewWorld);
        assert_eq!(
            state(&app),
            AppState::SetSail,
            "the cap let a sixth world by"
        );
        assert!(
            app.world().resource::<Status>().0.contains("discard"),
            "the press was refused without saying why: {:?}",
            app.world().resource::<Status>().0
        );

        let text = screen_text(&mut app);
        assert!(
            text.contains("no room for another world"),
            "the screen does not say it is full: {text}"
        );
        for (n, _) in paths.iter().enumerate() {
            assert!(
                text.contains(&format!("Test Water {:x}", 0x60 + n)),
                "world {n} of a full harbour is not on the screen"
            );
        }

        // And room made is a way through: the same press, one discard later.
        click(&mut app, MenuButton::AskDiscard(0));
        click(&mut app, MenuButton::Discard(0));
        click(&mut app, MenuButton::NewWorld);
        assert_eq!(state(&app), AppState::NewWorld);

        for path in &paths {
            let _ = std::fs::remove_file(path);
        }
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

    /// The whole ladder from the front of the game, and back down it again.
    /// Each rung is a screen somebody has to be able to leave the way they
    /// arrived, and Back on the bottom two means Options rather than the top.
    #[test]
    fn the_options_ladder_goes_up_from_the_main_menu_and_back_down() {
        let mut app = test_app(AppState::MainMenu);
        click(&mut app, MenuButton::Options);
        assert_eq!(state(&app), AppState::Options);

        click(&mut app, MenuButton::Display);
        assert_eq!(state(&app), AppState::Display);
        click(&mut app, MenuButton::Back);
        assert_eq!(state(&app), AppState::Options);

        click(&mut app, MenuButton::Controls);
        assert_eq!(state(&app), AppState::Controls);
        click(&mut app, MenuButton::Back);
        assert_eq!(state(&app), AppState::Options);

        click(&mut app, MenuButton::Back);
        assert_eq!(state(&app), AppState::MainMenu);
    }

    /// And Escape is Back on every rung of it, stopping at the menu it started
    /// from rather than carrying on into whatever is behind that.
    #[test]
    fn escape_walks_back_down_the_options_ladder_one_rung_at_a_time() {
        let mut app = test_app(AppState::MainMenu);
        click(&mut app, MenuButton::Options);
        click(&mut app, MenuButton::Display);
        assert_eq!(state(&app), AppState::Display);

        for expected in [AppState::Options, AppState::MainMenu] {
            press_key(&mut app, KeyCode::Escape);
            // The frame the transition lands on.
            app.update();
            assert_eq!(state(&app), expected);
        }
        // A press with nowhere left to go leaves the menu where it is.
        press_key(&mut app, KeyCode::Escape);
        app.update();
        assert_eq!(state(&app), AppState::MainMenu);
    }

    /// The one key two systems both hear, on the one screen where they would
    /// disagree about it. Escape arrives as a message and as a held button at
    /// once, and on the controls screen [`settings_keys`] must be the only one
    /// to act on it: with a row armed it means "not that key" and the screen
    /// stays put, and with none armed it is one rung down rather than two.
    #[test]
    fn one_escape_on_the_controls_screen_is_answered_once() {
        let mut app = test_app(AppState::MainMenu);
        click(&mut app, MenuButton::Options);
        click(&mut app, MenuButton::Controls);

        click(&mut app, MenuButton::Rebind(Action::MoveForward));
        hit_key(&mut app, KeyCode::Escape, "\u{1b}");
        // A frame in which a step back would have landed, had one been taken.
        app.update();
        assert_eq!(waiting_on(&app), None, "the row is still waiting for a key");
        assert_eq!(
            state(&app),
            AppState::Controls,
            "cancelling a row also left the screen"
        );

        hit_key(&mut app, KeyCode::Escape, "\u{1b}");
        app.update();
        assert_eq!(state(&app), AppState::Options, "one press, one rung");
    }

    /// What a display row currently reads on its right-hand button.
    fn row_says(app: &mut App, which: DisplayText) -> String {
        app.world_mut()
            .query::<(&DisplayText, &Text)>()
            .iter(app.world())
            .find(|(mark, _)| **mark == which)
            .map(|(_, text)| text.0.clone())
            .expect("no row for the setting")
    }

    fn display(app: &App) -> DisplaySettings {
        *app.world().resource::<DisplaySettings>()
    }

    /// A switch has to say which way it is set, and go on saying it — the row
    /// is built with the setting it has and rewritten whenever it changes, and
    /// reading the row once would test only half of that.
    #[test]
    fn the_display_rows_say_which_way_they_are_set() {
        let mut app = test_app(AppState::Display);
        assert_eq!(row_says(&mut app, DisplayText::Fullscreen), "Off");
        assert_eq!(row_says(&mut app, DisplayText::Shadows), "On");

        click(&mut app, MenuButton::ToggleFullscreen);
        assert!(display(&app).fullscreen);
        assert_eq!(row_says(&mut app, DisplayText::Fullscreen), "On");

        click(&mut app, MenuButton::ToggleShadows);
        assert!(!display(&app).shadows);
        assert_eq!(row_says(&mut app, DisplayText::Shadows), "Off");
    }

    /// The resolutions walk down and come round again, so somebody who has
    /// gone past the one they wanted gets back without a second button.
    #[test]
    fn the_resolution_button_walks_the_ladder_and_comes_round() {
        let mut app = test_app(AppState::Display);
        assert_eq!(row_says(&mut app, DisplayText::Resolution), "Native");

        for expected in ["2160p", "1440p", "1080p", "720p", "Native"] {
            click(&mut app, MenuButton::CycleResolution);
            assert_eq!(row_says(&mut app, DisplayText::Resolution), expected);
        }
    }

    /// A machine that has reported no monitors cannot promise a resolution
    /// below native, and the screen owns up to it rather than pretending.
    /// Headless is exactly that machine, which is what makes this testable.
    ///
    /// Only about filling the screen, though: a caveat is a word about a
    /// display mode, and a window has no need of one.
    #[test]
    fn a_resolution_the_screen_cannot_promise_is_owned_up_to() {
        let mut app = test_app(AppState::Display);
        assert_eq!(row_says(&mut app, DisplayText::Caveat), "");

        click(&mut app, MenuButton::CycleResolution);
        assert_eq!(
            row_says(&mut app, DisplayText::Caveat),
            "",
            "a windowed size was called impossible, and it is only a size"
        );

        click(&mut app, MenuButton::ToggleFullscreen);
        assert!(
            row_says(&mut app, DisplayText::Caveat).contains("no 2160p mode"),
            "the screen claimed a mode it has no monitor to ask about"
        );
    }

    /// The display screen must not tell its two ways in apart: the same
    /// settings and the same rows, whichever side it was opened from.
    #[test]
    fn the_display_screen_is_the_same_screen_over_a_paused_world() {
        let mut app = paused_app();
        click(&mut app, MenuButton::Options);
        click(&mut app, MenuButton::Display);

        click(&mut app, MenuButton::ToggleShadows);
        assert!(!display(&app).shadows);
        assert_eq!(row_says(&mut app, DisplayText::Shadows), "Off");
        assert_eq!(state(&app), AppState::InWorld);
    }

    #[test]
    fn a_row_waits_for_a_key_and_then_takes_it() {
        let mut app = test_app(AppState::Controls);

        click(&mut app, MenuButton::Rebind(Action::MoveForward));
        assert_eq!(waiting_on(&app), Some(Action::MoveForward));

        type_key(&mut app, KeyCode::KeyJ, "j");
        assert_eq!(bindings(&app).key(Action::MoveForward), KeyCode::KeyJ);
        assert_eq!(waiting_on(&app), None, "the row is still waiting");
    }

    #[test]
    fn a_key_is_named_by_what_it_typed_not_where_it_sits() {
        let mut app = test_app(AppState::Controls);

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
        go_to(&mut app, AppState::Controls);
        assert_eq!(row_text(&mut app, Action::SteerLeft), "A");

        click(&mut app, MenuButton::Rebind(Action::SteerLeft));
        assert_eq!(row_text(&mut app, Action::SteerLeft), "press a key");

        type_key(&mut app, KeyCode::KeyH, "h");
        app.update();
        assert_eq!(row_text(&mut app, Action::SteerLeft), "H");
    }

    #[test]
    fn escape_abandons_a_capture_and_changes_nothing() {
        let mut app = test_app(AppState::Controls);
        let before = bindings(&app).clone();

        click(&mut app, MenuButton::Rebind(Action::MoveBack));
        type_key(&mut app, KeyCode::Escape, "\u{1b}");

        assert_eq!(waiting_on(&app), None);
        assert_eq!(bindings(&app), &before);
        // And having cancelled, we're still on the screen rather than back out.
        assert_eq!(state(&app), AppState::Controls);
    }

    #[test]
    fn a_reserved_key_is_refused_and_the_row_keeps_waiting() {
        let mut app = test_app(AppState::Controls);

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
        let mut app = test_app(AppState::Controls);

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
        let mut app = test_app(AppState::Controls);

        click(&mut app, MenuButton::Rebind(Action::SteerLeft));
        type_key(&mut app, KeyCode::KeyZ, "z");
        click(&mut app, MenuButton::ResetKeys);

        assert_eq!(bindings(&app), &KeyBindings::default());
        assert_eq!(waiting_on(&app), None);
    }

    #[test]
    fn escape_leaves_the_controls_screen_when_no_row_is_waiting() {
        let mut app = test_app(AppState::Controls);
        type_key(&mut app, KeyCode::Escape, "\u{1b}");
        app.update();
        assert_eq!(state(&app), AppState::Options);
    }

    #[test]
    fn leaving_the_screen_forgets_a_waiting_row() {
        let mut app = test_app(AppState::Controls);

        click(&mut app, MenuButton::Rebind(Action::TurnRight));
        click(&mut app, MenuButton::Back);
        assert_eq!(state(&app), AppState::Options);

        go_to(&mut app, AppState::Controls);
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

        go_to(&mut app, AppState::Controls);
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

    /// The same ladder over a paused world, and the thing that matters at
    /// every rung of it: the world is still there. Leaving `AppState::InWorld`
    /// is what takes a world down — see [`Helm`] — so a settings screen that
    /// reached for an `AppState` would evict everyone sailing in a shared one.
    #[test]
    fn the_options_ladder_over_a_paused_world_never_leaves_the_world() {
        let mut app = paused_app();
        click(&mut app, MenuButton::Options);
        assert_eq!(helm(&app), Some(Helm::Options));
        assert_eq!(named(&mut app, "Options screen"), 1);

        click(&mut app, MenuButton::Display);
        assert_eq!(helm(&app), Some(Helm::Display));
        assert_eq!(named(&mut app, "Display screen"), 1);
        click(&mut app, MenuButton::Back);
        assert_eq!(helm(&app), Some(Helm::Options));
        assert_eq!(named(&mut app, "Display screen"), 0);

        click(&mut app, MenuButton::Controls);
        assert_eq!(helm(&app), Some(Helm::Controls));
        assert_eq!(named(&mut app, "Controls screen"), 1);
        click(&mut app, MenuButton::Back);
        assert_eq!(helm(&app), Some(Helm::Options));

        click(&mut app, MenuButton::Back);
        assert_eq!(helm(&app), Some(Helm::Paused));
        assert_eq!(named(&mut app, "Options screen"), 0);
        assert_eq!(state(&app), AppState::InWorld, "the world was given up");
    }

    /// Escape is Back on every rung of the ladder, and stops at the pause menu
    /// rather than carrying on out of the world.
    #[test]
    fn escape_walks_back_down_the_paused_ladder_one_rung_at_a_time() {
        let mut app = paused_app();
        click(&mut app, MenuButton::Options);
        click(&mut app, MenuButton::Display);

        for expected in [Helm::Options, Helm::Paused] {
            press_key(&mut app, KeyCode::Escape);
            // The frame the transition lands on.
            app.update();
            assert_eq!(helm(&app), Some(expected));
        }
        assert_eq!(state(&app), AppState::InWorld);
    }

    #[test]
    fn escape_backs_out_of_the_paused_controls_to_the_options_screen() {
        let mut app = paused_app();
        click(&mut app, MenuButton::Options);
        click(&mut app, MenuButton::Controls);
        type_key(&mut app, KeyCode::Escape, "");
        // The frame the transition lands on.
        app.update();
        assert_eq!(helm(&app), Some(Helm::Options));
        assert_eq!(state(&app), AppState::InWorld);
    }

    /// Escape means "not that key" while a row is armed, wherever the screen
    /// was opened from — so it must not also step back to the pause menu.
    #[test]
    fn escape_on_the_paused_controls_cancels_an_armed_row_first() {
        let mut app = paused_app();
        click(&mut app, MenuButton::Options);
        click(&mut app, MenuButton::Controls);
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
        click(&mut app, MenuButton::Options);
        click(&mut app, MenuButton::Controls);
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
        assert!(screen_text(&mut app).contains("leaving closes it on them"));
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
        assert!(!screen_text(&mut app).contains("leaving closes it on them"));
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
