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
use crate::camera::{MapCamera, MAX_DISTANCE};
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
/// three metres a second and is heard at a little over half of that in a flat
/// calm, nearer three-quarters in a blow, the weather lifting everything
/// under full cry while the ship is at the ceiling whatever the day.
///
/// So the gap between the two boats is a modest one, and narrower the harder
/// it blows. That is the square root's doing, and it is the price of the
/// property that matters more: a boat barely gathering way has to be audible,
/// and a curve steep enough at the bottom for that cannot also be steep at
/// three metres a second. A modest gap is still a gap, though, and it is the
/// whole of what says which boat this is — normalising each hull against its
/// *own* top speed was tried first and is what made them sound identical: a
/// dinghy going as fast as a dinghy can is not a ship.
const FULL_SPEED: f32 = 10.0;

/// How far the camera may sit from the hull before the *bow wash* begins to
/// fall away, in metres. Near enough the distance the view opens at — a couple
/// of metres inside it, so a camera nobody has touched hears all but a breath
/// of the wash and only pulling back deliberately costs anything — and holding
/// it flat inside this rather than letting the fall continue means the closest
/// the camera can be shoved is not also the loudest a boat ever gets.
///
/// Only the wash. Standing off from a boat is not standing off from the sea:
/// walk the length of a beach away from a hull and the surf does not go with
/// it. What the camera *climbing* does to the sea is [`ALOFT`].
const EARSHOT: f32 = 40.0;

/// How much of the water is left at the top of the zoom, where the camera is
/// as far off it as it can get.
///
/// [`EARSHOT`] is about standing off from a boat, and takes only the boat's
/// share. This is about leaving the water altogether: at the far end of the
/// zoom the camera is most of four hundred metres up, looking down at a sea
/// it is no longer on, and everything about the water — the bow wash and the
/// weather both — ought to be that far off. Held apart from the wash's own
/// falloff rather than folded into it because they are different facts, and
/// the two compound exactly as they should: a boat pulled right back to a
/// mountain-sized view is a small white mark on a quiet sea, not a hull
/// heard from her own deck.
///
/// Deliberately not silence. A sea that goes to nothing at the end of the
/// zoom reads as the sound breaking rather than as a view opening out.
const ALOFT: f32 = 0.3;

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
    if *state != AppState::InWorld {
        return false;
    }
    // Every helm named, and no arm swallowing the rest: this whole module
    // works by reading the world afresh so that nothing can be missed, and a
    // default arm is exactly how the next state added would arrive silent
    // without anybody deciding that it should. Made a compile error instead.
    match helm {
        Some(Helm::Sailing | Helm::Console | Helm::Chart) => true,
        Some(Helm::Paused | Helm::Options | Helm::Display | Helm::Controls) => false,
        None => false,
    }
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
/// Distance does two separate things, and the range is read twice for them.
/// The wash alone falls off as the inverse of it past [`EARSHOT`], twice as
/// far leaving half the boat's share — the sea's floor is no part of that,
/// see that constant. Then the whole of it, floor included, is taken down
/// towards [`ALOFT`] as the camera climbs away from the water.
///
/// That second fall is spread evenly across the zoom's *notches* rather than
/// across its metres, which is what the logarithm is for: the zoom is
/// geometric, so linear in metres was flat for most of the scroll and then
/// dropped away over the last few clicks, and a sea that quietens all at once
/// sounds like a fault rather than like pulling back.
fn loudness(way: f32, liveliness: f32, range: f32) -> f32 {
    let lying = (LYING * liveliness).min(1.0);
    let speed = (way.abs() / FULL_SPEED).min(1.0).sqrt();
    let near = EARSHOT / range.max(EARSHOT);
    let climbed = ((range / EARSHOT).max(1.0).ln() / (MAX_DISTANCE / EARSHOT).ln()).min(1.0);
    let aloft = 1.0 - (1.0 - ALOFT) * climbed;
    (lying + (1.0 - lying) * speed * near) * aloft
}

/// The two numbers [`loudness`] wants off the frame: the way this player's
/// hull is making, and how far the camera is sitting from it.
///
/// Split out of [`sound_the_wash`] because it is the only part of that system
/// with a judgement in it, and the only part reachable from a test — getting
/// at the system itself means an app with an audio plugin in it, which means
/// a real device opened in a test process for the sake of a fallback.
///
/// The two absences are read apart rather than together, which is not
/// fussiness: they mean different things. No hull is the ordinary answer
/// ashore — the `Boat` comes off with the crew, and somebody walking a beach
/// is still standing next to a sea — and it settles the way at nothing. No
/// camera is not an answer about the boat at all, only a frame in which the
/// range is unknown, and a hull under way through one should not be struck
/// dumb for it. Folded into one fallback they were, and it did exactly that.
fn way_and_range(hull: Option<(f32, Vec3)>, eye: Option<Vec3>) -> (f32, f32) {
    let way = hull.map_or(0.0, |(way, _)| way);
    let range = hull
        .zip(eye)
        .map_or(EARSHOT, |((_, at), eye)| at.distance(eye));
    (way, range)
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
    let hull = boats
        .iter()
        .next()
        .map(|(at, boat)| (boat.way(), at.translation));
    let eye = cameras.iter().next().map(|eye| eye.translation);
    let (way, range) = way_and_range(hull, eye);
    let wanted = if heard(state.get(), helm.as_ref().map(|helm| helm.get())) {
        loudness(way, sea.liveliness(), range)
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
        // Under every sea, not the one flattering sea: the gap is at its
        // narrowest in a blow, the weather lifting the dinghy while the ship
        // is already at the ceiling, and it is the narrowest case that has to
        // hold for the boats to sound like different sizes at all.
        for sea in [CALM, BREEZE, BLOW] {
            let dinghy = loudness(ROWED, sea, EARSHOT);
            let ship = loudness(FULL_SPEED, sea, EARSHOT);
            assert!(
                dinghy < ship * 0.75,
                "{dinghy} is too close to {ship} under a sea of {sea}"
            );
        }
    }

    #[test]
    fn shoving_the_camera_closer_than_earshot_buys_nothing() {
        let near = loudness(FULL_SPEED, BREEZE, EARSHOT * 0.25);
        assert_eq!(near, loudness(FULL_SPEED, BREEZE, EARSHOT));
    }

    #[test]
    fn pulling_the_camera_back_takes_the_boat_faster_than_the_sea() {
        let close = loudness(FULL_SPEED, BREEZE, EARSHOT);
        let far = loudness(FULL_SPEED, BREEZE, EARSHOT * 4.0);
        assert!(far < close, "{far} is not below {close}");
        // Standing off costs the wash its inverse and costs the sea only the
        // climb, so what is left at four times the range is a larger share of
        // sea than it was alongside. Both are scaled by the same climb, which
        // is why this can be asked as a ratio.
        let share = |range| loudness(0.0, BREEZE, range) / loudness(FULL_SPEED, BREEZE, range);
        assert!(
            share(EARSHOT * 4.0) > share(EARSHOT),
            "the wash did not fall off faster than the sea"
        );
    }

    #[test]
    fn a_camera_as_high_as_it_goes_takes_the_sea_with_it() {
        // The far end of the zoom is a view of a whole mountain, and a boat
        // in it is a mark on the water rather than something being stood on.
        let alongside = loudness(FULL_SPEED, BREEZE, EARSHOT);
        let aloft = loudness(FULL_SPEED, BREEZE, MAX_DISTANCE);
        assert!(aloft < alongside * 0.25, "{aloft} is still too loud");
        // The sea goes with it and the sea is still there: quiet, not gone.
        let sea = loudness(0.0, BREEZE, MAX_DISTANCE);
        assert!(sea > 0.0, "the sea fell silent at the top of the zoom");
        assert!(sea < loudness(0.0, BREEZE, EARSHOT), "{sea} is not quieter");
    }

    #[test]
    fn the_water_quietens_evenly_across_the_zoom() {
        // Not a level that holds and then drops over the last few clicks:
        // each notch of a geometric zoom should cost about the same.
        let step = 1.5;
        let mut range = EARSHOT;
        let mut costs = Vec::new();
        while range * step <= MAX_DISTANCE {
            let here = loudness(0.0, BREEZE, range);
            let out = loudness(0.0, BREEZE, range * step);
            costs.push(here - out);
            range *= step;
        }
        let (least, most) = costs
            .iter()
            .fold((f32::MAX, 0.0f32), |(lo, hi), &c| (lo.min(c), hi.max(c)));
        assert!(most < least * 1.5, "{costs:?} is not an even fall");
    }

    #[test]
    fn a_boat_under_way_is_heard_from_where_the_camera_is() {
        let (way, range) = way_and_range(Some((4.0, Vec3::ZERO)), Some(Vec3::X * 100.0));
        assert_eq!(way, 4.0);
        assert_eq!(range, 100.0);
    }

    #[test]
    fn ashore_there_is_no_boat_and_no_penalty_for_it() {
        // The player has stepped off, so the `Boat` has come off the hull with
        // them. Nothing under way, and a range that stands at EARSHOT rather
        // than at whatever the camera happens to be from a hull that is
        // no longer the point — the sea is not something to be far from.
        let (way, range) = way_and_range(None, Some(Vec3::X * 1000.0));
        assert_eq!(way, 0.0);
        assert_eq!(range, EARSHOT);
        assert_eq!(loudness(way, BREEZE, range), loudness(0.0, BREEZE, EARSHOT));
    }

    #[test]
    fn a_frame_without_a_camera_still_has_a_boat_in_it() {
        // An unknown range is not a stopped boat: read together, the two
        // absences made a hull under sail go silent for want of an eye.
        let (way, range) = way_and_range(Some((4.0, Vec3::ZERO)), None);
        assert_eq!(way, 4.0);
        assert_eq!(range, EARSHOT);
    }

    #[test]
    fn a_frame_with_neither_is_the_quiet_one() {
        assert_eq!(way_and_range(None, None), (0.0, EARSHOT));
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
