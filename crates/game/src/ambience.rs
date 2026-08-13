//! The sea heard behind the menu.

use bevy::audio::{AudioSinkPlayback, Volume};
use bevy::prelude::*;

use crate::{AppState, Helm};

/// The sea, as a file. Twenty-three seconds of water at a boat's bow, cut from
/// a longer recording at the point either end of the cut sounds alike and
/// crossfaded across the join — a loop the ear cannot find the start of. The
/// recording it came from runs for over a minute but quietens markedly towards
/// the end, so looping the whole of it would have been a sea that calms and
/// then abruptly picks up again.
const SEA: &str = "audio/menu-loop.ogg";

/// How long the sea takes to arrive or to leave, in seconds.
///
/// Switching the sink on and off outright cut the recording in and out at
/// whatever amplitude it happened to be at — mid-swell as often as not, which
/// the ear hears as a click rather than as water. Long enough to be a swell
/// rather than a step, short enough that the menu is not waiting on it.
const FADE: f32 = 0.6;

/// The one entity playing [`SEA`], so that the sea can be found again once the
/// screen behind it changes.
#[derive(Component)]
struct Sea;

pub struct AmbiencePlugin;

impl Plugin for AmbiencePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, start_the_sea)
            .add_systems(Update, follow_the_screen);
    }
}

/// Spawns the sea already paused and silent, and leaves starting it to
/// [`follow_the_screen`]. A run that opens straight into a world would
/// otherwise get the frame between the sink appearing and the first look at the
/// state, which is a real if brief sound. Silent as well as paused so that the
/// menu the app opens on is faded up to like any other, rather than being the
/// one screen the sea is already at full height for.
fn start_the_sea(mut commands: Commands, assets: Res<AssetServer>) {
    commands.spawn((
        Sea,
        AudioPlayer::new(assets.load(SEA)),
        PlaybackSettings::LOOP
            .paused()
            .with_volume(Volume::Linear(0.0)),
    ));
}

/// Fades the sea up on every menu screen and back down at the helm.
///
/// Faded rather than switched, and paused rather than despawned, and driven
/// from the state each frame rather than from `OnEnter`. Pausing keeps the
/// sea's place in the loop, so stepping into a world and back out of it does
/// not restart the recording at a point the ear recognises — but a sink resumed
/// at full volume lands in the middle of a wave, and *that* is what is heard as
/// the sea appearing from nowhere. So the pause only happens once the fade has
/// reached silence, and is undone before the fade off it begins. Reading the
/// state instead of its transitions also means a run that opens straight into a
/// world — a joined one does — starts silent without a separate case for it,
/// and that the pause menu is a menu like any other as far as the sea is
/// concerned.
///
/// The ramp is linear rather than the exponential ease the rest of the game
/// moves by, because both ends of this one have to actually arrive: 0.0 to know
/// the sink can be stopped, 1.0 so the menu is not left a hair under the sea's
/// full height forever.
fn follow_the_screen(
    state: Res<State<AppState>>,
    helm: Option<Res<State<Helm>>>,
    time: Res<Time>,
    sea: Option<Single<&mut AudioSink, With<Sea>>>,
) {
    // Nothing to do until the file has loaded and been given a sink.
    let Some(mut sea) = sea else { return };

    // Only the helm itself silences it: a world with the pause menu over it is
    // a menu, and gets the sea back for as long as the player is in it. The
    // console is the opposite — a line of typing over a world still being
    // looked at — so it stays as quiet as the helm it opened over.
    let sailing = *state.get() == AppState::InWorld
        && helm.is_none_or(|helm| matches!(*helm.get(), Helm::Sailing | Helm::Console));
    let wanted = if sailing { 0.0 } else { 1.0 };
    let now = sea.volume().to_linear();
    let step = time.delta_secs() / FADE;
    let volume = if wanted > now {
        (now + step).min(wanted)
    } else {
        (now - step).max(wanted)
    };
    sea.set_volume(Volume::Linear(volume));

    // Started before it can be heard, stopped only once it cannot be.
    if volume > 0.0 && sea.is_paused() {
        sea.play();
    } else if volume == 0.0 && !sea.is_paused() {
        sea.pause();
    }
}
