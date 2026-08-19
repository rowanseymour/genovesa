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
//! bearing on it is carried out into the picture, and against ground drawn at
//! the camera's pitch an unforeshortened dial reads as a sticker on the screen.
//! The tilt is the projection itself — a flat card on the ground plane, seen
//! from [`PITCH`][crate::camera::PITCH] above horizontal, is its upright
//! drawing squashed vertically by `sin(PITCH)`, applied *after* the card's own
//! spin so the letters shear the way paint on a deck would. Real 3D geometry
//! parented to the camera differs only by a keystone too small to see, and
//! would need letters as meshes and an exemption from the fog, the lighting and
//! the terrain's occlusion.
//!
//! The card carries a second reading: the reader's own mark at the middle of
//! it, spun to the way the boat is actually pointing.
//!
//! And a third: an arrow lying along the wind. A wind is only ever wanted
//! *against* something — the way home, the way the boat is pointed — and both
//! are bearings, so one card holding north, the bow and the wind together
//! answers "the wind is off my starboard bow" in a glance.
//!
//! Those two have to be told apart at a glance or the card is worse than
//! nothing, and they were not: with the wind the only arrow on the card it was
//! read as the heading, an arrow through the middle of a compass being a
//! heading on every instrument anybody has held. Ink alone would not have
//! separated them — two arrows differing only in colour is a puzzle with two
//! pieces — so they differ in what they are anchored to and in whether they
//! are filled. The bow is solid and owns the centre; the wind is a stroked
//! arrow that stops short of it and runs back out to windward, so it reads as
//! weather arriving from outside rather than as something the player is doing.
//! And because the card is aligned with the world, the bow lies the way the
//! hull in the picture does — the instrument visibly agreeing with what the
//! player is already looking at.
//!
//! And a fourth: a ring of marks round the rim, one arc per stretch of coast
//! within sight. This camera looks *down*, so land a few hundred metres off can
//! be outside the picture entirely while the player is close enough to walk up
//! its beach. The ring answers what the picture cannot: something is over
//! there, and it is that way.
//!
//! It marks what has not been charted louder than what has, the problem being
//! finding *new* islands, and it reaches exactly as far as the haze does — see
//! [`SIGHT`]. The card says what the player could have noticed and did not, and
//! nothing about what is over the horizon.
//!
//! Ashore the ring goes out altogether: everything it could mark from a beach
//! is either the island underfoot or in plain view across it.
//!
//! Nothing about it crosses the wire — the sweep reads the chunks this machine
//! was already sent, the arrangement the chart is built on too.

use std::f32::consts::TAU;

use bevy::prelude::*;
use bevy::text::{FontSize, FontSource};
use bevy::ui::{UiTransform, Val2};

use protocol::ground::{CHUNK_METRES, NORTH};

use crate::camera::{MapCamera, PITCH};
use crate::chart::Chart;
use crate::player::PlayerPlace;
use crate::sea::SeaConditions;
use crate::terrain::Ground;
use crate::AppState;

/// Diameter of the face, in pixels — an instrument, not a map: big enough to
/// read a letter off at a glance, small enough to sit in the corner unnoticed.
/// The letters take a fixed bite out of the rim whatever the face is, so this
/// is really the size of what is left in the middle: at sixty-four there was
/// no room to lay an arrow across the card without it fouling them.
///
/// It was eighty-eight before the card carried land. The ring wants the outer
/// band to itself and a letter has a size it is legible at, so the face had to
/// grow by about what the ring takes rather than the letters shrinking into
/// it. Larger again was tried at that size and lost: at a hundred and sixty
/// the letters are the same size in a much wider face, and the middle goes
/// hollow.
///
/// That objection is what [`spawn_bow`] answers. A card with the reader's own
/// mark standing at its centre has something for the room to be room *for*, so
/// the width the earlier trial gave back is taken again — and a little more,
/// to leave the wind somewhere to lie now that it no longer crosses the
/// middle.
const FACE_SIZE: f32 = 176.0;
/// How far the face sits in from the corner of the window.
const MARGIN: f32 = 12.0;
/// The cardinal letters' size. Not scaled with the face — see [`FACE_SIZE`] —
/// so it moves only when a letter has stopped being comfortable to read, and
/// not by whatever ratio the face last grew by.
const LETTER_SIZE: f32 = 15.0;
/// How far the letters sit in from the rim, which is the outer band given over
/// to the land ring — see [`RING_INSET`].
const INSET: f32 = 15.0;
/// Where each arm of the centre cross begins and ends, as distances from the
/// middle of the card.
///
/// A broken cross rather than two lines crossing, which is the bow's doing:
/// the mark at the centre is the one solid thing on the card, and hairlines
/// running out from under it in four directions turn it straight back into a
/// drawing of an arrow. The arms start clear of it instead, so the cross reads
/// as furniture the bow stands on rather than as part of the bow.
const CROSS_ARM: (f32, f32) = (26.0, 52.0);

/// The reader's own mark: the side, in pixels, of the square it is cut from.
///
/// Its point reaches half a diagonal from the middle and its blunt end half a
/// side, so the mark is longer ahead of the player than behind them — which is
/// the whole of how a shape this small says *this way* as well as *here*.
///
/// Nineteen was drawn and looked at, and read as a blob with a nib on it: a
/// mark this small has to be seen out of the corner of an eye, and there was
/// not enough of it either side of the point for the point to be what the eye
/// caught. What the size buys is *length* rather than bulk — it is spent
/// through [`BOW_NARROW`], which takes the width back off again.
const BOW: f32 = 27.0;

/// How much of its own width the bow keeps once it has been squeezed.
///
/// A corner of a square is a right angle, and a right angle is a *blunt* point
/// — drawn at this size it read as a lump with a nib on it, and taking the
/// flanks off square instead only made it a rounded square. What sharpens a
/// point is narrowing the shape behind it, so the mark is drawn as a square
/// stood on its corner and then squeezed across. Two thirds was tried and read
/// as a guitar pick: a right angle taken down to sixty-five degrees is still
/// blunt, and a mark only half again as long as it is wide has no length for
/// the eye to run along. Half of it leaves a point near fifty degrees on a
/// shape twice as long as it is wide, which is a dart.
///
/// That squeeze has to happen *between* the two turns — after the eighth that
/// stands the square on its corner and before the bearing — so it narrows the
/// mark across its own length rather than across the card. Which is why the
/// bow is three nested nodes and not one, the same arrangement and the same
/// reason as the face and the card it sits in.
const BOW_NARROW: f32 = 0.5;

/// How far each corner of the bow's square is taken off, as a fraction of its
/// side — point first, then the two flanks it runs back from, then the tail.
///
/// The point is left sharp, which is the only one that had to be. The flanks
/// are barely touched, so the sides run straight back from the point and carry
/// the direction; the tail is taken right off, which stops the mark reading as
/// a wedge with a base and lets it read as drawn.
const BOW_CORNERS: (f32, f32, f32) = (0.0, 0.16, 0.5);

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

/// Where the arrow's point comes to rest, as a distance from the middle of the
/// card in pixels.
///
/// The arm used to start at the centre and fly out from it, and that is what
/// made the card ambiguous. The point stops short of the bow instead and the
/// shaft runs *back* from it to windward, so the arrow arrives at the boat out
/// of the weather and goes past — which is what a wind does, and reads as
/// nothing the player is steering. The centre belongs to the bow alone.
const ARM_HEAD: f32 = 24.0;

/// How far the arm's tail lies from the middle, in pixels: at a calm, and at
/// [`FULL_WIND`]. The far end stops short of the letters — the arm is a
/// reading laid over the rose, not a hand touching its rim — and at a calm it
/// is a stub rather than nothing, so a dying wind shrinks back onto its own
/// point while the fade takes it.
///
/// A stiffening wind therefore grows *outward*, away from the boat, which is
/// the right way round: the reading lengthens without ever crowding the mark it
/// is there to be read against.
const ARM_TAIL: (f32, f32) = (34.0, 58.0);

/// The arm's thickness, in pixels: heavier than the cross, which is
/// furniture, and lighter than a letter.
const ARM_WIDTH: f32 = 2.0;

/// Each barb of the arrowhead: how long it runs, in pixels, and how far off
/// the shaft it is turned, in radians.
const BARB: (f32, f32) = (10.0, 0.55);

// The same furniture the menus are drawn with — one edge colour, one ink, one
// dimmed ink — so the instrument reads as a piece of the same chart.
const FACE: Color = Color::srgba(0.09, 0.11, 0.10, 0.60);
const EDGE: Color = Color::srgb(0.70, 0.69, 0.62);
const INK: Color = Color::srgb(0.88, 0.87, 0.80);
const INK_DIM: Color = Color::srgb(0.60, 0.60, 0.55);
/// The cross is furniture behind the letters, not a reading, so it is fainter
/// than either ink.
const CROSS: Color = Color::srgba(0.60, 0.60, 0.55, 0.45);

/// How far out land is marked, in metres.
///
/// The haze closes the picture at [`crate::HAZE_END`], so this is exactly
/// what is out there to be seen: the ring says what the player *could* have
/// noticed and did not, rather than seeing past the edge of the world. It has
/// to stay inside [`crate::terrain::STREAM_RADIUS`] or the sweep would be
/// asking about ground this machine was never sent — which is the assertion
/// below, since the two constants are set for different reasons and nothing
/// else would notice them crossing.
///
/// The assertion leaves a margin rather than allowing equality, because the
/// two radii are not struck from the same point: streaming is centred on the
/// camera's eased focus and the sweep on the player, so the far edge of the
/// sweep sits outside the far edge of the streaming by however far the focus
/// is trailing. That is a metre or so at any speed a boat makes, and half a
/// chunk is room enough for it — but at equality the guarantee is gone, and
/// an un-streamed chunk reads as open water rather than as an error.
const SIGHT: f32 = crate::HAZE_END;
const _: () = assert!(SIGHT + CHUNK_METRES / 2.0 <= crate::terrain::STREAM_RADIUS);

/// How many sectors the horizon is cut into.
///
/// The ring is a fixed pool of marks, one per sector, inked or left blank as
/// the sweep finds land — rather than marks spawned and despawned as coast
/// comes and goes, which is churn in the UI tree for a reading that changes
/// with every step anyway. Forty-eight is 7.5° apiece: fine enough that an
/// island's arc has a shape, coarse enough that a distant islet still lights
/// a mark rather than a hairline.
const SECTORS: usize = 48;

/// How far in from the rim the ring is drawn, and so how much of the outer
/// band the letters give up to it — see [`INSET`], which is the other half of
/// the same arrangement.
const RING_INSET: f32 = 6.0;
/// Where the marks lie, as a distance from the middle of the card.
const RING: f32 = FACE_SIZE / 2.0 - RING_INSET;

/// How thick a mark lies on the rim: for land the chart has not been to, and
/// for land it has.
///
/// Two channels rather than one, and both saying the same thing. Dimmed alone
/// was not enough: a near island subtends a great deal of rim, and a long arc
/// in [`INK_DIM`] sits close enough to [`EDGE`] to read as the border drawn
/// heavier rather than as coast. Thinner as well as dimmer separates them.
const BAND: (f32, f32) = (4.0, 2.0);

/// How much of the horizon one chunk may claim either side of its middle — so
/// an arc of twice this, a third of the ring.
///
/// A chunk a few paces off subtends nearly half the card, and [`subtends`] is
/// not wrong about that: the ground really does lie in all those directions.
/// But an arc that wide has stopped being a bearing, and ground that near is
/// the one thing this camera does show — the ring is for the island the
/// picture leaves out. So the clamp trims what is already under the player's
/// eye rather than what they are hunting for.
const WIDEST: f32 = TAU / 6.0;

/// How far the player moves before the ring is swept again, in metres.
///
/// The sweep walks every chunk within [`SIGHT`] and scans each one's height
/// grid, which is not work worth doing sixty times a second for an answer
/// that is already quantised to [`SECTORS`]: four metres moves a bearing by
/// less than a sector at any range the ring is drawn for, so nothing visible
/// waits on it. Ground arriving or coast being charted re-sweeps regardless —
/// both change the answer without the player having moved.
const STEP: f32 = 4.0;

/// Land the player has not charted: the reading the ring exists for, in the
/// card's own reading ink.
const NEW_LAND: Color = INK;
/// Land already on the chart. Still marked, because a bearing to somewhere
/// known is worth having, but it must not compete with the other.
const KNOWN_LAND: Color = INK_DIM;

/// The mark filling one sector of the ring, by index.
#[derive(Component)]
struct LandMark(usize);

/// What the ring was last swept against, so that standing still costs nothing.
#[derive(Resource, Default)]
struct Swept {
    /// Where the player stood, if the ring has ever been swept.
    at: Option<Vec2>,
    /// How many chunks of ground this machine held at the time — see
    /// [`mark_the_land`] for why the count and not a change tick.
    held: usize,
    /// How many chunks the chart had surveyed at the time, for the same
    /// reason and read the same way.
    surveyed: usize,
}

/// What the sweep found in one sector: how far off the nearest land in it is,
/// and whether the chart has been there.
#[derive(Clone, Copy)]
struct Sighting {
    distance: f32,
    surveyed: bool,
}

/// The ring: [`SECTORS`] marks laid round the rim, all blank until
/// [`mark_the_land`] inks them.
///
/// Children of the *card*, like the wind arm and for the same reason: the card
/// already carries the turn from the world to the view, so a mark's own
/// rotation is a bearing and nothing else. And because the card sits inside
/// the squashed face, a mark lands on the same ellipse the rim is drawn as —
/// which puts it in the picture where the ground it stands for is. The
/// foreshortening is doing real work here rather than only looking right.
///
/// Each mark is a bar lying *along* the rim rather than a tick standing off
/// it, so a run of them across the arc an island subtends reads as one band:
/// how much rim it takes says how much of the horizon that island fills, and
/// near and far separate without a second channel spent on saying so. Radial
/// ticks were tried and read as graduation — a protractor's degree marks,
/// furniture rather than a reading.
fn spawn_ring(card: &mut ChildSpawnerCommands) {
    // Wide enough to meet its neighbours: the chord a sector cuts across the
    // ring, plus a hair, so a run of marks is an arc and not a row of dashes.
    let chord = 2.0 * RING * (TAU / SECTORS as f32 / 2.0).sin() + 1.0;
    for sector in 0..SECTORS {
        card.spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            UiTransform {
                rotation: Rot2::radians(sector as f32 * TAU / SECTORS as f32),
                ..UiTransform::IDENTITY
            },
        ))
        .with_children(|spot| {
            spot.spawn((
                LandMark(sector),
                Node {
                    width: Val::Px(chord),
                    height: Val::Px(BAND.0),
                    ..default()
                },
                BackgroundColor(Color::NONE),
                UiTransform {
                    translation: Val2::px(0.0, -RING),
                    ..UiTransform::IDENTITY
                },
            ));
        });
    }
}

/// The bearing of a direction on the card: clockwise from north, the way
/// [`arm_bearing`] takes one and the way [`UiTransform`] turns.
fn bearing(direction: Vec2) -> f32 {
    let east = Vec2::new(-NORTH.y, NORTH.x);
    f32::atan2(direction.dot(east), direction.dot(NORTH))
}

/// Which sector a bearing falls in.
fn sector_of(bearing: f32) -> usize {
    let turn = (bearing / TAU).rem_euclid(1.0);
    ((turn * SECTORS as f32) as usize).min(SECTORS - 1)
}

/// The arc one chunk of ground subtends from `at`: its middle bearing, and
/// how far its corners reach either side of that.
///
/// Taken relative to the middle rather than as four bearings in their own
/// right, so a chunk lying across north — where bearings wrap — needs no
/// special case. Clamped to [`WIDEST`].
fn subtends(at: Vec2, chunk: IVec2) -> (f32, f32) {
    let corner = chunk.as_vec2() * CHUNK_METRES;
    let middle = bearing(corner + CHUNK_METRES / 2.0 - at);
    let mut widest: f32 = 0.0;
    for x in [corner.x, corner.x + CHUNK_METRES] {
        for y in [corner.y, corner.y + CHUNK_METRES] {
            let offset = bearing(Vec2::new(x, y) - at) - middle;
            // Back into (-pi, pi]: the difference of two bearings is an angle
            // rather than a bearing, and a chunk either side of north would
            // otherwise read as most of a turn wide.
            let offset = offset - TAU * (offset / TAU).round();
            widest = widest.max(offset.abs());
        }
    }
    (middle, widest.min(WIDEST))
}

/// What land there is round the player, by sector: the nearest in each, and
/// whether the chart has it.
///
/// Read off the chunks this machine was already sent, so the ring costs the
/// session nothing — a client written against the protocol alone could draw
/// the same one. A chunk counts as land only where some corner of it stands
/// above the sea: the server sends a payload for the shelf around an island
/// as well as for the island, and a ring lighting up for drowned ground would
/// be marking water.
///
/// The *nearest* land wins a sector rather than the newest, which decides the
/// one case where the two disagree: a charted island with something unknown
/// behind it. The near shore is what a player looking that way would find, so
/// the ring says so — the same thing the eye would, if the camera let it see
/// that far.
fn land_in_sight(ground: &Ground, chart: &Chart, at: Vec2) -> [Option<Sighting>; SECTORS] {
    let mut found = [None; SECTORS];
    let reach = (SIGHT / CHUNK_METRES).ceil() as i32;
    let home = (at / CHUNK_METRES).floor().as_ivec2();
    for dz in -reach..=reach {
        for dx in -reach..=reach {
            let chunk = home + IVec2::new(dx, dz);
            if !ground.above_water(chunk) {
                continue;
            }
            let corner = chunk.as_vec2() * CHUNK_METRES;
            let distance = at.clamp(corner, corner + CHUNK_METRES).distance(at);
            if distance > SIGHT {
                continue;
            }
            // The square the player is standing inside is skipped, not
            // clamped: there is no bearing to the ground underfoot, and
            // `subtends` would hand back whichever way the arithmetic fell out
            // of a zero-length direction, widened to `WIDEST` — a dim band
            // over a third of the ring, aimed at nothing, sitting on top of
            // the coast the ring is drawn to show. The chunks around it
            // describe that shore honestly.
            if distance == 0.0 {
                continue;
            }
            let sighting = Sighting {
                distance,
                surveyed: chart.surveyed(chunk),
            };
            let (middle, half) = subtends(at, chunk);
            // Walked in whole sectors rather than in radians, so both ends of
            // the arc are certainly lit whatever it rounds to.
            let first = sector_of(middle - half) as i32;
            let steps = ((2.0 * half) / (TAU / SECTORS as f32)).ceil() as i32;
            for step in 0..=steps {
                let sector = (first + step).rem_euclid(SECTORS as i32) as usize;
                if found[sector].is_none_or(|had: Sighting| distance < had.distance) {
                    found[sector] = Some(sighting);
                }
            }
        }
    }
    found
}

/// How one sector is drawn: its ink, and how thick a band it lays on the rim.
fn mark_drawn(sighting: Option<Sighting>) -> (Color, f32) {
    match sighting {
        None => (Color::NONE, BAND.1),
        Some(seen) if seen.surveyed => (KNOWN_LAND, BAND.1),
        Some(_) => (NEW_LAND, BAND.0),
    }
}

/// Sweeps the ground round the player and inks the ring for what it finds.
///
/// Held off until the player has moved [`STEP`], or until more ground has
/// arrived — a client that drops anchor while its neighbourhood is still
/// coming in must not hold a ring drawn before the island did.
///
/// The count of chunks held, rather than [`DetectChanges::is_changed`] on
/// [`Ground`]: `terrain`'s streaming takes it as `ResMut` and walks it every
/// frame whether or not anything came, so the change tick is always set and a
/// guard on it would quietly never fire. Counting is honest about what the
/// sweep actually depends on. It cannot tell an arrival from a departure in
/// the same frame, but a chunk only leaves when the player has moved far
/// enough to have tripped [`STEP`] several times over.
///
/// Coast being charted is watched the same way, and has to be: the chart
/// takes only a few soundings a frame and leaves the rest of the queue for
/// later, so a survey finishes well after the movement that earned it. A
/// player who drops anchor as their neighbourhood streams in would otherwise
/// sit looking at an island drawn as uncharted, already surveyed, until they
/// got under way again. The count of soundings rather than the whole tally:
/// that walks every coastline the player holds, which is not a thing to do
/// inside the guard whose job is to make standing still free.
fn mark_the_land(
    ground: Res<Ground>,
    chart: Res<Chart>,
    player: PlayerPlace,
    mut swept: ResMut<Swept>,
    mut marks: Query<(&LandMark, &mut Node, &mut BackgroundColor)>,
) {
    let Some(at) = player.on_the_map() else {
        return;
    };
    // Ashore the ring goes out. It exists for the land this camera does not
    // show, and a player standing on an island is looking at the island —
    // every mark it could draw would be for ground already filling the
    // picture, or for the hill they are walking up. Wiped rather than frozen,
    // and the sweep forgotten with it, so stepping back aboard reads the
    // horizon afresh instead of showing where the shore was when they landed.
    if !player.aboard() {
        if swept.at.is_some() {
            *swept = Swept::default();
            for (_, mut node, mut colour) in &mut marks {
                node.height = Val::Px(BAND.1);
                *colour = BackgroundColor(Color::NONE);
            }
        }
        return;
    }
    let held = ground.land_held();
    let surveyed = chart.surveys();
    let stood_still = swept
        .at
        .is_some_and(|last| last.distance_squared(at) < STEP * STEP);
    if stood_still && held == swept.held && surveyed == swept.surveyed {
        return;
    }
    swept.at = Some(at);
    swept.held = held;
    swept.surveyed = surveyed;

    let sightings = land_in_sight(&ground, &chart, at);
    for (mark, mut node, mut colour) in &mut marks {
        let (ink, band) = mark_drawn(sightings[mark.0]);
        // Written only where they differ: a `Mut` counts as changed the
        // moment it is dereferenced, and a sector whose reading has not moved
        // would put the whole ring through a relayout for nothing. Most
        // sweeps find most of the card saying what it said.
        let height = Val::Px(band);
        if node.height != height {
            node.height = height;
        }
        if colour.0 != ink {
            *colour = BackgroundColor(ink);
        }
    }
}
pub struct CompassPlugin;

impl Plugin for CompassPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SeaConditions>()
            .init_resource::<Swept>()
            .add_systems(OnEnter(AppState::InWorld), spawn_compass)
            .add_systems(
                Update,
                (
                    (turn_card, point_the_arm, point_the_bow),
                    // The ring reads both, and both come and go with the
                    // world — as the chart's own survey does.
                    mark_the_land
                        .run_if(resource_exists::<Chart>.and_then(resource_exists::<Ground>)),
                )
                    .run_if(in_state(AppState::InWorld)),
            );
    }
}

/// Marks the rotating card inside the face, so [`turn_card`] can find it.
#[derive(Component)]
struct CompassCard;

/// Marks the face — the tilted, stationary dial the card spins inside.
#[derive(Component)]
struct CompassFace;

/// Marks the bow — the reader's own mark at the middle of the card, spun to
/// their heading the same way and for the same reason the arm is.
#[derive(Component)]
struct Bow;

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

fn spawn_compass(mut commands: Commands, mut swept: ResMut<Swept>) {
    // A new world is a new sweep: the ring respawns blank, and a position
    // left over from the last one would hold it that way until the player
    // had moved.
    *swept = Swept::default();
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
                // The card in two halves with the two readings between them:
                // under the letters rather than over them, because where the
                // arm reaches its furthest it is nearly touching one, and an
                // arrowhead drawn across a glyph would cost the letter more
                // than it bought the arm.
                spawn_cross(card);
                spawn_arm(card);
                spawn_bow(card);
                spawn_ring(card);
                spawn_letters(card);
            });
        });
}

/// The four cardinal letters, N in the reading ink and the rest dimmed.
///
/// The other three are dimmed rather than dropped: they make N mean north
/// rather than "this way", and a turn read against them says how far round it
/// went.
fn spawn_letters(card: &mut ChildSpawnerCommands) {
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
            // The serif the menus resolve, for the same reason they do: a card
            // is a piece of chart furniture, and the machine's serif is the
            // hand charts are lettered in.
            TextFont {
                font: FontSource::Serif,
                font_size: FontSize::Px(LETTER_SIZE),
                ..default()
            },
            TextColor(ink),
        ));
    });
}

/// The hairline cross behind the letters, drawn as four arms standing off the
/// middle so the card reads as an instrument rather than as four floating
/// letters — see [`CROSS_ARM`] for why they stand off it.
///
/// A cross and not a star. The chart's rose is a sixteen-point star, and this
/// one deliberately is not: it is [`FACE_SIZE`] pixels squashed to the
/// camera's pitch with a bow and an arrow lying on it, and a star drawn under
/// those is a smudge they have to be picked out of. The flourish belongs where
/// the paper is looked at rather than glanced at.
fn spawn_cross(card: &mut ChildSpawnerCommands) {
    let (inner, outer) = CROSS_ARM;
    let (length, middle) = (outer - inner, (inner + outer) / 2.0);
    for (width, height, offset) in [
        (1.0, length, Val2::px(0.0, -middle)),
        (1.0, length, Val2::px(0.0, middle)),
        (length, 1.0, Val2::px(-middle, 0.0)),
        (length, 1.0, Val2::px(middle, 0.0)),
    ] {
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
                UiTransform {
                    translation: offset,
                    ..UiTransform::IDENTITY
                },
            ));
        });
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
                height: Val::Px(ARM_TAIL.0 - ARM_HEAD),
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

/// The reader's own mark at the middle of the card, spun to their heading by
/// [`point_the_bow`].
///
/// A child of the card for the arm's reason: the card carries the turn from
/// the world to the view already, so this node's own rotation is a bearing and
/// nothing else.
///
/// Cut from a square, stood on the corner left sharp and squeezed across it —
/// see [`BOW_NARROW`] for the squeeze and [`BOW_CORNERS`] for what is taken
/// off the other three. Nothing here is drawn from triangles because a UI node
/// is a rectangle and there are none to be had; a mesh would want a second
/// camera over the world for one mark.
///
/// What matters most is not the shape but that it is **solid**, the wind arrow
/// being strokes. That contrast is doing as much work as the bearing is: it is
/// what stops the eye having to ask which arrow it is looking at, which is the
/// question the card used to leave hanging.
fn spawn_bow(card: &mut ChildSpawnerCommands) {
    card.spawn(Node {
        position_type: PositionType::Absolute,
        width: Val::Percent(100.0),
        height: Val::Percent(100.0),
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        ..default()
    })
    .with_children(|spot| {
        // Three nodes, one turn each, innermost first: the square is stood on
        // its corner, the result is squeezed across, and the whole thing is
        // laid on the bearing. See [`BOW_NARROW`] for why they cannot be one.
        spot.spawn((
            Bow,
            Node {
                width: Val::Px(BOW),
                height: Val::Px(BOW),
                ..default()
            },
            UiTransform::IDENTITY,
        ))
        .with_children(|turned| {
            turned
                .spawn((
                    Node {
                        width: Val::Percent(100.0),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    UiTransform {
                        scale: Vec2::new(BOW_NARROW, 1.0),
                        ..UiTransform::IDENTITY
                    },
                ))
                .with_children(|narrowed| {
                    narrowed.spawn((
                        Node {
                            width: Val::Percent(100.0),
                            height: Val::Percent(100.0),
                            border_radius: BorderRadius {
                                top_left: Val::Percent(BOW_CORNERS.0 * 100.0),
                                top_right: Val::Percent(BOW_CORNERS.1 * 100.0),
                                bottom_left: Val::Percent(BOW_CORNERS.1 * 100.0),
                                bottom_right: Val::Percent(BOW_CORNERS.2 * 100.0),
                            },
                            ..default()
                        },
                        BackgroundColor(INK),
                        UiTransform {
                            rotation: Rot2::radians(TAU / 8.0),
                            ..UiTransform::IDENTITY
                        },
                    ));
                });
        });
    });
}

/// The turn that lays the bow along a heading.
///
/// The bearing and nothing else: the eighth of a turn that brings the square's
/// sharp corner round to the mark's own point is spent inside [`spawn_bow`],
/// where the squeeze can be applied after it.
fn bow_rotation(heading: Vec2) -> Rot2 {
    Rot2::radians(bearing(heading))
}

/// Lays the bow along the way the player is actually pointing — the hull's
/// bow, or the walker's own face, whichever is carrying them.
///
/// Hidden rather than left where it was when there is no heading to draw: a
/// mark saying *here, this way* in the middle of the card is a claim, and a
/// stale one would be read as confidently as a true one. Ashore it stays, the
/// ring being the only reading a beach makes nonsense of.
fn point_the_bow(
    player: PlayerPlace,
    mut bows: Query<(&mut UiTransform, &mut Visibility), With<Bow>>,
) {
    let heading = player.heading();
    for (mut transform, mut visibility) in &mut bows {
        match heading {
            Some(heading) => {
                *visibility = Visibility::Inherited;
                transform.rotation = bow_rotation(heading);
            }
            None => *visibility = Visibility::Hidden,
        }
    }
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
    let length = ARM_TAIL.0 + (ARM_TAIL.1 - ARM_TAIL.0) * hard - ARM_HEAD;
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
        // Pushed *down* its own axis — to windward, the arm having been turned
        // to the way the air is going — so the shaft lies between [`ARM_HEAD`]
        // and the tail rather than across the middle. Its top edge is then the
        // end nearest the bow, which is where the barbs are hung: they open
        // back towards the tail, so the point is the inner end and the arrow
        // flies at the boat.
        transform.translation = Val2::px(0.0, ARM_HEAD + length / 2.0);
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
    use crate::player::Player;
    use crate::Helm;
    use bevy::input::mouse::AccumulatedMouseScroll;
    use bevy::state::app::StatesPlugin;
    use bevy::time::TimePlugin;
    use protocol::ground::{quantize, ChunkPayload, Surface, Tone, FACET_TRIS, FACET_VERTS};

    /// A chunk standing well clear of the water.
    fn a_hill() -> ChunkPayload {
        payload(40.0)
    }

    /// A chunk of sea bed: ground the server sent, with none of it above the
    /// waterline.
    fn a_shoal() -> ChunkPayload {
        payload(-3.0)
    }

    fn payload(height: f32) -> ChunkPayload {
        ChunkPayload {
            heights: vec![quantize(height); FACET_VERTS * FACET_VERTS],
            surfaces: vec![Surface::plain(Tone::Grass); FACET_TRIS],
            water: None,
            plants: Vec::new(),
        }
    }

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

    /// Where the bow's point lands on the card under a heading, as a unit
    /// vector in the card's own pixels — the sharp corner of the square, put
    /// through the turn [`point_the_bow`] gives it. Screen coordinates, so y
    /// climbs downwards and the rotation runs clockwise.
    fn bow_point(heading: Vec2) -> Vec2 {
        let (sin, cos) = bow_rotation(heading).as_radians().sin_cos();
        // Up the node's own axis, the eighth of a turn that put it there
        // having been spent inside `spawn_bow`.
        let point = Vec2::new(0.0, -1.0);
        Vec2::new(point.x * cos - point.y * sin, point.x * sin + point.y * cos)
    }

    #[test]
    fn the_bow_points_the_way_the_player_is_headed() {
        // Steering north lays the point at the top of the card, under the N.
        let north = bow_point(NORTH);
        assert!(
            north.x.abs() < 1e-5 && north.y < 0.0,
            "north did not read as up the card: {north}"
        );

        // And east a quarter turn clockwise of that, where the E is — the
        // reading most likely to have come out mirrored.
        let east = bow_point(Vec2::new(-NORTH.y, NORTH.x));
        assert!(
            east.y.abs() < 1e-5 && east.x > 0.0,
            "east did not read as across the card: {east}"
        );
    }

    #[test]
    fn the_bow_stays_off_the_card_with_nobody_to_draw_it_for() {
        // A mark saying *here, this way* is a claim, and there is nobody in
        // this world to make it about.
        let mut app = test_app();
        app.update();
        let visibility = *app
            .world_mut()
            .query_filtered::<&Visibility, With<Bow>>()
            .single(app.world())
            .expect("the bow should exist");
        assert_eq!(visibility, Visibility::Hidden);
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
        assert!(
            ARM_HEAD + fresh <= ARM_TAIL.1,
            "the arm outgrew the room it has"
        );

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

    /// The sign conventions the ring shares with the arm, checked where they
    /// are cheap to check: a mark drawn on the wrong side of the card is
    /// exactly the mistake a screenshot would be read straight past.
    #[test]
    fn a_chunk_lights_the_sector_it_lies_in() {
        // Standing in the middle of chunk zero, so a chunk straight up the
        // grid is straight north rather than half a chunk off it.
        let at = Vec2::splat(CHUNK_METRES / 2.0);

        // Ten chunks due north, which is `-Y` — the top of the card, and so
        // sector zero.
        let (middle, half) = subtends(at, IVec2::new(0, -10));
        assert!(middle.abs() < 1e-5, "north did not read as straight up");
        assert!(half > 0.0 && half < WIDEST);
        assert_eq!(sector_of(middle), 0);

        // And ten chunks due east, a quarter of the way round clockwise.
        assert_eq!(sector_of(subtends(at, IVec2::new(10, 0)).0), SECTORS / 4);

        // A chunk lying across north spans the wrap without claiming most of
        // the horizon — the bug the middle-relative arithmetic is there for.
        let (_, across) = subtends(at, IVec2::new(-1, -10));
        assert!(
            across < 0.2,
            "a chunk near north subtended {across} radians"
        );
    }

    /// Uncharted land is the louder mark on both channels, which is the whole
    /// reason the ring is drawn.
    #[test]
    fn charted_land_is_drawn_quieter_than_new() {
        // The far sighting is the new one and the near sighting the charted
        // one, so what separates these two is new against known and not close
        // against distant. Distance decides nothing: an arc faded by range was
        // tried and drew the land most worth noticing faintest.
        let new = mark_drawn(Some(Sighting {
            distance: 800.0,
            surveyed: false,
        }));
        let known = mark_drawn(Some(Sighting {
            distance: 100.0,
            surveyed: true,
        }));
        assert_ne!(new.0, known.0, "new and charted land drew in one ink");
        assert!(new.1 > known.1, "new land drew no heavier than charted");
    }

    /// A hill on one chunk lights the sectors it stands in and no others.
    #[test]
    fn a_sweep_marks_the_bearing_land_lies_on() {
        let mut ground = Ground::default();
        // Due east of the origin, well inside sight.
        let island = IVec2::new(3, 0);
        ground.deliver(island, Some(a_hill()));

        let found = land_in_sight(&ground, &Chart::default(), Vec2::splat(CHUNK_METRES / 2.0));
        let lit: Vec<usize> = (0..SECTORS).filter(|s| found[*s].is_some()).collect();
        assert!(!lit.is_empty(), "land due east lit nothing");
        assert!(
            lit.contains(&(SECTORS / 4)),
            "land due east lit {lit:?} rather than the east sector"
        );
        // Nothing anywhere else on the card: an unswept ocean must not read
        // as coast.
        for quarter in [0, SECTORS / 2, 3 * SECTORS / 4] {
            assert!(
                found[quarter].is_none(),
                "sector {quarter} lit with no land"
            );
        }
        // Nothing charted, so all of it reads as new.
        assert!(lit.iter().all(|s| !found[*s].unwrap().surveyed));
    }

    /// Ashore, the ground underfoot is not a bearing: a card that read it as
    /// one would lay a band across a third of the ring that no real coast
    /// could displace.
    #[test]
    fn the_ground_underfoot_marks_nothing() {
        let mut ground = Ground::default();
        ground.deliver(IVec2::ZERO, Some(a_hill()));

        let found = land_in_sight(&ground, &Chart::default(), Vec2::splat(CHUNK_METRES / 2.0));
        assert!(
            found.iter().all(Option::is_none),
            "the chunk the player stands on claimed a bearing"
        );
    }

    /// Stands a player in a world with one island due east of them, aboard a
    /// boat or on their own feet, and runs a frame.
    fn a_player_off_an_island(aboard: bool) -> App {
        let mut app = test_app();

        let mut ground = Ground::default();
        ground.deliver(IVec2::new(3, 0), Some(a_hill()));
        app.insert_resource(ground);
        app.insert_resource(Chart::default());

        let at = Transform::from_xyz(CHUNK_METRES / 2.0, 0.0, CHUNK_METRES / 2.0);
        let player = app.world_mut().spawn((Player, at)).id();
        if aboard {
            // A hull under them: being aboard is being somebody's child, which
            // is what `PlayerPlace` resolves a carrier through.
            let boat = app.world_mut().spawn(at).id();
            app.world_mut().entity_mut(player).insert(ChildOf(boat));
        }
        app.update();
        app
    }

    /// How many marks the ring is showing.
    fn marks_lit(app: &mut App) -> usize {
        app.world_mut()
            .query::<(&LandMark, &BackgroundColor)>()
            .iter(app.world())
            .filter(|(_, colour)| colour.0.alpha() > 0.0)
            .count()
    }

    /// The ring is a sailing instrument. Ashore everything it could mark is
    /// the island underfoot or in plain view across it, so it goes out — and
    /// comes back on boarding rather than staying wiped.
    #[test]
    fn the_ring_is_only_drawn_afloat() {
        let mut afloat = a_player_off_an_island(true);
        assert!(marks_lit(&mut afloat) > 0, "afloat, land east lit nothing");

        let mut ashore = a_player_off_an_island(false);
        assert_eq!(
            marks_lit(&mut ashore),
            0,
            "ashore, the ring was still reading"
        );
    }

    /// The two answers the sweep must refuse: drowned ground, and land past
    /// the haze.
    #[test]
    fn only_land_within_sight_is_marked() {
        let at = Vec2::splat(CHUNK_METRES / 2.0);
        let chart = Chart::default();

        // A chunk the server sent for the shelf around an island, every corner
        // of it under water. Ground, but not land.
        let mut shelf = Ground::default();
        shelf.deliver(IVec2::new(3, 0), Some(a_shoal()));
        assert!(
            land_in_sight(&shelf, &chart, at)
                .iter()
                .all(Option::is_none),
            "drowned ground was marked as coast"
        );

        // And an island past the haze, which the player has no way of having
        // seen — the card would be claiming second sight. Inside the square
        // of chunks the sweep walks, so it is the range that turns this one
        // away and not the walk running out: its near edge is sixty metres
        // beyond sight.
        let beyond = (SIGHT / CHUNK_METRES).ceil() as i32;
        let mut far = Ground::default();
        far.deliver(IVec2::new(beyond, 0), Some(a_hill()));
        assert!(
            land_in_sight(&far, &chart, at).iter().all(Option::is_none),
            "land past the haze was marked"
        );
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
