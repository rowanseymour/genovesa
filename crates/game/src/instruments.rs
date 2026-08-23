//! The two readings that stand beside the compass: how much water is under
//! the boat, and how much of the day is left.
//!
//! Neither is given a face. The corner already has one card in it, and a
//! second and a third would turn the instruments into the picture's furniture
//! rather than something laid over it — so these are drawn as loose ink on the
//! world, in the card's own colours and its serif, and are read against the
//! card rather than against a rim of their own.
//!
//! **The lead** is a line hung down the screen with the waterline at its top,
//! graduated, and the weight resting where the bottom is. The reading is the
//! *position of the weight*: the figure beside it says the same thing in
//! metres, but a player who never reads it still sees the bottom coming up as
//! they close a shore, which is the question the instrument exists for. Water
//! deeper than the scale is drawn as the line running off the end of it with
//! nothing on it — no bottom found — and that is deliberately the shape the
//! reading will keep when the ocean is deepened past anchoring: the instrument
//! already says *this is more water than you can lie to*, and will not need a
//! second way of saying it.
//!
//! **The day's arc** is the sky seen side-on, with whichever body is up riding
//! across it — see [`crate::sky::aloft`]. It is a clock and not a bearing: the
//! body crosses from left to right whatever way the view is spun, because the
//! question is how much light is left rather than where the sun is. The arc
//! itself is drawn faint under it for the reason the compass gives its wind a
//! whole track to fly down: a lone glyph with nothing to be read against says
//! *the sun is up*, which the picture already said.
use std::f32::consts::TAU;

use bevy::image::Image;
use bevy::prelude::*;
use bevy::text::{FontSize, FontSource};

use crate::camera::PITCH;
use crate::compass::{FACE_SIZE, MARGIN};
use crate::glyph::raster;
use crate::player::PlayerPlace;
use crate::sky::{aloft, Aloft, Sky};
use crate::terrain::Ground;
use crate::{AppState, INK, INK_DIM};

/// How deep the lead is marked, in metres.
///
/// Metres and not fathoms, though a lead line is a fathom's instrument and
/// this one is drawn as period furniture. The chart's own scale bar is
/// lettered in metres and kilometres because that is how a distance would be
/// said out loud — see [`crate::chart`] — and a sounding said in fathoms
/// beside it would have the game speaking two measures at one player, the odd
/// one chosen for flavour. Flavour is what the *drawing* is for.
///
/// Not struck from [`protocol::ground::OCEAN_DEPTH`], either. The scale is how
/// much water this instrument can *answer for*, and the ocean's floor is how
/// much there is — tying them would grow the line every time the world got
/// deeper, when the right answer past a certain depth is that the lead has
/// stopped being the tool. See the module doc for what the line does when it
/// runs out.
const SCALE_METRES: f32 = 10.0;
/// Metres of water between one graduation and the next.
///
/// Two rather than one: at a metre the marks crowd close enough to read as a
/// texture down the line rather than as a scale, and the weight's own height
/// is most of the gap between them.
const MARKED_EVERY: f32 = 2.0;
/// Pixels of line to a metre of water.
const METRE_PIXELS: f32 = 13.0;
/// How far the marked part of the line runs, in pixels.
const SCALE: f32 = SCALE_METRES * METRE_PIXELS;

/// How far the line hangs clear of the card's left edge, in pixels — far
/// enough that the eye reads two instruments rather than one with a tail.
///
/// Measured to the *line* rather than to the box around it, that being the gap
/// a player actually sees: the box reaches on past the line to hold the
/// figure, and how much of it the digits take is a fact about the font.
const LEAD_GAP: f32 = 48.0;
/// The lead's column: how wide a box it is laid out in, and where the line
/// runs down it. The line is near the left edge and the rest is room for the
/// figure, which stands between the line and the card.
const COLUMN: f32 = 40.0;
const LINE_X: f32 = 2.0;

/// The line's stroke, and the length of a graduation's tick either side of it.
const LINE_STROKE: f32 = 1.5;
const TICK: f32 = 5.0;

/// How faint the line below the weight is drawn — line that is still on the
/// reel rather than in the water, so it is furniture and not a reading.
const SLACK: f32 = 0.35;

/// The waterline glyph at the top of the line: the box it is drawn in, how
/// many crests cross it, how far they rise and how heavy they are.
const WAVE: Vec2 = Vec2::new(26.0, 9.0);
const WAVE_CRESTS: f32 = 2.0;
const WAVE_AMP: f32 = 1.6;
const WAVE_STROKE: f32 = 1.2;

/// The weight on the end of the line: the box it is drawn in, and how wide it
/// is at the shoulder and at the toe — a plummet, tapering downwards, which is
/// what makes it read as hanging rather than as a tick that got fat.
const PLUMMET: Vec2 = Vec2::new(7.0, 11.0);
const PLUMMET_SHOULDER: f32 = 5.4;
const PLUMMET_TOE: f32 = 3.0;

/// The figure beside the weight: its type size, how far right of the line it
/// stands, and how far up from the weight's shoulder its own top sits, so the
/// digits straddle the reading rather than hanging under it.
const FIGURE_SIZE: f32 = 13.0;
const FIGURE_X: f32 = 13.0;
const FIGURE_RISE: f32 = 8.0;

/// How far the day's arc rises from its ends to its apex, in pixels, and how
/// heavy a line it is drawn as.
const ARC_RISE: f32 = 40.0;
const ARC_STROKE: f32 = 1.0;
/// The box the arc is drawn in: as wide as the card below it, so the two share
/// a width and read as one instrument stacked, plus a stroke of margin so the
/// apex is not shaved off by the edge.
const ARC_BOX: Vec2 = Vec2::new(FACE_SIZE + ARC_STROKE, ARC_RISE + ARC_STROKE);
/// The radius of the circle the arc is a piece of — the one through both ends
/// and the apex, which for a chord of [`FACE_SIZE`] and a rise of [`ARC_RISE`]
/// is what a sagitta comes to.
const ARC_RADIUS: f32 = (FACE_SIZE * FACE_SIZE / 4.0 + ARC_RISE * ARC_RISE) / (2.0 * ARC_RISE);
/// How far the arc's ends stand clear of the top of the compass's face.
const ARC_CLEAR: f32 = 10.0;

/// How faint the arc is: furniture for the body to be read against, at about
/// the weight the compass draws its own cross at.
const ARC_INK: Color = Color::srgba(0.60, 0.60, 0.55, 0.45);

/// How big the body riding the arc is drawn, in pixels.
const BODY: f32 = 17.0;

/// Texels across the glyphs, for the reason [`crate::glyph::raster`] gives:
/// generous for the size they are laid out at, the UI scaling with the window.
const BODY_TEXELS: u32 = 96;
const WAVE_TEXELS: u32 = 128;
const PLUMMET_TEXELS: u32 = 48;
const ARC_TEXELS: u32 = 512;

/// Marks the lead's column, which carries the whole instrument's visibility.
#[derive(Component)]
struct Lead;

/// One piece of the lead that a sounding moves or writes.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum LeadPart {
    /// The line in the water: inked from the waterline down to the weight.
    Run,
    /// The line still on the reel, below the weight.
    Slack,
    /// The weight itself.
    Weight,
    /// The figure beside it.
    Figure,
}

/// Marks the body riding the day's arc, and holds the two glyphs it swaps
/// between so neither has to be built again when the light changes hands.
#[derive(Component)]
struct DayBody {
    sun: Handle<Image>,
    moon: Handle<Image>,
}

/// What the lead found.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Sounding {
    /// The weight is on the bottom, this many metres down.
    Bottom(f32),
    /// Deeper than the line is marked for — see the module doc.
    NoBottom,
    /// Nothing to sound: the ground under the boat has not arrived. Refused
    /// rather than held, for the reason the compass hides its bow when there
    /// is no heading — a stale depth is read as confidently as a true one, and
    /// this is the reading a player would run aground trusting.
    Nothing,
}

pub struct InstrumentsPlugin;

impl Plugin for InstrumentsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::InWorld), spawn_instruments)
            .add_systems(
                Update,
                // Each on the one resource it reads, so an app that stands
                // this plugin up without the world's ground or its clock draws
                // the other instrument rather than falling over.
                (
                    heave_the_lead.run_if(resource_exists::<Ground>),
                    cross_the_sky.run_if(resource_exists::<Sky>),
                )
                    .run_if(in_state(AppState::InWorld)),
            );
    }
}

fn spawn_instruments(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let images = &mut *images;
    spawn_lead(&mut commands, images);
    spawn_day_arc(&mut commands, images);
}

/// The lead: the waterline, the marked scale, and the line with its weight on
/// the end, all hung in a column left of the card.
fn spawn_lead(commands: &mut Commands, images: &mut Assets<Image>) {
    commands
        .spawn((
            Name::new("Lead"),
            Lead,
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(lead_right()),
                bottom: Val::Px(MARGIN),
                width: Val::Px(COLUMN),
                height: Val::Px(SCALE + PLUMMET.y),
                ..default()
            },
            // Blank until the first sounding, which may be the frame after
            // this one: an instrument that opened reading no water at all would
            // be saying the boat is aground.
            Visibility::Hidden,
            DespawnOnExit(AppState::InWorld),
        ))
        .with_children(|lead| {
            // The furniture first, so the reading is drawn over it: the marks
            // are what the weight is read against, and a tick crossing the
            // line above the weight must not look like a second weight.
            spawn_wave(lead, images);
            spawn_marks(lead);
            spawn_line(lead);
            spawn_weight(lead, images);
            spawn_figure(lead);
        });
}

/// The waterline the scale is measured from, drawn as water rather than as a
/// nought: a graduated line needs a top, and the top of this one is the one
/// place on the instrument that is a *thing* and not a number.
fn spawn_wave(lead: &mut ChildSpawnerCommands, images: &mut Assets<Image>) {
    lead.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(LINE_X - WAVE.x / 2.0),
            top: Val::Px(-WAVE.y / 2.0),
            width: Val::Px(WAVE.x),
            height: Val::Px(WAVE.y),
            ..default()
        },
        ImageNode {
            image: images.add(raster(WAVE_TEXELS, wave_texels_tall(), on_the_wave)),
            color: INK_DIM,
            ..default()
        },
    ));
}

/// The graduations, each a bar lying across the line.
fn spawn_marks(lead: &mut ChildSpawnerCommands) {
    for mark in 1..=(SCALE_METRES / MARKED_EVERY) as u32 {
        lead.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(LINE_X - TICK),
                top: Val::Px(mark as f32 * MARKED_EVERY * METRE_PIXELS),
                width: Val::Px(TICK * 2.0),
                height: Val::Px(1.0),
                ..default()
            },
            BackgroundColor(INK_DIM),
        ));
    }
}

/// The line in two pieces: what is in the water, and what is still on the
/// reel below it. [`heave_the_lead`] moves the boundary between them.
fn spawn_line(lead: &mut ChildSpawnerCommands) {
    for (part, ink) in [
        (LeadPart::Run, INK),
        (LeadPart::Slack, INK_DIM.with_alpha(SLACK)),
    ] {
        lead.spawn((
            part,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(LINE_X - LINE_STROKE / 2.0),
                top: Val::Px(0.0),
                width: Val::Px(LINE_STROKE),
                height: Val::Px(0.0),
                ..default()
            },
            BackgroundColor(ink),
            Visibility::Inherited,
        ));
    }
}

fn spawn_weight(lead: &mut ChildSpawnerCommands, images: &mut Assets<Image>) {
    lead.spawn((
        LeadPart::Weight,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(LINE_X - PLUMMET.x / 2.0),
            top: Val::Px(0.0),
            width: Val::Px(PLUMMET.x),
            height: Val::Px(PLUMMET.y),
            ..default()
        },
        ImageNode {
            image: images.add(raster(
                PLUMMET_TEXELS,
                plummet_texels_tall(),
                on_the_plummet,
            )),
            color: INK,
            ..default()
        },
        Visibility::Inherited,
    ));
}

/// The sounding in figures, in the serif the card is lettered in.
fn spawn_figure(lead: &mut ChildSpawnerCommands) {
    lead.spawn((
        LeadPart::Figure,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(LINE_X + FIGURE_X),
            top: Val::Px(0.0),
            ..default()
        },
        Text::new(String::new()),
        TextFont {
            font: FontSource::Serif,
            font_size: FontSize::Px(FIGURE_SIZE),
            ..default()
        },
        TextColor(INK),
        Visibility::Inherited,
    ));
}

/// The day's arc, and the body that rides it.
fn spawn_day_arc(commands: &mut Commands, images: &mut Assets<Image>) {
    let sun = images.add(raster(BODY_TEXELS, BODY_TEXELS, on_the_sun));
    let moon = images.add(raster(BODY_TEXELS, BODY_TEXELS, on_the_moon));
    commands
        .spawn((
            Name::new("Day arc"),
            Node {
                position_type: PositionType::Absolute,
                // Centred over the card: the arc is the card's own width, so
                // the two share a middle and the corner reads as one column
                // of instruments.
                right: Val::Px(MARGIN - ARC_STROKE / 2.0),
                bottom: Val::Px(arc_bottom()),
                width: Val::Px(ARC_BOX.x),
                height: Val::Px(ARC_BOX.y),
                ..default()
            },
            DespawnOnExit(AppState::InWorld),
        ))
        .with_children(|sky| {
            sky.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                ImageNode {
                    image: images.add(raster(ARC_TEXELS, arc_texels_tall(), on_the_arc)),
                    color: ARC_INK,
                    ..default()
                },
            ));
            sky.spawn((
                DayBody {
                    sun: sun.clone(),
                    moon,
                },
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    width: Val::Px(BODY),
                    height: Val::Px(BODY),
                    ..default()
                },
                ImageNode {
                    image: sun,
                    color: INK,
                    ..default()
                },
            ));
        });
}

/// How far in from the right of the window the lead's box stands.
///
/// Placed by where the *line* has to fall — [`LEAD_GAP`] clear of the card —
/// rather than by where the box does, the box being a container for the line
/// and the figure beside it and not a thing anyone sees. The rest is what the
/// column carries to the left of the line and the right of the box.
const fn lead_right() -> f32 {
    MARGIN + FACE_SIZE + LEAD_GAP - COLUMN + LINE_X
}

/// The box must clear the card even though nothing is drawn at its right edge,
/// or the figure inside it would land on the rose; and it must be wide enough
/// to the right of the line for the figure to start inside it. Both fall out
/// of constants set for different reasons, and nothing else would notice them
/// crossing.
const _: () = assert!(lead_right() >= MARGIN + FACE_SIZE);
const _: () = assert!(COLUMN - LINE_X >= FIGURE_X);

/// Where the arc's ends sit above the bottom of the window.
///
/// Clear of the top of the compass's face, which is the card's box squashed to
/// the camera's pitch — the same projection [`crate::compass`] applies, asked
/// here rather than written down again.
fn arc_bottom() -> f32 {
    MARGIN + FACE_SIZE / 2.0 * (1.0 + PITCH.sin()) + ARC_CLEAR
}

/// What the lead finds under a boat at `at`.
fn sound(ground: &Ground, at: Vec2) -> Sounding {
    let Some(height) = ground.height(at.x, at.y) else {
        return Sounding::Nothing;
    };
    // Ground standing above the waterline sounds as no water rather than as
    // negative depth: a boat that has run itself up a beach is in nought
    // water, which is a true and useful thing for the instrument to say.
    let depth = (-height).max(0.0);
    if depth > SCALE_METRES {
        Sounding::NoBottom
    } else {
        Sounding::Bottom(depth)
    }
}

/// The sounding written out: whole metres, and the unit with them.
///
/// Rounded rather than exact, and that is the reading and not a shortcut. A
/// lead is read off the mark nearest the water, so the metre is as fine as
/// this instrument goes — a figure counting off centimetres would be the
/// machinery showing through a rope with knots in it.
///
/// The unit is written because the chart says its own distances the same way,
/// and a bare number beside a graduated line invites the reader to supply a
/// measure of their own.
fn figure(metres: f32) -> String {
    format!("{} m", metres.round() as u32)
}

/// Hangs the lead over the side and reads it.
///
/// The whole instrument goes out unless there is a sounding to draw — ashore,
/// the compass ring's arrangement and for its reason, since there is no water
/// under a walker and one left showing the last sounding of the boat they
/// stepped off is a claim about where they are standing; and afloat over
/// ground that has not arrived, for the reason at the `showing` below.
fn heave_the_lead(
    ground: Res<Ground>,
    player: PlayerPlace,
    mut column: Query<&mut Visibility, (With<Lead>, Without<LeadPart>)>,
    mut parts: Query<(&LeadPart, &mut Node, &mut Visibility, Option<&mut Text>)>,
) {
    let sounding = match (player.aboard(), player.on_the_map()) {
        (true, Some(at)) => sound(&ground, at),
        _ => Sounding::Nothing,
    };
    // A sounding and not just a boat: the instrument's furniture is the scale
    // its reading is read against, and a waterline with graduations under it
    // and nothing hanging from them reads as a lead that has lost its line
    // rather than as one with nothing to say. The world opens in exactly that
    // state — aboard, with the ground under the hull still on its way.
    let showing = if player.aboard() && sounding != Sounding::Nothing {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    for mut visibility in &mut column {
        if *visibility != showing {
            *visibility = showing;
        }
    }
    if showing == Visibility::Hidden {
        return;
    }

    // How far down the line the weight hangs, and how much of the line that
    // leaves on the reel below it.
    let down = match sounding {
        Sounding::Bottom(depth) => depth * METRE_PIXELS,
        // No bottom runs the line off the end of the marked scale rather than
        // stopping it at the last mark, which would read as a sounding of
        // five: what says *no bottom* is that there is nothing resting on the
        // end of it.
        Sounding::NoBottom => SCALE + PLUMMET.y,
        Sounding::Nothing => 0.0,
    };
    for (part, mut node, mut visibility, text) in &mut parts {
        let (top, height, shown) = match part {
            // The line itself is always drawn: past the return above there is
            // a sounding, and a sounding is a line in the water.
            LeadPart::Run => (0.0, down, true),
            LeadPart::Slack => (down, (SCALE - down).max(0.0), true),
            LeadPart::Weight => (down, PLUMMET.y, matches!(sounding, Sounding::Bottom(_))),
            LeadPart::Figure => (
                down - FIGURE_RISE,
                0.0,
                matches!(sounding, Sounding::Bottom(_)),
            ),
        };
        // Written only where they differ — the compass's economy: a boat under
        // way changes its sounding by a fraction of a pixel most frames, and a
        // node written every frame is a relayout every frame.
        let top = Val::Px(top);
        if node.top != top {
            node.top = top;
        }
        if !matches!(part, LeadPart::Figure) {
            let height = Val::Px(height);
            if node.height != height {
                node.height = height;
            }
        }
        let showing = if shown {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *visibility != showing {
            *visibility = showing;
        }
        if let (LeadPart::Figure, Some(mut text), Sounding::Bottom(depth)) = (part, text, sounding)
        {
            let reading = figure(depth);
            if text.0 != reading {
                text.0 = reading;
            }
        }
    }
}

/// Carries the body across the day's arc, and swaps which body it is as the
/// sky changes hands.
fn cross_the_sky(sky: Res<Sky>, mut bodies: Query<(&DayBody, &mut Node, &mut ImageNode)>) {
    let (body, through) = aloft(sky.phase());
    let at = along_the_arc(through) - Vec2::splat(BODY / 2.0);
    for (glyphs, mut node, mut image) in &mut bodies {
        let (left, top) = (Val::Px(at.x), Val::Px(at.y));
        if node.left != left {
            node.left = left;
        }
        if node.top != top {
            node.top = top;
        }
        let wanted = match body {
            Aloft::Sun => &glyphs.sun,
            Aloft::Moon => &glyphs.moon,
        };
        if image.image != *wanted {
            image.image = wanted.clone();
        }
    }
}

/// Where a fraction of a body's crossing falls inside the arc's box, in pixels
/// from its top-left corner — `0.0` at the left end, `1.0` at the right.
fn along_the_arc(through: f32) -> Vec2 {
    let widest = (FACE_SIZE / 2.0 / ARC_RADIUS).asin();
    let angle = (through.clamp(0.0, 1.0) * 2.0 - 1.0) * widest;
    Vec2::new(
        ARC_BOX.x / 2.0 + ARC_RADIUS * angle.sin(),
        ARC_STROKE / 2.0 + ARC_RADIUS * (1.0 - angle.cos()),
    )
}

/// Whether a point of the arc's box lies under its stroke. The box is the
/// bounding box of the arc itself, so the circle leaves it exactly at the two
/// ends and nothing below them needs turning away.
fn on_the_arc(at: Vec2) -> bool {
    let at = at * ARC_BOX;
    let centre = Vec2::new(ARC_BOX.x / 2.0, ARC_STROKE / 2.0 + ARC_RADIUS);
    ((at - centre).length() - ARC_RADIUS).abs() <= ARC_STROKE / 2.0
}

/// The sun: a disc with eight rays standing off it. Filled rather than drawn
/// as a ring, which at this size closes up into a disc with a dirty middle.
fn on_the_sun(at: Vec2) -> bool {
    /// The disc, and where the rays begin and end, as fractions of the box's
    /// half-width; then how wide a ray is at any radius.
    const DISC: f32 = 0.30;
    const RAY: (f32, f32) = (0.46, 0.92);
    const RAY_HALF: f32 = 0.05;

    let from_middle = at * 2.0 - 1.0;
    let reach = from_middle.length();
    if reach <= DISC {
        return true;
    }
    if !(RAY.0..=RAY.1).contains(&reach) {
        return false;
    }
    // How far round it is from the nearest of the eight, as an arc length, so
    // a ray is a bar of even width rather than a wedge.
    let step = TAU / 8.0;
    let off = from_middle.y.atan2(from_middle.x).rem_euclid(step);
    reach * off.min(step - off) <= RAY_HALF
}

/// The moon: a disc with a bite out of it, the horns standing up and down so
/// the glyph is as tall as the sun it replaces and the arc does not appear to
/// change height when the light changes hands.
fn on_the_moon(at: Vec2) -> bool {
    const MOON: f32 = 0.80;
    /// Where the bite's own middle sits and how big it is, as fractions of the
    /// moon — set so the crescent keeps a waist worth seeing at this size.
    const BITE: (f32, f32) = (0.55, 0.88);

    let from_middle = at * 2.0 - 1.0;
    from_middle.length() <= MOON
        && (from_middle - Vec2::new(MOON * BITE.0, 0.0)).length() > MOON * BITE.1
}

/// The waterline: a few crests crossing the top of the line.
fn on_the_wave(at: Vec2) -> bool {
    let at = at * WAVE;
    let crest = WAVE.y / 2.0 + (at.x / WAVE.x * TAU * WAVE_CRESTS).sin() * WAVE_AMP;
    (at.y - crest).abs() <= WAVE_STROKE / 2.0
}

/// The weight: a bar tapering from its shoulder to its toe.
fn on_the_plummet(at: Vec2) -> bool {
    let half = (PLUMMET_SHOULDER + (PLUMMET_TOE - PLUMMET_SHOULDER) * at.y) / 2.0;
    ((at.x - 0.5) * PLUMMET.x).abs() <= half
}

/// How tall each glyph's texture is: its box's own proportions at the width it
/// is rastered to, so nothing is drawn into a texture squarer than the mark.
fn wave_texels_tall() -> u32 {
    (WAVE_TEXELS as f32 * WAVE.y / WAVE.x).round() as u32
}
fn plummet_texels_tall() -> u32 {
    (PLUMMET_TEXELS as f32 * PLUMMET.y / PLUMMET.x).round() as u32
}
fn arc_texels_tall() -> u32 {
    (ARC_TEXELS as f32 * ARC_BOX.y / ARC_BOX.x).round() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::Ground;
    use protocol::ground::{
        quantize, ChunkPayload, Material, CELL_METRES, CORNERS, LIT_ALL_DAY, MATERIAL_COUNT,
    };

    /// A chunk of sea bed at one depth all over.
    fn bed_at(height: f32) -> ChunkPayload {
        ChunkPayload {
            heights: vec![quantize(height); CORNERS * CORNERS],
            materials: vec![Material::Sand; MATERIAL_COUNT],
            // Open water, which is what a bed with nothing standing on it is.
            lit: vec![LIT_ALL_DAY; CORNERS * CORNERS],
            water: None,
            plants: Vec::new(),
        }
    }

    /// Ground with one chunk of bed at the origin, and a point standing on it.
    fn over_a_bed(height: f32) -> (Ground, Vec2) {
        let mut ground = Ground::default();
        ground.deliver(IVec2::ZERO, Some(bed_at(height)));
        (ground, Vec2::splat(CELL_METRES))
    }

    /// A headless app with the instruments in it, already dropped into a
    /// world.
    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            // Assets because every glyph here is an image.
            bevy::asset::AssetPlugin::default(),
            bevy::time::TimePlugin,
            bevy::state::app::StatesPlugin,
            InstrumentsPlugin,
        ))
        // The hour, which the real app gets from `SkyPlugin` — the arc reads
        // it, and the whole of that plugin is a great deal of lighting to
        // stand up for one number.
        .init_resource::<Sky>()
        .init_asset::<Image>()
        .init_state::<AppState>();
        app.update();

        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::InWorld);
        app.update();
        app
    }

    /// Stands a player over a bed at some depth, aboard a hull or on their own
    /// feet, and runs a frame — being aboard is being somebody's child, which
    /// is what [`PlayerPlace`] resolves a carrier through.
    fn a_player_over(height: f32, aboard: bool) -> App {
        use crate::player::Player;

        let mut app = test_app();
        let (ground, at) = over_a_bed(height);
        app.insert_resource(ground);

        let stood = Transform::from_xyz(at.x, 0.0, at.y);
        let player = app.world_mut().spawn((Player, stood)).id();
        if aboard {
            let hull = app.world_mut().spawn(stood).id();
            app.world_mut().entity_mut(player).insert(ChildOf(hull));
        }
        app.update();
        app
    }

    /// What the lead is showing: whether the column is drawn at all, and the
    /// figure beside the weight.
    fn reading(app: &mut App) -> (bool, String) {
        let drawn = *app
            .world_mut()
            .query_filtered::<&Visibility, With<Lead>>()
            .single(app.world())
            .expect("the lead should exist")
            != Visibility::Hidden;
        let figure = app
            .world_mut()
            .query::<(&LeadPart, &Text)>()
            .iter(app.world())
            .find(|(part, _)| **part == LeadPart::Figure)
            .map(|(_, text)| text.0.clone())
            .unwrap_or_default();
        (drawn, figure)
    }

    /// The lead is a sailing instrument. There is no water under a walker, and
    /// one left showing the last sounding of the boat they stepped off is a
    /// claim about where they are standing.
    #[test]
    fn the_lead_is_only_hung_afloat() {
        let mut afloat = a_player_over(-4.0, true);
        let (drawn, figure) = reading(&mut afloat);
        assert!(drawn, "afloat, the lead was not hung");
        assert_eq!(figure, "4 m", "afloat, the lead read {figure:?}");

        let mut ashore = a_player_over(-4.0, false);
        assert!(
            !reading(&mut ashore).0,
            "ashore, the lead was still hanging"
        );
    }

    /// The state every world opens in: aboard, with the ground under the hull
    /// still on its way. The furniture has to wait for the reading it is drawn
    /// to be read against, or the instrument opens looking like a lead that
    /// has lost its line.
    #[test]
    fn the_lead_waits_for_ground_before_it_shows_its_scale() {
        let mut app = test_app();
        // `Ground` present but empty, which is how `terrain` inserts it at
        // world entry — the chunks arrive after.
        app.insert_resource(Ground::default());
        let stood = Transform::from_xyz(0.0, 0.0, 0.0);
        let hull = app.world_mut().spawn(stood).id();
        app.world_mut()
            .spawn((crate::player::Player, stood, ChildOf(hull)));
        app.update();

        assert!(
            !reading(&mut app).0,
            "the lead showed its scale with no ground to sound"
        );
    }

    /// The weight rides up the line as the bottom comes up, which is the whole
    /// reading — a figure that changed while the weight stood still would be
    /// two instruments disagreeing.
    #[test]
    fn the_weight_rides_up_as_the_bottom_does() {
        // Within a pixel, the bed's height having been through the wire's own
        // two-centimetre steps on the way here.
        let plummet = |app: &mut App| {
            let top = app
                .world_mut()
                .query::<(&LeadPart, &Node)>()
                .iter(app.world())
                .find(|(part, _)| **part == LeadPart::Weight)
                .map(|(_, node)| node.top)
                .expect("the weight should exist");
            let Val::Px(top) = top else {
                panic!("the weight hung at {top:?} rather than at a depth");
            };
            top
        };

        let mut deep = a_player_over(-8.0, true);
        let mut shoal = a_player_over(-2.0, true);
        assert!((plummet(&mut deep) - 8.0 * METRE_PIXELS).abs() < 1.0);
        assert!((plummet(&mut shoal) - 2.0 * METRE_PIXELS).abs() < 1.0);
        assert_eq!(reading(&mut shoal).1, "2 m");
    }

    #[test]
    fn both_instruments_stand_in_a_world_and_nowhere_else() {
        let mut app = test_app();
        assert_eq!(
            app.world_mut().query::<&Lead>().iter(app.world()).count(),
            1,
            "the lead was not hung"
        );
        assert_eq!(
            app.world_mut()
                .query::<&DayBody>()
                .iter(app.world())
                .count(),
            1,
            "nothing was put in the sky"
        );

        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::MainMenu);
        app.update();
        assert_eq!(
            app.world_mut().query::<&Lead>().iter(app.world()).count(),
            0,
            "the lead outlived the world"
        );
    }

    #[test]
    fn the_lead_reads_the_water_under_the_boat() {
        let (ground, at) = over_a_bed(-3.0);
        let Sounding::Bottom(depth) = sound(&ground, at) else {
            panic!("a bed three metres down did not sound as a bottom");
        };
        assert!((depth - 3.0).abs() < 0.01, "sounded {depth} metres");
    }

    #[test]
    fn water_past_the_scale_finds_no_bottom() {
        // The reading the deep ocean will give when it is deepened — see the
        // module doc. Nothing rests on the end of the line.
        let (ground, at) = over_a_bed(-(SCALE_METRES + 1.0));
        assert_eq!(sound(&ground, at), Sounding::NoBottom);
    }

    #[test]
    fn ground_that_has_not_arrived_is_refused_rather_than_guessed() {
        // The one reading a player could run aground trusting.
        assert_eq!(
            sound(&Ground::default(), Vec2::ZERO),
            Sounding::Nothing,
            "an unstreamed chunk sounded as water"
        );
    }

    #[test]
    fn a_bed_above_the_waterline_sounds_as_no_water() {
        let (ground, at) = over_a_bed(3.0);
        assert_eq!(sound(&ground, at), Sounding::Bottom(0.0));
    }

    #[test]
    fn the_figure_is_read_off_the_nearest_metre() {
        assert_eq!(figure(3.0), "3 m");
        // Rounded to the mark, not truncated to it: a lead is read off the
        // mark nearest the water.
        assert_eq!(figure(3.4), "3 m");
        assert_eq!(figure(3.6), "4 m");
        assert_eq!(figure(0.4), "0 m");
    }

    #[test]
    fn the_arc_runs_from_end_to_end_through_its_apex() {
        let (rise, noon, set) = (along_the_arc(0.0), along_the_arc(0.5), along_the_arc(1.0));
        // The ends sit at the bottom corners of the box and the apex midway
        // between them at the top — the arithmetic most likely to come out
        // inside out.
        assert!(rise.x < 1.0, "the arc began {} in from its end", rise.x);
        assert!(
            (set.x - ARC_BOX.x).abs() < 1.0,
            "the arc ended {} short",
            ARC_BOX.x - set.x
        );
        assert!((noon.x - ARC_BOX.x / 2.0).abs() < 0.01);
        assert!(
            noon.y < rise.y && noon.y < set.y,
            "the apex was not highest"
        );
        assert!((rise.y - set.y).abs() < 0.01, "the arc was lopsided");
        assert!((noon.y - ARC_STROKE / 2.0).abs() < 0.01);
        assert!((rise.y - (ARC_RISE + ARC_STROKE / 2.0)).abs() < 0.01);
    }

    #[test]
    fn the_body_climbs_and_falls_across_the_arc() {
        // Monotone in x, so the crossing never doubles back, and the middle is
        // higher than either quarter.
        let quarters: Vec<Vec2> = (0..=4).map(|q| along_the_arc(q as f32 / 4.0)).collect();
        for pair in quarters.windows(2) {
            assert!(pair[0].x < pair[1].x, "the body went backwards");
        }
        assert!(quarters[1].y > quarters[2].y && quarters[3].y > quarters[2].y);
    }

    #[test]
    fn the_arc_stands_clear_of_the_compass() {
        // The card's face is its box squashed to the camera's pitch, so the
        // top of the ellipse — not of the node — is what the arc has to clear.
        let face_top = MARGIN + FACE_SIZE / 2.0 * (1.0 + PITCH.sin());
        assert!(
            arc_bottom() >= face_top,
            "the arc's ends sat {} px inside the card",
            face_top - arc_bottom()
        );
    }

    /// The box is placed by where the *line* inside it has to land, so the two
    /// can drift apart as the column is retuned. What keeps the box off the
    /// card is asserted at the constants themselves.
    #[test]
    fn the_line_hangs_its_gap_clear_of_the_card() {
        let card_edge = MARGIN + FACE_SIZE;
        let line = lead_right() + COLUMN - LINE_X;
        assert!(
            (line - (card_edge + LEAD_GAP)).abs() < 0.01,
            "the line hung {} px from the card rather than {LEAD_GAP}",
            line - card_edge
        );
    }

    /// The glyphs have to be marks and not blobs: each is checked where its
    /// shape actually differs from the disc it would be if the arithmetic
    /// collapsed.
    #[test]
    fn the_moon_is_a_crescent_and_the_sun_is_not() {
        // The sun is solid through the middle; the moon is bitten out of it.
        assert!(on_the_sun(Vec2::splat(0.5)));
        assert!(!on_the_moon(Vec2::splat(0.5)));
        // And the moon keeps its back: the far left of the box is under ink.
        assert!(on_the_moon(Vec2::new(0.12, 0.5)));
    }

    #[test]
    fn the_sun_has_rays_with_gaps_between_them() {
        // Straight out to the right is a ray; an eighth of a turn off it, in
        // the gap, is not — what a wedge instead of a bar would fill in.
        let out = |turn: f32, reach: f32| {
            Vec2::splat(0.5) + Vec2::new(turn.cos(), turn.sin()) * reach / 2.0
        };
        assert!(on_the_sun(out(0.0, 0.7)));
        assert!(!on_the_sun(out(TAU / 16.0, 0.7)));
    }

    #[test]
    fn the_plummet_tapers_downwards() {
        // Narrower at the toe than at the shoulder, which is what makes it
        // read as hanging: a point just inside the shoulder's width is off
        // the glyph by the time it reaches the toe.
        let off_middle = 0.5 + PLUMMET_TOE / 2.0 / PLUMMET.x + 0.02;
        assert!(on_the_plummet(Vec2::new(off_middle, 0.05)));
        assert!(!on_the_plummet(Vec2::new(off_middle, 0.95)));
    }

    #[test]
    fn the_wave_crosses_the_line_it_tops() {
        // Ink somewhere down the middle of the box, and clear air at its
        // corners: a wave that had collapsed to a straight bar would pass the
        // first of these and fail nothing else.
        assert!((0..WAVE_TEXELS)
            .any(|x| { on_the_wave(Vec2::new(x as f32 / WAVE_TEXELS as f32, 0.5)) }));
        assert!(!on_the_wave(Vec2::new(0.5, 0.02)));
        assert!(!on_the_wave(Vec2::new(0.5, 0.98)));
    }

    /// A texture squarer or flatter than the mark it holds stretches the mark
    /// when the node draws it — which is the kind of wrong that looks like a
    /// badly drawn glyph rather than like a bug.
    #[test]
    fn every_glyph_is_rastered_at_the_shape_of_its_mark() {
        for (wide, tall, mark) in [
            (WAVE_TEXELS, wave_texels_tall(), WAVE),
            (PLUMMET_TEXELS, plummet_texels_tall(), PLUMMET),
            (ARC_TEXELS, arc_texels_tall(), ARC_BOX),
        ] {
            assert!(tall > 0, "a glyph was rastered into no rows at all");
            let drawn = wide as f32 / tall as f32;
            let laid_out = mark.x / mark.y;
            assert!(
                (drawn / laid_out - 1.0).abs() < 0.02,
                "a glyph laid out at {laid_out:.2} wide to tall was rastered at {drawn:.2}"
            );
        }
    }
}
