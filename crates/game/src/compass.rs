//! The compass: a small card in a corner of the screen with its N pinned to
//! the world's north.
//!
//! Both the boat and the view can turn, so neither would make a steady
//! compass: a card fixed to the bow would spin through every tack, and one
//! fixed to the screen would say nothing at all. The card is aligned with the
//! world instead — however the view is spun, its N sits over
//! [`protocol::ground::NORTH`], the direction a map puts at the top of the
//! page — so a bearing read off it means the same thing to every player in
//! the world, whichever way each of them happens to be looking.
//!
//! It reads the camera's eased yaw rather than the target, so the card swings
//! with the picture through a turn instead of arriving ahead of it.
//!
//! The card is drawn *lying on the sea* rather than flat on the glass: a
//! bearing on it is meant to be carried out into the picture, and against
//! ground drawn at the camera's pitch an unforeshortened dial reads as a
//! sticker on the screen instead of a direction in the world. The tilt is the
//! projection itself — a flat card on the ground plane, seen from
//! [`PITCH`][crate::camera::PITCH] above horizontal, is its upright drawing
//! squashed vertically by `sin(PITCH)`, applied *after* the card's own spin
//! so the letters shear the way paint on a deck would. Real 3D geometry
//! parented to the camera was rejected: over a dial this size it differs from
//! the squash only by a keystone too small to see, and it would need letters
//! as meshes and an exemption from the fog, the lighting and the terrain's
//! occlusion to survive drawing at all.

use bevy::prelude::*;
use bevy::text::{FontSize, FontSource};
use bevy::ui::UiTransform;

use protocol::ground::NORTH;

use crate::camera::{MapCamera, PITCH};
use crate::AppState;

/// Diameter of the face, in pixels — an instrument, not a map: big enough to
/// read a letter off at a glance, small enough to sit in the corner unnoticed.
const FACE_SIZE: f32 = 64.0;
/// How far the face sits in from the corner of the window.
const MARGIN: f32 = 12.0;
/// The cardinal letters' size.
const LETTER_SIZE: f32 = 13.0;
/// How far the letters sit in from the rim.
const INSET: f32 = 4.0;
/// How long each arm of the centre cross runs, short of the letters.
const CROSS_ARM: f32 = 14.0;

// The same furniture the menus are drawn with — one edge colour, one ink, one
// dimmed ink — so the instrument reads as a piece of the same chart.
const FACE: Color = Color::srgba(0.09, 0.11, 0.10, 0.60);
const EDGE: Color = Color::srgb(0.70, 0.69, 0.62);
const INK: Color = Color::srgb(0.88, 0.87, 0.80);
const INK_DIM: Color = Color::srgb(0.60, 0.60, 0.55);
/// The cross is furniture behind the letters, not a reading, so it is fainter
/// than either ink.
const CROSS: Color = Color::srgba(0.60, 0.60, 0.55, 0.45);

pub struct CompassPlugin;

impl Plugin for CompassPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::InWorld), spawn_compass)
            .add_systems(Update, turn_card.run_if(in_state(AppState::InWorld)));
    }
}

/// Marks the rotating card inside the face, so [`turn_card`] can find it.
#[derive(Component)]
struct CompassCard;

/// Marks the face — the tilted, stationary dial the card spins inside.
#[derive(Component)]
struct CompassFace;

fn spawn_compass(mut commands: Commands) {
    commands
        .spawn((
            Name::new("Compass"),
            CompassFace,
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(MARGIN),
                bottom: Val::Px(MARGIN),
                width: Val::Px(FACE_SIZE),
                height: Val::Px(FACE_SIZE),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::MAX,
                ..default()
            },
            BackgroundColor(FACE),
            BorderColor::all(EDGE),
            // The tilt. This node squashes and the card inside it spins, and
            // the nesting is what orders the two: a UI node's transform
            // composes onto its children's, so the rotation happens in the
            // card's own plane and the foreshortening projects the result —
            // which is how the world's ground reaches the screen too.
            UiTransform {
                scale: Vec2::new(1.0, PITCH.sin()),
                ..UiTransform::IDENTITY
            },
            DespawnOnExit(AppState::InWorld),
        ))
        .with_children(|face| {
            face.spawn((
                CompassCard,
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                UiTransform::IDENTITY,
            ))
            .with_children(|card| {
                spawn_cross(card);
                // The other three letters are dimmed rather than dropped:
                // they make N mean north rather than "this way", and a turn
                // read against them says how far round it went.
                spawn_letter(card, "N", INK, JustifyContent::Center, AlignItems::Start);
                spawn_letter(card, "E", INK_DIM, JustifyContent::End, AlignItems::Center);
                spawn_letter(card, "S", INK_DIM, JustifyContent::Center, AlignItems::End);
                spawn_letter(
                    card,
                    "W",
                    INK_DIM,
                    JustifyContent::Start,
                    AlignItems::Center,
                );
            });
        });
}

/// One cardinal letter, placed by alignment rather than arithmetic: each sits
/// in its own full-size overlay, pushed to its edge of the card, so nothing
/// here needs to know how wide a glyph came out.
fn spawn_letter(
    card: &mut ChildSpawnerCommands,
    letter: &str,
    ink: Color,
    justify: JustifyContent,
    align: AlignItems,
) {
    card.spawn(Node {
        position_type: PositionType::Absolute,
        width: Val::Percent(100.0),
        height: Val::Percent(100.0),
        padding: UiRect::all(Val::Px(INSET)),
        justify_content: justify,
        align_items: align,
        ..default()
    })
    .with_children(|spot| {
        spot.spawn((
            Text::new(letter),
            // The serif the menus resolve, for the same reason they do: the
            // card is a piece of chart furniture, and the machine's serif is
            // the hand charts are lettered in.
            TextFont {
                font: FontSource::Serif,
                font_size: FontSize::Px(LETTER_SIZE),
                ..default()
            },
            TextColor(ink),
        ));
    });
}

/// The hairline cross behind the letters — the rose's arms, drawn as two
/// centred lines so the card reads as an instrument rather than four
/// floating letters.
fn spawn_cross(card: &mut ChildSpawnerCommands) {
    for (width, height) in [(1.0, CROSS_ARM * 2.0), (CROSS_ARM * 2.0, 1.0)] {
        card.spawn(Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        })
        .with_children(|spot| {
            spot.spawn((
                Node {
                    width: Val::Px(width),
                    height: Val::Px(height),
                    ..default()
                },
                BackgroundColor(CROSS),
            ));
        });
    }
}

/// How the card is turned for a camera yaw: the rotation that lays the N
/// drawn at its top along north as the screen currently shows it.
///
/// The camera looks along `-(sin yaw, cos yaw)` on the ground plane, and its
/// screen-right is that a quarter turn on, `(cos yaw, -sin yaw)`. North's
/// bearing on screen — clockwise from straight up — is then the angle whose
/// cosine is north's share of forward and whose sine its share of right.
/// [`UiTransform`] rotations are clockwise too, so the angle is used as it
/// comes. (For north's actual `-Z` the whole thing collapses to the yaw
/// itself; the derivation is spelled out because the sign conventions are
/// exactly where a compass goes quietly wrong.)
fn card_rotation(yaw: f32) -> Rot2 {
    let forward = Vec2::new(-yaw.sin(), -yaw.cos());
    let right = Vec2::new(yaw.cos(), -yaw.sin());
    Rot2::radians(f32::atan2(NORTH.dot(right), NORTH.dot(forward)))
}

fn turn_card(cameras: Query<&MapCamera>, mut cards: Query<&mut UiTransform, With<CompassCard>>) {
    let Ok(camera) = cameras.single() else {
        return;
    };
    for mut transform in &mut cards {
        transform.rotation = card_rotation(camera.yaw);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::MapCameraPlugin;
    use bevy::input::mouse::AccumulatedMouseScroll;
    use bevy::state::app::StatesPlugin;
    use bevy::time::TimePlugin;

    /// A headless app with the camera to read and the compass to point,
    /// already dropped into a match.
    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins((TimePlugin, StatesPlugin, MapCameraPlugin, CompassPlugin))
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

    /// Reads the one card's rotation.
    fn rotation(app: &mut App) -> Rot2 {
        app.world_mut()
            .query_filtered::<&UiTransform, With<CompassCard>>()
            .single(app.world())
            .expect("the compass card should exist")
            .rotation
    }

    /// Turns the camera to a yaw outright, eased value and target both.
    fn spin_camera(app: &mut App, yaw: f32) {
        let mut camera = app
            .world_mut()
            .query::<&mut MapCamera>()
            .single_mut(app.world_mut())
            .expect("camera should exist");
        let view = crate::camera::View {
            yaw,
            ..Default::default()
        };
        camera.snap_to(view);
    }

    #[test]
    fn north_is_up_when_facing_north() {
        // Yaw zero looks along -Z, which is north itself: the card has
        // nothing to correct.
        assert!(card_rotation(0.0).angle_to(Rot2::IDENTITY).abs() < 1e-5);
    }

    #[test]
    fn north_is_to_the_right_when_facing_west() {
        // Looking west puts north on the camera's right hand, so the card
        // turns a quarter clockwise to lay its N there.
        let turned = card_rotation(std::f32::consts::FRAC_PI_2);
        assert!(
            turned
                .angle_to(Rot2::radians(std::f32::consts::FRAC_PI_2))
                .abs()
                < 1e-5
        );
    }

    #[test]
    fn unbounded_yaw_reads_like_its_bearing() {
        // The camera's yaw runs unbounded rather than wrapping — see
        // `MapCamera::yaw` — so whole turns must fall out of the reading.
        let a = card_rotation(0.4);
        let b = card_rotation(0.4 + std::f32::consts::TAU);
        assert!(a.angle_to(b).abs() < 1e-4);
    }

    #[test]
    fn lies_at_the_worlds_pitch() {
        let mut app = test_app();
        let scale = app
            .world_mut()
            .query_filtered::<&UiTransform, With<CompassFace>>()
            .single(app.world())
            .expect("the face should exist")
            .scale;
        assert_eq!(scale.x, 1.0);
        assert!((scale.y - PITCH.sin()).abs() < 1e-6);
    }

    #[test]
    fn card_follows_the_camera() {
        let mut app = test_app();
        spin_camera(&mut app, 1.25);
        // Two frames: one for the camera to settle, one for the card to have
        // certainly read it, whichever order the two systems ran in.
        app.update();
        app.update();
        assert!(rotation(&mut app).angle_to(card_rotation(1.25)).abs() < 1e-5);
    }

    #[test]
    fn only_exists_in_world() {
        let mut app = test_app();
        assert_eq!(
            app.world_mut()
                .query::<&CompassCard>()
                .iter(app.world())
                .count(),
            1
        );

        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::MainMenu);
        app.update();
        assert_eq!(
            app.world_mut()
                .query::<&CompassCard>()
                .iter(app.world())
                .count(),
            0
        );
    }
}
