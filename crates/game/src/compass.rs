//! The compass: a small card in a corner of the screen with its N pinned to
//! the world's north.
//!
//! Both the boat and the view can turn, so neither would make a steady
//! compass. The card is aligned with the world instead — its N sits over
//! [`protocol::ground::NORTH`] however the view is spun — so a bearing read
//! off it means the same thing to every player in the world. It follows the
//! camera's eased yaw rather than the target, so it swings with the picture
//! through a turn instead of arriving ahead of it.
//!
//! The card is drawn *lying on the sea* rather than flat on the glass: against
//! ground drawn at the camera's pitch an unforeshortened dial reads as a
//! sticker on the screen. The tilt is the projection itself — a flat card on
//! the ground plane seen from [`crate::camera::PITCH`] above horizontal is its
//! upright drawing squashed by `sin(PITCH)`, applied after the card's
//! own spin so the letters shear the way paint on a deck would. Real geometry
//! parented to the camera differs by a keystone too small to see, and would
//! need letters as meshes and an exemption from the fog and the lighting.
//!
//! Three readings ride on it besides north. The bow, at the middle, drawn as
//! the chart's [`READERS_MARK`] so one glyph means *you* on every instrument.
//! The wind, a stream of chevrons crossing the whole card and flying leeward,
//! its strength in pace and ink rather than in reach — an arm that grew with
//! the wind was tried, and a lone length has nothing on the card to be read
//! against, so a fresh breeze and a light air looked alike. And a ring of arcs
//! round the rim, one per stretch of coast within sight.
//!
//! The bow and the wind have to be told apart at a glance or the card is worse
//! than nothing, and what separates them is kind rather than position: the bow
//! is the one solid, stationary thing on the card and the wind is stroked and
//! *moving*. That is what buys the wind the centre — a stream can run under
//! the mark without being mistaken for it, where a second arrow could not —
//! and the chevrons point the way they fly, so a screenshot still reads.
//!
//! The ring answers what the picture cannot. This camera looks *down*, so land
//! a few hundred metres off can be outside the frame while the player is close
//! enough to walk up its beach. It marks the uncharted louder than the charted,
//! the problem being finding *new* islands; it reaches exactly as far as the
//! haze does, rain and all (see [`SIGHT`]), saying what the player could have
//! noticed and not what is over the horizon; and it goes out ashore, where everything it
//! could mark is either the island underfoot or in plain view across it.
//! Nothing about it crosses the wire — the sweep reads the chunks this machine
//! was already sent.
use std::f32::consts::TAU;

use bevy::image::Image;
use bevy::prelude::*;
use bevy::text::{FontSize, FontSource};
use bevy::ui::{UiTransform, Val2};

use protocol::ground::{CHUNK_METRES, NORTH};

use crate::camera::{MapCamera, PITCH};
use crate::chart::{Chart, READERS_MARK};
use crate::player::PlayerPlace;
use crate::sea::{SeaConditions, WIND_NAMED};
use crate::terrain::Ground;
use crate::{AppState, EDGE, FACE, INK, INK_DIM};

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
/// to give the wind's stream a run worth watching either side of the mark.
///
/// Public because the instruments beside it are placed off it — see
/// [`crate::instruments`], which stands the lead and the day's arc clear of
/// the card rather than writing down where the corner's furniture ends.
pub(crate) const FACE_SIZE: f32 = 176.0;
/// How far the face sits in from the corner of the window. Shared with the
/// instruments beside it, for [`FACE_SIZE`]'s reason.
pub(crate) const MARGIN: f32 = 12.0;
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

/// The reader's own mark: how tall it stands on the card, point to tail, in
/// pixels. About what the dart before it measured, which was the length a
/// mark this small needed before the eye caught it — see [`spawn_bow`] for
/// what the mark is now and where its shape lives.
const BOW_HEIGHT: f32 = 30.0;

/// Texels across the bow's little texture, drawn once at [`spawn_bow`].
///
/// Generous for the size the mark is laid out at, deliberately: the whole UI
/// scales up with the window — see [`crate::settings`] — and a texture drawn
/// for the laid-out size would soften the point on any monitor bigger than
/// the layouts were drawn for.
const BOW_TEXELS: u32 = 112;

/// The wind the stream is drawn at its full ink for, in metres per second — a
/// fresh breeze rather than the hardest wind there is. Where the scale ends
/// is the client's to pick: the server sends a velocity and has no opinion
/// about how hard that should look, and a scale that only filled at a rare
/// gale would spend most of a day reading as a light air. Above this the ink
/// has nowhere further to go, which is the right lie for an instrument this
/// size — past a fresh breeze the sea itself is saying the rest, though the
/// pace, which costs the card nothing, keeps counting: see [`DRIFT`].
const FULL_WIND: f32 = 10.0;

/// How far either side of the middle the stream runs, in pixels. Its ends
/// stop short of the letters — the stream is a reading laid over the rose,
/// not a hand touching its rim — but between them it crosses the whole card:
/// a line through the centre reads as *weather over the place*, where the
/// arm this replaced, hung off one side of the middle, kept being read as a
/// thing standing at a bearing.
const TRACK: f32 = 58.0;

/// How many chevrons ride the stream. Their spacing falls out as the track
/// over the count, so the line stays evenly manned however the two are tuned.
const CHEVRONS: usize = 8;

/// One chevron of the stream: the length of each of its two bars, in pixels,
/// and how far off the line of flight each is turned, in radians. A little
/// wider-set than the barbs of the arrowhead this grew out of: with no shaft
/// to be read against, the vee is the whole glyph.
const CHEVRON: (f32, f32) = (9.0, 0.6);

/// The stream's stroke, in pixels: heavier than the cross, which is
/// furniture, and lighter than a letter.
const STROKE: f32 = 2.0;

/// The pace a wind puts on the stream, in pixels per second for each metre
/// per second of it. Never clamped, unlike the ink: past [`FULL_WIND`] the
/// stream has no more darkness to add, but the pace keeps telling the truth.
const DRIFT: f32 = 4.0;

/// How much of each end of the track is spent easing a chevron in or out, in
/// pixels. The fade is what hides the wrap: a chevron leaves to leeward
/// already faded to nothing and is next seen growing in to windward, so the
/// pool circulates without a mark ever popping into place.
const END_FADE: f32 = 12.0;

/// The cross is furniture behind the letters, not a reading, so it is fainter
/// than either ink.
const CROSS: Color = Color::srgba(0.60, 0.60, 0.55, 0.45);

/// How far out land is marked under a dry sky, in metres — rain closes it in
/// with the haze, to [`crate::sky::sight`].
///
/// The haze closes the picture at [`crate::HAZE_END`], so this is exactly
/// what is out there to be seen: the ring says what the player *could* have
/// noticed and did not, rather than seeing past the edge of the world. In a
/// squall that is a couple of hundred metres, and an island being steered
/// for goes off the ring as it goes from the picture. It has
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

/// How far the sight must have moved, in metres, before the ring is swept
/// again for it — a rain coming on closes it by most of a kilometre in a few
/// seconds, which is not a sweep a frame.
const SIGHT_STEP: f32 = 16.0;

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
    /// How far the haze let the player see — see [`crate::sky::sight`].
    sight: f32,
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
/// Children of the *card*, like the wind stream and for the same reason: the card
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
/// [`wind_bearing`] takes one and the way [`UiTransform`] turns.
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
fn land_in_sight(
    ground: &Ground,
    chart: &Chart,
    at: Vec2,
    sight: f32,
) -> [Option<Sighting>; SECTORS] {
    let mut found = [None; SECTORS];
    let sight = sight.min(SIGHT);
    let reach = (sight / CHUNK_METRES).ceil() as i32;
    let home = (at / CHUNK_METRES).floor().as_ivec2();
    for dz in -reach..=reach {
        for dx in -reach..=reach {
            let chunk = home + IVec2::new(dx, dz);
            if !ground.above_water(chunk) {
                continue;
            }
            let corner = chunk.as_vec2() * CHUNK_METRES;
            let distance = at.clamp(corner, corner + CHUNK_METRES).distance(at);
            if distance > sight {
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
    conditions: Res<SeaConditions>,
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
    let sight = crate::sky::sight(conditions.rain());
    let stood_still = swept
        .at
        .is_some_and(|last| last.distance_squared(at) < STEP * STEP);
    if stood_still
        && held == swept.held
        && surveyed == swept.surveyed
        && (sight - swept.sight).abs() < SIGHT_STEP
    {
        return;
    }
    swept.at = Some(at);
    swept.held = held;
    swept.surveyed = surveyed;
    swept.sight = sight;

    let sightings = land_in_sight(&ground, &chart, at, sight);
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
                    (turn_card, drive_the_stream, point_the_bow),
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
/// their heading the same way and for the same reason the stream is.
#[derive(Component)]
struct Bow;

/// The wind stream — the node that spins inside the card, on top of the
/// card's own spin, so the bearing it shows is the world's and not the
/// view's. It carries the drift: how far along the track the pool has been
/// blown, kept wrapped to a lap by [`drive_the_stream`].
#[derive(Component)]
struct WindStream {
    phase: f32,
}

/// One chevron of the stream, by its place in the pool.
#[derive(Component)]
struct Chevron(usize);

/// One bar of a chevron's vee — the pieces that take ink — named by the
/// chevron it belongs to, so each chevron can carry its own fade.
#[derive(Component)]
struct ChevronInk(usize);

fn spawn_compass(
    mut commands: Commands,
    mut swept: ResMut<Swept>,
    mut images: ResMut<Assets<Image>>,
) {
    // A new world is a new sweep: the ring respawns blank, and a position
    // left over from the last one would hold it that way until the player
    // had moved.
    *swept = Swept::default();
    let images = &mut *images;
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
                // stream reaches its furthest it is nearly touching one, and
                // a chevron drawn across a glyph would cost the letter more
                // than it bought the stream. The bow after the stream, so the
                // chevrons pass under the mark rather than over it.
                spawn_cross(card);
                spawn_stream(card);
                spawn_bow(card, images);
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

/// The wind stream: a fixed pool of chevrons laid along one line through the
/// middle of the card, spun to the wind's bearing and blown along it by
/// [`drive_the_stream`].
///
/// A child of the card rather than of the face, which is what makes it a
/// *bearing* rather than a picture of where the wind is on screen: the card
/// already carries the turn from the world to the view, so this node's own
/// rotation is the wind's angle from north and nothing else, and the two
/// compose the way they do on paper.
///
/// A pool that wraps rather than chevrons spawned and despawned as they run
/// off the end — the ring's arrangement, for the ring's reason: churn in the
/// UI tree for a population that never changes.
fn spawn_stream(card: &mut ChildSpawnerCommands) {
    card.spawn((
        WindStream { phase: 0.0 },
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
    .with_children(|stream| {
        for place in 0..CHEVRONS {
            stream
                .spawn((
                    Chevron(place),
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
                .with_children(|spot| {
                    // A zero-height anchor at the spot's middle: the bars pin
                    // their upper ends to its top edge, which *is* the
                    // middle, so the vee's point is the piece the offset
                    // places and the vee opens back to windward behind it.
                    spot.spawn(Node {
                        width: Val::Px(STROKE),
                        height: Val::Px(0.0),
                        ..default()
                    })
                    .with_children(|anchor| {
                        spawn_vee_bar(anchor, place, 1.0);
                        spawn_vee_bar(anchor, place, -1.0);
                    });
                });
        }
    });
}

/// One bar of a chevron's vee, hung off its anchor's top edge.
///
/// The bar is drawn straight and turned about its own middle, which swings
/// the end that was at the point away from it. The translation is what puts
/// that end back — rotate, then undo the movement of the one end that is
/// meant to stay still — and it is the reason a bar is a transform rather
/// than a position.
fn spawn_vee_bar(anchor: &mut ChildSpawnerCommands, chevron: usize, side: f32) {
    let (length, angle) = CHEVRON;
    let turn = side * angle;
    anchor.spawn((
        ChevronInk(chevron),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            top: Val::Px(0.0),
            width: Val::Px(STROKE),
            height: Val::Px(length),
            ..default()
        },
        // Blank until the first frame inks it, like the ring's marks.
        BackgroundColor(Color::NONE),
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
/// A child of the card for the stream's reason: the card carries the turn from
/// the world to the view already, so this node's own rotation is a bearing and
/// nothing else.
///
/// The glyph is the chart's [`READERS_MARK`], rasterised here into a little
/// texture and tinted with the card's ink. A notched arrowhead is concave and
/// a UI node is a rectangle, so it is nothing a pile of nodes can be: the
/// dart that used to stand here was the best three of them could do, and it
/// read as a leaf. A mesh would want a second camera over the world for one
/// mark; an image is just an asset, and it lets the card wear the exact
/// glyph the chart does rather than a cousin of it.
///
/// The texture spans the mark's furthest reach in every direction, so the
/// origin the glyph turns about on the chart sits at the node's centre and
/// the one rotation serves both instruments.
///
/// What matters most is not the shape but that it is **solid and still**, the
/// wind being strokes on the move. That contrast is doing as much work as the
/// bearing is — see the module doc for the card it rescued.
fn spawn_bow(card: &mut ChildSpawnerCommands, images: &mut Assets<Image>) {
    // The mark in card pixels: sized by its height, its box by its reach.
    let (reach, tall) = mark_measure();
    let span = BOW_HEIGHT / tall * (2.0 * reach);
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
            Bow,
            Node {
                width: Val::Px(span),
                height: Val::Px(span),
                ..default()
            },
            ImageNode {
                image: images.add(bow_image()),
                color: INK,
                ..default()
            },
            UiTransform::IDENTITY,
        ));
    });
}

/// The mark's measurements in its own units: its furthest reach from the
/// origin on either axis, and its height point to tail. Taken off the corners
/// rather than written down again, so the mark cannot quietly outgrow its
/// texture.
fn mark_measure() -> (f32, f32) {
    let reach = READERS_MARK
        .iter()
        .map(|corner| corner.x.abs().max(corner.y.abs()))
        .fold(0.0, f32::max);
    let (top, bottom) = READERS_MARK
        .iter()
        .fold((f32::MIN, f32::MAX), |(top, bottom), corner| {
            (top.max(corner.y), bottom.min(corner.y))
        });
    (reach, top - bottom)
}

/// The mark drawn into texels: white ink on clear glass, the tint left to the
/// [`ImageNode`], with the coverage supersampled so the edges stay edges at
/// any rotation the card puts the glyph through.
fn bow_image() -> Image {
    let (reach, _) = mark_measure();
    crate::glyph::raster(BOW_TEXELS, BOW_TEXELS, |at| {
        let sample = (at * 2.0 - 1.0) * reach;
        // The texture's y runs down and the mark's runs up.
        covered(Vec2::new(sample.x, -sample.y))
    })
}

/// Whether a point in the mark's own units lies under its ink.
///
/// Asked as the two triangles the chart builds the arrowhead from, with the
/// edges counted in: the pair share the mark's spine, and a strict test would
/// leave a hairline seam of missed samples down it.
fn covered(at: Vec2) -> bool {
    let [point, left, notch, right] = READERS_MARK;
    in_triangle(at, point, left, notch) || in_triangle(at, point, notch, right)
}

/// Whether `at` lies in the triangle `abc`, wound anticlockwise: inside is
/// every edge's cross product coming up positive.
fn in_triangle(at: Vec2, a: Vec2, b: Vec2, c: Vec2) -> bool {
    (b - a).perp_dot(at - a) >= 0.0
        && (c - b).perp_dot(at - b) >= 0.0
        && (a - c).perp_dot(at - c) >= 0.0
}

/// The turn that lays the bow along a heading — the bearing and nothing else,
/// the glyph being drawn point-up in its texture.
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

/// Which way the stream lies on the card under a wind, or `None` when the
/// wind is too slack to have a bearing at all — see [`WIND_NAMED`], the bar
/// the water and the sails share. The weather never blows that softly; a
/// wind that slack is one the console ordered, and what is left of it as it
/// dies has a bearing made of noise. So the stream holds the last bearing it
/// had and simply fades — see [`drive_the_stream`] — which reads as the wind
/// dropping rather than as the instrument spinning.
///
/// The card's own up is north, so the stream's angle is the wind's bearing:
/// clockwise from north, the way a bearing is always taken, and the way
/// [`UiTransform`] turns. East is north a quarter turn clockwise on the page,
/// which for a page with x to the right and y down is `(-n.y, n.x)`.
///
/// The chevrons fly *with* the air rather than pointing into the eye of it.
/// A wind is named for where it comes from, so this is the arguable half of
/// the design — but marks that flew backwards would need explaining every
/// time they were looked at, and they have to be read at a glance, over a
/// pennant and a sea that are both unarguably going the other way.
fn wind_bearing(wind: Vec2) -> Option<Rot2> {
    if wind.length() < WIND_NAMED {
        return None;
    }
    let east = Vec2::new(-NORTH.y, NORTH.x);
    Some(Rot2::radians(f32::atan2(wind.dot(east), wind.dot(NORTH))))
}

/// How hard the stream is inked, for a wind speed in metres per second.
///
/// Two fades multiplied, doing different jobs. The first is the reading — a
/// light air is a fainter stream than a gale — and it keeps a floor, or a
/// real wind would be drawn too faint to find. The second is the calm, which
/// takes the stream off the card altogether rather than leaving marks adrift
/// on a bearing the wind has stopped having. There is no number anywhere and
/// there is not meant to be: the question a player has is which way and
/// roughly how much, and a card that answered in metres per second would be
/// the machinery showing through.
fn wind_ink(speed: f32) -> f32 {
    let hard = (speed / FULL_WIND).clamp(0.0, 1.0);
    (0.45 + 0.55 * hard) * (speed / WIND_NAMED).clamp(0.0, 1.0)
}

/// Where one chevron lies along the track, in pixels from the middle of the
/// card down the stream's own axis — negative is leeward, the way the node's
/// up ends up pointing once it is turned to the bearing.
///
/// The pool is dealt out evenly and the phase carries every chevron together;
/// a full lap of it brings each one home, which is what lets
/// [`drive_the_stream`] keep the phase wrapped rather than counting forever.
fn chevron_offset(place: usize, phase: f32) -> f32 {
    let spacing = 2.0 * TRACK / CHEVRONS as f32;
    TRACK - (place as f32 * spacing + phase).rem_euclid(2.0 * TRACK)
}

/// How much of its ink a chevron keeps at an offset: all of it along the
/// middle of the track, easing to nothing over [`END_FADE`] at either end.
fn end_window(offset: f32) -> f32 {
    ((TRACK - offset.abs()) / END_FADE).clamp(0.0, 1.0)
}

/// Blows the stream along the wind where the player is.
///
/// The drawn wind, not the forecast — [`SeaConditions::wind_at`] — for the
/// reason the card reads the camera's eased yaw: an instrument that arrived
/// at the new weather before the water did would be pointing at a sea that
/// is not there yet. And the wind *here*, cut down by whatever land stands
/// upwind, rather than the weather's over the whole world: the card is the
/// player's instrument, its pace and ink are what they read a hull's speed
/// from, and a card streaming a gale over a hull crawling in a lee would be
/// the boat disagreeing with its own compass — the one thing
/// [`protocol::sheltered`] is built to prevent. Outside a match there is no
/// "here", and the stream flies the weather's own wind.
///
/// A calm leaves the rotation alone, so the stream fades out where it last
/// pointed and comes back wherever the new wind is — and the drift dies with
/// the ink, so what fades is a stream stopping rather than marks still
/// marching along a dead bearing.
fn drive_the_stream(
    time: Res<Time>,
    conditions: Res<SeaConditions>,
    ground: Option<Res<Ground>>,
    player: PlayerPlace,
    mut streams: Query<(&mut WindStream, &mut UiTransform)>,
    mut chevrons: Query<(&Chevron, &mut UiTransform), Without<WindStream>>,
    mut ink: Query<(&ChevronInk, &mut BackgroundColor)>,
) {
    let wind = match player.at() {
        Some(at) => conditions.wind_at(ground.as_deref(), at.xz()),
        None => conditions.wind(),
    };
    let speed = wind.length();
    let alpha = wind_ink(speed);

    let mut phase = 0.0;
    for (mut stream, mut transform) in &mut streams {
        if let Some(bearing) = wind_bearing(wind) {
            transform.rotation = bearing;
        }
        stream.phase = (stream.phase + DRIFT * speed * time.delta_secs()).rem_euclid(2.0 * TRACK);
        phase = stream.phase;
    }
    for (chevron, mut transform) in &mut chevrons {
        transform.translation = Val2::px(0.0, chevron_offset(chevron.0, phase));
    }
    for (chevron, mut colour) in &mut ink {
        let inked = INK.with_alpha(alpha * end_window(chevron_offset(chevron.0, phase)));
        // Written only where it differs, the ring's economy: in a calm every
        // bar holds the nothing it already had.
        if colour.0 != inked {
            *colour = BackgroundColor(inked);
        }
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
    use protocol::ground::{quantize, ChunkPayload, Material, CELL_COUNT, CORNERS};

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
            heights: vec![quantize(height); CORNERS * CORNERS],
            materials: vec![Material::Grass; CELL_COUNT],
            lit: vec![protocol::ground::LIT_ALL_DAY; CORNERS * CORNERS],
            water: None,
            plants: Vec::new(),
        }
    }

    /// A headless app with the camera to read and the compass to point,
    /// already dropped into a match.
    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            // Assets because the bow is an image — see `spawn_bow`.
            bevy::asset::AssetPlugin::default(),
            TimePlugin,
            StatesPlugin,
            MapCameraPlugin,
            CompassPlugin,
        ))
        .init_asset::<Image>()
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
    fn the_stream_flies_with_the_wind() {
        // East is a quarter turn clockwise from north on the card, and a wind
        // *blowing* east lays the stream along it — the reading is where the
        // air is going, not where a sailor would say it came from.
        let east = Vec2::new(-NORTH.y, NORTH.x);
        let blowing_east = wind_bearing(east * 8.0).expect("a fresh breeze has a bearing");
        assert!(
            blowing_east
                .angle_to(Rot2::radians(std::f32::consts::FRAC_PI_2))
                .abs()
                < 1e-5
        );

        // A northerly is air moving south, so the stream lies down the card.
        let northerly = wind_bearing(-NORTH * 8.0).expect("a fresh breeze has a bearing");
        assert!(
            northerly
                .angle_to(Rot2::radians(std::f32::consts::PI))
                .abs()
                < 1e-4
        );

        // And a wind out of the south flies the chevrons at the N, which is
        // the reading most likely to have been drawn backwards.
        let southerly = wind_bearing(NORTH * 8.0).expect("a fresh breeze has a bearing");
        assert!(southerly.angle_to(Rot2::IDENTITY).abs() < 1e-5);
    }

    #[test]
    fn the_stream_darkens_with_the_wind_and_the_ink_tops_out() {
        assert!(
            wind_ink(2.0) < wind_ink(FULL_WIND),
            "a fresh breeze drew no darker than a light air"
        );
        // Past the top of the scale the ink has nowhere further to go — the
        // pace is the channel left carrying a gale.
        assert_eq!(wind_ink(FULL_WIND), wind_ink(40.0));
    }

    #[test]
    fn the_stream_stays_on_its_track_and_wraps() {
        for place in 0..CHEVRONS {
            let offset = chevron_offset(place, 17.3);
            assert!(
                offset.abs() <= TRACK,
                "chevron {place} left the track: {offset}"
            );
        }

        // A full lap of phase is a round trip, which is what lets the drift
        // wrap instead of counting forever.
        let (out, back) = (chevron_offset(3, 5.0), chevron_offset(3, 5.0 + 2.0 * TRACK));
        assert!((out - back).abs() < 1e-3);

        // The drift runs leeward — a growing phase carries a chevron toward
        // negative offsets until it wraps to windward again.
        assert!(chevron_offset(0, 1.0) < chevron_offset(0, 0.5));

        // And both ends ease to nothing, so the wrap can never pop.
        assert_eq!(end_window(TRACK), 0.0);
        assert_eq!(end_window(-TRACK), 0.0);
        assert_eq!(end_window(0.0), 1.0);
    }

    #[test]
    fn a_calm_takes_the_stream_off_the_card() {
        // No bearing worth drawing, and nothing drawn: the stream keeps the
        // rotation it had and the ink goes to nothing.
        assert!(wind_bearing(Vec2::new(0.2, -0.1)).is_none());
        assert_eq!(wind_ink(0.0), 0.0);
        assert!(wind_ink(WIND_NAMED / 2.0) < wind_ink(WIND_NAMED));
    }

    /// The glyph the bow is rasterised from: solid where the chart draws ink
    /// and clear in the notch, which is what separates an arrowhead from a
    /// triangle.
    #[test]
    fn the_bow_wears_the_charts_mark() {
        let [point, _, notch, _] = READERS_MARK;
        // On the spine below the tip, where the two triangles meet: a strict
        // edge test would miss here and seam the mark.
        assert!(covered((point + notch) / 2.0));
        // In the notch, between the tails: the cut that makes it an arrow.
        assert!(!covered(Vec2::new(0.0, notch.y - 1.5)));
        // And clear off the glyph entirely.
        assert!(!covered(Vec2::new(notch.x + 6.0, 0.0)));
    }

    #[test]
    fn the_stream_reads_the_sea_it_is_drawn_over() {
        // The card is wired to the drawn sea rather than to the forecast, so
        // the stream and the water are under one wind. Before any weather has
        // landed that is the assumed day the sea opens on.
        let mut app = test_app();
        app.update();
        let wind = app.world().resource::<SeaConditions>().wind();
        let (stream, transform) = app
            .world_mut()
            .query::<(&WindStream, &UiTransform)>()
            .single(app.world())
            .expect("the stream should exist");
        let expected = wind_bearing(wind).expect("the assumed day is not a calm");
        assert!(transform.rotation.angle_to(expected).abs() < 1e-5);

        // The whole pool rides the track, each chevron where the shared
        // phase puts it.
        let phase = stream.phase;
        let chevrons: Vec<(usize, Val2)> = app
            .world_mut()
            .query::<(&Chevron, &UiTransform)>()
            .iter(app.world())
            .map(|(chevron, transform)| (chevron.0, transform.translation))
            .collect();
        assert_eq!(chevrons.len(), CHEVRONS);
        for (place, translation) in chevrons {
            assert_eq!(
                translation,
                Val2::px(0.0, chevron_offset(place, phase)),
                "chevron {place} was off its offset"
            );
        }
    }

    /// The sign conventions the ring shares with the stream, checked where they
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
        ground.deliver(island, None, Some(a_hill()));

        let found = land_in_sight(
            &ground,
            &Chart::default(),
            Vec2::splat(CHUNK_METRES / 2.0),
            SIGHT,
        );
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
        ground.deliver(IVec2::ZERO, None, Some(a_hill()));

        let found = land_in_sight(
            &ground,
            &Chart::default(),
            Vec2::splat(CHUNK_METRES / 2.0),
            SIGHT,
        );
        assert!(
            found.iter().all(Option::is_none),
            "the chunk the player stands on claimed a bearing"
        );
    }

    #[test]
    fn the_stream_flies_the_wind_where_the_player_is() {
        // One world with a lee on one side of a chunk line and open water on
        // the other, the same gale over both, and the player stood first on
        // one side and then the other. The ink is the strength reading, and
        // it has to be the strength *here*: a card that streamed the weather's
        // gale over a player in a lee would be the boat disagreeing with its
        // own compass, which is exactly what reading the local wind is for.
        // A gale rather than the reference breeze, since only past the
        // wire's floor does the lee move the ink at all.
        use protocol::ground::{BEARINGS, LEAST_EXPOSURE, SHELTER_COUNT};
        let most_ink = |x: f32| -> f32 {
            let mut app = test_app();
            let mut ground = Ground::default();
            let deepest = (LEAST_EXPOSURE * 255.0).round() as u8;
            ground.deliver(
                IVec2::new(-1, 0),
                Some(vec![[deepest; BEARINGS]; SHELTER_COUNT]),
                None,
            );
            ground.deliver(IVec2::ZERO, None, None);
            app.insert_resource(ground);
            app.insert_resource(SeaConditions::blowing(Vec2::new(0.0, -16.0)));
            app.world_mut()
                .spawn((Player, Transform::from_xyz(x, 0.0, CHUNK_METRES / 2.0)));
            app.update();
            app.world_mut()
                .query_filtered::<&BackgroundColor, With<ChevronInk>>()
                .iter(app.world())
                .map(|colour| colour.0.alpha())
                .fold(0.0, f32::max)
        };
        let open = most_ink(CHUNK_METRES / 2.0);
        let lee = most_ink(-CHUNK_METRES / 2.0);
        assert!(
            lee < open,
            "the stream carried {lee} of its ink in the lee against {open} in the open"
        );
        // And what it shows in the lee is the wind the hull would sail, not
        // something of its own: the ink of exactly `sheltered`'s answer, read
        // off the byte the lattice actually stores rather than the constant
        // it was rounded from.
        let deepest = (LEAST_EXPOSURE * 255.0).round() / 255.0;
        let sailed = protocol::sheltered(Vec2::new(0.0, -16.0), deepest);
        assert!(
            (lee - wind_ink(sailed.length())).abs() < 1e-5,
            "the lee's ink {lee} is not the ink of the sheltered wind"
        );
    }

    /// Stands a player in a world with one island due east of them, aboard a
    /// boat or on their own feet, and runs a frame.
    fn a_player_off_an_island(aboard: bool) -> App {
        let mut app = test_app();

        let mut ground = Ground::default();
        ground.deliver(IVec2::new(3, 0), None, Some(a_hill()));
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

    /// The stream's bearing and the most ink any chevron is carrying, for a
    /// player aboard or on their own feet under one wind.
    fn stream(aboard: bool) -> (Rot2, f32) {
        let mut app = a_player_off_an_island(aboard);
        app.insert_resource(SeaConditions::blowing(Vec2::new(FULL_WIND, 0.0)));
        app.update();
        let bearing = app
            .world_mut()
            .query_filtered::<&UiTransform, With<WindStream>>()
            .single(app.world())
            .expect("the stream should exist")
            .rotation;
        let ink = app
            .world_mut()
            .query_filtered::<&BackgroundColor, With<ChevronInk>>()
            .iter(app.world())
            .map(|colour| colour.0.alpha())
            .fold(0.0, f32::max);
        (bearing, ink)
    }

    /// Ashore the card keeps its wind and its bow, which is the module doc's
    /// claim that the ring is the *only* reading a beach makes nonsense of.
    ///
    /// Two tests above would fail if the stream were gated on being aboard,
    /// but only because a player afoot is the cheapest one to stand up — they
    /// are about which wind is drawn, not about where it is drawn, and either
    /// could be rewritten around a deck without meaning to give this up.
    ///
    /// The wind is the one that matters. A passage is worked out standing on
    /// the sand, and a card that went blank the moment a player stepped off
    /// the deck would be asking them to board to find out whether boarding
    /// was worth it.
    ///
    /// The bearing rather than the ink, because the ink is carried along the
    /// track by a phase the clock drives, and two apps do not run the same
    /// number of microseconds. The bearing is the wind's own and nothing
    /// else's — but a hidden stream would keep its bearing too, so the ink is
    /// asked for as well, which only has to be *there*.
    #[test]
    fn the_card_keeps_its_wind_ashore() {
        let (afloat, afloat_ink) = stream(true);
        let (ashore, ashore_ink) = stream(false);
        assert!(afloat_ink > 0.0, "afloat, the stream carried no ink at all");
        assert!(ashore_ink > 0.0, "ashore, the stream stopped being drawn");
        assert!(
            afloat.angle_to(ashore).abs() < 1e-5,
            "ashore the stream lay {ashore:?} against {afloat:?} afloat"
        );
    }

    /// And the bow with it: ashore it points the way the walker is facing,
    /// which is a heading like any other — see [`point_the_bow`].
    #[test]
    fn the_bow_keeps_pointing_ashore() {
        for aboard in [true, false] {
            let mut app = a_player_off_an_island(aboard);
            let shown = *app
                .world_mut()
                .query_filtered::<&Visibility, With<Bow>>()
                .single(app.world())
                .expect("the bow should exist");
            assert_ne!(
                shown,
                Visibility::Hidden,
                "the bow went out with aboard = {aboard}"
            );
        }
    }

    /// The ring reaches as far as the haze lets the player see, so an island
    /// a dry day shows goes off the ring in a squall and comes back after it.
    #[test]
    fn a_squall_takes_the_land_off_the_ring() {
        let mut app = a_player_off_an_island(true);
        assert!(marks_lit(&mut app) > 0, "a dry day marked no island");

        app.insert_resource(SeaConditions::default().raining(1.0));
        app.update();
        assert_eq!(marks_lit(&mut app), 0, "the ring saw through the squall");

        app.insert_resource(SeaConditions::default());
        app.update();
        assert!(marks_lit(&mut app) > 0, "the island never came back");
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
        shelf.deliver(IVec2::new(3, 0), None, Some(a_shoal()));
        assert!(
            land_in_sight(&shelf, &chart, at, SIGHT)
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
        far.deliver(IVec2::new(beyond, 0), None, Some(a_hill()));
        assert!(
            land_in_sight(&far, &chart, at, SIGHT)
                .iter()
                .all(Option::is_none),
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
