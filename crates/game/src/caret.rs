//! The caret on lettering being written in the world rather than in a menu.
//!
//! The menus' fields are Bevy's own `EditableText`, which draws its caret
//! itself; that widget is UI-only, so a name lettered onto the chart wears
//! this instead. A bar rather than a `|` glyph, because a glyph sits in the
//! middle of a whole letter's width and so stands off the word.
//!
//! It is lit afresh whenever the lettering it follows changes, so it never
//! vanishes under the key that was just pressed.

use bevy::prelude::*;
use bevy::text::TextLayoutInfo;

/// How long the caret is lit, and then how long it is dark.
const HALF_BLINK: f32 = 0.5;

/// The bar's width, and the clear space between it and the last letter, as
/// fractions of the lettering's line height.
const WIDTH: f32 = 0.08;
const GAP: f32 = 0.06;

pub struct CaretPlugin;

impl Plugin for CaretPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, (place, blink));
    }
}

/// Marks a caret. Spawn it with [`caret`].
#[derive(Component)]
pub struct Caret {
    lit_at: f32,
}

/// A caret, to be spawned as a child of a centred `Text2d`.
pub fn caret(color: Color) -> impl Bundle {
    (
        Caret { lit_at: 0.0 },
        Sprite::from_color(color, Vec2::ZERO),
        Transform::default(),
    )
}

/// Stands the bar just past the last letter. The lettering is laid out after
/// it is spawned, and again whenever it changes, so this follows the layout.
fn place(
    lettering: Query<&TextLayoutInfo, Changed<TextLayoutInfo>>,
    mut carets: Query<(&ChildOf, &mut Sprite, &mut Transform), With<Caret>>,
) {
    for (parent, mut sprite, mut transform) in &mut carets {
        let Ok(layout) = lettering.get(parent.parent()) else {
            continue;
        };
        let line = layout.size.y;
        sprite.custom_size = Some(Vec2::new(WIDTH * line, line));
        transform.translation.x = layout.size.x / 2.0 + (GAP + WIDTH / 2.0) * line;
        // Over the letters it follows.
        transform.translation.z = 0.01;
    }
}

/// A lettering whose text changed this frame.
type Typed = Or<(Added<Text2d>, Changed<Text2d>)>;

fn blink(
    time: Res<Time>,
    mut carets: Query<(&mut Caret, &ChildOf, &mut Visibility)>,
    typed: Query<(), Typed>,
) {
    let now = time.elapsed_secs();
    for (mut caret, parent, mut visibility) in &mut carets {
        if caret.is_added() || typed.contains(parent.parent()) {
            caret.lit_at = now;
        }
        let lit = (((now - caret.lit_at) / HALF_BLINK) as u32).is_multiple_of(2);
        visibility.set_if_neq(if lit {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn lit(app: &mut App) -> bool {
        let world = app.world_mut();
        let mut visibility = world.query_filtered::<&Visibility, With<Caret>>();
        *visibility.single(world).unwrap() != Visibility::Hidden
    }

    fn wait(app: &mut App, seconds: f32) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(seconds));
        app.update();
    }

    #[test]
    fn it_blinks_and_a_key_lights_it_again() {
        let mut app = App::new();
        app.init_resource::<Time>().add_plugins(CaretPlugin);
        let lettering = app
            .world_mut()
            .spawn(Text2d::new("Isla"))
            .with_child(caret(Color::BLACK))
            .id();
        app.update();
        assert!(lit(&mut app), "a new caret is dark");

        wait(&mut app, HALF_BLINK * 1.5);
        assert!(!lit(&mut app), "the caret did not go dark");
        wait(&mut app, HALF_BLINK);
        assert!(lit(&mut app), "the caret did not come back");
        wait(&mut app, HALF_BLINK);
        assert!(!lit(&mut app));

        app.world_mut()
            .get_mut::<Text2d>(lettering)
            .unwrap()
            .0
            .push('s');
        app.update();
        assert!(lit(&mut app), "a key left the caret dark");
    }

    #[test]
    fn it_stands_just_past_the_last_letter() {
        let mut app = App::new();
        app.init_resource::<Time>().add_plugins(CaretPlugin);
        app.world_mut()
            .spawn((
                Text2d::new("Isla"),
                TextLayoutInfo {
                    size: Vec2::new(40.0, 20.0),
                    ..default()
                },
            ))
            .with_child(caret(Color::BLACK));
        app.update();

        let world = app.world_mut();
        let mut caret = world.query_filtered::<(&Sprite, &Transform), With<Caret>>();
        let (sprite, transform) = caret.single(world).unwrap();
        let size = sprite.custom_size.unwrap();
        let near_edge = transform.translation.x - size.x / 2.0;
        assert!(
            near_edge > 20.0 && near_edge < 22.0,
            "the bar's near edge is at {near_edge}, not just past the word's end at 20"
        );
        assert_eq!(size.y, 20.0, "the bar is not the height of the line");
    }
}
