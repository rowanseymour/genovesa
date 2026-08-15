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
//!
//! The card carries a second reading: an arrow lying along the wind. It is
//! here rather than in a panel of its own because a wind is only ever wanted
//! *against* something — the way home, the way the boat is pointed — and both
//! of those are bearings. One card holding north and the wind together
//! answers "the wind is off my starboard bow" in a glance, where two
//! instruments would leave the player doing the subtraction. That it costs
//! nothing to draw is the smaller half of the argument.

use bevy::prelude::*;
use bevy::text::{FontSize, FontSource};
use bevy::ui::{UiTransform, Val2};

use protocol::ground::NORTH;

use crate::camera::{MapCamera, PITCH};
use crate::sea::SeaConditions;
use crate::AppState;

/// Diameter of the face, in pixels — an instrument, not a map: big enough to
/// read a letter off at a glance, small enough to sit in the corner unnoticed.
/// The letters take a fixed bite out of the rim whatever the face is, so this
/// is really the size of what is left in the middle: at sixty-four there was
/// no room to lay an arrow across the card without it fouling them.
const FACE_SIZE: f32 = 88.0;
/// How far the face sits in from the corner of the window.
const MARGIN: f32 = 12.0;
/// The cardinal letters' size.
const LETTER_SIZE: f32 = 13.0;
/// How far the letters sit in from the rim.
const INSET: f32 = 4.0;
/// How long each arm of the centre cross runs, short of the letters.
const CROSS_ARM: f32 = 14.0;

/// The wind the arm is drawn at its full reach for, in metres per second — a
/// fresh breeze rather than the hardest wind there is. Where the scale ends
/// is the client's to pick: the server sends a velocity and has no opinion
/// about how hard that should look, and a scale that only filled at a rare
/// gale would spend most of a day reading as a light air. Above this the arm
/// has nowhere further to go, which is the right lie for an instrument this
/// size — past a fresh breeze the sea itself is saying the rest.
const FULL_WIND: f32 = 10.0;

/// Below this, in metres per second, there is no wind worth naming and the
/// arm goes off the card. A dying wind's bearing is noise — the weather's
/// calms are its walk passing the origin, where the direction swings freely —
/// so the arm holds the last bearing it had and simply fades, which reads as
/// the wind dropping rather than as the instrument spinning. Half a metre a
/// second, the speed the sea stops re-aiming its waves at, so the card and
/// the water call a calm at the same moment.
const CALM: f32 = 0.5;

/// How far the arm reaches from the centre, in pixels: at a calm, and at
/// [`FULL_WIND`]. The far end stops short of the letters — the arm is a
/// reading laid over the rose, not a hand touching its rim — and the near end
/// is a stub rather than nothing, so a dying wind shrinks towards a point
/// while the fade takes it.
const ARM: (f32, f32) = (10.0, 25.0);

/// The arm's thickness, in pixels: heavier than the cross, which is
/// furniture, and lighter than a letter.
const ARM_WIDTH: f32 = 2.0;

/// Each barb of the arrowhead: how long it runs, in pixels, and how far off
/// the shaft it is turned, in radians.
const BARB: (f32, f32) = (7.0, 0.55);

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
        app.init_resource::<SeaConditions>()
            .add_systems(OnEnter(AppState::InWorld), spawn_compass)
            .add_systems(
                Update,
                (turn_card, point_the_arm).run_if(in_state(AppState::InWorld)),
            );
    }
}

/// Marks the rotating card inside the face, so [`turn_card`] can find it.
#[derive(Component)]
struct CompassCard;

/// Marks the face — the tilted, stationary dial the card spins inside.
#[derive(Component)]
struct CompassFace;

/// Marks the wind arm — the arrow that spins inside the card, on top of the
/// card's own spin, so the bearing it shows is the world's and not the view's.
#[derive(Component)]
struct WindArm;

/// Marks the arm's shaft, the one piece of it that changes length.
#[derive(Component)]
struct WindShaft;

/// Marks every piece the arm is drawn from — shaft and barbs alike — so the
/// whole arrow can be inked in one pass without naming its parts again.
#[derive(Component)]
struct WindInk;

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
                let rose = Rose {
                    ink: INK,
                    dim: INK_DIM,
                    cross: CROSS,
                    letters: LETTER_SIZE,
                    inset: INSET,
                    arm: CROSS_ARM,
                };
                // The rose in two halves with the wind arm between them:
                // under the letters rather than over them, because where the
                // arm reaches its furthest it is nearly touching one, and an
                // arrowhead drawn across a glyph would cost the letter more
                // than it bought the arm. The chart's rose carries no arm and
                // draws both halves in one stroke — see [`Rose::draw`].
                rose.cross(card);
                spawn_arm(card);
                rose.letters(card);
            });
        });
}

/// What is drawn on a compass card: four cardinal letters and the hairline
/// cross behind them.
///
/// Written once and drawn twice, because the app has two instruments that are
/// the same instrument seen differently — this one, which lies foreshortened on
/// the sea and spins with the view, and the chart's, which lies flat on paper
/// and never moves at all. What differs between them is the ink they are drawn
/// in and how big; what must not differ is which letter goes where, which is
/// what a second copy of this would eventually get wrong.
pub struct Rose {
    /// The N, which is the reading.
    pub ink: Color,
    /// The other three, dimmed rather than dropped: they make N mean north
    /// rather than "this way", and a turn read against them says how far round
    /// it went.
    pub dim: Color,
    /// The arms, which are furniture behind the letters rather than a reading,
    /// so fainter than either ink.
    pub cross: Color,
    /// How big a letter is set, and how far in from the rim it sits.
    pub letters: f32,
    pub inset: f32,
    /// How far each arm of the cross runs from the middle, short of the
    /// letters.
    pub arm: f32,
}

impl Rose {
    /// The whole rose in one stroke, for a card with no other reading to
    /// interleave. The compass calls the halves itself instead, its wind arm
    /// belonging between them.
    pub fn draw(&self, card: &mut ChildSpawnerCommands) {
        self.cross(card);
        self.letters(card);
    }

    /// The four cardinal letters, N in the reading ink and the rest dimmed —
    /// see [`Rose::dim`] for why they are there at all.
    fn letters(&self, card: &mut ChildSpawnerCommands) {
        self.letter(
            card,
            "N",
            self.ink,
            JustifyContent::Center,
            AlignItems::Start,
        );
        self.letter(card, "E", self.dim, JustifyContent::End, AlignItems::Center);
        self.letter(card, "S", self.dim, JustifyContent::Center, AlignItems::End);
        self.letter(
            card,
            "W",
            self.dim,
            JustifyContent::Start,
            AlignItems::Center,
        );
    }

    /// One cardinal letter, placed by alignment rather than arithmetic: each
    /// sits in its own full-size overlay, pushed to its edge of the card, so
    /// nothing here needs to know how wide a glyph came out.
    fn letter(
        &self,
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
            padding: UiRect::all(Val::Px(self.inset)),
            justify_content: justify,
            align_items: align,
            ..default()
        })
        .with_children(|spot| {
            spot.spawn((
                Text::new(letter),
                // The serif the menus resolve, for the same reason they do: a
                // card is a piece of chart furniture, and the machine's serif
                // is the hand charts are lettered in.
                TextFont {
                    font: FontSource::Serif,
                    font_size: FontSize::Px(self.letters),
                    ..default()
                },
                TextColor(ink),
            ));
        });
    }

    /// The hairline cross behind the letters — the rose's arms, drawn as two
    /// centred lines so the card reads as an instrument rather than as four
    /// floating letters.
    fn cross(&self, card: &mut ChildSpawnerCommands) {
        for (width, height) in [(1.0, self.arm * 2.0), (self.arm * 2.0, 1.0)] {
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
                    BackgroundColor(self.cross),
                ));
            });
        }
    }
}

/// The wind arm: an arrow lying on the card, spun to the wind's bearing by
/// [`point_the_arm`].
///
/// A child of the card rather than of the face, which is what makes it a
/// *bearing* rather than a picture of where the wind is on screen: the card
/// already carries the turn from the world to the view, so this node's own
/// rotation is the wind's angle from north and nothing else, and the two
/// compose the way they do on paper.
fn spawn_arm(card: &mut ChildSpawnerCommands) {
    card.spawn((
        WindArm,
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        UiTransform::IDENTITY,
    ))
    .with_children(|arm| {
        arm.spawn((
            WindShaft,
            WindInk,
            Node {
                width: Val::Px(ARM_WIDTH),
                height: Val::Px(ARM.0),
                ..default()
            },
            BackgroundColor(INK),
            UiTransform::IDENTITY,
        ))
        .with_children(|shaft| {
            spawn_barb(shaft, 1.0);
            spawn_barb(shaft, -1.0);
        });
    });
}

/// One barb of the arrowhead, hung off the shaft's point.
///
/// A child of the shaft rather than a third thing placed by arithmetic of its
/// own: the shaft's box *is* the arm, so a barb pinned to its top edge stays
/// at the point however long the wind makes the shaft, and nothing here has
/// to be rewritten frame by frame.
///
/// The bar is drawn straight and turned about its own middle, which swings
/// the end that was at the point away from it. The translation is what puts
/// that end back — rotate, then undo the movement of the one end that is
/// meant to stay still — and it is the reason a barb is a transform rather
/// than a position.
fn spawn_barb(shaft: &mut ChildSpawnerCommands, side: f32) {
    let (length, angle) = BARB;
    let turn = side * angle;
    shaft.spawn((
        WindInk,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            top: Val::Px(0.0),
            width: Val::Px(ARM_WIDTH),
            height: Val::Px(length),
            ..default()
        },
        BackgroundColor(INK),
        UiTransform {
            rotation: Rot2::radians(turn),
            translation: Val2::px(
                -length / 2.0 * turn.sin(),
                -length / 2.0 * (1.0 - turn.cos()),
            ),
            ..UiTransform::IDENTITY
        },
    ));
}

/// Which way the arm lies on the card under a wind, or `None` when the wind
/// is too slack to have a bearing at all — see [`CALM`].
///
/// The card's own up is north, so the arm's angle is the wind's bearing:
/// clockwise from north, the way a bearing is always taken, and the way
/// [`UiTransform`] turns. East is north a quarter turn clockwise on the page,
/// which for a page with x to the right and y down is `(-n.y, n.x)`.
///
/// The arrow flies *with* the air rather than pointing into the eye of it.
/// A wind is named for where it comes from, so this is the arguable half of
/// the design — but an arrow that flew backwards would need explaining every
/// time it was looked at, and it has to be read at a glance, over a pennant
/// and a sea that are both unarguably going the other way.
fn arm_bearing(wind: Vec2) -> Option<Rot2> {
    if wind.length() < CALM {
        return None;
    }
    let east = Vec2::new(-NORTH.y, NORTH.x);
    Some(Rot2::radians(f32::atan2(wind.dot(east), wind.dot(NORTH))))
}

/// How far the arm reaches and how hard it is inked, for a wind speed in
/// metres per second.
///
/// Both run off the one ramp, so a stiffening wind lengthens and darkens the
/// arm together and neither reading has to be found on its own. There is no
/// number anywhere and there is not meant to be: the question a player has is
/// which way and roughly how much, and a card that answered in metres per
/// second would be the machinery showing through.
fn arm_reach(speed: f32) -> (f32, f32) {
    let hard = (speed / FULL_WIND).clamp(0.0, 1.0);
    let length = ARM.0 + (ARM.1 - ARM.0) * hard;
    // Two fades multiplied, doing different jobs. The first is the reading —
    // a light air is a fainter arm than a gale — and it keeps a floor, or a
    // real wind would be drawn too faint to find. The second is the calm,
    // which takes the arm off the card altogether rather than leaving a stub
    // aimed at a bearing the wind has stopped having.
    let alpha = (0.45 + 0.55 * hard) * (speed / CALM).clamp(0.0, 1.0);
    (length, alpha)
}

/// Lays the arm along the wind the sea is drawn under.
///
/// The drawn wind, not the forecast — [`SeaConditions::wind`] — for the
/// reason the card reads the camera's eased yaw: an instrument that arrived
/// at the new weather before the water did would be pointing at a sea that
/// is not there yet.
fn point_the_arm(
    conditions: Res<SeaConditions>,
    mut arms: Query<&mut UiTransform, (With<WindArm>, Without<WindShaft>)>,
    mut shafts: Query<(&mut Node, &mut UiTransform), With<WindShaft>>,
    mut ink: Query<&mut BackgroundColor, With<WindInk>>,
) {
    let wind = conditions.wind();
    let (length, alpha) = arm_reach(wind.length());

    // A calm leaves the rotation alone, so the arm fades out where it last
    // pointed and comes back wherever the new wind is.
    if let Some(bearing) = arm_bearing(wind) {
        for mut transform in &mut arms {
            transform.rotation = bearing;
        }
    }
    for (mut node, mut transform) in &mut shafts {
        node.height = Val::Px(length);
        // Centred on the card and pushed half its own length up its own axis,
        // so it runs from the middle out to the point rather than across the
        // middle — and the barbs, hung off its top edge, come with it.
        transform.translation = Val2::px(0.0, -length / 2.0);
    }
    for mut colour in &mut ink {
        *colour = BackgroundColor(INK.with_alpha(alpha));
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
    use crate::Helm;
    use bevy::input::mouse::AccumulatedMouseScroll;
    use bevy::state::app::StatesPlugin;
    use bevy::time::TimePlugin;

    /// A headless app with the camera to read and the compass to point,
    /// already dropped into a match.
    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins((TimePlugin, StatesPlugin, MapCameraPlugin, CompassPlugin))
            .init_state::<AppState>()
            .add_sub_state::<Helm>()
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
    fn the_arm_flies_with_the_wind() {
        // East is a quarter turn clockwise from north on the card, and a wind
        // *blowing* east lays the arrow along it — the reading is where the
        // air is going, not where a sailor would say it came from.
        let east = Vec2::new(-NORTH.y, NORTH.x);
        let blowing_east = arm_bearing(east * 8.0).expect("a fresh breeze has a bearing");
        assert!(
            blowing_east
                .angle_to(Rot2::radians(std::f32::consts::FRAC_PI_2))
                .abs()
                < 1e-5
        );

        // A northerly is air moving south, so the arm lies down the card.
        let northerly = arm_bearing(-NORTH * 8.0).expect("a fresh breeze has a bearing");
        assert!(
            northerly
                .angle_to(Rot2::radians(std::f32::consts::PI))
                .abs()
                < 1e-4
        );

        // And a wind out of the south points the arm at the N, which is the
        // reading most likely to have been drawn backwards.
        let southerly = arm_bearing(NORTH * 8.0).expect("a fresh breeze has a bearing");
        assert!(southerly.angle_to(Rot2::IDENTITY).abs() < 1e-5);
    }

    #[test]
    fn the_arm_grows_and_darkens_with_the_wind() {
        let (light, faint) = arm_reach(2.0);
        let (fresh, dark) = arm_reach(FULL_WIND);
        assert!(
            light < fresh,
            "a fresh breeze drew no longer than a light air"
        );
        assert!(
            faint < dark,
            "a fresh breeze drew no darker than a light air"
        );
        assert!(fresh <= ARM.1, "the arm outgrew the room it has");

        // Past the top of the scale there is nowhere further to go, and a
        // gale must not run the arrow out through the letters.
        assert_eq!((fresh, dark), arm_reach(40.0));
    }

    #[test]
    fn a_calm_takes_the_arm_off_the_card() {
        // No bearing worth drawing, and nothing drawn: the arm keeps the
        // rotation it had and the ink goes to nothing.
        assert!(arm_bearing(Vec2::new(0.2, -0.1)).is_none());
        assert_eq!(arm_reach(0.0).1, 0.0);
        assert!(arm_reach(CALM / 2.0).1 < arm_reach(CALM).1);
    }

    #[test]
    fn the_arm_reads_the_sea_it_is_drawn_over() {
        // The card is wired to the drawn sea rather than to the forecast, so
        // the arm and the water are under one wind. Before any weather has
        // landed that is the assumed day the sea opens on.
        let mut app = test_app();
        app.update();
        let wind = app.world().resource::<SeaConditions>().wind();
        let arm = app
            .world_mut()
            .query_filtered::<&UiTransform, With<WindArm>>()
            .single(app.world())
            .expect("the arm should exist")
            .rotation;
        let expected = arm_bearing(wind).expect("the assumed day is not a calm");
        assert!(arm.angle_to(expected).abs() < 1e-5);

        let shaft = app
            .world_mut()
            .query_filtered::<&Node, With<WindShaft>>()
            .single(app.world())
            .expect("the shaft should exist")
            .height;
        assert_eq!(shaft, Val::Px(arm_reach(wind.length()).0));
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
