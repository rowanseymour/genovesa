//! A camera with a fixed pitch that pans and turns over the map.

use bevy::input::mouse::{AccumulatedMouseScroll, MouseScrollUnit};
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;

use crate::terrain::{MapConfig, TerrainGenerator};
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
const MIN_DISTANCE: f32 = 20.0;
/// Far enough out to see a whole mountain. The terrain's peaks run to a couple
/// of hundred metres, and from closer than this the camera sits below the
/// summit of anything worth looking at.
const MAX_DISTANCE: f32 = 380.0;
const DEFAULT_DISTANCE: f32 = 42.0;

/// How much one notch of scroll changes the distance. Geometric, so a notch
/// moves the view by the same proportion however far out it already is.
const ZOOM_STEP: f32 = 1.15;
/// Scroll pixels that count as one notch. A wheel reports lines, but a trackpad
/// reports pixels — tens or hundreds of them per frame — and treating the two
/// alike makes a single flick cross the whole zoom range.
const PIXELS_PER_NOTCH: f32 = 50.0;

/// How close the eye may get to the ground beneath it, in metres. Panning onto
/// rising ground lifts the camera rather than letting a hillside swallow it.
const MIN_CLEARANCE: f32 = 12.0;

/// Metres per second at the default zoom level.
const PAN_SPEED: f32 = 45.0;
/// How quickly panning and zooming ease towards their targets.
const SMOOTHING: f32 = 12.0;

/// The camera's ground-level target. The camera itself sits back and above it.
#[derive(Component)]
pub struct MapCamera {
    /// Point on the ground plane the camera is centred on.
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
    /// False until the camera has been put down on the ground for this match.
    /// [`recentre`] can't do it: the terrain resource doesn't exist yet when it
    /// runs, so the first frame that can see the ground snaps to it instead of
    /// easing down from sea level.
    grounded: bool,
}

impl Default for MapCamera {
    fn default() -> Self {
        // `KASSITER_ZOOM` sets the starting distance in metres, for checking
        // how the terrain reads at a given framing without reaching for the
        // scroll wheel every run.
        let distance = std::env::var("KASSITER_ZOOM")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(DEFAULT_DISTANCE)
            .clamp(MIN_DISTANCE, MAX_DISTANCE);

        // `KASSITER_FOCUS=x,z` starts the camera over a given point on the map
        // in metres from its centre, so a particular stretch of coast can be
        // looked at — or screenshotted — without panning there by hand.
        let focus = std::env::var("KASSITER_FOCUS")
            .ok()
            .and_then(|v| parse_focus(&v))
            .unwrap_or(Vec3::ZERO);

        // `KASSITER_YAW` starts the view turned to a given bearing in degrees,
        // so the same spot can be screenshotted from several sides without
        // holding Q or E for exactly the right length of time.
        let yaw = std::env::var("KASSITER_YAW")
            .ok()
            .and_then(|v| v.trim().parse::<f32>().ok())
            .map(f32::to_radians)
            .unwrap_or(YAW);

        Self {
            focus,
            target_focus: focus,
            distance,
            target_distance: distance,
            yaw,
            target_yaw: yaw,
            grounded: false,
        }
    }
}

/// Reads an `x,z` pair of metres. The height is left at zero — `follow_terrain`
/// puts the camera down on the ground on its first frame.
fn parse_focus(value: &str) -> Option<Vec3> {
    let (x, z) = value.split_once(',')?;
    Some(Vec3::new(
        x.trim().parse().ok()?,
        0.0,
        z.trim().parse().ok()?,
    ))
}

pub struct MapCameraPlugin;

impl Plugin for MapCameraPlugin {
    fn build(&self, app: &mut App) {
        // The camera outlives any one match — the UI needs one to render into
        // even while we're sitting on the main menu.
        app.add_systems(Startup, spawn_camera)
            .add_systems(OnEnter(AppState::InWorld), recentre)
            .add_systems(
                Update,
                (pan, zoom, rotate, follow_terrain, apply_transform)
                    .chain()
                    .run_if(in_state(AppState::InWorld)),
            );
    }
}

/// Puts the camera back over the middle of the map at the start of a match.
fn recentre(mut cameras: Query<&mut MapCamera>) {
    for mut camera in &mut cameras {
        *camera = MapCamera::default();
    }
}

fn spawn_camera(mut commands: Commands) {
    let camera = MapCamera::default();
    commands.spawn((
        Name::new("Camera"),
        Camera3d::default(),
        MapCamera::default(),
        Transform::from_translation(eye(&camera)).looking_at(camera.focus, Vec3::Y),
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

/// The direction "away from the viewer" on the ground plane, at a given yaw.
fn forward(yaw: f32) -> Vec3 {
    -Vec3::new(yaw.sin(), 0.0, yaw.cos())
}

fn pan(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    config: Res<MapConfig>,
    mut cameras: Query<&mut MapCamera>,
) {
    let mut input = Vec2::ZERO;
    if keys.any_pressed([KeyCode::ArrowUp, KeyCode::KeyW]) {
        input.y += 1.0;
    }
    if keys.any_pressed([KeyCode::ArrowDown, KeyCode::KeyS]) {
        input.y -= 1.0;
    }
    if keys.any_pressed([KeyCode::ArrowRight, KeyCode::KeyD]) {
        input.x += 1.0;
    }
    if keys.any_pressed([KeyCode::ArrowLeft, KeyCode::KeyA]) {
        input.x -= 1.0;
    }

    if input == Vec2::ZERO {
        return;
    }
    let input = input.normalize();

    for mut camera in &mut cameras {
        // Movement is relative to the way the camera faces, so "up" always means
        // "away from the viewer" however far round the view has been turned.
        // Taken from the eased `yaw`, not the target, so panning mid-turn goes
        // where the picture on screen says it should.
        let forward = forward(camera.yaw);
        let right = forward.cross(Vec3::Y);

        // Panning covers more ground when zoomed out, which keeps the apparent
        // speed on screen roughly constant.
        let speed = PAN_SPEED * (camera.distance / DEFAULT_DISTANCE);
        let delta = (forward * input.y + right * input.x) * speed * time.delta_secs();

        // Let the camera drift a little past the coastline, but not so far that
        // the map disappears off screen. Only the ground plane is clamped —
        // `follow_terrain` owns the height.
        let limit = config.half_extent() * 1.05;
        let mut target = camera.target_focus + delta;
        target.x = target.x.clamp(-limit.x, limit.x);
        target.z = target.z.clamp(-limit.y, limit.y);
        camera.target_focus = target;
    }
}

/// Keeps the focus point on the ground, so panning onto a hill raises the whole
/// camera with it instead of burying it.
fn follow_terrain(terrain: Option<Res<TerrainGenerator>>, mut cameras: Query<&mut MapCamera>) {
    // Absent in the menu, and in tests that only care about panning maths.
    let Some(terrain) = terrain else {
        return;
    };

    for mut camera in &mut cameras {
        let ground = terrain.height(camera.target_focus.x, camera.target_focus.z);
        camera.target_focus.y = ground;

        if !camera.grounded {
            camera.focus.y = ground;
            camera.grounded = true;
        }
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

/// Q and E swing the view round for as long as they're held, so it can be left
/// facing any direction rather than only the four the map was laid out on.
fn rotate(keys: Res<ButtonInput<KeyCode>>, time: Res<Time>, mut cameras: Query<&mut MapCamera>) {
    let mut direction = 0.0;
    if keys.pressed(KeyCode::KeyQ) {
        direction += 1.0;
    }
    if keys.pressed(KeyCode::KeyE) {
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
    terrain: Option<Res<TerrainGenerator>>,
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
        if let Some(terrain) = &terrain {
            let floor = terrain.height(eye.x, eye.z) + MIN_CLEARANCE;
            eye.y = eye.y.max(floor);
        }

        transform.translation = eye;
        transform.look_at(camera.focus, Vec3::Y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::state::app::StatesPlugin;
    use bevy::time::TimePlugin;

    /// A headless app with just enough plumbing to run the camera systems,
    /// already dropped into a match.
    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins((TimePlugin, StatesPlugin, MapCameraPlugin))
            .init_state::<AppState>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<AccumulatedMouseScroll>()
            .insert_resource(MapConfig {
                chunks: UVec2::splat(1),
                seed: 1,
            });
        app.update();

        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::InWorld);
        app.update();
        app
    }

    /// The same app with real terrain under it, so the camera has ground to
    /// follow and to keep clear of.
    fn test_app_on_terrain() -> App {
        let mut app = test_app();
        let config = MapConfig::square(1024, 20_040_112);
        app.insert_resource(config)
            .insert_resource(TerrainGenerator::new(&config));
        app.update();
        app
    }

    fn hold(app: &mut App, key: KeyCode) {
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(key);
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

    /// Puts the view at a given yaw outright. Turning there by holding a key
    /// would take as long as the wall clock says, which a headless run has no
    /// patience for.
    fn turn_to(app: &mut App, yaw: f32) {
        let mut camera = app
            .world_mut()
            .query::<&mut MapCamera>()
            .single_mut(app.world_mut())
            .expect("camera should exist");
        camera.yaw = yaw;
        camera.target_yaw = yaw;
    }

    /// Runs frames with whatever keys are down. Clears the just-pressed flags
    /// between them the way the real input plugin does, so a key held here
    /// reads as held rather than as pressed afresh every frame.
    fn run_frames(app: &mut App, count: usize) {
        for _ in 0..count {
            app.update();
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .clear();
        }
    }

    /// Seconds of clock the app has run for. Frames take however long they take
    /// in a headless run, so anything driven by `delta_secs` has to be measured
    /// against the time that actually passed rather than a frame count.
    fn elapsed(app: &App) -> f32 {
        app.world().resource::<Time>().elapsed_secs()
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
    fn arrow_up_pans_away_from_the_viewer() {
        let mut app = test_app();
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 20);

        let focus = focus(&mut app);
        // The camera looks down the -X/-Z diagonal, so "up" moves that way.
        assert!(focus.x < 0.0, "expected -X movement, got {focus:?}");
        assert!(focus.z < 0.0, "expected -Z movement, got {focus:?}");
        // Panning is on the ground plane only.
        assert_eq!(focus.y, 0.0);
    }

    #[test]
    fn opposite_keys_pan_in_opposite_directions() {
        let mut app = test_app();
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 20);
        let up = focus(&mut app);

        let mut app = test_app();
        hold(&mut app, KeyCode::ArrowDown);
        run_frames(&mut app, 20);
        let down = focus(&mut app);

        // Only the direction is comparable — the distance covered depends on
        // how much wall-clock time each run happened to take.
        let (up, down) = (up.normalize(), down.normalize());
        assert!(
            (up + down).length() < 1e-3,
            "{up:?} and {down:?} are not opposites"
        );
    }

    #[test]
    fn right_is_perpendicular_to_up() {
        let mut app = test_app();
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 20);
        let up = focus(&mut app).normalize();

        let mut app = test_app();
        hold(&mut app, KeyCode::ArrowRight);
        run_frames(&mut app, 20);
        let right = focus(&mut app).normalize();

        assert!(
            up.dot(right).abs() < 1e-3,
            "{up:?} and {right:?} are not perpendicular"
        );
    }

    #[test]
    fn panning_stops_at_the_map_edge() {
        let mut app = test_app();
        hold(&mut app, KeyCode::ArrowUp);
        // Far more frames than it takes to cross a single-chunk map.
        run_frames(&mut app, 600);

        let limit = 64.0 * 1.05;
        let focus = focus(&mut app);
        assert!(
            focus.x >= -limit - 1e-3 && focus.z >= -limit - 1e-3,
            "{focus:?} escaped the map"
        );
    }

    #[test]
    fn wasd_matches_the_arrow_keys() {
        let mut app = test_app();
        hold(&mut app, KeyCode::ArrowLeft);
        run_frames(&mut app, 20);
        let arrows = focus(&mut app);

        let mut app = test_app();
        hold(&mut app, KeyCode::KeyA);
        run_frames(&mut app, 20);
        let wasd = focus(&mut app);

        let (arrows, wasd) = (arrows.normalize(), wasd.normalize());
        assert!((arrows - wasd).length() < 1e-3, "{arrows:?} != {wasd:?}");
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
    fn panning_follows_the_view_round() {
        let mut app = test_app();
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 20);
        let before = focus(&mut app).normalize();

        // An angle that isn't a multiple of a quarter turn, which the view can
        // now come to rest at.
        let turn = 0.7;
        let mut app = test_app();
        turn_to(&mut app, YAW + turn);
        hold(&mut app, KeyCode::ArrowUp);
        let start = focus(&mut app);
        run_frames(&mut app, 20);
        let after = (focus(&mut app) - start).normalize();

        // Turning the view turns "away from me" with it.
        let expected = Quat::from_rotation_y(turn) * before;
        assert!(
            (after - expected).length() < 1e-2,
            "{after:?} is not {before:?} turned by {turn} rad"
        );
    }

    #[test]
    fn the_focus_sits_on_the_ground() {
        let mut app = test_app_on_terrain();
        let terrain = TerrainGenerator::new(&MapConfig::square(1024, 20_040_112));

        // The first frame that can see the terrain puts the camera down on it
        // rather than easing from sea level.
        let start = read(&mut app, |c| c.focus);
        assert!(
            (start.y - terrain.height(start.x, start.z)).abs() < 1e-3,
            "camera started at {} rather than on the ground",
            start.y
        );

        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 60);

        let target = focus(&mut app);
        assert!(
            (target.y - terrain.height(target.x, target.z)).abs() < 1e-3,
            "focus left the ground while panning"
        );
    }

    #[test]
    fn the_eye_never_gets_inside_the_ground() {
        let terrain = TerrainGenerator::new(&MapConfig::square(1024, 20_040_112));

        // Put the camera down all over the map rather than panning to each
        // spot: panning is wall-clock driven, so a headless run covers almost
        // no ground. The closest zoom is the dangerous one — that's where the
        // eye sits lowest — and a handful of yaws puts it on a different side
        // of whatever it's looking at.
        let mut app = test_app_on_terrain();
        let mut clamped = 0;

        // Yaws that aren't the four diagonals the map was laid out on, since
        // the view is free to stop between them now.
        for turn in 0..5 {
            for iz in 0..16 {
                for ix in 0..16 {
                    let spot = Vec3::new(ix as f32 * 64.0 - 480.0, 0.0, iz as f32 * 64.0 - 480.0);

                    {
                        let mut camera = app
                            .world_mut()
                            .query::<&mut MapCamera>()
                            .single_mut(app.world_mut())
                            .expect("camera should exist");
                        camera.focus = spot;
                        camera.target_focus = spot;
                        camera.distance = MIN_DISTANCE;
                        camera.target_distance = MIN_DISTANCE;
                        camera.yaw = YAW + turn as f32 * std::f32::consts::TAU / 5.0;
                        camera.target_yaw = camera.yaw;
                        // Snap onto the ground rather than easing towards it.
                        camera.grounded = false;
                    }
                    app.update();

                    let eye = app
                        .world_mut()
                        .query_filtered::<&Transform, With<MapCamera>>()
                        .single(app.world())
                        .expect("camera should exist")
                        .translation;

                    let clearance = eye.y - terrain.height(eye.x, eye.z);
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
    fn a_starting_focus_reads_as_a_pair_of_metres() {
        assert_eq!(parse_focus("120,-45"), Some(Vec3::new(120.0, 0.0, -45.0)));
        assert_eq!(parse_focus(" 8.5 , 2 "), Some(Vec3::new(8.5, 0.0, 2.0)));

        // Anything that isn't a pair is ignored rather than guessed at, so a
        // typo in the variable leaves the camera where it would have been.
        for bad in ["", "120", "120;45", "a,b", "1,2,3"] {
            assert_eq!(parse_focus(bad), None, "{bad:?} should not parse");
        }
    }
}
