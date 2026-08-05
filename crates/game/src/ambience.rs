//! The sea heard behind the menu.

use bevy::audio::AudioSinkPlayback;
use bevy::prelude::*;

use crate::AppState;

/// The sea, as a file. Twenty-three seconds of water at a boat's bow, cut from
/// a longer recording at the point either end of the cut sounds alike and
/// crossfaded across the join — a loop the ear cannot find the start of. The
/// recording it came from runs for over a minute but quietens markedly towards
/// the end, so looping the whole of it would have been a sea that calms and
/// then abruptly picks up again.
const SEA: &str = "menu.ogg";

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

/// Spawns the sea already paused, and leaves starting it to
/// [`follow_the_screen`]. A run that opens straight into a world would
/// otherwise get the frame between the sink appearing and the first look at the
/// state, which is a real if brief sound.
fn start_the_sea(mut commands: Commands, assets: Res<AssetServer>) {
    commands.spawn((
        Sea,
        AudioPlayer::new(assets.load(SEA)),
        PlaybackSettings::LOOP.paused(),
    ));
}

/// Plays the sea on every menu screen and holds it in the world.
///
/// Paused rather than despawned, and driven from the state each frame rather
/// than from `OnEnter`: pausing keeps the sea's place in the loop, so stepping
/// into a world and back out of it does not restart the recording at a point
/// the ear recognises. Reading the state instead of its transitions also means
/// a run that opens straight into a world — a joined one does — starts silent
/// without a separate case for it.
fn follow_the_screen(state: Res<State<AppState>>, sea: Option<Single<&AudioSink, With<Sea>>>) {
    // Nothing to do until the file has loaded and been given a sink.
    let Some(sea) = sea else { return };

    let wanted = *state.get() != AppState::InWorld;
    if wanted == sea.is_paused() {
        sea.toggle_playback();
    }
}
