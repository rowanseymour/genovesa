//! The sheet the menus stand on.
//!
//! Every screen outside a world — the main menu and the three it leads to — is
//! drawn on the chart's own paper: ruled, netted with rhumbs thrown from roses
//! standing on its crossings, edged with the neatline, and lettered with a rose
//! in the corner. The menu itself is the cartouche an engraver would have put
//! the title in; see [`crate::menu`], which inks it.
//!
//! The menu used to stand on nothing — the world's clear colour with a dark
//! wash over it, the one surface in the game with no facets and no light on it.
//! The choice was between the two places the game already has, the water and
//! the paper; open water lost, there being no ground under it until a world has
//! been chosen.
//!
//! Paper wins for a reason beyond looking better: a chart is what the player
//! spends the game making, so the front of the game is the first sheet of it,
//! blank because nothing has been sailed yet. Which is also why none of the
//! drawing here is this module's own — the ruling, the net and the roses are
//! [`crate::chart`]'s, lent out. There is one sheet in this game, and two would
//! drift.
//!
//! Nothing here generates anything, opens a session or knows a seed. The paper
//! is a chart of nowhere.

use bevy::camera::{Camera, ClearColorConfig, RenderTarget};
use bevy::prelude::*;
use bevy::text::{FontSize, FontSource};

use crate::camera::MapCamera;
use crate::chart;
use crate::AppState;

/// Marks everything the sheet is made of, so all of it goes together.
#[derive(Component)]
struct Standing;

/// Marks the sheet's own camera, which the paper and its furniture hang off.
#[derive(Component)]
struct SheetCamera;

/// Marks the neatline and the paper that masks the engraving outside it —
/// both re-ruled when the window changes size, because both are *at* the
/// window while the paper slides under them.
#[derive(Component)]
struct SheetEdge;
#[derive(Component)]
struct SheetMask;

/// Marks the rose pinned in the sheet's corner.
#[derive(Component)]
struct SheetRose;

/// How much paper is drawn, in pixels. Generous rather than fitted to the
/// window: the sheet drifts, and a sheet built to the window would run out at
/// its edge — where the neatline is, which is the one place a chart must not
/// be seen ending.
const PAPER_EXTENT: Vec2 = Vec2::new(4200.0, 3000.0);

/// How far apart the sheet is ruled, in pixels. The chart rules in round
/// distances because it is a chart of somewhere; this is a chart of nowhere,
/// so it is ruled at the spacing the chart's own ruling comes out at.
const PAPER_SQUARE: f32 = 110.0;

/// How the sheet drifts: how far it wanders from where it started, in pixels,
/// and how long one turn of the wander takes, in seconds.
///
/// A circle rather than a slide, so the paper never leaves what was drawn of
/// it and nothing has to be rebuilt. Four minutes is slow enough that it never
/// reads as movement — what it reads as is a sheet lying on a table with
/// somebody's hand resting on it.
const DRIFT_REACH: f32 = 90.0;
const DRIFT_PERIOD: f32 = 240.0;

pub struct BackdropPlugin;

impl Plugin for BackdropPlugin {
    fn build(&self, app: &mut App) {
        // The sheet stands behind every screen that is not a world, so it is
        // raised and struck by the world rather than by any one of them. The
        // guard on `Startup` is for the runs that open straight into a world —
        // `--state inworld`, and every run that takes pictures — which would
        // otherwise engrave a whole sheet on their first frame and strike it
        // on their second.
        // Normally `UiPlugin`'s, initialised here the way the menu does its
        // shared resources, so the tests have one too.
        app.init_resource::<UiScale>()
            .add_systems(Startup, raise.run_if(not(in_state(AppState::InWorld))))
            .add_systems(OnEnter(AppState::InWorld), strike)
            .add_systems(OnExit(AppState::InWorld), raise)
            .add_systems(
                Update,
                (follow_the_target, drift_the_sheet, rule_the_sheet)
                    .run_if(not(in_state(AppState::InWorld))),
            );
    }
}

/// What it takes to lay the sheet: somewhere to spawn it, and the two stores it
/// is drawn out of. One parameter rather than three for the reason `chart`'s
/// `Engraver` is one — they are one job.
#[derive(bevy::ecs::system::SystemParam)]
struct Engraver<'w, 's> {
    commands: Commands<'w, 's>,
    meshes: ResMut<'w, Assets<Mesh>>,
    colours: ResMut<'w, Assets<ColorMaterial>>,
}

/// Lays the sheet, and holds the world's camera off it.
///
/// The world's camera draws nothing on a menu screen, so all its clear does
/// there is wipe the paper the moment after it is laid. Held off rather than
/// the sheet being drawn over the top of it: the UI is on the world's camera,
/// and moving the UI would be moving every menu in the game.
fn raise(
    mut engraver: Engraver,
    mut world_camera: Query<(&mut Camera, &RenderTarget), With<MapCamera>>,
) {
    let mut target = RenderTarget::default();
    for (mut camera, world_target) in &mut world_camera {
        camera.clear_color = ClearColorConfig::None;
        target = world_target.clone();
    }
    lay_the_sheet(&mut engraver, target);
}

/// Rolls it up, and gives the world's camera its clear back.
fn strike(
    mut commands: Commands,
    standing: Query<Entity, With<Standing>>,
    mut world_camera: Query<&mut Camera, With<MapCamera>>,
) {
    for entity in &standing {
        commands.entity(entity).despawn();
    }
    for mut camera in &mut world_camera {
        camera.clear_color = ClearColorConfig::Default;
    }
}

/// The sheet: a camera that clears to parchment, the engraving on it, and the
/// rose and the edge that make it a chart rather than a texture.
fn lay_the_sheet(engraver: &mut Engraver, target: RenderTarget) {
    let Engraver {
        commands,
        meshes,
        colours,
    } = engraver;
    let camera = commands
        .spawn((
            Name::new("Menu sheet"),
            Standing,
            SheetCamera,
            Camera2d,
            Camera {
                // Before the world's camera, which is where the menus draw:
                // the paper is what they are drawn *on*.
                order: -1,
                clear_color: ClearColorConfig::Custom(chart::PAPER),
                ..default()
            },
            target,
            Transform::default(),
        ))
        .id();

    let paper = chart::engraved_paper(
        Rect::from_center_size(Vec2::ZERO, PAPER_EXTENT),
        PAPER_SQUARE,
    );
    // In the order an engraver would have laid them: the net under the ruling
    // under the roses, each fainter than what runs over it.
    for (name, mesh, ink, z) in [
        ("Menu rhumbs", paper.net, chart::INK_GHOST, 0.0),
        ("Menu ruling", paper.ruling, chart::INK_FAINT, 0.1),
        ("Menu roses, lit", paper.lit, chart::INK_DIM, 0.2),
        ("Menu roses", paper.roses, chart::INK, 0.3),
    ] {
        commands.spawn((
            Name::new(name),
            Standing,
            Mesh2d(meshes.add(mesh)),
            MeshMaterial2d(colours.add(ColorMaterial::from_color(ink))),
            Transform::from_xyz(0.0, 0.0, z),
        ));
    }

    // The edge and the rose are children of the camera, which is what pins
    // them to the window while the paper slides under them. Their meshes are
    // written over in place when the window changes size — see
    // [`rule_the_sheet`] — so they are spawned blank.
    let mut blank = |name: &'static str, ink: Color, z: f32| {
        (
            Name::new(name),
            Mesh2d(meshes.add(Mesh::from(Rectangle::new(0.0, 0.0)))),
            MeshMaterial2d(colours.add(ColorMaterial::from_color(ink))),
            Transform::from_xyz(0.0, 0.0, z),
            ChildOf(camera),
        )
    };
    commands.spawn((
        SheetMask,
        blank("Menu sheet edge, masked", chart::PAPER, 2.4),
    ));
    commands.spawn((SheetEdge, blank("Menu sheet edge", chart::INK, 2.5)));

    let rose = commands
        .spawn((
            Name::new("Menu rose"),
            SheetRose,
            Transform::default(),
            Visibility::Visible,
            ChildOf(camera),
        ))
        .id();
    let (inked, dimmed) = chart::drawn_rose();
    for (mesh, ink) in [(inked, chart::INK), (dimmed, chart::INK_DIM)] {
        commands.spawn((
            Mesh2d(meshes.add(mesh)),
            MeshMaterial2d(colours.add(ColorMaterial::from_color(ink))),
            Transform::from_xyz(0.0, 0.0, 2.6),
            ChildOf(rose),
        ));
    }
    // N in the reading ink and the other three dimmed, as on the chart's own
    // corner rose and for the same reason: they are what make N mean north
    // rather than "this way".
    for (letter, turns) in chart::ROSE_LETTERING {
        commands.spawn((
            Text2d::new(letter),
            TextFont {
                font: FontSource::Serif,
                font_size: FontSize::Px(chart::ROSE_LETTER_SIZE),
                ..default()
            },
            TextColor(if turns == 0.0 {
                chart::INK
            } else {
                chart::INK_DIM
            }),
            Transform::from_translation(
                (chart::on_the_card(turns) * chart::ROSE_LETTER_OUT).extend(2.7),
            ),
            ChildOf(rose),
        ));
    }
}

/// Keeps the sheet's camera pointed wherever the world's is.
///
/// A run taking pictures draws to an off-screen image rather than to a window,
/// and is told so after the cameras exist — so the sheet cannot be given its
/// target when it is laid, and has to follow.
fn follow_the_target(
    world: Query<Ref<RenderTarget>, (With<MapCamera>, Without<SheetCamera>)>,
    mut sheet: Query<(&mut RenderTarget, Ref<Camera>), With<SheetCamera>>,
) {
    let Ok(world) = world.single() else {
        return;
    };
    for (mut target, camera) in &mut sheet {
        // A target is a texture handle with no equality to compare, so the
        // copy is made when there is a reason to rather than when it would
        // change anything: the world's target being rewritten, or this sheet
        // being new and never having had one.
        if world.is_changed() || camera.is_added() {
            *target = (*world).clone();
        }
    }
}

/// Slides the paper under the window.
fn drift_the_sheet(time: Res<Time>, mut sheets: Query<&mut Transform, With<SheetCamera>>) {
    let turns = time.elapsed_secs() / DRIFT_PERIOD * std::f32::consts::TAU;
    for mut transform in &mut sheets {
        transform.translation.x = turns.cos() * DRIFT_REACH;
        transform.translation.y = turns.sin() * DRIFT_REACH;
    }
}

/// Re-rules the sheet's edge and re-corners its rose when the window changes —
/// and keeps the engraving in step with the UI's own scale.
///
/// The menus scale to the window — see [`crate::settings`] — and the sheet has
/// to scale *with* them: the cartouche stands off the neatline by a margin the
/// two agree on in pixels, and a menu that shrank over paper that did not
/// would put its rule on the sheet's own. So the sheet is drawn in pixels of
/// the size the menus were laid out for, and the camera shows it enlarged or
/// reduced exactly as the UI is. Nothing on this paper measures anything, so
/// nothing is put wrong by that — unlike the chart's own sheet, which holds
/// its pixels and pins its furniture to them instead.
fn rule_the_sheet(
    mut meshes: ResMut<Assets<Mesh>>,
    mut ruled: Local<Option<Vec2>>,
    scale: Res<UiScale>,
    mut cameras: Query<(&Camera, &mut Projection), With<SheetCamera>>,
    edges: Query<&Mesh2d, With<SheetEdge>>,
    masks: Query<&Mesh2d, With<SheetMask>>,
    mut roses: Query<&mut Transform, With<SheetRose>>,
) {
    let Ok((camera, mut projection)) = cameras.single_mut() else {
        return;
    };
    let Some(window) = camera.logical_viewport_size() else {
        return;
    };
    if let Projection::Orthographic(ortho) = &mut *projection {
        let wanted = 1.0 / scale.0;
        if ortho.scale != wanted {
            ortho.scale = wanted;
        }
    }
    // The window in the sheet's pixels, which is what everything below is
    // ruled and cornered in.
    let size = window / scale.0;
    if *ruled == Some(size) {
        return;
    }
    *ruled = Some(size);

    let window = Rect::from_center_size(Vec2::ZERO, size);
    let (edge, mask) = chart::ruled_edge(window, PAPER_SQUARE / chart::NEATLINE_CELLS);
    for (drawn, ruled) in [(edges.iter().next(), edge), (masks.iter().next(), mask)] {
        if let Some(mut slot) = drawn.and_then(|drawn| meshes.get_mut(&drawn.0)) {
            *slot = ruled;
        }
    }

    let inset = chart::ROSE_EXTENT + chart::PAPER_MARGIN;
    for mut transform in &mut roses {
        transform.translation.x = size.x / 2.0 - inset;
        transform.translation.y = -(size.y / 2.0) + inset;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FRAME;
    use bevy::state::app::StatesPlugin;
    use bevy::time::{TimePlugin, TimeUpdateStrategy};

    /// A headless app on a menu screen, with a camera for the sheet to take
    /// its target from — the map camera being what the sheet follows, and
    /// [`crate::camera`]'s plugin wanting a good deal more of the app than
    /// this needs.
    fn menu_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            TaskPoolPlugin::default(),
            AssetPlugin::default(),
            TimePlugin,
            StatesPlugin,
            BackdropPlugin,
        ))
        .init_state::<AppState>()
        .init_asset::<Mesh>()
        .init_asset::<ColorMaterial>()
        // Headless frames take no real time, and the drift is a curve in
        // seconds — so the clock is stepped by hand.
        .insert_resource(TimeUpdateStrategy::ManualDuration(FRAME))
        .world_mut()
        .spawn((
            MapCamera::default(),
            Camera::default(),
            RenderTarget::default(),
        ));
        app.update();
        app
    }

    fn sheets(app: &mut App) -> usize {
        app.world_mut()
            .query_filtered::<(), With<SheetCamera>>()
            .iter(app.world())
            .count()
    }

    fn enter(app: &mut App, state: AppState) {
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(state);
        app.update();
    }

    #[test]
    fn a_menu_stands_on_a_sheet_and_a_world_takes_it_away() {
        // And the world gets its clear back with it: the sheet is only visible
        // because the world's camera has been told not to wipe it, so a world
        // entered without that being undone would draw over nothing.
        let mut app = menu_app();
        assert_eq!(sheets(&mut app), 1, "the menu was left standing on nothing");

        enter(&mut app, AppState::InWorld);
        assert_eq!(sheets(&mut app), 0, "the sheet was carried into the world");
        let world_camera = app
            .world_mut()
            .query_filtered::<&Camera, With<MapCamera>>()
            .single(app.world())
            .expect("the app has a camera")
            .clear_color;
        assert!(
            matches!(world_camera, ClearColorConfig::Default),
            "the world's camera never got its clear back"
        );
    }

    #[test]
    fn leaving_a_world_lays_the_sheet_again() {
        // One sheet, not two: leaving and entering repeatedly is what a player
        // trying seeds does, and a sheet left behind each time would pile
        // parchment up until the frame rate went.
        let mut app = menu_app();
        for _ in 0..3 {
            enter(&mut app, AppState::InWorld);
            enter(&mut app, AppState::MainMenu);
        }
        assert_eq!(
            sheets(&mut app),
            1,
            "the menu came back to the wrong number of sheets"
        );
    }

    /// Where the sheet has slid to.
    fn drifted(app: &mut App) -> Vec2 {
        app.world_mut()
            .query_filtered::<&Transform, With<SheetCamera>>()
            .single(app.world())
            .expect("a menu has a sheet")
            .translation
            .truncate()
    }

    #[test]
    fn the_paper_slides_but_never_runs_out() {
        // Two halves of one promise. The sheet moves — a menu left up is a
        // chart under somebody's hand, not a picture — and however long it is
        // left, what was engraved still reaches past the window on every side,
        // since the neatline is the one place a chart must not be seen ending.
        let mut app = menu_app();
        let laid = drifted(&mut app);
        for _ in 0..60 {
            app.update();
        }
        assert_ne!(drifted(&mut app), laid, "the sheet is nailed down");

        let window = Vec2::new(crate::WINDOW.x as f32, crate::WINDOW.y as f32);
        assert!(
            PAPER_EXTENT
                .cmpgt(window + Vec2::splat(2.0 * DRIFT_REACH))
                .all(),
            "a drifted sheet can run out inside the window"
        );
    }
}
