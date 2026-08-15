//! Cloud shadows: the only part of the weather a camera pointed at the ground
//! can see.
//!
//! There are no clouds in this world, and there is no room for any. The eye
//! looks down at the ground from somewhere between thirty metres and three
//! hundred, wherever the wheel has left it, and anything hung in the air
//! between the two would spend its life covering the picture rather than
//! decorating it. What a player would see of a real sky from up there is its
//! *shadow* — patches of shade wandering across the sea and up the hillsides
//! — so that is the whole of what is drawn, and the clouds themselves are
//! never modelled at all.
//!
//! Which means this is not a thing in the world so much as a thing done to the
//! light. Bevy will mask a directional light with a texture — the trick a film
//! lamp does with a gobo — and a masked light carries the pattern onto every
//! surface it touches, in one pass, for nothing: the ground, the water, the
//! hull, the palms and whatever is walking about are all shaded by the same
//! sun and so all fall under the same cloud, without a line of this file
//! knowing that any of them exist. Doing it in the materials instead would
//! have meant the same function written into the terrain's shader, the sea's
//! and every model's, and three of them would have gone out of step.
//!
//! The mask lies in the plane facing the sun, and is projected down the light
//! like everything else the light does. Three consequences, and each is a
//! decision made here:
//!
//! - A shadow lengthens as the sun sinks, which is what a cloud's shadow does
//!   — but a mask flat against a grazing sun stretches without limit, and a
//!   world striped from horizon to horizon is not weather. [`LONGEST`] caps
//!   it by squeezing the mask along the axis the sun leans in.
//! - Where the mask sits is where the light *stands*, which for a directional
//!   light means nothing else at all — so walking it downwind is how the
//!   clouds travel. Hence [`Clouds::stand_the_light`]: `sky` decides which way
//!   the light faces, and this decides where it faces from.
//! - At night the light is the moon, and the mask would ride round with it.
//!   The clouds are taken off it instead — see [`clouds_by_day`], which also
//!   says why the moment of taking them off cannot be seen.

use bevy::asset::RenderAssetUsages;
use bevy::image::{Image, ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::light::DirectionalLightTexture;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::sea::Forecast;
use crate::sky::{Sky, SkyLight};
use crate::{scramble, unit, AppState};

/// How far the pattern goes before it repeats, in metres, measured on flat
/// ground under a high sun.
///
/// It has to beat what the eye can take in, or the repeat is what you notice
/// about the weather. What the eye can take in is [`crate::HAZE_END`] — 900 m
/// in every direction — and a tile of 1.4 km does not quite banish the twin:
/// two points a tile apart can both stand inside a circle that wide, so a
/// repeat can show along the far edge of the widest view. What it buys is that
/// when one does, it is out where the haze has most of it.
///
/// Wider would hide the repeat outright and cost nothing to draw. What stops
/// it is that every cloud is a feature of one tile, so a tile of several
/// kilometres holds either continents of shade or nothing at all.
const TILE: f32 = 1_400.0;

/// Texels along each side of the mask. Over [`TILE`] that is a texel every
/// 2.7 m, against ground facets 2 m across: the edge of a shadow lands within
/// a facet or so of where it should, which is as fine as anything else in this
/// picture is drawn.
const TEXELS: usize = 512;

/// Cells across the tile in the coarsest octave of the noise, and how many
/// octaves are laid over it.
///
/// Eight cells puts a cloud every 175 m or so, and that size has to work at
/// both ends of the wheel. Zoomed out it is a shadow a good part of the view
/// across, with two or three of them sharing the picture; zoomed in it is
/// wider than the whole view, and what a player sees there is not a shape at
/// all but the light going and coming back every few tens of seconds as an
/// edge sweeps over them. Both of those read as weather. A cloud small enough
/// to be a shape at the near end of the wheel would be stipple at the far one.
///
/// Three octaves take the detail down to 44 m, which is where the ragged edge
/// of a shadow comes from; a fourth only adds wobble finer than the ground's
/// own facets.
const CELLS: usize = 8;
const OCTAVES: usize = 3;

/// The band of the noise the cloud's edge is drawn across: clear sky below
/// [`COVER`], full shade above `COVER + EDGE`.
///
/// The threshold sets how much of the sky is clouded, and the width sets how
/// soft the edge is. Both are deliberately hard for what they are: this world
/// is drawn in flat tones with hard boundaries — the surf breaks in bands, the
/// hillsides are facets — and a shadow that faded gently over a hundred metres
/// would be the one soft thing in the picture.
const COVER: f32 = 0.52;
const EDGE: f32 = 0.05;

/// How much of the sun still reaches the ground under a cloud.
///
/// Not zero, and not close to it. What this multiplies is the sun alone: the
/// sky's own light goes on filling the shade whatever is in front of the sun,
/// which is what keeps a cloud shadow a second tone on the water rather than a
/// hole in it. A real cumulus takes nearly all of the direct light; taking a
/// third of it here already reads as an overcast passing.
const SHADE: f32 = 0.62;

/// How much faster than the wind on the water the clouds travel.
///
/// Air moves quicker with height, and a cloud is carried by the air it is in
/// rather than the air the sea feels. It is also the one number that decides
/// whether the weather looks alive from a hilltop, so it is a little more than
/// honesty would ask for.
const ALOFT: f32 = 1.6;

/// The longest a shadow may be drawn against its own width, as the sun sinks.
///
/// Below about a quarter of the way up the sky the mask is squeezed to hold
/// this ratio — see the module doc. Two and a half is enough that a low sun
/// visibly draws the shade out downwind, and short of the point where a cloud
/// stops being a shape and becomes a stripe across the whole island.
const LONGEST: f32 = 2.5;

/// The clouds, as they stand: the mask itself, and how far downwind the
/// weather has carried it.
///
/// The app's, rather than any one world's, exactly as the mask is — a world
/// left and another entered goes on from the sky the first one had, which is
/// no more arbitrary than starting from zero would be and is one less thing
/// to remember to do.
#[derive(Resource, Default)]
pub struct Clouds {
    /// Woven once, at startup by [`weave_the_mask`], and never written to
    /// again — the weather is all in where the mask is put, not in what it
    /// says.
    mask: Handle<Image>,
    /// Where the pattern has got to, in metres of world, since the app opened.
    /// Only ever added to, so a session leaves it tens of kilometres from
    /// zero; that is fine to a hair at `f32` for far longer than anyone plays
    /// in one sitting, and wrapping it would be a jump the moment the sun was
    /// low enough to have stretched the tile.
    drift: Vec2,
}

impl Clouds {
    /// Where the light is to stand, and how the mask on it is to be shaped,
    /// given the direction the light comes from. [`crate::sky`] is the only
    /// caller, and owns everything else about the light.
    ///
    /// A directional light shines the same way from anywhere, so the position
    /// here is lighting nothing: it is the origin of the mask, and walking it
    /// a metre north walks every cloud shadow a metre north. The scale is the
    /// mask's size in the plane facing the sun, squeezed once the sun is low
    /// enough for [`LONGEST`] to bite.
    ///
    /// `from` has to be a light the sky has aimed, which is never allowed
    /// below `sky::GRAZE`. A light exactly on the horizon would squeeze the
    /// mask to nothing, and a transform that cannot be inverted hands the
    /// shader NaN for every coordinate it asks for.
    pub fn stand_the_light(&self, from: Vec3) -> Transform {
        // Standing on the waterline, downwind of where it began. The height is
        // nothing to argue about — sliding the mask along the light's own
        // direction changes nothing it projects — and the light is aimed down
        // the sun's bearing rather than pointed back at the middle of the
        // world, which after an hour of weather would be a quite different
        // direction.
        let mut stand = Transform::from_translation(Vec3::new(self.drift.x, 0.0, self.drift.y))
            .looking_to(-from, Vec3::Y);
        // Half the tile, since the mask spans two units of the plane it lies
        // in, and squeezed along the axis the sun leans in — which is the
        // whole of the cap [`LONGEST`] describes.
        let squeeze = (from.y * LONGEST).min(1.0);
        stand.scale = Vec3::new(TILE / 2.0, TILE / 2.0 * squeeze, 1.0);
        stand
    }
}

pub struct CloudsPlugin;

impl Plugin for CloudsPlugin {
    fn build(&self, app: &mut App) {
        // The wind the clouds go with, which several plugins init and the
        // network fills in — initialising a resource twice is free, and each
        // plugin's tests run it alone.
        app.init_resource::<Forecast>()
            .init_resource::<Clouds>()
            .add_systems(Startup, weave_the_mask)
            .add_systems(
                Update,
                (drift_downwind, clouds_by_day).run_if(in_state(AppState::InWorld)),
            );
    }
}

/// Weaves the one mask the world's weather is drawn with, before any of it is
/// wanted. Startup rather than the resource's own making, so that this plugin
/// can be added ahead of whatever registers the image assets — which is what
/// lets [`crate::sky`] bring it along without either of them minding the
/// order.
fn weave_the_mask(mut clouds: ResMut<Clouds>, mut images: ResMut<Assets<Image>>) {
    clouds.mask = images.add(mask_image());
}

/// Carries the clouds along on the wind.
///
/// The wind taken is the one the server last named rather than the eased one
/// the sea is drawn under: a sea takes minutes to answer a change of wind and
/// the air *is* the change, so the clouds simply go at whatever is blowing.
/// Nothing shows at the moment a new forecast lands, either — what steps is
/// the speed, and where the clouds are goes on from where they were.
///
/// Named beyond this module only so that [`crate::sky`] can put its light
/// after it, the light being where the clouds are drawn from.
pub fn drift_downwind(time: Res<Time>, forecast: Res<Forecast>, mut clouds: ResMut<Clouds>) {
    // Before the first forecast — the first fraction of a second of a session
    // — the sky simply stands still. There is no assumed wind worth inventing
    // for it, the way the sea needs one to have any waves at all.
    let wind = forecast.wind.unwrap_or(Vec2::ZERO);
    clouds.drift += wind * ALOFT * time.delta_secs();
}

/// Puts the clouds on the light through the day and takes them off it at
/// night.
///
/// The mask belongs to the light, and at night the light is the moon on the
/// far side of the world — so left alone the clouds would swing round with it
/// and cross the ground backwards. Moonlit cloud shadows would be a fair thing
/// to want; what makes them not worth having is that the two skies cannot be
/// crossfaded. A mask is a texture, with no dial on it to turn down, so the
/// clouds can only arrive and leave whole.
///
/// Which is why the moment chosen is the one the light itself changes bodies
/// at, asked of the sky rather than worked out here — see [`Sky::is_night`],
/// and [`crate::sky`] for why that is a few tenths of an hour into the dark
/// rather than at the horizon. The sun is under the world by then and its
/// light is held grazing along the water, so it lands on almost nothing:
/// whatever the mask says at that instant, it is saying it about a light that
/// has stopped reaching the ground. The clouds go out with the day, and
/// nothing on screen moves as they do.
fn clouds_by_day(
    mut commands: Commands,
    sky: Res<Sky>,
    clouds: Res<Clouds>,
    light: Single<(Entity, Has<DirectionalLightTexture>), With<SkyLight>>,
) {
    let (light, masked) = light.into_inner();
    match (!sky.is_night(), masked) {
        (true, false) => {
            commands.entity(light).insert(DirectionalLightTexture {
                image: clouds.mask.clone(),
                // Endlessly, in both directions: the world has no edges and
                // neither can its weather.
                tiled: true,
            });
        }
        (false, true) => {
            commands.entity(light).remove::<DirectionalLightTexture>();
        }
        _ => {}
    }
}

/// The mask: how much of the sun gets through, texel by texel, as one tile of
/// a pattern that repeats for ever in both directions.
///
/// Only the red channel is read, so a byte a texel is the whole picture. It is
/// sampled without mipmaps, and every reader of it is magnifying — a texel is
/// [`TILE`] / [`TEXELS`] metres of ground, which is wider than a screen pixel
/// out to well past the haze — so there is nothing here for a filter to
/// shimmer on.
fn mask_image() -> Image {
    let shade: Vec<u8> = (0..TEXELS)
        .flat_map(|y| {
            (0..TEXELS).map(move |x| {
                let at = Vec2::new(x as f32, y as f32) / TEXELS as f32;
                let cloud = smoothstep(COVER, COVER + EDGE, clouds_at(at));
                (255.0 * (1.0 - cloud * (1.0 - SHADE))).round() as u8
            })
        })
        .collect();

    let mut image = Image::new(
        Extent3d {
            width: TEXELS as u32,
            height: TEXELS as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        shade,
        TextureFormat::R8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    // Repeating and bilinear, and both are load-bearing. Bevy samples a light
    // mask with whatever sampler its own image carries, and a mask that
    // clamped at the edge would smear one row of texels across the world
    // beyond the first tile; a nearest one would draw the shadows as squares.
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    image
}

/// How much cloud stands over a point of the tile, as a number about a half —
/// octaves of value noise, each wrapping at its own lattice so the tile meets
/// itself on all four sides.
fn clouds_at(at: Vec2) -> f32 {
    let mut sum = 0.0;
    let mut total = 0.0;
    for octave in 0..OCTAVES {
        let amplitude = 0.5f32.powi(octave as i32);
        // Scrambled rather than handed the octave's own number, which is the
        // difference between three noise fields and one field read three
        // times: a salt is exclusive-ored into the cell coordinate, and the
        // small numbers 0, 1, 2 land in the very bits the coordinate's x sits
        // in — so octave 1 would be octave 0 with every cell swapped with its
        // neighbour, and the sum of the three would have a grain to it.
        sum += amplitude * lumps(at, (CELLS << octave) as i32, scramble(octave as u32));
        total += amplitude;
    }
    sum / total
}

/// One octave: value noise on a lattice of `cells` squares across the tile,
/// smoothed between them. Cell coordinates are taken modulo the lattice, which
/// is what makes the tile seamless — the cell off the right-hand edge is the
/// one on the left.
fn lumps(at: Vec2, cells: i32, salt: u32) -> f32 {
    let scaled = at * cells as f32;
    let cell = scaled.floor();
    let within = scaled - cell;
    let cell = cell.as_ivec2();

    let corner = |dx: i32, dy: i32| {
        let x = (cell.x + dx).rem_euclid(cells) as u32;
        let y = (cell.y + dy).rem_euclid(cells) as u32;
        unit(x | (y << 16), salt)
    };
    let across = Vec2::new(
        smoothstep(0.0, 1.0, within.x),
        smoothstep(0.0, 1.0, within.y),
    );
    let low = corner(0, 0).lerp(corner(1, 0), across.x);
    let high = corner(0, 1).lerp(corner(1, 1), across.x);
    low.lerp(high, across.y)
}

/// The smooth step from `0.0` at `from` to `1.0` at `to` — the same curve the
/// shaders' `smoothstep` draws, so a soft edge here matches a soft edge there.
fn smoothstep(from: f32, to: f32, at: f32) -> f32 {
    let t = ((at - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use bevy::asset::AssetPlugin;
    use bevy::state::app::StatesPlugin;
    use bevy::time::{TimePlugin, TimeUpdateStrategy};

    use super::*;
    use crate::testing::FRAME;

    /// A headless world with the clouds running and a light for them to hang
    /// on, standing at an hour of the caller's choosing. No sky plugin: what
    /// these tests are about is what the clouds do to a light, and the sky's
    /// own half — where the light points and what colour it is — is tested
    /// where it is written.
    fn cloudy_app(phase: f32) -> App {
        let mut app = App::new();
        app.add_plugins((
            AssetPlugin::default(),
            TimePlugin,
            StatesPlugin,
            CloudsPlugin,
        ))
        .insert_resource(TimeUpdateStrategy::ManualDuration(FRAME))
        .init_state::<AppState>()
        .init_asset::<Image>()
        .init_resource::<Sky>();
        app.world_mut().resource_mut::<Sky>().told(phase);
        app.world_mut().spawn((SkyLight, Transform::default()));
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::InWorld);
        app.update();
        app
    }

    /// Whether the one light in the app is wearing the clouds.
    fn clouded(app: &mut App) -> bool {
        app.world_mut()
            .query_filtered::<Entity, (With<SkyLight>, With<DirectionalLightTexture>)>()
            .single(app.world())
            .is_ok()
    }

    /// A sun somewhere up the sky, for the tests that only need the light to
    /// be coming from a plausible direction.
    const SUN: Vec3 = Vec3::new(0.3, 0.8, 0.5196152);

    /// The mask, read back as the fraction of the sun each texel lets through.
    fn mask() -> Vec<f32> {
        mask_image()
            .data
            .expect("a mask is built with its texels in hand")
            .iter()
            .map(|&byte| byte as f32 / 255.0)
            .collect()
    }

    #[test]
    fn the_sky_is_part_cloud_and_part_open() {
        // The one number the noise has to be talked into. A threshold a little
        // out puts the world under a permanent overcast or gives it a sky with
        // three clouds in it, and neither reads as weather.
        let mask = mask();
        let clouded = mask.iter().filter(|&&pass| pass < 0.9).count() as f32 / mask.len() as f32;
        assert!(
            (0.2..0.6).contains(&clouded),
            "{:.0}% of the sky is clouded",
            clouded * 100.0
        );

        // And the shade under one is shade rather than a suggestion of it.
        let darkest = mask.iter().fold(1.0f32, |deepest, &pass| deepest.min(pass));
        assert!(
            darkest < SHADE + 0.01,
            "the deepest shade only reaches {darkest}"
        );
    }

    #[test]
    fn the_pattern_meets_itself_at_the_tile_edge() {
        // Tiled endlessly across the world, so a seam would be on screen every
        // few hundred metres: a point at one edge of the tile has to be what
        // the point wrapping onto it from the other edge says.
        for i in 0..TEXELS {
            let at = i as f32 / TEXELS as f32;
            let across = (clouds_at(Vec2::new(0.0, at)) - clouds_at(Vec2::new(1.0, at))).abs();
            let down = (clouds_at(Vec2::new(at, 0.0)) - clouds_at(Vec2::new(at, 1.0))).abs();
            assert!(across < 1e-5 && down < 1e-5, "the tile has a seam at {at}");
        }

        // And the sampler has to wrap, which is the half of it a seamless
        // pattern cannot do on its own: Bevy reads a light's mask with
        // whatever sampler the image itself carries, and one clamping at the
        // edge would smear a single row of texels over every metre of world
        // beyond the first tile.
        let ImageSampler::Descriptor(sampler) = mask_image().sampler else {
            panic!("the mask goes to the GPU with a sampler of its own");
        };
        assert_eq!(sampler.address_mode_u, ImageAddressMode::Repeat);
        assert_eq!(sampler.address_mode_v, ImageAddressMode::Repeat);
    }

    #[test]
    fn the_shadows_walk_the_ground_by_exactly_the_drift() {
        // What the module rests on, read through Bevy's own arithmetic rather
        // than restated: the mask's coordinates are the light's transform
        // inverted, applied to a world point — so a light walked `d` has to
        // put the shadow that was over a point over the point `d` beyond it.
        // Nothing else here would notice a drift laid into the wrong axis, or
        // one carrying the weather upwind.
        let drift = Vec2::new(37.0, -64.0);
        let still = Clouds::default().stand_the_light(SUN);
        let blown = Clouds {
            mask: Handle::default(),
            drift,
        }
        .stand_the_light(SUN);

        let onto_the_mask =
            |stand: &Transform, at: Vec3| stand.compute_affine().inverse().transform_point3(at);
        for ground in [
            Vec3::ZERO,
            Vec3::new(120.0, 0.0, -80.0),
            Vec3::new(-900.0, 30.0, 400.0),
        ] {
            let walked = ground + Vec3::new(drift.x, 0.0, drift.y);
            let gap = onto_the_mask(&blown, walked) - onto_the_mask(&still, ground);
            assert!(
                gap.length() < 1e-4,
                "the shade over {ground} did not walk to {walked}: {gap}"
            );
        }
    }

    #[test]
    fn a_sinking_sun_draws_the_shadows_out_but_only_so_far() {
        // How long a shadow is drawn against how wide it is. The mask is
        // `scale.x` across the sun's bearing whatever the hour; along the
        // bearing, `scale.y` of it is spread over the `up` of the ground the
        // sun can see — so their ratio is the shape a cloud casts.
        let clouds = Clouds {
            mask: Handle::default(),
            drift: Vec2::ZERO,
        };
        let stretch = |up: f32| {
            let from = Vec3::new(0.0, up, (1.0 - up * up).sqrt());
            let stand = clouds.stand_the_light(from);
            stand.scale.y / up / stand.scale.x
        };
        // Overhead: a cloud's shadow is as wide as the cloud.
        assert!((stretch(1.0) - 1.0).abs() < 1e-4, "{}", stretch(1.0));
        // A third of the way up the sky: drawn out to twice its width.
        assert!((stretch(0.5) - 2.0).abs() < 1e-3, "{}", stretch(0.5));
        // And down on the horizon it is held there rather than smeared across
        // the whole world.
        assert!((stretch(0.02) - LONGEST).abs() < 1e-3, "{}", stretch(0.02));
    }

    #[test]
    fn the_harder_the_wind_the_faster_the_shadows_cross_the_water() {
        // The whole of what the weather says to a player who can only see the
        // ground: shadows that hurry when it blows. Two skies under one clock,
        // and the gale's has to have gone further.
        let mut breeze = cloudy_app(0.5);
        let mut gale = cloudy_app(0.5);
        breeze.world_mut().resource_mut::<Forecast>().wind = Some(Vec2::new(4.0, 0.0));
        gale.world_mut().resource_mut::<Forecast>().wind = Some(Vec2::new(16.0, 0.0));
        for _ in 0..30 {
            breeze.update();
            gale.update();
        }

        let (breeze, gale) = (
            breeze.world().resource::<Clouds>().drift,
            gale.world().resource::<Clouds>().drift,
        );
        assert!(breeze.x > 0.0, "the clouds did not move downwind at all");
        assert!(
            (gale.x / breeze.x - 4.0).abs() < 0.1,
            "four times the wind moved them {} times as far",
            gale.x / breeze.x
        );
        // Downwind, and only downwind: a pattern sliding across the wind would
        // be weather nothing in the world agrees with.
        assert_eq!(breeze.y, 0.0);
    }

    #[test]
    fn the_clouds_are_a_daytime_thing() {
        // On through the day, off through the night — and the switch is the
        // same one the light itself changes bodies at, which is why it cannot
        // be seen. See [`clouds_by_day`].
        let mut noon = cloudy_app(0.5);
        assert!(clouded(&mut noon), "a clear noon has no clouds in it");

        let mut midnight = cloudy_app(0.0);
        assert!(
            !clouded(&mut midnight),
            "the clouds followed the light round to the moon"
        );
    }
}
