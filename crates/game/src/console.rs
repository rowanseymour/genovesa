//! The debug console: the key left of 1, and the two languages typed into it.
//!
//! One rule decides where a line runs, and it is syntactic on purpose. A line
//! that starts `client` names a variable of *this machine* — what it draws
//! and the view it draws through, listed in [`Picture`] — and never leaves
//! it. Any other line is about the
//! *world*, and crosses the wire verbatim as
//! [`protocol::ToServer::Command`]: the vocabulary belongs to the server,
//! this module does not parse a word of it, and whatever text comes back as
//! [`protocol::ToClient::Reply`] is printed here. So `client haze off`
//! doctors one player's picture and admits it on the readout, while
//! `world time 18:00` moves the sun for everyone in the session.
//!
//! The word is `client` because the rule is about *which machine a line runs
//! on*, and that is the thing the word can say out loud. It used to be `set`,
//! which was as good a rule and taught nothing: a player had to be told that
//! the setting words were the local ones, and the server's own dials — which
//! set things too — had to stay bare verbs to keep out of the way. `client`
//! against the server's `world` puts the two halves of the split in the
//! grammar, and leaves the bare verbs to the lines that actually *act*.
//!
//! The console is part of every build, unlike the readout it switches on: it is
//! *how* debug states are reached, and a player who stumbles into it can type
//! `help` at a server that will answer. It opens only at the helm — the menus
//! have text fields of their own — and while it is up the keyboard is its
//! alone, [`Helm::Console`] being a state exactly so every system reading the
//! player's hands sits out.
//!
//! The key is hard-wired as [`KeyCode::Backquote`] — a *position*, the key
//! left of 1, whatever a layout prints on it — and reserved from rebinding,
//! for the reason the arrows are: it is the way in and out of a mode, and a
//! key that could be given away could strand whoever gave it.
//!
//! Tab completes, but only the words this client can *know*: the local
//! grammar, read off [`Picture`] rather than listed here, and the server's
//! phrases — which are not guessed at but taught, arriving on joining as
//! [`protocol::ToClient::Vocabulary`], so completion grows with the server
//! the way the vocabulary itself does. Both sides go a word further than
//! their verbs wherever the argument is a fixed few — `client stats on`,
//! `world weather gale` — and stop where it is the player's own, an hour or a
//! place being nothing to guess at.

use std::collections::VecDeque;

use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::text::FontSize;

use crate::camera::{View, MAX_DISTANCE, MIN_DISTANCE};
use crate::debug::{Kind, Look, Looking, Machine, Picture, Toggles, Value, BACKDROP, TEXT};
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
            // The view a `client` line can read and move. Initialised here as
            // well as by the camera's own plugin — either can be built first,
            // and a console without a camera still has to answer.
            .init_resource::<View>()
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
    /// The server's phrases, as taught on joining — what tab offers for
    /// every word of a line the server's grammar fixes, alongside the local
    /// `client`. Empty until the teaching arrives, when tab knows only the
    /// local grammar.
    phrases: Vec<String>,
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

    /// Takes the server's phrase list — see [`crate::net::receive`], which is
    /// where a [`protocol::ToClient::Vocabulary`] lands.
    pub fn teach(&mut self, phrases: Vec<String>) {
        self.phrases = phrases;
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
        let matches: Vec<String> = self
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
                let lead = shared_lead(words).to_string();
                if lead.len() == partial.len() {
                    // Nothing grows: the choices themselves are the answer,
                    // said where the replies land.
                    let choices = words.join("  ");
                    self.say(&choices);
                    return;
                }
                lead
            }
        };
        if keep + grown.len() <= MAX_LINE {
            self.input.truncate(keep);
            self.input.push_str(&grown);
            self.recall = None;
        }
    }

    /// The words that could stand after `before`: the next word of every
    /// phrase this console knows that opens with exactly those words, each
    /// offered once however many phrases carry it — `world` is one choice and
    /// not two, though several phrases begin with it.
    ///
    /// The two grammars are walked as one list. A player typing does not care
    /// which end of the wire will serve the line, and neither half is guessed
    /// at: the local phrases are [`local`], read off [`Picture`], and the
    /// server's are what it taught on joining. Where a line's next word is
    /// the player's own — a place, an hour — no phrase carries it and nothing
    /// is offered, which is the same answer as for a word nobody knows.
    fn completions(&self, before: &str) -> Vec<String> {
        let earlier: Vec<&str> = before.split_whitespace().collect();
        let mine = local();
        let mut offered: Vec<String> = Vec::new();
        for phrase in mine.iter().chain(&self.phrases) {
            let mut words = phrase.split_whitespace();
            if !earlier.iter().all(|word| words.next() == Some(*word)) {
                continue;
            }
            if let Some(next) = words.next() {
                if !offered.iter().any(|it| it == next) {
                    offered.push(next.to_string());
                }
            }
        }
        offered
    }

    /// Whether a line has been said, word for word — for the tests, which
    /// otherwise could only reach the scrollback through the drawn text.
    #[cfg(test)]
    pub(crate) fn said(&self, line: &str) -> bool {
        self.lines.iter().any(|said| said == line)
    }

    /// Whether a phrase has been taught — for the net tests, which otherwise
    /// could only see the vocabulary through tab.
    #[cfg(test)]
    pub(crate) fn knows(&self, phrase: &str) -> bool {
        self.phrases.iter().any(|known| known == phrase)
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

/// The word that keeps a line on this machine — the whole of the local
/// grammar's first half, and the one word [`dispatch`] looks at.
const LOCAL: &str = "client";

/// Where a line goes, decided by [`dispatch`]: answered here, or sent to the
/// server whose world it is about.
#[derive(Debug, PartialEq)]
pub(crate) enum Dispatch {
    /// A `client` line: it ran against [`Toggles`], and this is its answer —
    /// [`Err`] where the line was refused. Both are words the player reads,
    /// and whoever asked flattens them; the two are told apart this far for
    /// the reason `server::console` says at its own [`Result`].
    Local(Result<String, String>),
    /// Anything else: the server's to interpret, verbatim.
    Remote,
}

/// Runs a line's local half, or says it is not local at all. The one place
/// the grammar's rule lives: `client` never leaves the machine, nothing else
/// ever stays on it.
///
/// Reached by the keyboard through [`submit`] and by the control socket
/// through [`crate::control`], which is the point of it being a function of a
/// line rather than of what is on screen: one grammar, whichever mouth speaks
/// it, so a `client` variable is not something the socket has to be taught
/// separately.
pub(crate) fn dispatch(line: &str, picture: &mut Picture) -> Dispatch {
    let words: Vec<&str> = line.split_whitespace().collect();
    match words.split_first() {
        Some((&LOCAL, rest)) => Dispatch::Local(client(rest, picture)),
        _ => Dispatch::Remote,
    }
}

/// The `client` variables, in the order a bare `client` lists them — what tab
/// completes after `client`, and what a miss is told to try instead, this
/// being the half of the grammar that lives on this machine.
///
/// Read off [`Picture::names`] rather than listed again here, so the words
/// this offers are the words [`client`] serves, always.
fn variables() -> Vec<&'static str> {
    Picture::names().collect()
}

/// The longest lead every word here shares — at least what was typed, each
/// already starting with that.
fn shared_lead(words: &[String]) -> &str {
    let mut lead = words[0].as_str();
    for word in &words[1..] {
        while !word.starts_with(lead) {
            let mut shorter = lead.chars();
            shorter.next_back();
            lead = shorter.as_str();
        }
    }
    lead
}

/// The `client` grammar: `client` lists every variable, `client <var>` reads
/// one, `client <var> <value>` writes one — the three forms the server's
/// `world` shelf has, for the reason its own module gives. Always answered:
/// a console that says nothing back reads as a console that heard nothing.
///
/// Not every variable takes all three. `position` reads and does not turn,
/// which is what a reading is: where somebody is is not a dial, and the ways
/// to change it are sailing, walking, and being taken.
fn client(args: &[&str], picture: &mut Picture) -> Result<String, String> {
    match args {
        // Whatever can be read from where this line was typed. A screen with
        // no world under it has no view, so the view's three say nothing
        // rather than saying three times that there is no view — which is
        // what somebody who asked for one of them by name is told.
        [] => Ok(variables()
            .into_iter()
            .filter_map(|var| read(var, picture).ok())
            .collect::<Vec<_>>()
            .join(" / ")),
        [var] => read(var, picture),
        [var, value] => match picture.variable(var) {
            Some(Value::Switch(state)) => switch(var, value, state),
            Some(Value::Rows(state)) => resolution(var, value, state),
            Some(Value::Looking(_, None)) => Err(NO_VIEW.to_string()),
            Some(Value::Looking(Look::Position, _)) => {
                Err(format!("`{var}` reads and does not turn"))
            }
            Some(Value::Looking(Look::Zoom, Some(looking))) => distance(var, value, looking),
            Some(Value::Looking(Look::Yaw, Some(looking))) => bearing(var, value, looking),
            None => Err(no_such(var)),
        },
        _ => Err("one variable, one value — `client resolution 720`".to_string()),
    }
}

/// What a line about the view is told where there is no view: the menus,
/// which have a console reachable through the socket and no world behind it
/// — the camera outlives every world, so what is missing there is something
/// to point it at rather than the camera. See `Machine::afloat`.
const NO_VIEW: &str = "there is no view on this screen to speak of";

/// The words that can stand after a variable, where they are a fixed few.
/// What the kinds are called is this module's business, [`Picture`] only
/// saying which kind a name is; see [`Kind`].
fn values(var: &str) -> Vec<String> {
    match Picture::kind(var) {
        Some(Kind::Switch) => vec![ON.to_string(), OFF.to_string()],
        // Nothing, though the rungs are a fixed few and `resolution` takes
        // one of them. The only mouth that completes is the keyboard — the
        // socket has no tab — and a run with a keyboard is a run with a
        // window, where [`resolution`] refuses every rung there is. Offering
        // them would complete a player into the one line this grammar can
        // never serve where it was typed.
        Some(Kind::Rows) => Vec::new(),
        // Metres, a bearing, or nothing at all — none of them a word to
        // offer, which is not the same as a word nobody has listed.
        Some(Kind::Looking) | None => Vec::new(),
    }
}

/// Every phrase this machine's own grammar serves, as far as its words are
/// fixed: `client`, its variables, and the values they take. The mirror of
/// the server's `phrases`, built the same way and for the same reason — tab
/// offers what the grammar serves because it is reading the grammar, not a
/// list somebody kept beside it.
fn local() -> Vec<String> {
    std::iter::once(LOCAL.to_string())
        .chain(Picture::names().flat_map(|var| {
            std::iter::once(format!("{LOCAL} {var}")).chain(
                values(var)
                    .into_iter()
                    .map(move |value| format!("{LOCAL} {var} {value}")),
            )
        }))
        .collect()
}

/// What one variable reads as — the same words a write answers with, so the
/// answer to setting is the proof it took.
///
/// The view's readings come off the camera rather than off what was last
/// asked for: what a driver wants to know is where the picture *is*, which
/// after a put down or a wheel of the mouse is not what anybody typed. See
/// [`Looking`], which is where the two are told apart.
fn read(var: &str, picture: &mut Picture) -> Result<String, String> {
    let Some(value) = picture.variable(var) else {
        return Err(no_such(var));
    };
    Ok(match value {
        Value::Switch(on) => onoff(var, *on),
        Value::Rows(Some(rows)) => format!("{var} {rows}p"),
        Value::Rows(None) => format!("{var} the window's own"),
        Value::Looking(_, None) => return Err(NO_VIEW.to_string()),
        Value::Looking(look, Some(looking)) => {
            let now = looking.now();
            match look {
                Look::Zoom => format!("{var} {}", now.distance.round()),
                Look::Yaw => format!("{var} {}", degrees(now.yaw)),
                // The place, said as the ground is: two numbers, the height
                // being the camera's business and not a place anybody names.
                Look::Position => format!("{var} {} {}", now.focus.x.round(), now.focus.z.round()),
            }
        }
    })
}

/// A bearing as the console says it: whole degrees of the compass. The
/// camera's own yaw runs unbounded — easing never wants to wrap — so what it
/// holds after a few turns is not a number anybody would type back.
///
/// Rounded before it is folded, not after: 359.7° rounds to 360, and a
/// compass has no such bearing. Folding last sends it to the 0 the write of
/// the same heading answers with.
fn degrees(yaw: f32) -> f32 {
    yaw.to_degrees().round().rem_euclid(360.0)
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
fn switch(var: &str, value: &str, state: &mut bool) -> Result<String, String> {
    match value {
        ON => *state = true,
        OFF => *state = false,
        _ => return Err(format!("`{var}` is {ON} or {OFF}")),
    }
    Ok(onoff(var, *state))
}

/// The two words a switch takes, named because the writer, the reading and
/// the completion all say them.
const ON: &str = "on";
const OFF: &str = "off";

/// `client zoom <m>`: how far off the camera stands. The bounds are the
/// camera's own, so a line cannot ask for a view the wheel could not reach.
fn distance(var: &str, value: &str, mut looking: Looking) -> Result<String, String> {
    let metres: f32 = value
        .parse()
        .map_err(|_| format!("`{value}` is not a distance in metres"))?;
    if !(MIN_DISTANCE..=MAX_DISTANCE).contains(&metres) {
        return Err(format!(
            "`{var}` is between {MIN_DISTANCE} and {MAX_DISTANCE} metres, not {metres}"
        ));
    }
    let wanted = View {
        distance: metres,
        ..looking.now()
    };
    looking.look(wanted);
    Ok(format!("{var} {}", metres.round()))
}

/// `client yaw <deg>`: which way the camera looks from, in degrees of the
/// compass, kept as the radians the view holds.
fn bearing(var: &str, value: &str, mut looking: Looking) -> Result<String, String> {
    let given: f32 = value
        .parse()
        .ok()
        .filter(|degrees: &f32| degrees.is_finite())
        .ok_or_else(|| format!("`{value}` is not a bearing in degrees"))?;
    let wanted = View {
        yaw: given.to_radians(),
        ..looking.now()
    };
    looking.look(wanted);
    Ok(format!("{var} {}", degrees(wanted.yaw)))
}

/// Sets how big a picture `shot` writes, by the rungs the display screen
/// offers — `1440` and not `2560x1440`, the width following from the shape a
/// screen would have had. There is nothing to set in a run with a window: a
/// picture of a window is the window, whatever is asked for here, and a
/// number that quietly did nothing would be worse than a refusal.
fn resolution(var: &str, value: &str, state: &mut Option<u32>) -> Result<String, String> {
    let rungs = crate::settings::rungs();
    let Some(rows) = state.as_mut() else {
        return Err(
            "a picture is the size of the window in a run that has one — \
                    `resolution` is for a run without"
                .to_string(),
        );
    };
    match value.parse::<u32>().ok().filter(|it| rungs.contains(it)) {
        Some(chosen) => {
            *rows = chosen;
            Ok(format!("{var} {chosen}p"))
        }
        None => Err(format!(
            "`{var}` is one of {} — `client {var} 1440`",
            values(var).join(", ")
        )),
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
    mut machine: Machine,
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
                // Taken afresh for the line rather than held across the loop:
                // a `client zoom` moves the camera, and the next line typed
                // should read what the last one did.
                submit(&mut console, &mut machine.picture(), online.as_deref());
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
fn submit(console: &mut Console, picture: &mut Picture, online: Option<&Online>) {
    let line = console.input.trim().to_string();
    console.input.clear();
    console.recall = None;
    if line.is_empty() {
        return;
    }

    console.say(&format!("> {line}"));
    console.history.push(line.clone());

    match dispatch(&line, picture) {
        Dispatch::Local(Ok(reply) | Err(reply)) => console.say(&reply),
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

    /// What a `client` line answered with, for the tests that care about the
    /// words and not which of the two kinds of answer it was.
    fn said(args: &[&str], toggles: &mut Toggles) -> String {
        let (Ok(reply) | Err(reply)) = client(args, &mut Picture::of(toggles));
        reply
    }

    /// What a `client` line refused with — a test that meant to be refused
    /// and was answered instead fails here rather than on the prose.
    fn refused(args: &[&str], toggles: &mut Toggles) -> String {
        client(args, &mut Picture::of(toggles)).expect_err("the line should have been refused")
    }

    /// A picture with a camera in it, which is what the helm has and no menu
    /// screen does — the view's variables have nothing to say without one.
    fn looking_at(view: View) -> (Toggles, View, crate::camera::MapCamera) {
        (
            Toggles::default(),
            view,
            crate::camera::MapCamera::looking(view),
        )
    }

    #[test]
    fn a_client_line_never_leaves_the_machine_and_nothing_else_stays() {
        let mut toggles = Toggles::default();
        assert_eq!(
            dispatch("client stats on", &mut Picture::of(&mut toggles)),
            Dispatch::Local(Ok("stats on".to_string()))
        );
        assert!(toggles.stats);

        // The rule is the first word and nothing else: these are the
        // server's, however local they might sound.
        let mut picture = Picture::of(&mut toggles);
        assert_eq!(dispatch("spawn shark", &mut picture), Dispatch::Remote);
        assert_eq!(dispatch("help", &mut picture), Dispatch::Remote);
        assert_eq!(dispatch("world time 18:00", &mut picture), Dispatch::Remote);
    }

    #[test]
    fn every_switch_answers_and_takes() {
        let mut toggles = Toggles::default();
        assert_eq!(said(&["haze", "off"], &mut toggles), "haze off");
        assert!(!toggles.haze);
        assert_eq!(said(&["wireframe", "on"], &mut toggles), "wireframe on");
        assert!(toggles.wireframe);
        assert_eq!(said(&["haze", "on"], &mut toggles), "haze on");
        assert!(toggles.haze);
    }

    #[test]
    fn a_bare_client_reads_and_a_named_one_reads_one() {
        let mut toggles = Toggles {
            wireframe: true,
            ..Default::default()
        };
        assert_eq!(
            said(&[], &mut toggles),
            "stats off / shadows on / haze on / wireframe on / \
             resolution the window's own"
        );
        assert_eq!(said(&["haze"], &mut toggles), "haze on");
    }

    /// The view's three: read off the camera, which is what is actually
    /// drawn, and written through it. `position` reads and does not turn —
    /// where somebody is is not a dial, and the ways to change it are sailing,
    /// walking, and being taken.
    #[test]
    fn the_view_reads_off_the_camera_and_the_place_only_reads() {
        let (mut toggles, mut view, mut camera) = looking_at(View {
            focus: Vec3::new(98.0, 12.0, -317.0),
            distance: 240.0,
            yaw: -std::f32::consts::PI,
        });
        {
            let mut picture = Picture {
                toggles: &mut toggles,
                looking: Some(Looking {
                    view: &mut view,
                    camera: &mut camera,
                }),
            };
            let mut ask = |args: &[&str]| client(args, &mut picture);

            assert_eq!(ask(&["zoom"]), Ok("zoom 240".to_string()));
            // Folded to a bearing of the compass, the camera's own running
            // unbounded.
            assert_eq!(ask(&["yaw"]), Ok("yaw 180".to_string()));
            // Two numbers: the height under the camera is nowhere anybody
            // names.
            assert_eq!(ask(&["position"]), Ok("position 98 -317".to_string()));

            // A write takes, and the reading follows the camera rather than
            // repeating back what was asked for.
            assert_eq!(ask(&["zoom", "120"]), Ok("zoom 120".to_string()));
            assert_eq!(ask(&["zoom"]), Ok("zoom 120".to_string()));

            // What the camera would refuse, the line refuses.
            for asked in [["zoom", "5"], ["zoom", "5000"], ["yaw", "sideways"]] {
                assert!(ask(&asked).is_err(), "`{}` was allowed", asked.join(" "));
            }

            let refused = ask(&["position", "0"]).expect_err("a place is not settable");
            assert!(refused.contains("does not turn"), "unhelpful: {refused}");
        }

        // And it moved the camera itself, not merely the `View` a camera
        // would be put back to — though that too, or entering a world again
        // would undo it.
        assert_eq!(camera.distance, 120.0, "the camera did not move");
        assert_eq!(view.distance, 120.0, "a camera put back would undo it");
    }

    /// Every screen but the helm has no world to look at, and says so — a
    /// variable that exists and has nothing to say is not a variable nobody
    /// has.
    #[test]
    fn a_screen_with_no_view_says_so_rather_than_denying_the_variable() {
        let mut toggles = Toggles::default();
        for asked in [vec!["zoom"], vec!["yaw", "90"], vec!["position"]] {
            let refused = client(&asked, &mut Picture::of(&mut toggles))
                .expect_err("there is no camera here");
            assert!(refused.contains("no view"), "unhelpful: {refused}");
        }

        // And a bare `client` simply leaves them out rather than saying it
        // three times.
        let listed = said(&[], &mut toggles);
        assert!(
            !listed.contains("zoom") && listed.contains("haze"),
            "a screen with no view listed one anyway: {listed}"
        );
    }

    /// A picture is the window in a run that has one, so the variable reads as
    /// that rather than as a number nothing would draw at.
    #[test]
    fn the_picture_size_is_the_windows_until_there_is_no_window() {
        let mut windowed = Toggles::default();
        assert_eq!(
            said(&["resolution"], &mut windowed),
            "resolution the window's own"
        );
        let no_choosing = refused(&["resolution", "1080"], &mut windowed);
        assert!(no_choosing.contains("window"), "unhelpful: {no_choosing}");
        assert_eq!(windowed.resolution, None, "a refused size took anyway");

        let mut windowless = Toggles {
            resolution: Some(1440),
            ..Default::default()
        };
        assert_eq!(said(&["resolution"], &mut windowless), "resolution 1440p");
        assert_eq!(
            said(&["resolution", "720"], &mut windowless),
            "resolution 720p"
        );
        assert_eq!(windowless.resolution, Some(720));

        // The rungs the display screen offers, and only those: a size off the
        // ladder has no width to follow from a shape nobody can ask a
        // windowless run for.
        for rung in crate::settings::rungs() {
            assert_eq!(
                said(&["resolution", &rung.to_string()], &mut windowless),
                format!("resolution {rung}p")
            );
        }
        let last = crate::settings::rungs().last().copied();
        let not_a_rung = refused(&["resolution", "1234"], &mut windowless);
        assert!(not_a_rung.contains("1440"), "unhelpful: {not_a_rung}");
        assert_eq!(windowless.resolution, last, "a refused size took anyway");
    }

    #[test]
    fn a_wrong_client_line_is_answered_not_swallowed() {
        let mut toggles = Toggles::default();
        let unknown = refused(&["fog", "off"], &mut toggles);
        assert!(unknown.contains("`fog`"), "unhelpful: {unknown}");

        let not_a_switch = refused(&["stats", "maybe"], &mut toggles);
        assert!(
            not_a_switch.contains("on or off"),
            "unhelpful: {not_a_switch}"
        );
        assert!(!toggles.stats, "a refused value took anyway");
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
    fn a_client_line_typed_at_the_console_lands_in_the_toggles() {
        let mut app = test_app();
        press_backquote(&mut app);

        type_line(&mut app, "client wireframe on");

        assert!(app.world().resource::<Toggles>().wireframe);
        let console = app.world().resource::<Console>();
        assert_eq!(
            console.lines.iter().cloned().collect::<Vec<_>>(),
            ["> client wireframe on", "wireframe on"]
        );
        assert_eq!(console.history, ["client wireframe on"]);
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

        type_line(&mut app, "client haze off");
        type_line(&mut app, "client haze on");

        type_key(&mut app, KeyCode::ArrowUp, "");
        assert_eq!(app.world().resource::<Console>().input, "client haze on");
        type_key(&mut app, KeyCode::ArrowUp, "");
        assert_eq!(app.world().resource::<Console>().input, "client haze off");
        // The top of the history holds rather than wrapping.
        type_key(&mut app, KeyCode::ArrowUp, "");
        assert_eq!(app.world().resource::<Console>().input, "client haze off");

        type_key(&mut app, KeyCode::ArrowDown, "");
        assert_eq!(app.world().resource::<Console>().input, "client haze on");
        // And past the newest is the fresh prompt again.
        type_key(&mut app, KeyCode::ArrowDown, "");
        assert_eq!(app.world().resource::<Console>().input, "");
    }

    /// What the real server teaches, as the completion tests' vocabulary —
    /// the teaching itself is the net module's to test.
    fn taught(app: &mut App) {
        app.world_mut().resource_mut::<Console>().teach(
            ["help", "spawn", "world time", "world weather"]
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
        type_word(&mut app, "client w");
        type_key(&mut app, KeyCode::Tab, "");
        assert_eq!(input(&app), "client wireframe ");

        // And past a shelf, which is what the phrases are taught for: the
        // dial behind `world` is as completable as `world` itself.
        type_key(&mut app, KeyCode::Enter, "\r");
        type_word(&mut app, "world t");
        type_key(&mut app, KeyCode::Tab, "");
        assert_eq!(input(&app), "world time ");
    }

    /// The values a variable takes complete too, both sides of the wire: the
    /// local ones off [`Toggles`], the server's off what it taught. This is
    /// the word past the verb, and it is where a player spends most of their
    /// typing — `client stats on` is three words of which two are fixed.
    #[test]
    fn tab_completes_the_values_as_well_as_the_words() {
        let mut app = test_app();
        app.world_mut().resource_mut::<Console>().teach(
            ["world weather calm", "world weather gale"]
                .map(String::from)
                .to_vec(),
        );
        press_backquote(&mut app);

        // A switch takes two words, and two letters say which — `o` is the
        // lead they share, and tab grows a word rather than picking one.
        type_word(&mut app, "client stats of");
        type_key(&mut app, KeyCode::Tab, "");
        assert_eq!(input(&app), "client stats off ");

        // With nothing typed, tab grows the lead the two share and then
        // offers them — the same two presses any pair of choices takes.
        type_key(&mut app, KeyCode::Enter, "\r");
        type_word(&mut app, "client shadows ");
        type_key(&mut app, KeyCode::Tab, "");
        assert_eq!(input(&app), "client shadows o");
        type_key(&mut app, KeyCode::Tab, "");
        assert!(app.world().resource::<Console>().said("on  off"));

        // And the server's values, which are taught in the same phrases its
        // dials are.
        type_key(&mut app, KeyCode::Enter, "\r");
        type_word(&mut app, "world weather g");
        type_key(&mut app, KeyCode::Tab, "");
        assert_eq!(input(&app), "world weather gale ");
    }

    /// What tab offers after a variable is what the grammar would serve
    /// where tab can be pressed — which is a keyboard, and so a window.
    ///
    /// That is why the rungs are not offered though they are a fixed few: a
    /// run with a window is a run where every one of them is refused, and a
    /// run without a window has nobody to press tab. A value offered where
    /// it cannot be taken is worse than no offer at all.
    #[test]
    fn the_values_offered_are_the_ones_that_could_be_taken() {
        assert!(values("stats").iter().any(|it| it == "off"));
        assert!(
            values("resolution").is_empty(),
            "the rungs are offered where every one of them is refused"
        );
        assert!(
            values("zoom").is_empty(),
            "metres are the player's own, not a word to offer"
        );
        assert!(
            values("fog").is_empty(),
            "a variable nobody has offers values"
        );
    }

    #[test]
    fn tab_grows_what_it_can_and_offers_what_it_cannot() {
        let mut app = test_app();
        press_backquote(&mut app);

        // `t` grows to the `ti` that `time` and `tide` share, and no further.
        app.world_mut()
            .resource_mut::<Console>()
            .teach(["time", "tide"].map(String::from).to_vec());
        type_key(&mut app, KeyCode::KeyA, "t");
        type_key(&mut app, KeyCode::Tab, "");
        assert_eq!(input(&app), "ti");

        // And with nothing left to grow, the choices themselves are the
        // answer and the line stands.
        type_key(&mut app, KeyCode::Tab, "");
        assert_eq!(input(&app), "ti");
        assert!(app.world().resource::<Console>().said("time  tide"));
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
            .said("client  help  spawn  world"));

        // After `client `: every variable. A shelf offered once, though two
        // phrases stand behind it.
        type_word(&mut app, "client ");
        type_key(&mut app, KeyCode::Tab, "");
        assert_eq!(input(&app), "client ");
        assert!(app
            .world()
            .resource::<Console>()
            .said("stats  shadows  haze  wireframe  resolution  zoom  yaw  position"));

        // And after `world `: every dial behind it.
        type_key(&mut app, KeyCode::Enter, "\r");
        type_word(&mut app, "world ");
        type_key(&mut app, KeyCode::Tab, "");
        assert_eq!(input(&app), "world ");
        assert!(app.world().resource::<Console>().said("time  weather"));
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

        type_key(&mut app, KeyCode::KeyA, "c");
        type_key(&mut app, KeyCode::Tab, "");
        assert_eq!(input(&app), "client ");
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
