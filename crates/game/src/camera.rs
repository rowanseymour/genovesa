//! A camera with a fixed pitch, kept centred on the player and free to turn
//! and zoom around them.

use bevy::input::mouse::{AccumulatedMouseScroll, MouseScrollUnit};
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;

use crate::bindings::{Action, KeyBindings};
use crate::boat::Boat;
use crate::terrain::{Archipelago, WorldTerrain};
use crate::AppState;

/// Downward tilt of the camera, from horizontal.
const PITCH: f32 = std::f32::consts::FRAC_PI_4 * 1.15;
/// Starting rotation about the vertical axis. 45° frames the map down a
/// diagonal.
const YAW: f32 = std::f32::consts::FRAC_PI_4;

/// How fast Q and E turn the view, in radians per second — a quarter turn a
/// second, so a full way round takes four seconds.
const ROTATION_SPEED: f32 = std::f32::consts::FRAC_PI_2;

/// Zoom range, as the camera's distance from its focus point in metres. At the
/// default the visible ground is roughly 50 m across at the near edge and 100 m
/// at the far one.
pub const MIN_DISTANCE: f32 = 20.0;
/// Far enough out to see a whole mountain. The terrain's peaks run to a couple
/// of hundred metres, and from closer than this the camera sits below the
/// summit of anything worth looking at.
pub const MAX_DISTANCE: f32 = 380.0;
const DEFAULT_DISTANCE: f32 = 42.0;

/// How much one notch of scroll changes the distance. Geometric, so a notch
/// moves the view by the same proportion however far out it already is.
const ZOOM_STEP: f32 = 1.15;
/// Scroll pixels that count as one notch. A wheel reports lines, but a trackpad
/// reports pixels — tens or hundreds of them per frame — and treating the two
/// alike makes a single flick cross the whole zoom range.
const PIXELS_PER_NOTCH: f32 = 50.0;

/// How close the eye may get to the ground beneath it, in metres. Following
/// the player onto rising ground lifts the camera rather than letting a
/// hillside swallow it.
const MIN_CLEARANCE: f32 = 12.0;

/// How quickly following, turning and zooming ease towards their targets.
const SMOOTHING: f32 = 12.0;

/// Somewhere to point the camera, as a whole. Enough to describe a view
/// completely, so that one can be asked for on the command line, put back at
/// the start of a match, or stepped through a list of shots.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// Point on the ground the camera is centred on — and, the camera being
    /// pinned to the player, where the player is put down. The height is
    /// ignored: [`follow_player`] takes it from the surface they ride.
    pub focus: Vec3,
    /// How far back the camera sits, in metres.
    pub distance: f32,
    /// Bearing to look from, in radians.
    pub yaw: f32,
}

impl Default for View {
    fn default() -> Self {
        Self {
            focus: Vec3::ZERO,
            distance: DEFAULT_DISTANCE,
            yaw: YAW,
        }
    }
}

impl View {
    /// Opens the view on a world: focused where that world is entered, and
    /// turned to face the island the entry stands off — so a match opens
    /// with its first land dead ahead of a bow already pointing at it.
    ///
    /// `point` is where this player is actually put down, which in a served
    /// world is the server's call — the world's own spawn, scattered a few
    /// boat-lengths, and not for a client to recompute. `None` takes the
    /// world's spawn itself, which is what a world of one's own enters on.
    /// The *facing* always asks the local layout, that being the same layout
    /// on every machine.
    ///
    /// A world whose layout offers nowhere to enter — broken rather than
    /// empty, see [`Archipelago::spawn`] — leaves the bearing alone and falls
    /// back to the origin, which every world keeps clear.
    pub fn enter(&mut self, world: &Archipelago, point: Option<Vec2>) {
        let spawn = world.spawn();
        let at = point.or(spawn.map(|s| s.point)).unwrap_or(Vec2::ZERO);
        self.focus = Vec3::new(at.x, 0.0, at.y);
        if let Some(spawn) = spawn {
            self.face(spawn.island.centre());
        }
    }

    /// Turns the view to look from its focus towards a ground point. The
    /// boat launches pointing down the view's yaw, so this also points the
    /// bow there — it is what entry uses to open facing the island the
    /// spawn stands off, first land dead ahead rather than at the camera's
    /// back. A target at the focus itself names no direction and leaves the
    /// yaw where it was.
    pub fn face(&mut self, target: Vec2) {
        let towards = target - Vec2::new(self.focus.x, self.focus.z);
        if towards == Vec2::ZERO {
            return;
        }
        // The eye sits at +(sin yaw, cos yaw) from the focus, so looking
        // along `towards` is the yaw whose sines oppose it.
        self.yaw = f32::atan2(-towards.x, -towards.y);
    }
}

/// The camera's ground-level target. The camera itself sits back and above it.
#[derive(Component)]
pub struct MapCamera {
    /// Point on the ground plane the camera is centred on — the player,
    /// whenever there is one to centre on.
    pub focus: Vec3,
    /// Where `focus` is heading — the camera eases towards it.
    target_focus: Vec3,
    pub distance: f32,
    target_distance: f32,
    /// Rotation about the vertical axis, in radians. Free to sit at any angle,
    /// and left to run unbounded rather than wrapped at a full turn, so easing
    /// towards `target_yaw` never has to reason about which way round the short
    /// way is.
    pub yaw: f32,
    target_yaw: f32,
    /// False until the camera has been put down on the player's surface for
    /// this match. [`recentre`] can't do it: the terrain resource doesn't
    /// exist yet when it runs, so the first frame that can see the ground
    /// snaps to it instead of easing down from sea level.
    grounded: bool,
}

impl Default for MapCamera {
    fn default() -> Self {
        Self::looking(View::default())
    }
}

impl MapCamera {
    /// A camera already at a view, rather than easing towards it.
    pub fn looking(view: View) -> Self {
        let mut camera = Self {
            focus: Vec3::ZERO,
            target_focus: Vec3::ZERO,
            distance: 0.0,
            target_distance: 0.0,
            yaw: 0.0,
            target_yaw: 0.0,
            grounded: false,
        };
        camera.snap_to(view);
        camera
    }

    /// Puts the camera at a view outright. Both the eased values and the
    /// targets are set, so nothing slides there over the following frames —
    /// which is what a screenshot of a named viewpoint needs.
    pub fn snap_to(&mut self, view: View) {
        let distance = view.distance.clamp(MIN_DISTANCE, MAX_DISTANCE);
        self.focus = view.focus;
        self.target_focus = view.focus;
        self.distance = distance;
        self.target_distance = distance;
        self.yaw = view.yaw;
        self.target_yaw = view.yaw;
        // The focus carries no useful height — dropping it back on the ground
        // is `follow_player`'s job, on the next frame.
        self.grounded = false;
    }
}

pub struct MapCameraPlugin;

impl Plugin for MapCameraPlugin {
    fn build(&self, app: &mut App) {
        // The camera outlives any one match — the UI needs one to render into
        // even while we're sitting on the main menu.
        //
        // The bindings are shared with the settings screen, which registers
        // them too; whichever plugin is built first wins and the other is a
        // no-op, so either can be used on its own.
        app.init_resource::<View>()
            .init_resource::<KeyBindings>()
            .add_systems(Startup, spawn_camera)
            .add_systems(OnEnter(AppState::InWorld), recentre)
            .add_systems(
                Update,
                (follow_player, zoom, rotate, apply_transform)
                    .chain()
                    .run_if(in_state(AppState::InWorld)),
            );
    }
}

/// Puts the camera back at the starting view at the start of a match.
fn recentre(view: Res<View>, mut cameras: Query<&mut MapCamera>) {
    for mut camera in &mut cameras {
        camera.snap_to(*view);
    }
}

fn spawn_camera(mut commands: Commands, view: Res<View>) {
    let camera = MapCamera::looking(*view);
    let transform = Transform::from_translation(eye(&camera)).looking_at(camera.focus, Vec3::Y);
    commands.spawn((
        Name::new("Camera"),
        Camera3d::default(),
        camera,
        transform,
        // Aerial haze, both to stop the far side of the map looking flat and to
        // hide where the sea plane is cut off by the far clip plane. The colour
        // has to match `ClearColor` and the fade has to finish before the far
        // plane, or that cut shows up as a hard line along the horizon.
        DistanceFog {
            color: crate::SKY,
            falloff: FogFalloff::Linear {
                start: crate::HAZE_START,
                end: crate::HAZE_END,
            },
            ..default()
        },
        Msaa::Sample4,
    ));
}

/// Where the camera eye sits, given its focus point, zoom distance and yaw.
fn eye(camera: &MapCamera) -> Vec3 {
    let offset = Vec3::new(
        camera.yaw.sin() * PITCH.cos(),
        PITCH.sin(),
        camera.yaw.cos() * PITCH.cos(),
    );
    camera.focus + offset * camera.distance
}

/// Keeps the camera centred on the player's boat. The boat rides the surface
/// it is over, so following its whole translation is also what raises the
/// camera onto a hillside and keeps it at the waterline over open sea.
fn follow_player(
    terrain: Option<Res<WorldTerrain>>,
    boats: Query<&Transform, With<Boat>>,
    mut cameras: Query<&mut MapCamera>,
) {
    // Absent in tests that only care about turning and zooming; a match
    // always has one.
    let Ok(boat) = boats.single() else {
        return;
    };

    for mut camera in &mut cameras {
        camera.target_focus = boat.translation;

        if camera.grounded {
            continue;
        }
        // Put down outright the first time the ground under the player can be
        // answered, rather than easing there from sea level. Asked of the
        // terrain rather than read off the boat because the boat's own height
        // is a leftover until that same ground arrives — the same surface,
        // one frame earlier.
        let Some(surface) = terrain
            .as_ref()
            .and_then(|t| t.surface(boat.translation.x, boat.translation.z))
        else {
            continue;
        };
        let focus = Vec3::new(boat.translation.x, surface, boat.translation.z);
        camera.focus = focus;
        camera.target_focus = focus;
        camera.grounded = true;
    }
}

fn zoom(scroll: Res<AccumulatedMouseScroll>, mut cameras: Query<&mut MapCamera>) {
    if scroll.delta.y == 0.0 {
        return;
    }

    let notches = match scroll.unit {
        MouseScrollUnit::Line => scroll.delta.y,
        MouseScrollUnit::Pixel => scroll.delta.y / PIXELS_PER_NOTCH,
    };

    for mut camera in &mut cameras {
        // Scaling by a power rather than subtracting a fraction keeps the step
        // even across the range and can't drive the distance through zero.
        camera.target_distance =
            (camera.target_distance * ZOOM_STEP.powf(-notches)).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }
}

/// The turn keys swing the view round for as long as they're held, so it can be
/// left facing any direction rather than only the four the map was laid out on.
/// Unlike movement these have no arrow-key fallback — a view that can't be
/// turned is awkward, not stranded.
fn rotate(
    keys: Res<ButtonInput<KeyCode>>,
    bindings: Res<KeyBindings>,
    time: Res<Time>,
    mut cameras: Query<&mut MapCamera>,
) {
    let mut direction = 0.0;
    if keys.pressed(bindings.key(Action::TurnLeft)) {
        direction += 1.0;
    }
    if keys.pressed(bindings.key(Action::TurnRight)) {
        direction -= 1.0;
    }
    if direction == 0.0 {
        return;
    }

    for mut camera in &mut cameras {
        // Drives the target rather than the yaw itself, so the easing in
        // `apply_transform` still smooths the start and the stop of a turn.
        camera.target_yaw += direction * ROTATION_SPEED * time.delta_secs();
    }
}

fn apply_transform(
    time: Res<Time>,
    terrain: Option<Res<WorldTerrain>>,
    mut cameras: Query<(&mut MapCamera, &mut Transform)>,
) {
    // Frame-rate independent exponential easing.
    let t = 1.0 - (-SMOOTHING * time.delta_secs()).exp();

    for (mut camera, mut transform) in &mut cameras {
        camera.focus = camera.focus.lerp(camera.target_focus, t);
        camera.distance = camera.distance + (camera.target_distance - camera.distance) * t;
        camera.yaw = camera.yaw + (camera.target_yaw - camera.yaw) * t;

        let mut eye = eye(&camera);

        // The focus riding the ground isn't enough on its own: zoomed in on a
        // slope, the eye sits well downhill of what it's looking at and can end
        // up inside the hillside behind it. Lifting it straight up steepens the
        // angle a little, which is a far better failure than being underground.
        // Ground still generating reads as absent, like the focus's own — see
        // `follow_player`.
        if let Some(floor) = terrain
            .as_ref()
            .and_then(|t| t.0.ready_height(eye.x, eye.z))
        {
            eye.y = eye.y.max(floor + MIN_CLEARANCE);
        }

        transform.translation = eye;
        transform.look_at(camera.focus, Vec3::Y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{elapsed, hold, rebind, run_frames, test_world};
    use bevy::state::app::StatesPlugin;
    use bevy::time::TimePlugin;

    /// A headless app with just enough plumbing to run the camera systems,
    /// already dropped into a match.
    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins((TimePlugin, StatesPlugin, MapCameraPlugin))
            .init_state::<AppState>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<AccumulatedMouseScroll>();
        app.update();

        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::InWorld);
        app.update();
        app
    }

    /// The same app with real ground under it, and the middle of the island
    /// that ground belongs to — somewhere to point the camera.
    fn test_app_on_terrain() -> (App, std::sync::Arc<Archipelago>, Vec3) {
        let (world, spec) = test_world();
        let centre = spec.centre();

        let mut app = test_app();
        app.insert_resource(WorldTerrain(world.clone()));
        spawn_boat(&mut app, Vec3::ZERO);
        app.update();
        (app, world, Vec3::new(centre.x, 0.0, centre.y))
    }

    /// Drops a bare boat entity into the world — enough for the camera to
    /// follow. The real one, mesh and all, is `BoatPlugin`'s to spawn.
    fn spawn_boat(app: &mut App, at: Vec3) {
        app.world_mut()
            .spawn((Boat::default(), Transform::from_translation(at)));
    }

    /// Puts the player down at a spot outright, mid-match, with the camera
    /// already settled there, and lets one frame run so the terrain systems
    /// ground the view.
    fn place_player(app: &mut App, spot: Vec3, distance: f32, yaw: f32) {
        let mut boats = app
            .world_mut()
            .query_filtered::<&mut Transform, With<Boat>>();
        boats
            .single_mut(app.world_mut())
            .expect("boat should exist")
            .translation = spot;
        let mut camera = app
            .world_mut()
            .query::<&mut MapCamera>()
            .single_mut(app.world_mut())
            .expect("camera should exist");
        camera.focus = spot;
        camera.target_focus = spot;
        camera.distance = distance;
        camera.target_distance = distance;
        camera.yaw = yaw;
        camera.target_yaw = yaw;
        // Snap onto the ground rather than easing towards it.
        camera.grounded = false;
        app.update();
    }

    /// Reads one field off the single camera.
    fn read<T>(app: &mut App, f: impl Fn(&MapCamera) -> T) -> T {
        f(app
            .world_mut()
            .query::<&MapCamera>()
            .single(app.world())
            .expect("camera should exist"))
    }

    fn focus(app: &mut App) -> Vec3 {
        read(app, |c| c.target_focus)
    }

    /// Radians per second the view turns at while `key` is held.
    fn turn_rate(key: KeyCode) -> f32 {
        let mut app = test_app();
        hold(&mut app, key);
        let start = read(&mut app, |c| c.target_yaw);
        let before = elapsed(&app);
        run_frames(&mut app, 20);
        let seconds = elapsed(&app) - before;
        assert!(seconds > 0.0, "no time passed while the key was held");
        (read(&mut app, |c| c.target_yaw) - start) / seconds
    }

    #[test]
    fn spawns_a_single_camera_looking_at_the_origin() {
        let mut app = test_app();
        let count = app
            .world_mut()
            .query::<&MapCamera>()
            .iter(app.world())
            .count();
        assert_eq!(count, 1);
        assert_eq!(focus(&mut app), Vec3::ZERO);
    }

    #[test]
    fn a_rebound_key_turns_the_view() {
        let mut app = test_app();
        rebind(&mut app, Action::TurnLeft, KeyCode::KeyN);

        let start = read(&mut app, |c| c.target_yaw);
        hold(&mut app, KeyCode::KeyN);
        run_frames(&mut app, 20);

        assert!(
            read(&mut app, |c| c.target_yaw) > start,
            "the newly bound key did not turn the view"
        );
    }

    #[test]
    fn the_camera_follows_the_boat() {
        let mut app = test_app();
        spawn_boat(&mut app, Vec3::new(30.0, 2.0, -14.0));
        app.update();
        assert_eq!(focus(&mut app), Vec3::new(30.0, 2.0, -14.0));

        // And keeps following: wherever the boat goes, the focus goes whole —
        // height included, since the boat rides the surface the camera wants.
        let mut boats = app
            .world_mut()
            .query_filtered::<&mut Transform, With<Boat>>();
        boats
            .single_mut(app.world_mut())
            .expect("boat should exist")
            .translation = Vec3::new(-5.0, 0.5, 41.0);
        app.update();
        assert_eq!(focus(&mut app), Vec3::new(-5.0, 0.5, 41.0));
    }

    #[test]
    fn q_and_e_turn_the_view_opposite_ways_at_the_rotation_speed() {
        let anticlockwise = turn_rate(KeyCode::KeyQ);
        let clockwise = turn_rate(KeyCode::KeyE);

        // A rate rather than an angle, since it's being proportional to how
        // long the key was held that makes the turn the same on any machine —
        // an angle on its own can't tell a held turn from a stepped one.
        let tolerance = ROTATION_SPEED * 0.01;
        assert!(
            (anticlockwise - ROTATION_SPEED).abs() < tolerance,
            "Q turned at {anticlockwise} rad/s, not {ROTATION_SPEED}"
        );
        assert!(
            (clockwise + ROTATION_SPEED).abs() < tolerance,
            "E turned at {clockwise} rad/s, not -{ROTATION_SPEED}"
        );
    }

    #[test]
    fn releasing_a_turn_key_stops_the_view() {
        let mut app = test_app();
        hold(&mut app, KeyCode::KeyQ);
        run_frames(&mut app, 20);

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::KeyQ);
        app.update();
        let stopped = read(&mut app, |c| c.target_yaw);
        run_frames(&mut app, 20);

        assert_eq!(read(&mut app, |c| c.target_yaw), stopped);
    }

    #[test]
    fn the_focus_snaps_onto_the_ground_the_player_is_dropped_on() {
        let (mut app, world, centre) = test_app_on_terrain();

        // Dropped onto the island, the first frame that can see the terrain
        // puts the camera down on it rather than easing from sea level. The
        // boat placed by `place_player` still carries a stale height, which is
        // exactly the situation at the start of a match — the snap has to ask
        // the ground, not the boat.
        place_player(&mut app, centre, DEFAULT_DISTANCE, YAW);
        let start = read(&mut app, |c| c.focus);
        assert!(
            (start.y - world.height(start.x, start.z)).abs() < 1e-3,
            "camera started at {} rather than on the ground",
            start.y
        );
    }

    #[test]
    fn the_eye_never_gets_inside_the_ground() {
        // Put the player down all over an island rather than sailing to each
        // spot: movement is wall-clock driven, so a headless run covers almost
        // no ground. The closest zoom is the dangerous one — that's where the
        // eye sits lowest — and a handful of yaws puts it on a different side
        // of whatever it's looking at.
        let (mut app, world, centre) = test_app_on_terrain();
        let spec = world
            .island_at(centre.x, centre.z)
            .expect("the camera is on an island");
        let half = spec.extent() * 0.5 - 32.0;
        let mut clamped = 0;

        // Yaws that aren't the four diagonals islands are laid out on, since
        // the view is free to stop between them.
        for turn in 0..5 {
            for iz in 0..12 {
                for ix in 0..12 {
                    let spot = centre
                        + Vec3::new(
                            (ix as f32 / 11.0 * 2.0 - 1.0) * half.x,
                            0.0,
                            (iz as f32 / 11.0 * 2.0 - 1.0) * half.y,
                        );
                    place_player(
                        &mut app,
                        spot,
                        MIN_DISTANCE,
                        YAW + turn as f32 * std::f32::consts::TAU / 5.0,
                    );

                    let eye = app
                        .world_mut()
                        .query_filtered::<&Transform, With<MapCamera>>()
                        .single(app.world())
                        .expect("camera should exist")
                        .translation;

                    let clearance = eye.y - world.height(eye.x, eye.z);
                    assert!(
                        clearance >= MIN_CLEARANCE - 1e-3,
                        "eye was {clearance} m above the ground at {eye:?}"
                    );
                    if clearance > MIN_CLEARANCE + 1e-3 {
                        continue;
                    }
                    clamped += 1;
                }
            }
        }

        // And the clamp has to be doing something, or this proves nothing.
        assert!(clamped > 0, "the clearance clamp never engaged");
    }

    #[test]
    fn a_trackpad_and_a_wheel_zoom_by_the_same_amount() {
        let zoom_by = |unit: MouseScrollUnit, delta: f32| {
            let mut app = test_app();
            app.insert_resource(AccumulatedMouseScroll {
                unit,
                delta: Vec2::new(0.0, delta),
            });
            app.update();
            read(&mut app, |c| c.target_distance)
        };

        let wheel = zoom_by(MouseScrollUnit::Line, 3.0);
        let trackpad = zoom_by(MouseScrollUnit::Pixel, 3.0 * PIXELS_PER_NOTCH);

        assert!(wheel < DEFAULT_DISTANCE, "scrolling up should zoom in");
        assert!(
            (wheel - trackpad).abs() < 1e-3,
            "wheel gave {wheel} m but the trackpad gave {trackpad} m"
        );
    }

    #[test]
    fn zooming_stays_inside_its_range() {
        // A single huge trackpad flick, which is what used to drive the
        // distance straight through zero and out the other side.
        for delta in [-100_000.0, 100_000.0] {
            let mut app = test_app();
            app.insert_resource(AccumulatedMouseScroll {
                unit: MouseScrollUnit::Pixel,
                delta: Vec2::new(0.0, delta),
            });
            app.update();

            let distance = read(&mut app, |c| c.target_distance);
            assert!(
                (MIN_DISTANCE..=MAX_DISTANCE).contains(&distance),
                "{delta} px of scroll gave a distance of {distance} m"
            );
        }
    }

    #[test]
    fn eye_sits_above_and_back_from_the_focus() {
        let camera = MapCamera::default();
        let eye = eye(&camera);

        assert!(eye.y > 0.0, "camera should be above the ground");
        assert!((eye - camera.focus).length() - DEFAULT_DISTANCE < 1e-3);
        // Looking down the diagonal, so the eye is offset on both axes.
        assert!(eye.x > 0.0 && eye.z > 0.0);
    }

    #[test]
    fn a_match_starts_at_the_view_it_was_given() {
        let view = View {
            focus: Vec3::new(98.0, 0.0, -317.0),
            distance: 150.0,
            yaw: 1.25,
        };
        let mut app = test_app();
        app.insert_resource(view);
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::InWorld);
        app.update();

        assert_eq!(focus(&mut app), view.focus);
        assert_eq!(read(&mut app, |c| c.target_distance), view.distance);
        assert_eq!(read(&mut app, |c| c.target_yaw), view.yaw);
    }

    #[test]
    fn snapping_to_a_view_leaves_nothing_still_easing() {
        let mut camera = MapCamera::default();
        camera.snap_to(View {
            focus: Vec3::new(-40.0, 0.0, 12.0),
            distance: 200.0,
            yaw: -0.5,
        });

        // Eased value and target agree, so the next frame renders the view
        // asked for rather than one on its way there.
        assert_eq!(camera.focus, camera.target_focus);
        assert_eq!(camera.distance, camera.target_distance);
        assert_eq!(camera.yaw, camera.target_yaw);
    }

    #[test]
    fn snapping_holds_the_zoom_within_range() {
        let mut camera = MapCamera::default();
        camera.snap_to(View {
            distance: MAX_DISTANCE * 10.0,
            ..View::default()
        });
        assert_eq!(camera.target_distance, MAX_DISTANCE);
    }
}
