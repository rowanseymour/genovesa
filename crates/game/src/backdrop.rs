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
/// reduced as the UI is — as far as there is paper to show. Nothing on this
/// paper measures anything, so nothing is put wrong by that — unlike the
/// chart's own sheet, which holds its pixels and pins its furniture to them
/// instead.
fn rule_the_sheet(
    mut meshes: ResMut<Assets<Mesh>>,
    mut ruled: Local<Option<(Vec2, AssetId<Mesh>)>>,
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
    let (Ok(rules), Ok(cover)) = (edges.single(), masks.single()) else {
        return;
    };

    // How much paper the camera shows: the UI's scale, so that the engraving
    // keeps step with the menus standing on it — but never more of it than was
    // engraved. Nothing caps how large the UI goes, and a window shaped past
    // anything a display is (a rotated ultrawide, a sliver dragged tall) asks
    // to be shown a stretch of paper in one direction that the sheet was never
    // drawn to reach. Held here rather than answered with a bigger sheet: past
    // this the menu is very slightly out of proportion to the ruling under it,
    // which nothing on this paper measures, where the alternative is a chart
    // with its edge in the middle of the window.
    let room = (PAPER_EXTENT - Vec2::splat(2.0 * DRIFT_REACH)) / window;
    let showing = (1.0 / scale.0).min(room.min_element());
    // Read past change detection and written only where it differs: a `Mut`
    // counts as changed the moment it is dereferenced, so the comparison alone
    // would put the camera through its recompute on every frame of every menu.
    let moved = match projection.bypass_change_detection() {
        Projection::Orthographic(ortho) if ortho.scale != showing => {
            ortho.scale = showing;
            true
        }
        _ => false,
    };
    if moved {
        projection.set_changed();
    }

    // The window in the sheet's pixels, which is what everything below is
    // ruled and cornered in.
    let size = window * showing;
    // Remembered against the meshes as well as the size, because the sheet is
    // struck and laid again on every visit to a world and the new one's edge
    // and mask are spawned blank: a memo that only knew the size would find
    // the window unchanged and leave every sheet after the first with no
    // neatline, and its ruling and net running out to the window's own edge.
    if *ruled == Some((size, rules.0.id())) {
        return;
    }
    *ruled = Some((size, rules.0.id()));

    let window = Rect::from_center_size(Vec2::ZERO, size);
    let (edge, mask) = chart::ruled_edge(window, PAPER_SQUARE / chart::NEATLINE_CELLS);
    for (drawn, ruled) in [(rules, edge), (cover, mask)] {
        if let Some(mut slot) = meshes.get_mut(&drawn.0) {
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
    use bevy::camera::primitives::MeshAabb;
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

    /// Gives the sheet's camera something to be drawn to, the way the renderer
    /// would once there is a window: so many physical pixels at so many of
    /// them to the logical one. Without it a camera can say nothing about how
    /// big it is, and everything that rules the sheet stands down.
    fn on_a_window(app: &mut App, logical: Vec2, factor: f32) {
        let mut cameras = app
            .world_mut()
            .query_filtered::<&mut Camera, With<SheetCamera>>();
        let mut camera = cameras.single_mut(app.world_mut()).expect("a sheet");
        camera.computed.target_info = Some(bevy::camera::RenderTargetInfo {
            physical_size: (logical * factor).as_uvec2(),
            scale_factor: factor,
        });
    }

    /// How far the neatline reaches across the sheet. A sheet's edge is spawned
    /// as an empty rectangle and written over once the window is known, so an
    /// edge that reaches nowhere is an edge that was never ruled.
    fn neatline(app: &mut App) -> Vec2 {
        let drawn = app
            .world_mut()
            .query_filtered::<&Mesh2d, With<SheetEdge>>()
            .single(app.world())
            .expect("a sheet has an edge")
            .0
            .clone();
        app.world()
            .resource::<Assets<Mesh>>()
            .get(&drawn)
            .expect("the edge was drawn out of something")
            .compute_aabb()
            .map(|drawn| drawn.half_extents.truncate() * 2.0)
            .unwrap_or_default()
    }

    #[test]
    fn a_sheet_laid_again_is_ruled_again() {
        // The neatline and the paper that masks the engraving outside it are
        // spawned blank and written when the window is first known — so a
        // sheet laid a second time, after a world has been in and out, has to
        // be ruled a second time as well. Remembering only what size the last
        // one was ruled to, the window would be found unchanged and every menu
        // after the first world would stand on paper with no edge to it, its
        // ruling and net running out inside the window.
        let mut app = menu_app();
        let window = Vec2::new(1280.0, 720.0);
        on_a_window(&mut app, window, 1.0);
        app.update();
        let ruled = neatline(&mut app);
        assert!(
            ruled.cmpgt(window / 2.0).all(),
            "the first sheet was never ruled"
        );

        enter(&mut app, AppState::InWorld);
        enter(&mut app, AppState::MainMenu);
        on_a_window(&mut app, window, 1.0);
        app.update();
        assert_eq!(
            neatline(&mut app),
            ruled,
            "the sheet came back with no edge"
        );
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

    /// How much sheet the camera is showing in one pixel of window.
    fn showing(app: &mut App) -> f32 {
        let projection = app
            .world_mut()
            .query_filtered::<&Projection, With<SheetCamera>>()
            .single(app.world())
            .expect("a menu has a sheet")
            .clone();
        match projection {
            Projection::Orthographic(ortho) => ortho.scale,
            _ => panic!("the sheet is drawn flat"),
        }
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

        // The window is not the measure of that any more: the camera shows the
        // sheet at the UI's scale, so what has to fit inside the engraving is
        // whatever the camera has been zoomed out to — see [`rule_the_sheet`],
        // which is what holds it. Watched over windows shaped past anything a
        // display is, at scales either side of one, because those are the two
        // ways the shown stretch grows.
        for logical in [
            Vec2::new(1280.0, 720.0),
            Vec2::new(3440.0, 1440.0),
            Vec2::new(1440.0, 3440.0),
            Vec2::new(640.0, 2000.0),
            Vec2::new(400.0, 300.0),
        ] {
            for scale in [0.5, 1.0, 2.0, 3.0] {
                app.world_mut().resource_mut::<UiScale>().0 = scale;
                on_a_window(&mut app, logical, 1.0);
                app.update();

                let shown = logical * showing(&mut app) + Vec2::splat(2.0 * DRIFT_REACH);
                assert!(
                    shown.cmple(PAPER_EXTENT).all(),
                    "a {logical} window at {scale} shows {shown} of a {PAPER_EXTENT} sheet"
                );
            }
        }
    }
}
