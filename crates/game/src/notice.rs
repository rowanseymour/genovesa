//! A line of word from the world, shown briefly over the view.
//!
//! The wire mostly carries state, and state is drawn where it lives — the
//! ground as ground, a cairn as stones. This is for the one kind of answer
//! that is words: the claim key refused, whether by the world
//! ([`protocol::ToClient::Uncharted`]) or by the key itself before it asked —
//! see [`crate::player::claim_the_island`], the one other writer. The line
//! sits low over the view, holds long enough to be read, and fades — the
//! world said a thing, and the world is not a dialog to be dismissed.
//!
//! The words are kept here, together, because they are one voice: a refusal
//! judged this side and one judged by the world must read as the same
//! speaker, and the one the two ends both give — more coast to chart — must
//! be one string.

use bevy::prelude::*;
use bevy::text::{FontSize, FontSource, FontStyle};

use crate::AppState;

/// The claim key's refusals, in the world's words. Why each is given is the
/// key's business — see [`crate::player::claim_the_island`].
pub const AFLOAT: &str = "A cairn is raised on foot";
pub const A_SKERRY: &str = "No cairn will stand on a rock this small";
pub const UNCHARTED: &str = "There is more coast here than you have charted";

/// What the world just said. Inserting it is the whole of asking for it to be
/// shown; a new one takes the line over from whatever was fading there.
/// Showing, holding and fading are this module's business, and it removes the
/// resource when the line has gone.
#[derive(Resource)]
pub struct Notice {
    text: String,
    /// Seconds it has been up.
    shown: f32,
}

impl Notice {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            shown: 0.0,
        }
    }

    /// The line as it reads.
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// How long the line holds before it starts to fade, and how long the fade
/// takes: long enough to be read twice, short enough that the sky is not
/// wearing a caption.
const HOLD_SECONDS: f32 = 4.0;
const FADE_SECONDS: f32 = 1.5;

/// The hand the line is written in: the menu subtitle's — a serif italic from
/// the machine's own font database — because what it carries is the world
/// speaking, not the machine's readout.
const NOTICE_SIZE: f32 = 22.0;

/// How far up from the bottom of the window the line sits, clear of the
/// instruments in the corners.
const ABOVE_BOTTOM: f32 = 96.0;

/// Marks the one node a notice is shown in.
#[derive(Component)]
struct NoticeLine;

pub struct NoticePlugin;

impl Plugin for NoticePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<crate::net::Uncharted>()
            .add_systems(Update, hear.in_set(crate::net::Wire::Read))
            .add_systems(
                Update,
                // After both writers, so a word and a fading line meeting on
                // one frame resolve in that order: unordered, `speak`'s
                // teardown commands could apply after a writer's insert and
                // delete a notice that was never drawn — and ordered, a
                // fresh word shows the same frame it is heard.
                speak
                    .after(hear)
                    .after(crate::player::Afoot)
                    .run_if(in_state(AppState::InWorld).and_then(resource_exists::<Notice>)),
            )
            .add_systems(OnExit(AppState::InWorld), hush);
    }
}

/// Turns the world's word into the line it means. No queue — the newest word
/// takes the line over, as [`Notice`] says.
fn hear(mut commands: Commands, mut heard: MessageReader<crate::net::Uncharted>) {
    if heard.read().next().is_some() {
        commands.insert_resource(Notice::new(UNCHARTED));
    }
}

/// Keeps the line current: spawns it for a notice that has none, rewrites it
/// when a new notice takes over, fades it as its time runs, and takes both
/// the line and the resource down when the fade is done.
fn speak(
    mut commands: Commands,
    time: Res<Time>,
    mut notice: ResMut<Notice>,
    mut line: Query<(Entity, &mut Text, &mut TextColor), With<NoticeLine>>,
) {
    // A fresh notice reads as `shown` starting over, whether it arrived by
    // insert (replacing the resource resets the clock by construction) or is
    // simply this one's first frame.
    let faded = ((notice.shown - HOLD_SECONDS) / FADE_SECONDS).clamp(0.0, 1.0);
    let ink = Color::srgba(1.0, 1.0, 1.0, 0.92 * (1.0 - faded));
    notice.shown += time.delta_secs();

    match line.single_mut() {
        Ok((entity, mut text, mut color)) => {
            if faded >= 1.0 {
                commands.entity(entity).despawn();
                commands.remove_resource::<Notice>();
                return;
            }
            if text.0 != notice.text {
                text.0 = notice.text.clone();
            }
            color.0 = ink;
        }
        Err(_) => {
            commands.spawn((
                Name::new("Notice"),
                NoticeLine,
                Text::new(notice.text.clone()),
                TextFont {
                    font: FontSource::Serif,
                    font_size: FontSize::Px(NOTICE_SIZE),
                    style: FontStyle::Italic,
                    ..default()
                },
                TextColor(ink),
                Node {
                    position_type: PositionType::Absolute,
                    bottom: Val::Px(ABOVE_BOTTOM),
                    left: Val::Percent(0.0),
                    right: Val::Percent(0.0),
                    justify_content: JustifyContent::Center,
                    ..default()
                },
                TextLayout::justify(Justify::Center),
                DespawnOnExit(AppState::InWorld),
            ));
        }
    }
}

/// Leaves nothing behind on the way out of a world; the line itself despawns
/// with the state.
fn hush(mut commands: Commands) {
    commands.remove_resource::<Notice>();
}
