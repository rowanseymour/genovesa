//! The boat the player gets about in, and the keys that steer it.
//!
//! The hull is a placeholder, meant to be replaced wholesale by a modelled
//! asset: a dozen triangles cut to plausible dimensions for a small boat —
//! long enough to see which end is the bow from a camera forty metres up, and
//! nothing more. What is here to stay is the *entity*: the player's place in
//! the world, which the movement keys drive ([`steer`]) and the camera stays
//! centred on.
//!
//! It faces down its own -Z, so [`Transform::forward`] is the way it is
//! pointing and steering can leave the axis convention alone. Its origin is on
//! the waterline rather than at the keel or the deck, which is what lets
//! [`float`] put it down by simply setting the height of the surface it is on.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::PrimitiveTopology;
use bevy::prelude::*;

use crate::bindings::{Action, KeyBindings};
use crate::camera::{self, MapCamera, View};
use crate::terrain::WorldTerrain;
use crate::AppState;

/// Length overall, in metres. A small sailing boat: at the default zoom the
/// visible ground is some tens of metres across, so this reads as a boat
/// rather than as a speck, and the same at the far end of the zoom range it is
/// still a mark on the water rather than gone.
const LENGTH: f32 = 7.0;
/// Width at the widest point.
const BEAM: f32 = 2.4;
/// Deck height above the waterline.
const FREEBOARD: f32 = 0.9;
/// Keel depth below it. The sea is translucent, so this much of the hull shows
/// through the water as a darker shape under the deck.
const DRAFT: f32 = 0.8;

/// Height of the mast above the deck. Tall out of proportion to the hull,
/// deliberately: from a camera looking down at 50° a mast is most of what says
/// which way the boat is leaning and where it is against the ground behind it,
/// and its shadow is what pins it to the water.
const MAST_HEIGHT: f32 = 6.0;
const MAST_THICKNESS: f32 = 0.16;

/// Where the hull is widest, as a fraction of its length aft of amidships. Aft
/// of it, so that the taper to the bow is nearly twice the length of the one to
/// the transom — the fine entry and full stern is what tells one end from the
/// other when the camera is looking straight down at it.
const SHOULDER: f32 = 0.15;

/// Where the mast stands, the same way — about a third of the way back from the
/// bow, which is where a boat this shape would carry one.
const MAST_STATION: f32 = -0.15;

/// Metres per second under way. Brisk beyond honesty for a seven-metre hull,
/// but the boat is how the world is crossed: at this speed the ground in view
/// at the default zoom slides by in a few seconds, and the next island is
/// minutes away rather than tens of minutes.
const SPEED: f32 = 10.0;

/// How fast the bow comes round, in radians per second. Together with
/// [`SPEED`] this fixes the turning circle at about five metres — tight enough
/// to feel answerable from a camera forty metres up, wide enough that coming
/// about reads as a turn rather than a spin.
const TURN_RATE: f32 = 2.0;

/// Timber. Nothing on an island or in the sea is anywhere near this hue, so the
/// boat is findable in a landscape of greens and blues without being lit any
/// differently from them.
const HULL_COLOR: Color = Color::srgb(0.62, 0.28, 0.22);
/// Bare spar, pale enough to stand off both the water and the hull.
const SPAR_COLOR: Color = Color::srgb(0.86, 0.80, 0.68);

/// The player's boat. One per match, spawned where the world is entered.
#[derive(Component)]
pub struct Boat;

pub struct BoatPlugin;

impl Plugin for BoatPlugin {
    fn build(&self, app: &mut App) {
        // Steering before floating, so ground gained or lost by this frame's
        // movement is under the hull the same frame rather than the next.
        app.add_systems(OnEnter(AppState::InWorld), launch)
            .add_systems(
                Update,
                (steer, float).chain().run_if(in_state(AppState::InWorld)),
            );
    }
}

/// Puts the boat in the world at the point the world is entered, pointing the
/// way the opening view looks.
///
/// The view names where the player enters the world, so the boat goes there
/// rather than anywhere of its own choosing. That leaves it aground when a
/// world opens inland, which is what the entry point currently is; the
/// movement keys will drive it to the water.
fn launch(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    view: Res<View>,
) {
    // Matte, like everything else in this look — see the terrain's own
    // materials for why a specular highlight would be wrong here.
    let mut matte = |base_color| {
        materials.add(StandardMaterial {
            base_color,
            perceptual_roughness: 1.0,
            metallic: 0.0,
            reflectance: 0.0,
            ..default()
        })
    };
    let hull_material = matte(HULL_COLOR);
    let spar_material = matte(SPAR_COLOR);

    commands.spawn((
        Name::new("Boat"),
        Boat,
        DespawnOnExit(AppState::InWorld),
        Mesh3d(meshes.add(hull_mesh())),
        MeshMaterial3d(hull_material),
        // A rotation of `yaw` about the vertical takes -Z to the camera's own
        // forward, so the boat starts pointing away from the viewer.
        Transform::from_xyz(view.focus.x, 0.0, view.focus.z)
            .with_rotation(Quat::from_rotation_y(view.yaw)),
        children![(
            Name::new("Mast"),
            Mesh3d(meshes.add(Cuboid::new(MAST_THICKNESS, MAST_HEIGHT, MAST_THICKNESS))),
            MeshMaterial3d(spar_material),
            Transform::from_xyz(0.0, FREEBOARD + MAST_HEIGHT * 0.5, LENGTH * MAST_STATION),
        )],
    ));

    // Said out loud for the same reason a run without a seed says which world
    // it picked: a placeholder nobody can find is indistinguishable from one
    // that never spawned, and `--focus` takes exactly these two numbers.
    info!("boat launched at {}, {}", view.focus.x, view.focus.z);
}

/// Keeps the boat on the surface it is over: the sea, or the ground where the
/// ground is above the sea.
///
/// The same rule the other players' markers ride, and for the same reason —
/// ground still streaming in reads as absent and the boat keeps the height it
/// had, rather than dropping to sea level for the few frames an island takes to
/// arrive. The waterline is the origin, so a boat that has run aground is
/// half-buried in the hillside; that is what aground looks like, and steering
/// is what will keep it off.
fn float(terrain: Option<Res<WorldTerrain>>, mut boats: Query<&mut Transform, With<Boat>>) {
    for mut transform in &mut boats {
        let Some(ground) = terrain.as_ref().and_then(|t| {
            t.0.ready_height(transform.translation.x, transform.translation.z)
        }) else {
            continue;
        };
        transform.translation.y = ground.max(0.0);
    }
}

/// Drives the boat by the movement keys, read relative to the way the view
/// faces: forward is "away from the viewer" however far round the view has
/// been turned, exactly as the keys read on screen. The bow comes round
/// towards the asked-for direction while the hull advances along its heading,
/// so a sideways key carves an arc rather than strafing — the one hint of
/// handling this placeholder keeps ahead of anything resembling physics.
///
/// Nothing here knows about land: a hull driven onto a hillside ploughs
/// through it, half-buried by [`float`]. Collision is deferred, deliberately —
/// steering that *feels* right comes before running aground having
/// consequences.
fn steer(
    keys: Res<ButtonInput<KeyCode>>,
    bindings: Res<KeyBindings>,
    time: Res<Time>,
    cameras: Query<&MapCamera>,
    mut boats: Query<&mut Transform, With<Boat>>,
) {
    let mut input = Vec2::ZERO;
    if bindings.held(&keys, Action::MoveForward, KeyCode::ArrowUp) {
        input.y += 1.0;
    }
    if bindings.held(&keys, Action::MoveBack, KeyCode::ArrowDown) {
        input.y -= 1.0;
    }
    if bindings.held(&keys, Action::MoveRight, KeyCode::ArrowRight) {
        input.x += 1.0;
    }
    if bindings.held(&keys, Action::MoveLeft, KeyCode::ArrowLeft) {
        input.x -= 1.0;
    }
    if input == Vec2::ZERO {
        return;
    }

    // Taken from the eased yaw, not its target, so steering mid-turn goes
    // where the picture on screen says it should.
    let Ok(view) = cameras.single() else {
        return;
    };
    let forward = camera::forward(view.yaw);
    let right = forward.cross(Vec3::Y);
    let asked = (forward * input.y + right * input.x).normalize();

    for mut boat in &mut boats {
        // The shortest way round from the heading to the asked-for direction,
        // no more of it than the turn rate allows this frame.
        let turn = angle_to(*boat.forward(), asked);
        let most = TURN_RATE * time.delta_secs();
        boat.rotate_y(turn.clamp(-most, most));

        let advance = boat.forward() * SPEED * time.delta_secs();
        boat.translation += advance;
    }
}

/// Signed angle about the vertical from one ground-plane direction to another,
/// in [-π, π] — positive the way [`Transform::rotate_y`] turns.
fn angle_to(from: Vec3, to: Vec3) -> f32 {
    f32::atan2(from.z * to.x - from.x * to.z, from.x * to.x + from.z * to.z)
}

/// The hull, as the triangles it is made of, wound so that every face looks
/// outwards.
///
/// Five points around the deck and two along the keel: a bow, a shoulder either
/// side where the beam is widest, and a transom narrower than the shoulder. The
/// sides fall from the deck to a keel line rather than to a flat bottom, so the
/// hull is a shallow V and reads as a boat from the side as well as from above.
fn hull_faces() -> [[Vec3; 3]; 10] {
    let (half_length, half_beam) = (LENGTH * 0.5, BEAM * 0.5);
    let shoulder = LENGTH * SHOULDER;

    // Deck, from the bow round to the transom.
    let bow = Vec3::new(0.0, FREEBOARD, -half_length);
    let port_shoulder = Vec3::new(-half_beam, FREEBOARD, shoulder);
    let port_quarter = Vec3::new(-half_beam * 0.8, FREEBOARD, half_length);
    let starboard_quarter = Vec3::new(half_beam * 0.8, FREEBOARD, half_length);
    let starboard_shoulder = Vec3::new(half_beam, FREEBOARD, shoulder);

    // Keel. It starts short of the bow, which is what gives the stem its rake.
    let forefoot = Vec3::new(0.0, -DRAFT, -half_length * 0.7);
    let heel = Vec3::new(0.0, -DRAFT, half_length);

    [
        // Deck, fanned from the bow.
        [bow, port_shoulder, port_quarter],
        [bow, port_quarter, starboard_quarter],
        [bow, starboard_quarter, starboard_shoulder],
        // Port side, bow to transom.
        [bow, forefoot, port_shoulder],
        [port_shoulder, forefoot, heel],
        [port_shoulder, heel, port_quarter],
        // Starboard, the same three mirrored.
        [starboard_shoulder, forefoot, bow],
        [heel, forefoot, starboard_shoulder],
        [starboard_quarter, heel, starboard_shoulder],
        // Transom.
        [port_quarter, heel, starboard_quarter],
    ]
}

/// The hull as a mesh: un-indexed with one normal per face, which is what makes
/// each facet a flat tone in the same way the terrain's are.
fn hull_mesh() -> Mesh {
    let faces = hull_faces();
    let mut positions = Vec::with_capacity(faces.len() * 3);
    let mut normals = Vec::with_capacity(faces.len() * 3);

    for face in faces {
        let normal = face_normal(&face);
        for corner in face {
            positions.push(corner.to_array());
            normals.push(normal.to_array());
        }
    }

    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
}

/// Which way a face points, from the order its corners are wound in.
fn face_normal(face: &[Vec3; 3]) -> Vec3 {
    (face[1] - face[0]).cross(face[2] - face[0]).normalize()
}

#[cfg(test)]
mod tests {
    use bevy::state::app::StatesPlugin;
    use bevy::time::TimePlugin;

    use super::*;

    /// A headless app with the boat systems running, already in a match, and a
    /// camera looking from `yaw` for the steering to read its directions off.
    /// The boat itself launches facing the default view regardless, which is
    /// what lets a turned camera and an unturned boat be set up against each
    /// other.
    fn test_app_looking(yaw: f32) -> App {
        let mut app = App::new();
        app.add_plugins((TimePlugin, StatesPlugin, BoatPlugin))
            .init_state::<AppState>()
            .init_resource::<View>()
            .init_resource::<KeyBindings>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>();
        app.world_mut()
            .spawn(MapCamera::looking(View { yaw, ..default() }));
        app.update();
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::InWorld);
        app.update();
        app
    }

    fn test_app() -> App {
        test_app_looking(View::default().yaw)
    }

    fn boat(app: &mut App) -> Transform {
        *app.world_mut()
            .query_filtered::<&Transform, With<Boat>>()
            .single(app.world())
            .expect("a match should have a boat in it")
    }

    /// The bow's bearing, in the same terms as a camera yaw — what
    /// `camera::forward` of it would face.
    fn heading_yaw(app: &mut App) -> f32 {
        let forward = boat(app).forward();
        f32::atan2(-forward.x, -forward.z)
    }

    fn hold(app: &mut App, key: KeyCode) {
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(key);
    }

    /// Puts an action on a key, as the controls screen does.
    fn rebind(app: &mut App, action: Action, key: KeyCode) {
        app.world_mut()
            .resource_mut::<KeyBindings>()
            .bind(action, key, None);
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

    /// Seconds of clock the app has run for. Frames take however long they
    /// take in a headless run, so anything driven by `delta_secs` has to be
    /// measured against the time that actually passed rather than a frame
    /// count.
    fn elapsed(app: &App) -> f32 {
        app.world().resource::<Time>().elapsed_secs()
    }

    /// Radians per second the bow comes round at in `app` while `key` is held.
    /// A rate rather than an angle — proportionality to how long the key was
    /// held is what makes a turn the same on any machine.
    fn turn_rate(mut app: App, key: KeyCode) -> f32 {
        let start_yaw = heading_yaw(&mut app);
        let before = elapsed(&app);
        hold(&mut app, key);
        run_frames(&mut app, 20);
        let seconds = elapsed(&app) - before;
        assert!(seconds > 0.0, "no time passed while the key was held");
        (heading_yaw(&mut app) - start_yaw) / seconds
    }

    #[test]
    fn every_hull_face_looks_outwards() {
        // Winding is invisible until something is drawn — a face wound the
        // wrong way round is simply culled, and the hull gets a hole in it that
        // only shows from one angle. So the whole hull is checked against a
        // point inside it: a closed convex-ish shell has every face pointing
        // away from its own middle.
        let faces = hull_faces();
        let corners: Vec<Vec3> = faces.iter().flatten().copied().collect();
        let middle = corners.iter().sum::<Vec3>() / corners.len() as f32;

        for face in faces {
            let outward = (face[0] + face[1] + face[2]) / 3.0 - middle;
            let normal = face_normal(&face);
            assert!(
                normal.dot(outward) > 0.0,
                "a face at {outward:?} from the middle points {normal:?}, which is inwards"
            );
        }
    }

    #[test]
    fn the_hull_is_a_flat_shaded_mesh() {
        let mesh = hull_mesh();
        assert_eq!(mesh.count_vertices(), hull_faces().len() * 3);
        assert!(
            mesh.indices().is_none(),
            "flat shading needs no index buffer"
        );
    }

    #[test]
    fn the_boat_points_its_bow_the_way_it_faces() {
        // What steering will be written against: -Z is the bow, so a boat
        // turned to a heading moves along its own forward.
        let mut app = test_app();
        let heading = boat(&mut app).forward();
        let yaw = app.world().resource::<View>().yaw;

        let expected = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
        assert!(
            (*heading - expected).length() < 1e-5,
            "a boat at yaw {yaw} faces {heading:?}, not {expected:?}"
        );
    }

    #[test]
    fn the_forward_key_drives_the_boat_the_way_the_view_faces() {
        let mut app = test_app();
        let before = boat(&mut app).translation;
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 20);
        let moved = boat(&mut app).translation - before;

        // Launched already facing the view's way, so no turning muddies it:
        // the whole of the movement is "away from the viewer".
        let forward = camera::forward(app.world().resource::<View>().yaw);
        assert!(moved.length() > 0.0, "the boat never moved");
        assert!(
            moved.normalize().dot(forward) > 0.999,
            "the boat went {moved:?} rather than along {forward:?}"
        );
        // And along the surface, not through it — height is `float`'s alone.
        assert_eq!(moved.y, 0.0);
    }

    #[test]
    fn under_way_the_boat_makes_its_speed() {
        let mut app = test_app();
        let before = boat(&mut app).translation;
        let start = elapsed(&app);
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 20);
        let seconds = elapsed(&app) - start;
        assert!(seconds > 0.0, "no time passed while the key was held");

        let made = (boat(&mut app).translation - before).length() / seconds;
        assert!(
            (made - SPEED).abs() < SPEED * 0.01,
            "the boat made {made} m/s, not {SPEED}"
        );
    }

    #[test]
    fn the_bow_comes_round_at_the_turn_rate() {
        // A sideways key asks for a direction square to the heading, so the
        // whole run is spent turning: the clamp is what's being measured.
        // Starboard is a negative turn, the same way round as the camera's E.
        let rate = turn_rate(test_app(), KeyCode::ArrowRight);
        let tolerance = TURN_RATE * 0.01;
        assert!(
            (rate + TURN_RATE).abs() < tolerance,
            "the bow came round at {rate} rad/s, not -{TURN_RATE}"
        );
    }

    #[test]
    fn steering_reads_its_directions_off_the_view() {
        // The camera looks a quarter-turn round from the way the boat faces,
        // so the *forward* key now asks for a turn: to port, the way that
        // brings the bow round to what the viewer sees as away. A boat
        // steering by its own frame would sail straight on instead.
        let quarter = std::f32::consts::FRAC_PI_2;
        let app = test_app_looking(View::default().yaw + quarter);
        let rate = turn_rate(app, KeyCode::ArrowUp);
        let tolerance = TURN_RATE * 0.01;
        assert!(
            (rate - TURN_RATE).abs() < tolerance,
            "the bow came round at {rate} rad/s, not {TURN_RATE}"
        );
    }

    #[test]
    fn opposed_keys_hold_the_boat_still() {
        let mut app = test_app();
        let before = boat(&mut app).translation;
        hold(&mut app, KeyCode::ArrowUp);
        hold(&mut app, KeyCode::ArrowDown);
        run_frames(&mut app, 20);
        assert_eq!(boat(&mut app).translation, before);
    }

    #[test]
    fn a_rebound_key_steers_and_the_key_it_replaced_stops() {
        let mut app = test_app();
        rebind(&mut app, Action::MoveForward, KeyCode::KeyJ);

        let before = boat(&mut app).translation;
        hold(&mut app, KeyCode::KeyJ);
        run_frames(&mut app, 20);
        assert_ne!(
            boat(&mut app).translation,
            before,
            "the newly bound key did not steer"
        );

        // J was nobody's key, so nothing was traded for it and W is now bound
        // to nothing at all. Holding it has to leave the boat where it lies.
        let mut app = test_app();
        rebind(&mut app, Action::MoveForward, KeyCode::KeyJ);
        let before = boat(&mut app).translation;
        hold(&mut app, KeyCode::KeyW);
        run_frames(&mut app, 20);
        assert_eq!(
            boat(&mut app).translation,
            before,
            "W still steers after being rebound away"
        );
    }

    #[test]
    fn the_arrow_keys_steer_whatever_the_bindings_say() {
        let mut app = test_app();
        // Hand every movement action to keys nowhere near the arrows.
        rebind(&mut app, Action::MoveForward, KeyCode::KeyI);
        rebind(&mut app, Action::MoveBack, KeyCode::KeyK);
        rebind(&mut app, Action::MoveLeft, KeyCode::KeyJ);
        rebind(&mut app, Action::MoveRight, KeyCode::KeyL);

        let before = boat(&mut app).translation;
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 20);
        assert_ne!(
            boat(&mut app).translation,
            before,
            "the arrow keys stopped steering once the letters moved"
        );
    }

    #[test]
    fn a_match_launches_the_boat_where_the_world_is_entered() {
        let mut app = test_app();
        app.insert_resource(View {
            focus: Vec3::new(98.0, 0.0, -317.0),
            ..default()
        });
        // Back to the menu and in again: entering is what launches a boat, and
        // the one from the last world went with it.
        for state in [AppState::MainMenu, AppState::InWorld] {
            app.world_mut()
                .resource_mut::<NextState<AppState>>()
                .set(state);
            app.update();
        }

        let at = boat(&mut app).translation;
        assert_eq!(Vec2::new(at.x, at.z), Vec2::new(98.0, -317.0));
    }

    #[test]
    fn the_boat_rides_the_surface_it_is_over() {
        use std::sync::Arc;
        use world::archipelago::{Archipelago, WorldConfig, CHUNK_METRES};

        // A world with one island already generated, so the ready-height
        // queries have something to answer with.
        let world = Arc::new(Archipelago::new(&WorldConfig { seed: 1 }));
        let spec = world
            .islands_within(Vec2::splat(-6_000.0), Vec2::splat(6_000.0))
            .into_iter()
            .max_by_key(|s| s.chunks.x * s.chunks.y)
            .expect("a world should have an island within a few kilometres");
        world.island(spec);

        let mut app = test_app();
        app.insert_resource(WorldTerrain(world.clone()));

        /// Where the boat comes to rest when it is put down at a spot.
        fn put_down(app: &mut App, spot: Vec2) -> f32 {
            let mut transform = app
                .world_mut()
                .query_filtered::<&mut Transform, With<Boat>>()
                .single_mut(app.world_mut())
                .expect("a match should have a boat in it");
            transform.translation.x = spot.x;
            transform.translation.z = spot.y;
            app.update();
            boat(app).translation.y
        }

        // Dry land: the highest ground on the island, where the answer is
        // furthest from the waterline.
        let centre = spec.centre();
        let mut peak = (centre, 0.0);
        let half = spec.extent() * 0.5;
        for iz in 0..24 {
            for ix in 0..24 {
                let spot = centre
                    + Vec2::new(
                        (ix as f32 / 23.0 * 2.0 - 1.0) * half.x,
                        (iz as f32 / 23.0 * 2.0 - 1.0) * half.y,
                    );
                let height = world.height(spot.x, spot.y);
                if height > peak.1 {
                    peak = (spot, height);
                }
            }
        }
        assert!(peak.1 > 0.0, "the island is entirely under water");
        assert_eq!(
            put_down(&mut app, peak.0),
            peak.1,
            "the boat is not sitting on the ground it is over"
        );

        // And the skirt, which is open water: the waterline exactly, however
        // deep the seabed under it.
        let offshore = centre + Vec2::new(half.x + CHUNK_METRES * 0.5, 0.0);
        assert!(
            world.height(offshore.x, offshore.y) < 0.0,
            "the point picked to be open water is dry land"
        );
        assert_eq!(
            put_down(&mut app, offshore),
            0.0,
            "the boat is not floating at the waterline"
        );
    }
}
