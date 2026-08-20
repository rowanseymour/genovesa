//! The water: the sea a world is heard as, and the hull moving through it.
//!
//! One recording does both, and the two are not the same sound so much as the
//! same sound at two heights. A boat lying to her anchor still has water
//! working at her side, and how much depends on how big a sea is running; get
//! under way and that rises into the wash at the bow. So the weather sets a
//! floor, speed climbs from it, and neither is a separate thing to be switched
//! on. The floor was missing to begin with and a boat brought up in a blow
//! went completely silent, which read as the sound breaking rather than as a
//! boat stopping.
//!
//! Nothing here is triggered. What is playing and how loudly is worked out
//! from the world afresh every frame — the sea's own liveliness, the boat's
//! speed, the camera's distance, the screen the app is on — so there is no
//! event to miss, nothing to keep in step with anything, and a state the game
//! arrives in by some route nobody thought of still sounds like what it is.
//!
//! Menus are silent, including the pause menu standing over a world that is
//! still sailing behind it. See [`heard`].

use bevy::audio::{AudioSinkPlayback, Volume};
use bevy::prelude::*;

use crate::boat::Boat;
use crate::camera::MapCamera;
use crate::sea::SeaConditions;
use crate::{AppState, Helm};

/// The water at a boat's bow, as a file. Twenty-three seconds of it, cut from
/// a longer recording at the point either end of the cut sounds alike and
/// crossfaded across the join — a loop the ear cannot find the start of. The
/// recording it came from runs for over a minute but quietens markedly towards
/// the end, so looping the whole of it would have been a hull that slows and
/// then abruptly picks up again.
///
/// It is a recording of exactly what it is played for: a boat under way, heard
/// at the bow. Nothing in it has to be made to sound like water. It is doing
/// double duty at the quiet end, where what is wanted is a hull lying still
/// with a sea working at her — the right answer there is a second recording of
/// exactly that, crossfaded against this one, and until there is one a very
/// quiet bow is a fair enough stand-in for a noisy anchorage.
const WASH: &str = "audio/bow-wash.ogg";

/// The speed at which the wash is at full cry, in metres a second.
///
/// A property of the recording rather than of any hull — it is how fast the
/// boat in it was going, near enough — which is what makes the boats sound
/// different sizes without either of them being told to. The ship at her best
/// makes about this and is heard at full; the rowboat pulled flat out makes
/// three metres a second and is heard at rather less than half. Normalising
/// each hull against its *own* top speed was tried first and is what made
/// them sound identical: a dinghy going as fast as a dinghy can is not a ship.
const FULL_SPEED: f32 = 10.0;

/// How far the camera may sit from the hull before the *bow wash* begins to
/// fall away, in metres. Near enough the distance the view opens at, so a
/// camera nobody has touched hears the whole of it and only pulling back costs
/// anything — and holding it flat inside this rather than letting the fall
/// continue means the closest the camera can be shoved is not also the loudest
/// a boat ever gets.
///
/// Only the wash. The sea itself is not somewhere the camera can be far from,
/// so nothing about the range touches the floor the weather sets: walk the
/// length of a beach away from a boat and the surf does not go with it.
const EARSHOT: f32 = 40.0;

/// How much of the wash a hull lying still is worth under the reference
/// breeze the sea's amplitudes are written for.
///
/// Scaled by how big a sea is actually running, so this is the middle of a
/// range rather than a level: a flat calm comes out near enough silent and a
/// blow at something over twice this. Loud enough to be company at anchor,
/// quiet enough that setting the sails is still an event.
const LYING: f32 = 0.16;

/// How fast the wash may change, as the seconds a move across the whole range
/// would take.
///
/// A limit rather than a fade, and it does two jobs because they are the same
/// job. Switching the sink on and off outright cut the recording in and out at
/// whatever amplitude it happened to be at — mid-swell as often as not, which
/// the ear hears as a click rather than as water — and it is also what a boat
/// stopped dead by the helm would do, [`Boat::way`] being snapped to exactly
/// zero at the tail of every glide. Slow enough to be a swell rather than a
/// step, quick enough that a pause is quiet by the time the menu is looked at.
const SLEW: f32 = 0.6;

/// The one entity playing [`WASH`], so that the wash can be found again once
/// what it is playing for has moved.
#[derive(Component)]
struct Wash;

pub struct AmbiencePlugin;

impl Plugin for AmbiencePlugin {
    fn build(&self, app: &mut App) {
        // What the weather is read off. Also initialised by the plugins that
        // draw and ride the sea; initialising a resource twice is free, and
        // each plugin's tests run it alone.
        app.init_resource::<SeaConditions>()
            .add_systems(Startup, hang_the_wash)
            .add_systems(Update, sound_the_wash);
    }
}

/// Whether a world is being heard at all on the screen the app is on.
///
/// A menu is silent, and so is the pause menu, though the hull behind it is
/// still carrying its way — nothing about the world stops for a pause, so this
/// is the only thing that makes one quiet. The chart and the console are the
/// opposite: a sheet or a line of typing held up in front of a world that is
/// still being sailed, and a boat does not stop moving water because its
/// skipper is reading.
fn heard(state: &AppState, helm: Option<&Helm>) -> bool {
    *state == AppState::InWorld && matches!(helm, Some(Helm::Sailing | Helm::Console | Helm::Chart))
}

/// How loud the water should be for a hull making `way` metres a second under
/// a sea of `liveliness`, seen from `range` metres away — see
/// [`SeaConditions::liveliness`] for what that number is.
///
/// A floor and a climb from it, rather than terms added up. The floor is the
/// weather's: what the sea alone is worth, and all a boat lying still is
/// heard as. The climb is the boat's, and it runs from wherever the floor
/// left off up to exactly 1 — so a hull driven hard is at full cry whatever
/// the weather, and a stiff breeze shows up as everything below that being
/// louder rather than as the top of the range moving. Added instead, a gale
/// would have pushed a boat under sail past the ceiling and flattened the
/// whole difference between six knots and ten.
///
/// Speed is taken as a square root so a boat just gathering way can already be
/// heard and the top of the range is where the change gets small — straight,
/// the first knot or two made no sound at all, which is the one part of
/// getting under way that ought to be audible. `way` is signed and the sign is
/// thrown away on purpose: a hull backing water is pushing the same water
/// about as one going ahead.
///
/// Distance is the boat's alone, and falls off as its inverse past
/// [`EARSHOT`], twice as far being half as loud. The sea's floor is left out
/// of it — see that constant.
fn loudness(way: f32, liveliness: f32, range: f32) -> f32 {
    let lying = (LYING * liveliness).min(1.0);
    let speed = (way.abs() / FULL_SPEED).min(1.0).sqrt();
    let near = EARSHOT / range.max(EARSHOT);
    lying + (1.0 - lying) * speed * near
}

/// Spawns the wash already paused and silent, and leaves the starting of it to
/// [`sound_the_wash`]. A run that opens straight into a world would otherwise
/// get the frame between the sink appearing and the first look at the boat,
/// which is a real if brief sound.
fn hang_the_wash(mut commands: Commands, assets: Res<AssetServer>) {
    commands.spawn((
        Wash,
        AudioPlayer::new(assets.load(WASH)),
        PlaybackSettings::LOOP
            .paused()
            .with_volume(Volume::Linear(0.0)),
    ));
}

/// Follows the boat with the wash, frame by frame.
///
/// Paused rather than despawned when it falls to nothing, because pausing
/// keeps the sink's place in the loop: a boat that stops and gets under way
/// again does not restart the recording at a point the ear recognises. But a
/// sink resumed at full volume lands in the middle of a wave, and *that* is
/// what is heard as the sea appearing from nowhere — so the pause only happens
/// once the volume has reached silence, and is undone before the climb off it
/// begins.
///
/// The ramp is linear rather than the exponential ease the rest of the game
/// moves by, because both ends of this one have to actually arrive: 0.0 to
/// know the sink can be stopped, and the wanted level so that a boat held at a
/// steady speed is not left a hair under its own sound forever.
fn sound_the_wash(
    state: Res<State<AppState>>,
    helm: Option<Res<State<Helm>>>,
    time: Res<Time>,
    sea: Res<SeaConditions>,
    boats: Query<(&Transform, &Boat)>,
    cameras: Query<&Transform, With<MapCamera>>,
    wash: Option<Single<&mut AudioSink, With<Wash>>>,
) {
    // Nothing to do until the file has loaded and been given a sink.
    let Some(mut wash) = wash else { return };

    // One boat, because one hull is this player's — the same reason the wake
    // keeps one track. Told hulls carry neither way nor canvas across the
    // wire, so a passing stranger is silent until that changes.
    //
    // And no boat at all is an ordinary answer rather than a missing one:
    // ashore the `Boat` comes off the hull with the crew, and somebody
    // walking a beach is still standing next to a sea. A hull nobody is
    // steering makes no way, which is all this needs of it.
    let making = boats
        .iter()
        .next()
        .zip(cameras.iter().next())
        .map_or((0.0, EARSHOT), |((hull, boat), eye)| {
            (boat.way(), hull.translation.distance(eye.translation))
        });
    let wanted = if heard(state.get(), helm.as_ref().map(|helm| helm.get())) {
        loudness(making.0, sea.liveliness(), making.1)
    } else {
        0.0
    };

    let now = wash.volume().to_linear();
    let step = time.delta_secs() / SLEW;
    let volume = if wanted > now {
        (now + step).min(wanted)
    } else {
        (now - step).max(wanted)
    };
    wash.set_volume(Volume::Linear(volume));

    // Started before it can be heard, stopped only once it cannot be.
    if volume > 0.0 && wash.is_paused() {
        wash.play();
    } else if volume == 0.0 && !wash.is_paused() {
        wash.pause();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The speed the rowboat pulls at, flat out — see `boat`'s `ROWBOAT`.
    const ROWED: f32 = 3.0;

    /// Seas to try things under, as [`SeaConditions::liveliness`] reports
    /// them: the ends of the range `sea`'s `amplitude_scale` can return, and
    /// the reference breeze it is written for in the middle.
    const CALM: f32 = 0.12;
    const BREEZE: f32 = 1.0;
    const BLOW: f32 = 2.4;

    #[test]
    fn a_hull_lying_still_is_heard_because_the_sea_is() {
        let anchored = loudness(0.0, BREEZE, EARSHOT);
        assert!(anchored > 0.0, "a boat at anchor in a breeze is silent");
        // Company, not an event: setting the sails has to be a change.
        assert!(anchored < 0.25, "{anchored} is too much for a boat at rest");
    }

    #[test]
    fn the_harder_it_blows_the_more_there_is_of_the_sea() {
        let calm = loudness(0.0, CALM, EARSHOT);
        let breeze = loudness(0.0, BREEZE, EARSHOT);
        let blow = loudness(0.0, BLOW, EARSHOT);
        assert!(calm < breeze && breeze < blow, "{calm} {breeze} {blow}");
        // A flat calm is all but silent, and deliberately not silent: the sea
        // is still there, and going to nothing reads as the sound failing.
        assert!(calm > 0.0 && calm < 0.05, "{calm} is not a flat calm");
    }

    #[test]
    fn backing_water_sounds_like_making_way() {
        assert_eq!(
            loudness(-4.0, BREEZE, EARSHOT),
            loudness(4.0, BREEZE, EARSHOT)
        );
    }

    #[test]
    fn driving_harder_is_louder_until_it_is_not() {
        let quiet = loudness(2.0, BREEZE, EARSHOT);
        let loud = loudness(FULL_SPEED, BREEZE, EARSHOT);
        assert!(quiet < loud, "{quiet} is not below {loud}");
        assert_eq!(loud, 1.0);
        // Whatever the helm does above it — a following gale, a hull put
        // somewhere by the console — the sea does not get louder than the sea.
        assert_eq!(loudness(FULL_SPEED * 10.0, BREEZE, EARSHOT), 1.0);
    }

    #[test]
    fn the_weather_lifts_the_floor_and_leaves_the_ceiling() {
        // The whole point of climbing from the floor rather than adding to
        // it: a blow is heard everywhere below full speed, and a boat driven
        // hard is at full cry on any day at all.
        assert!(loudness(4.0, BLOW, EARSHOT) > loudness(4.0, CALM, EARSHOT));
        assert_eq!(
            loudness(FULL_SPEED, BLOW, EARSHOT),
            loudness(FULL_SPEED, CALM, EARSHOT)
        );
    }

    #[test]
    fn a_boat_barely_moving_can_still_be_heard() {
        // A tenth of the way to full speed is well over a tenth of the sound:
        // this is the curve's whole reason for being a curve.
        let creeping = loudness(FULL_SPEED * 0.1, CALM, EARSHOT);
        assert!(creeping > 0.25, "{creeping} is too quiet to notice");
    }

    #[test]
    fn a_dinghy_flat_out_is_no_ship() {
        let dinghy = loudness(ROWED, CALM, EARSHOT);
        let ship = loudness(FULL_SPEED, CALM, EARSHOT);
        assert!(dinghy < ship * 0.6, "{dinghy} is too close to {ship}");
    }

    #[test]
    fn shoving_the_camera_closer_than_earshot_buys_nothing() {
        let near = loudness(FULL_SPEED, BREEZE, EARSHOT * 0.25);
        assert_eq!(near, loudness(FULL_SPEED, BREEZE, EARSHOT));
    }

    #[test]
    fn pulling_the_camera_back_takes_the_boat_and_leaves_the_sea() {
        let close = loudness(FULL_SPEED, BREEZE, EARSHOT);
        let far = loudness(FULL_SPEED, BREEZE, EARSHOT * 4.0);
        assert!(far < close, "{far} is not below {close}");
        // Only the boat's share of it went: what is left is the quarter of
        // the climb the distance allows, standing on the sea's own floor,
        // which no camera is ever far from.
        let floor = loudness(0.0, BREEZE, EARSHOT);
        assert!(
            (far - (floor + (close - floor) / 4.0)).abs() < 1e-6,
            "{far} is not a quarter of the way from {floor} to {close}"
        );
    }

    #[test]
    fn a_beach_a_long_way_from_a_boat_still_sounds_like_a_beach() {
        // Ashore the hull has no `Boat` on it and the camera has followed the
        // player off it — which is the case the system hands on as no way and
        // a distance that no longer means anything.
        let ashore = loudness(0.0, BREEZE, EARSHOT * 20.0);
        assert_eq!(ashore, loudness(0.0, BREEZE, EARSHOT));
    }

    #[test]
    fn every_menu_is_silent() {
        for state in [
            AppState::MainMenu,
            AppState::SetSail,
            AppState::NewWorld,
            AppState::JoinWorld,
            AppState::Options,
            AppState::Display,
            AppState::Controls,
        ] {
            assert!(!heard(&state, None), "{state:?} is not silent");
        }
    }

    #[test]
    fn the_pause_menu_is_silent_though_the_world_sails_on() {
        for helm in [Helm::Paused, Helm::Options, Helm::Display, Helm::Controls] {
            assert!(
                !heard(&AppState::InWorld, Some(&helm)),
                "{helm:?} is not silent"
            );
        }
    }

    #[test]
    fn a_world_being_looked_at_is_a_world_being_heard() {
        for helm in [Helm::Sailing, Helm::Console, Helm::Chart] {
            assert!(heard(&AppState::InWorld, Some(&helm)), "{helm:?} is silent");
        }
    }

    #[test]
    fn a_world_with_no_helm_yet_is_silent() {
        // The frame between entering the world and the sub-state existing.
        assert!(!heard(&AppState::InWorld, None));
    }
}
