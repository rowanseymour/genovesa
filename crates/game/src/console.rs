//! The debug console: the key left of 1, and the two languages typed into it.
//!
//! One rule decides where a line runs, and it is syntactic on purpose. A line
//! that starts `set` names a variable of *this client* — what this machine
//! draws, listed in [`Toggles`] — and never leaves the machine. Any other
//! line is an imperative about the *world*, and crosses the wire verbatim as
//! [`protocol::ToServer::Command`]: the vocabulary belongs to the server,
//! this module does not parse a word of it, and whatever text comes back as
//! [`protocol::ToClient::Reply`] is printed here. So `set shadows off`
//! doctors one player's picture and admits it on the readout, while
//! `time 18:00` moves the sun for everyone in the session — and the prompt
//! itself teaches the difference.
//!
//! The console is part of every build, unlike the readout it switches on:
//! it is *how* debug states are reached now, and a player who stumbles into
//! it can type `help` at a server that will answer. It opens only at the
//! helm — the menus have text fields of their own, and a backquote typed
//! into an address must not summon anything — and while it is up the
//! keyboard is its alone, [`Helm::Console`] being a state exactly so that
//! every system reading the player's hands sits out.
//!
//! The key is hard-wired as [`KeyCode::Backquote`] — a *position*, the key
//! left of 1, whatever a layout prints on it — and reserved from rebinding,
//! for the reason the arrows are: it is the way in and out of a mode, and a
//! key that could be given away could strand whoever gave it.
//!
//! Tab completes, but only the words this client can *know*: `set` and its
//! variables, which are this module's own grammar, and the server's verbs —
//! which are not guessed at but taught, arriving on joining as
//! [`protocol::ToClient::Vocabulary`], so completion grows with the server
//! the way the vocabulary itself does.

use std::collections::VecDeque;

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::text::FontSize;

use crate::boat::HullKind;
use crate::debug::{Toggles, BACKDROP, BOAT, REACH, SWITCHES, TEXT};
use crate::net::Online;
use crate::Helm;

/// How many lines the console remembers. Enough that nothing a session says
/// scrolls away in practice, bounded so a long session cannot grow one
/// string list forever.
const SCROLLBACK: usize = 100;

/// How many lines of that are on screen above the prompt.
const SHOWN: usize = 10;

/// The longest line the console will hold, in characters — comfortably
/// inside the frame the protocol allows a client, so nothing typed here can
/// fail to fit the wire.
const MAX_LINE: usize = 200;

pub struct ConsolePlugin;

impl Plugin for ConsolePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Console>()
            .init_resource::<Toggles>()
            .add_systems(Update, open_console.run_if(in_state(Helm::Sailing)))
            // Not gated on the console being up, exactly as the menus' typing
            // systems are not gated on their screens: a message reader that
            // only ran while open would be handed, on opening, whatever was
            // pressed on the way in.
            .add_systems(Update, console_keys)
            .add_systems(OnEnter(Helm::Console), spawn_console)
            // Only when there is something new to show, and after the keys
            // that would be the new thing. The text is rebuilt and assigned
            // wholesale, which marks it changed and puts the whole panel back
            // through text layout — that was happening at frame rate for the
            // whole time a player sat with the console open, to say what it
            // already said. What it says changes on a keystroke or an
            // arriving reply and at no other time; the panel is spawned
            // holding its first line, so there is no blank frame to cover.
            .add_systems(
                Update,
                refresh_console
                    .after(console_keys)
                    .run_if(in_state(Helm::Console).and_then(resource_changed::<Console>)),
            );
    }
}

/// The console's memory: what has been said, what is being typed, and what
/// was typed before. Lives for the whole run rather than for the state, so
/// closing the console loses nothing and a reply that arrives while it is
/// closed is waiting when it opens.
#[derive(Resource, Default)]
pub struct Console {
    /// What is on the screen, oldest first.
    lines: VecDeque<String>,
    /// The line being typed.
    input: String,
    /// Every line submitted, oldest first — what the up arrow walks back
    /// through, which is how "lean on a key and watch the picture answer"
    /// survived the number keys: up, enter, up, enter.
    history: Vec<String>,
    /// Where the up arrow has got to in that history, `None` when the input
    /// is the player's own fresh line.
    recall: Option<usize>,
    /// The server's verbs, as taught on joining — what tab offers for a
    /// line's first word alongside `set`. Empty until the teaching arrives,
    /// when tab knows only the local grammar.
    verbs: Vec<String>,
}

impl Console {
    /// Puts text on the console, a line at a time — a server's reply is one
    /// string that may carry several.
    pub fn say(&mut self, text: &str) {
        for line in text.lines() {
            self.lines.push_back(line.to_string());
            while self.lines.len() > SCROLLBACK {
                self.lines.pop_front();
            }
        }
    }

    /// Takes the server's word list — see [`crate::net::receive`], which is
    /// where a [`protocol::ToClient::Vocabulary`] lands.
    pub fn teach(&mut self, verbs: Vec<String>) {
        self.verbs = verbs;
    }

    /// What tab does to the line being typed: grows the last word to the
    /// longest lead every matching word shares, finishes it — trailing space
    /// and all — when one word alone matches, and says the choices when
    /// several do and no growing is possible. On an empty word that makes
    /// tab the index: it offers everything that could stand there.
    fn complete(&mut self) {
        let (before, partial) = match self.input.rsplit_once(char::is_whitespace) {
            Some(split) => split,
            None => ("", self.input.as_str()),
        };
        let matches: Vec<&str> = self
            .completions(before)
            .into_iter()
            .filter(|word| word.starts_with(partial))
            .collect();

        let keep = self.input.len() - partial.len();
        let grown = match matches.as_slice() {
            [] => return,
            // One word left: the whole of it, and the space after — the only
            // thing left to type is the next word.
            [word] => format!("{word} "),
            words => {
                let lead = shared_lead(words);
                if lead.len() == partial.len() {
                    // Nothing grows: the choices themselves are the answer,
                    // said where the replies land.
                    let choices = words.join("  ");
                    self.say(&choices);
                    return;
                }
                lead.to_string()
            }
        };
        if keep + grown.len() <= MAX_LINE {
            self.input.truncate(keep);
            self.input.push_str(&grown);
            self.recall = None;
        }
    }

    /// The words that could stand after `before`, which are the ones this
    /// client can know: for a line's first word, `set` and whatever verbs
    /// the server taught; after a lone `set`, the variables. Anything deeper
    /// is the server's business, unknowable here and not guessed at.
    fn completions(&self, before: &str) -> Vec<&str> {
        let mut earlier = before.split_whitespace();
        match (earlier.next(), earlier.next()) {
            (None, _) => std::iter::once("set")
                .chain(self.verbs.iter().map(String::as_str))
                .collect(),
            (Some("set"), None) => variables(),
            _ => Vec::new(),
        }
    }

    /// Whether a line has been said, word for word — for the tests, which
    /// otherwise could only reach the scrollback through the drawn text.
    #[cfg(test)]
    pub(crate) fn said(&self, line: &str) -> bool {
        self.lines.iter().any(|said| said == line)
    }

    /// Whether a verb has been taught — for the net tests, which otherwise
    /// could only see the vocabulary through tab.
    #[cfg(test)]
    pub(crate) fn knows(&self, verb: &str) -> bool {
        self.verbs.iter().any(|known| known == verb)
    }

    /// What the console shows: the last few lines said, and the prompt with
    /// the line being typed.
    fn text(&self) -> String {
        let mut shown: Vec<&str> = self
            .lines
            .iter()
            .rev()
            .take(SHOWN)
            .rev()
            .map(String::as_str)
            .collect();
        let prompt = format!("> {}_", self.input);
        shown.push(&prompt);
        shown.join("\n")
    }
}

/// Where a line goes, decided by [`dispatch`]: answered here, or sent to the
/// server whose world it is about.
#[derive(Debug, PartialEq)]
enum Dispatch {
    /// A `set` line: it ran against [`Toggles`], and this is its answer.
    Local(String),
    /// Anything else: the server's to interpret, verbatim.
    Remote,
}

/// Runs a line's local half, or says it is not local at all. The one place
/// the grammar's rule lives: `set` never leaves the machine, nothing else
/// ever stays on it.
fn dispatch(line: &str, toggles: &mut Toggles) -> Dispatch {
    let words: Vec<&str> = line.split_whitespace().collect();
    match words.split_first() {
        Some((&"set", rest)) => Dispatch::Local(set(rest, toggles)),
        _ => Dispatch::Remote,
    }
}

/// The `set` variables, in the order a bare `set` lists them — what tab
/// completes after `set`, and what a miss is told to try instead, this being
/// the half of the grammar that lives on this machine.
///
/// Read off [`SWITCHES`] rather than listed again here, so the words this
/// offers are the words [`set`] serves, always.
fn variables() -> Vec<&'static str> {
    SWITCHES
        .iter()
        .map(|switch| switch.name)
        .chain([REACH, BOAT])
        .collect()
}

/// The longest lead every word here shares — at least what was typed, each
/// already starting with that.
fn shared_lead<'a>(words: &[&'a str]) -> &'a str {
    let mut lead = words[0];
    for word in &words[1..] {
        while !word.starts_with(lead) {
            let mut shorter = lead.chars();
            shorter.next_back();
            lead = shorter.as_str();
        }
    }
    lead
}

/// The `set` grammar: `set` lists every variable, `set <var>` reads one,
/// `set <var> <value>` writes one. Always answered — a console that says
/// nothing back reads as a console that heard nothing.
fn set(args: &[&str], toggles: &mut Toggles) -> String {
    match args {
        [] => {
            let mut said = Vec::new();
            for var in variables() {
                if let Some(reading) = read(var, toggles) {
                    said.push(reading);
                }
            }
            said.join(" / ")
        }
        [var] => read(var, toggles).unwrap_or_else(|| no_such(var)),
        [var, value] if *var == REACH => reach(value, &mut toggles.reach),
        [var, value] if *var == BOAT => boat(value, &mut toggles.boat),
        [var, value] => match toggles.switch(var) {
            Some(state) => switch(var, value, state),
            None => no_such(var),
        },
        _ => "one variable, one value — `set reach 450`".to_string(),
    }
}

/// What one variable reads as — the same words a write answers with, so the
/// answer to setting is the proof it took. `None` for a name that is not a
/// variable at all.
fn read(var: &str, toggles: &mut Toggles) -> Option<String> {
    if var == REACH {
        return Some(format!("{REACH} {:.0}m", toggles.reach));
    }
    if var == BOAT {
        return Some(format!("{BOAT} {}", toggles.boat.name()));
    }
    toggles.switch(var).map(|on| onoff(var, *on))
}

fn onoff(var: &str, on: bool) -> String {
    format!("{var} {}", if on { "on" } else { "off" })
}

fn no_such(var: &str) -> String {
    let offered = variables();
    let (last, rest) = offered.split_last().expect("variables to offer");
    format!(
        "nothing here called `{var}` — {} or {last}",
        rest.join(", ")
    )
}

/// Throws a boolean switch, answering with the state it is now in — the same
/// words a read gives, so the answer to setting is the proof it took.
fn switch(var: &str, value: &str, state: &mut bool) -> String {
    match value {
        "on" => *state = true,
        "off" => *state = false,
        _ => return format!("`{var}` is on or off"),
    }
    onoff(var, *state)
}

/// Sets the shadow reach, in metres. `default` is the world's own — see
/// [`Toggles::reach`] — and the bounds only refuse what the cascades could
/// not survive: a reach of nothing, or one so deep the maps are all in the
/// haze.
fn reach(value: &str, state: &mut f32) -> String {
    let metres = match value {
        "default" => Some(crate::HAZE_END),
        _ => value
            .parse::<f32>()
            .ok()
            .filter(|m| (10.0..=10_000.0).contains(m)),
    };
    match metres {
        Some(metres) => {
            *state = metres;
            format!("reach {metres:.0}m")
        }
        None => "`reach` is metres — `set reach 450`, or `set reach default`".to_string(),
    }
}

/// Sets which kind of boat this client rigs its own hull as — the dev
/// stand-in for the rowboat being dealt by the world; see
/// [`crate::debug::Toggles::boat`]. Answered with the state it is now in,
/// like every other write here.
fn boat(value: &str, state: &mut HullKind) -> String {
    match HullKind::named(value) {
        Some(kind) => {
            *state = kind;
            format!("{BOAT} {}", kind.name())
        }
        None => format!("`{BOAT}` is ship or rowboat — `set boat rowboat`"),
    }
}

/// Opens the console. Only at the helm: the menus own the keyboard on every
/// other screen, and the controls screen in particular binds whatever key it
/// is given.
fn open_console(keys: Res<ButtonInput<KeyCode>>, mut next: ResMut<NextState<Helm>>) {
    if keys.just_pressed(KeyCode::Backquote) {
        next.set(Helm::Console);
    }
}

/// The console's whole keyboard: typing, editing, history, submitting, and
/// the two ways out. Reads what keys *typed* for the line itself, exactly as
/// the menus' text fields do, and positions for the keys that mean something
/// — so the way out is the key that came in, wherever it is and whatever it
/// prints.
fn console_keys(
    helm: Option<Res<State<Helm>>>,
    mut presses: MessageReader<KeyboardInput>,
    mut console: ResMut<Console>,
    mut toggles: ResMut<Toggles>,
    online: Option<Res<Online>>,
    mut next: Option<ResMut<NextState<Helm>>>,
) {
    let open = helm.is_some_and(|helm| *helm.get() == Helm::Console);

    for press in presses.read() {
        // A held key repeats, which is what a text field wants: holding
        // backspace should clear the line rather than one character of it.
        if !open || press.state != ButtonState::Pressed {
            continue;
        }

        match press.key_code {
            KeyCode::Escape | KeyCode::Backquote => {
                if let Some(next) = next.as_mut() {
                    next.set(Helm::Sailing);
                }
            }
            KeyCode::Backspace => {
                console.input.pop();
                console.recall = None;
            }
            KeyCode::Enter | KeyCode::NumpadEnter => {
                submit(&mut console, &mut toggles, online.as_deref());
            }
            // The history, walked with the arrows: up into it, down back out,
            // and past the newest entry is the empty prompt again.
            KeyCode::ArrowUp => {
                let back = match console.recall {
                    Some(at) => at.saturating_sub(1),
                    None if console.history.is_empty() => continue,
                    None => console.history.len() - 1,
                };
                console.recall = Some(back);
                console.input = console.history[back].clone();
            }
            KeyCode::ArrowDown => {
                let Some(at) = console.recall else { continue };
                if at + 1 < console.history.len() {
                    console.recall = Some(at + 1);
                    console.input = console.history[at + 1].clone();
                } else {
                    console.recall = None;
                    console.input.clear();
                }
            }
            KeyCode::Tab => console.complete(),
            _ => {
                // What the press *typed* — with the space named rather than
                // read off it, because a space arrives as [`Key::Space`] and
                // not as a character, exactly as `bindings::typed_label`
                // found before this did.
                let typed = match &press.logical_key {
                    Key::Character(typed) => typed.as_str(),
                    Key::Space => " ",
                    _ => continue,
                };
                for character in typed.chars().filter(|c| !c.is_control() && *c != '`') {
                    if console.input.len() < MAX_LINE {
                        console.input.push(character);
                    }
                }
                console.recall = None;
            }
        }
    }
}

/// Takes the line as typed: echoes it, runs its local half or puts it on the
/// wire, and remembers it for the up arrow.
fn submit(console: &mut Console, toggles: &mut Toggles, online: Option<&Online>) {
    let line = console.input.trim().to_string();
    console.input.clear();
    console.recall = None;
    if line.is_empty() {
        return;
    }

    console.say(&format!("> {line}"));
    console.history.push(line.clone());

    match dispatch(&line, toggles) {
        Dispatch::Local(reply) => console.say(&reply),
        Dispatch::Remote => match online {
            // Fire and forget, like everything a connection says: the answer
            // arrives through the reader thread as a Reply, and lands here
            // through `crate::net::receive`.
            Some(online) => online.connection.command(line),
            // Unreachable while every world is a served world, but the line
            // was typed and silence would read as a hang.
            None => console.say("nobody is serving this world"),
        },
    }
}

/// Marks the console's one text block, so the refresh can find it.
#[derive(Component)]
struct ConsoleText;

fn spawn_console(mut commands: Commands, console: Res<Console>) {
    commands
        .spawn((
            Name::new("Console"),
            DespawnOnExit(Helm::Console),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(8.0),
                bottom: Val::Px(8.0),
                width: Val::Px(560.0),
                padding: UiRect::axes(Val::Px(10.0), Val::Px(6.0)),
                ..default()
            },
            BackgroundColor(BACKDROP),
            // Over the readout's own layer, which shares its corner colours:
            // the two can overlap at small windows, and the one being typed
            // into should win.
            GlobalZIndex(2),
        ))
        .with_children(|panel| {
            panel.spawn((
                ConsoleText,
                // Holding what the console already has to say — the prompt,
                // and whatever was said while it was shut.
                Text::new(console.text()),
                TextFont {
                    font_size: FontSize::Px(14.0),
                    ..default()
                },
                TextColor(TEXT),
            ));
        });
}

fn refresh_console(console: Res<Console>, mut texts: Query<&mut Text, With<ConsoleText>>) {
    for mut text in &mut texts {
        text.0 = console.text();
    }
}

#[cfg(test)]
mod tests {
    use bevy::state::app::StatesPlugin;

    use super::*;
    use crate::AppState;

    #[test]
    fn set_never_leaves_the_machine_and_nothing_else_stays() {
        let mut toggles = Toggles::default();
        assert_eq!(
            dispatch("set stats on", &mut toggles),
            Dispatch::Local("stats on".to_string())
        );
        assert!(toggles.stats);

        // The rule is the first word and nothing else: these are the
        // server's, however local they might sound.
        assert_eq!(dispatch("spawn shark", &mut toggles), Dispatch::Remote);
        assert_eq!(dispatch("help", &mut toggles), Dispatch::Remote);
        assert_eq!(dispatch("time 18:00", &mut toggles), Dispatch::Remote);
    }

    #[test]
    fn every_switch_answers_and_takes() {
        let mut toggles = Toggles::default();
        assert_eq!(set(&["shadows", "off"], &mut toggles), "shadows off");
        assert!(!toggles.shadows);
        assert_eq!(set(&["haze", "off"], &mut toggles), "haze off");
        assert_eq!(set(&["wireframe", "on"], &mut toggles), "wireframe on");
        assert_eq!(set(&["reach", "450"], &mut toggles), "reach 450m");
        assert_eq!(toggles.reach, 450.0);
        assert_eq!(set(&["reach", "default"], &mut toggles), "reach 900m");
        assert_eq!(toggles.reach, crate::HAZE_END);
        assert_eq!(set(&["shadows", "on"], &mut toggles), "shadows on");
        assert!(toggles.shadows);
    }

    #[test]
    fn a_bare_set_reads_and_a_named_one_reads_one() {
        let mut toggles = Toggles {
            wireframe: true,
            ..Default::default()
        };
        assert_eq!(
            set(&[], &mut toggles),
            "stats off / shadows on / haze on / wireframe on / reach 900m / boat ship"
        );
        assert_eq!(set(&["haze"], &mut toggles), "haze on");
    }

    #[test]
    fn a_wrong_set_is_answered_not_swallowed() {
        let mut toggles = Toggles::default();
        let unknown = set(&["fog", "off"], &mut toggles);
        assert!(unknown.contains("`fog`"), "unhelpful: {unknown}");

        let not_a_switch = set(&["stats", "maybe"], &mut toggles);
        assert!(
            not_a_switch.contains("on or off"),
            "unhelpful: {not_a_switch}"
        );
        assert!(!toggles.stats, "a refused value took anyway");

        let not_metres = set(&["reach", "far"], &mut toggles);
        assert!(not_metres.contains("metres"), "unhelpful: {not_metres}");
        assert_eq!(toggles.reach, crate::HAZE_END);
    }

    /// A headless app at the helm, with the console systems and the states
    /// they steer.
    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins((StatesPlugin, ConsolePlugin))
            .init_state::<AppState>()
            .add_sub_state::<Helm>()
            .init_resource::<ButtonInput<KeyCode>>()
            .add_message::<KeyboardInput>();
        app.update();
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::InWorld);
        app.update();
        app
    }

    fn helm(app: &App) -> Helm {
        *app.world().resource::<State<Helm>>().get()
    }

    fn type_key(app: &mut App, key: KeyCode, typed: &str) {
        app.world_mut().write_message(KeyboardInput {
            key_code: key,
            // A space is a *named* key to winit, not a character — the same
            // shape `bindings::typed_label` handles — so the fake press has
            // to arrive the way a real spacebar does.
            logical_key: match typed {
                "" => Key::Unidentified(bevy::input::keyboard::NativeKey::Unidentified),
                " " => Key::Space,
                typed => Key::Character(typed.into()),
            },
            state: ButtonState::Pressed,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
        app.update();
    }

    /// Types text without submitting it — which key each character came off
    /// hardly matters, except that a space has to arrive as the spacebar.
    fn type_word(app: &mut App, text: &str) {
        for character in text.chars() {
            let key = if character == ' ' {
                KeyCode::Space
            } else {
                KeyCode::KeyA
            };
            type_key(app, key, &character.to_string());
        }
    }

    fn type_line(app: &mut App, line: &str) {
        type_word(app, line);
        type_key(app, KeyCode::Enter, "\r");
    }

    /// Presses the key left of 1 and lets the state change land — a
    /// transition queued in one update applies at the top of the next.
    fn press_backquote(app: &mut App) {
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Backquote);
        app.update();
        app.update();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset(KeyCode::Backquote);
    }

    #[test]
    fn the_backquote_opens_and_closes_and_escape_leaves() {
        let mut app = test_app();
        assert_eq!(helm(&app), Helm::Sailing);

        press_backquote(&mut app);
        assert_eq!(helm(&app), Helm::Console);

        // The way out is the way in — read as a position, so it works
        // whatever the key prints — and the panel goes with the state.
        type_key(&mut app, KeyCode::Backquote, "`");
        app.update();
        assert_eq!(helm(&app), Helm::Sailing);

        press_backquote(&mut app);
        assert_eq!(helm(&app), Helm::Console);
        type_key(&mut app, KeyCode::Escape, "\u{1b}");
        app.update();
        assert_eq!(helm(&app), Helm::Sailing);
    }

    #[test]
    fn a_set_line_typed_at_the_console_lands_in_the_toggles() {
        let mut app = test_app();
        press_backquote(&mut app);

        type_line(&mut app, "set wireframe on");

        assert!(app.world().resource::<Toggles>().wireframe);
        let console = app.world().resource::<Console>();
        assert_eq!(
            console.lines.iter().cloned().collect::<Vec<_>>(),
            ["> set wireframe on", "wireframe on"]
        );
        assert_eq!(console.history, ["set wireframe on"]);
        assert!(console.input.is_empty());
    }

    #[test]
    fn the_console_spawns_with_the_state_and_shows_the_prompt() {
        let mut app = test_app();
        press_backquote(&mut app);
        type_key(&mut app, KeyCode::KeyA, "s");
        app.update();

        let text = app
            .world_mut()
            .query_filtered::<&Text, With<ConsoleText>>()
            .single(app.world())
            .expect("the console never spawned")
            .0
            .clone();
        assert_eq!(text, "> s_");
    }

    #[test]
    fn the_up_arrow_walks_the_history_and_down_walks_out() {
        let mut app = test_app();
        press_backquote(&mut app);

        type_line(&mut app, "set haze off");
        type_line(&mut app, "set haze on");

        type_key(&mut app, KeyCode::ArrowUp, "");
        assert_eq!(app.world().resource::<Console>().input, "set haze on");
        type_key(&mut app, KeyCode::ArrowUp, "");
        assert_eq!(app.world().resource::<Console>().input, "set haze off");
        // The top of the history holds rather than wrapping.
        type_key(&mut app, KeyCode::ArrowUp, "");
        assert_eq!(app.world().resource::<Console>().input, "set haze off");

        type_key(&mut app, KeyCode::ArrowDown, "");
        assert_eq!(app.world().resource::<Console>().input, "set haze on");
        // And past the newest is the fresh prompt again.
        type_key(&mut app, KeyCode::ArrowDown, "");
        assert_eq!(app.world().resource::<Console>().input, "");
    }

    /// What the real server teaches, as the completion tests' vocabulary —
    /// the teaching itself is the net module's to test.
    fn taught(app: &mut App) {
        app.world_mut().resource_mut::<Console>().teach(
            ["help", "spawn", "time", "weather"]
                .map(String::from)
                .to_vec(),
        );
    }

    fn input(app: &App) -> String {
        app.world().resource::<Console>().input.clone()
    }

    #[test]
    fn tab_finishes_a_lone_match_with_the_space_after() {
        let mut app = test_app();
        taught(&mut app);
        press_backquote(&mut app);

        // The server's verb, taught rather than known.
        type_key(&mut app, KeyCode::KeyA, "s");
        type_key(&mut app, KeyCode::KeyA, "p");
        type_key(&mut app, KeyCode::Tab, "");
        assert_eq!(input(&app), "spawn ");

        // And the local grammar's variable, after the word that names it.
        type_key(&mut app, KeyCode::Enter, "\r");
        type_word(&mut app, "set w");
        type_key(&mut app, KeyCode::Tab, "");
        assert_eq!(input(&app), "set wireframe ");
    }

    #[test]
    fn tab_grows_what_it_can_and_offers_what_it_cannot() {
        let mut app = test_app();
        taught(&mut app);
        press_backquote(&mut app);

        // `s` could still be `set` or `spawn`: nothing grows, so the choices
        // are said and the line stands.
        type_key(&mut app, KeyCode::KeyA, "s");
        type_key(&mut app, KeyCode::Tab, "");
        assert_eq!(input(&app), "s");
        assert!(app.world().resource::<Console>().said("set  spawn"));

        // `t` grows to the `ti` that `time` and `tide` share, and no further.
        app.world_mut()
            .resource_mut::<Console>()
            .teach(["time", "tide"].map(String::from).to_vec());
        type_key(&mut app, KeyCode::Backspace, "");
        type_key(&mut app, KeyCode::KeyA, "t");
        type_key(&mut app, KeyCode::Tab, "");
        assert_eq!(input(&app), "ti");
    }

    #[test]
    fn tab_on_an_empty_word_is_the_index() {
        let mut app = test_app();
        taught(&mut app);
        press_backquote(&mut app);

        // An empty line: everything a first word could be.
        type_key(&mut app, KeyCode::Tab, "");
        assert_eq!(input(&app), "");
        assert!(app
            .world()
            .resource::<Console>()
            .said("set  help  spawn  time  weather"));

        // After `set `: every variable.
        type_word(&mut app, "set ");
        type_key(&mut app, KeyCode::Tab, "");
        assert_eq!(input(&app), "set ");
        assert!(app
            .world()
            .resource::<Console>()
            .said("stats  shadows  haze  wireframe  reach  boat"));
    }

    #[test]
    fn tab_never_guesses_at_the_servers_arguments() {
        let mut app = test_app();
        taught(&mut app);
        press_backquote(&mut app);

        // What follows `spawn` is the server's vocabulary, not taught and
        // not guessed: tab does nothing, quietly.
        type_word(&mut app, "spawn sh");
        type_key(&mut app, KeyCode::Tab, "");
        assert_eq!(input(&app), "spawn sh");
        assert!(app.world().resource::<Console>().lines.is_empty());
    }

    #[test]
    fn before_the_teaching_tab_knows_only_the_local_grammar() {
        let mut app = test_app();
        press_backquote(&mut app);

        type_key(&mut app, KeyCode::KeyA, "s");
        type_key(&mut app, KeyCode::Tab, "");
        assert_eq!(input(&app), "set ");
    }

    #[test]
    fn typing_while_sailing_reaches_nothing() {
        let mut app = test_app();
        // No console up: the keys belong to the helm, and the reader must
        // still consume the stream so nothing is delivered late on opening.
        type_key(&mut app, KeyCode::KeyA, "a");
        type_key(&mut app, KeyCode::Enter, "\r");
        let console = app.world().resource::<Console>();
        assert!(console.input.is_empty());
        assert!(console.lines.is_empty());
    }

    #[test]
    fn the_scrollback_is_bounded() {
        let mut console = Console::default();
        for i in 0..(SCROLLBACK + 20) {
            console.say(&format!("line {i}"));
        }
        assert_eq!(console.lines.len(), SCROLLBACK);
        assert_eq!(console.lines.front().map(String::as_str), Some("line 20"));
    }
}
