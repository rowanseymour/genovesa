//! The sky over the world: what hour it is, the light that comes with it,
//! and the night a boat lies at anchor waiting out.
//!
//! The hour is the server's, exactly as the weather is — every player in a
//! world is under the same sun, and a client that kept its own would have
//! two boats a hundred metres apart sailing in different afternoons. What
//! arrives is a [`protocol::ToClient::Daylight`], a beat apart; what is drawn
//! is [`Sky`], which runs the same clock between tellings and eases onto each
//! new word, so the sun moves continuously rather than in one-second steps.
//!
//! Everything else here is drawing, and belongs to the client alone: which
//! way the light comes from, what colour it is, and how dark the night gets.
//! The server says only what time it is.
//!
//! One light does the whole day: it stands in the sun and then in the moon,
//! changing places in the dark at either end of the night rather than at the
//! horizon (see [`light_from`]). A second light for the moon would have cost
//! a second shadow pass over the whole scene for a moment nobody can see.
//!
//! That one light carries the weather as well as the hour. The cloud shadows
//! are a mask on it, and where they fall is decided by where the light is put
//! — so [`crate::clouds`] hangs the mask and says where the light stands,
//! while everything here goes on being about which way it faces and what
//! colour it burns.
//!
//! The night is dark on purpose — dark enough that sailing on through it is a
//! bad idea, which is what makes anchoring for it a decision rather than a
//! formality. What it costs the player is a few real minutes, so a boat with
//! no way on can ask for the night to be over and watch it run past: see
//! [`ask_for_dawn`], and [`protocol::ToServer::WantDawn`] for what a server
//! does with the asking.

use bevy::color::Mix;
use bevy::light::{DirectionalLight, DirectionalLightShadowMap};
use bevy::pbr::DistanceFog;
use bevy::prelude::*;
use bevy::text::{FontSize, FontSource};

use std::f32::consts::TAU;

use crate::bindings::{Action, KeyBindings};
use crate::boat::Boat;
use crate::clouds::{Clouds, CloudsPlugin};
use crate::net::Online;
use crate::{eased, AppState, Helm};

/// The hour a world is drawn at until a server says otherwise: a morning,
/// which is what worlds open on. Nothing rides on it being the right hour —
/// the welcome carries the true one and the first word of it snaps rather
/// than eases — and this is only so that the frame before it lands is a
/// plausible sky rather than a black one.
const ASSUMED: f32 = 0.35;

/// How fast the drawn hour closes on the hour the server last named, in
/// e-foldings per second.
///
/// The two are running the same clock, so in an ordinary minute this has
/// nothing to do: the correction it applies is the drift between two
/// machines' idea of a second, which is nothing. What it is really for is the
/// night being run off at speed, where the server's word arrives up to a
/// hundredth of a day ahead of us every beat — fast enough to keep the moon
/// crossing smoothly, slow enough that no single telling reads as a jump.
const CATCH_UP: f32 = 2.0;

/// How near the server's hour the drawn one has to get before it counts as
/// having caught up — see [`Sky::caught_up`].
///
/// It cannot be nothing: [`CATCH_UP`] closes the gap by e-foldings, so the
/// last of one is never quite arrived at. Half a thousandth of a day is 0.3 s
/// of the ten-minute day, about a third of what the sun crosses between two
/// tellings, and nothing the eye holds between two pictures.
const CAUGHT_UP: f32 = 0.0005;

/// How far the sun's arc leans from straight overhead, in radians — a little
/// over twenty degrees, which puts noon short of the zenith.
///
/// The tropics would have it near enough vertical, and vertical is the one
/// thing this look cannot use: a sun straight overhead lights every facet of
/// a hillside equally and the relief the whole flat-shaded style is built out
/// of disappears at midday. Leaning the arc keeps a shadow under everything
/// at every hour, and it also keeps the light off the pole, where pointing it
/// at the ground has no unique answer.
const TILT: f32 = 0.38;

/// What hour a screen with no world behind it is lit and cleared to: noon,
/// which is the daylight the menus were drawn against when the world had
/// only the one hour. Nothing is *at* it — a menu is at no time of day — and
/// it exists because the clear colour and the ambient light belong to the
/// app rather than to the world, so leaving one at midnight would otherwise
/// hand the menus a black screen and nothing to light it by.
const NO_HOUR: f32 = 0.5;

/// The lowest the light is allowed to come from, as the sine of its
/// altitude: a degree or so above the horizon — see [`light_from`].
const GRAZE: f32 = 0.02;

/// How often a client waiting for dawn says so, in seconds. Comfortably
/// inside the server's `WAIT_LAPSE`, so a held key reads as one unbroken
/// wish, and rare enough that holding it down is a message every few frames
/// rather than every one.
const ASK_EVERY: f32 = 0.4;

/// How far the night's offer sits up from the bottom of the window, in
/// pixels — clear of the compass in the corner beside it.
const OFFER_MARGIN: f32 = 26.0;
/// The offer's type size.
const OFFER_SIZE: f32 = 15.0;
/// The same ink the compass letters and the menus use, so a line of prose
/// over the world reads as a piece of the same chart.
const OFFER_INK: Color = Color::srgb(0.88, 0.87, 0.80);

/// One moment of the day, and what the world looks like at it. The moments
/// are keyframes: [`light_at`] reads any hour off the pair either side of it.
#[derive(Clone, Copy)]
struct Hour {
    /// Where in the day this stands, as a phase — see
    /// [`protocol::ToClient::Daylight`].
    at: f32,
    /// The colour of whichever body is up, and how hard it burns.
    light: Color,
    lux: f32,
    /// The sky's own light, which is what everything in shadow is lit by —
    /// and at night, near enough what everything is lit by.
    fill: Color,
    brightness: f32,
    /// The sky itself: what the camera clears to, and what the haze fades
    /// the far ground into. The two are one colour or the horizon shows as a
    /// line — see [`crate::camera::haze`].
    sky: Color,
}

/// The day, as a dozen moments to read the hours between.
///
/// Sunrise at 0.25 and sunset at 0.75 are where the light swaps bodies, so
/// both are keyed almost dark: the colour has to be one that suits the last
/// of the moon and the first of the sun at once, because at that instant the
/// same light is about to become the other one. The drama of dawn and dusk
/// is carried by the *sky* — which is at its most coloured exactly there —
/// and by the low golden light either side of it, which is a few tens of
/// seconds of a ten-minute day.
///
/// The first entry must sit at 0.0, since that is what the wrap round
/// midnight is read against.
const HOURS: [Hour; 12] = [
    Hour {
        at: 0.0, // midnight
        light: Color::srgb(0.55, 0.66, 1.00),
        lux: 260.0,
        fill: Color::srgb(0.34, 0.44, 0.78),
        brightness: 40.0,
        sky: Color::srgb(0.020, 0.030, 0.075),
    },
    Hour {
        at: 0.18,
        light: Color::srgb(0.55, 0.66, 1.00),
        lux: 260.0,
        fill: Color::srgb(0.34, 0.44, 0.78),
        brightness: 55.0,
        sky: Color::srgb(0.040, 0.055, 0.130),
    },
    Hour {
        at: 0.22, // daybreak: the light changes bodies, and the sky lifts
        light: Color::srgb(0.62, 0.60, 0.85),
        lux: 280.0,
        fill: Color::srgb(0.50, 0.52, 0.75),
        brightness: 260.0,
        sky: Color::srgb(0.260, 0.230, 0.380),
    },
    Hour {
        at: 0.25, // sunrise proper: the first of it comes in along the water
        light: Color::srgb(1.00, 0.62, 0.42),
        lux: 1_400.0,
        fill: Color::srgb(0.80, 0.66, 0.66),
        brightness: 620.0,
        sky: Color::srgb(0.880, 0.560, 0.420),
    },
    Hour {
        at: 0.29, // the sun clear of the horizon: long light, hard shadows
        light: Color::srgb(1.00, 0.78, 0.52),
        lux: 4_500.0,
        fill: Color::srgb(0.86, 0.84, 0.92),
        brightness: 1_000.0,
        sky: Color::srgb(0.780, 0.740, 0.800),
    },
    Hour {
        at: 0.34, // morning
        light: Color::srgb(1.00, 0.90, 0.78),
        lux: 6_800.0,
        fill: Color::srgb(0.84, 0.88, 1.00),
        brightness: 1_250.0,
        sky: Color::srgb(0.660, 0.790, 0.920),
    },
    Hour {
        at: 0.50, // noon — the hour the world had before it had any others
        light: Color::srgb(1.00, 0.98, 0.94),
        lux: 8_500.0,
        fill: Color::srgb(0.82, 0.89, 1.00),
        brightness: 1_400.0,
        sky: crate::SKY,
    },
    Hour {
        at: 0.66, // afternoon
        light: Color::srgb(1.00, 0.90, 0.76),
        lux: 6_800.0,
        fill: Color::srgb(0.84, 0.88, 1.00),
        brightness: 1_250.0,
        sky: Color::srgb(0.660, 0.780, 0.900),
    },
    Hour {
        at: 0.71, // the golden hour, redder than its morning twin
        light: Color::srgb(1.00, 0.72, 0.42),
        lux: 4_500.0,
        fill: Color::srgb(0.88, 0.80, 0.82),
        brightness: 980.0,
        sky: Color::srgb(0.840, 0.720, 0.700),
    },
    Hour {
        at: 0.75, // sunset: the sun on the horizon, and everything red
        light: Color::srgb(1.00, 0.50, 0.34),
        lux: 1_500.0,
        fill: Color::srgb(0.82, 0.60, 0.60),
        brightness: 600.0,
        sky: Color::srgb(0.920, 0.420, 0.280),
    },
    Hour {
        at: 0.79, // nightfall: the colour drains upwards, the moon takes over
        light: Color::srgb(0.62, 0.58, 0.88),
        lux: 320.0,
        fill: Color::srgb(0.48, 0.48, 0.72),
        brightness: 240.0,
        sky: Color::srgb(0.300, 0.200, 0.380),
    },
    Hour {
        at: 0.85,
        light: Color::srgb(0.55, 0.66, 1.00),
        lux: 260.0,
        fill: Color::srgb(0.34, 0.44, 0.80),
        brightness: 60.0,
        sky: Color::srgb(0.070, 0.080, 0.180),
    },
];

/// The hour the world is drawn at.
///
/// Kept rather than asked, unlike the server's own clock, because this one is
/// chasing something: `phase` is where the sun is drawn and `told` is where
/// the server last said it should be, and the gap between them is what
/// [`advance_the_day`] closes.
#[derive(Resource)]
pub struct Sky {
    /// The hour as drawn.
    phase: f32,
    /// The hour the server last named, run on by this client's own clock in
    /// the time since — so the two are comparable at any moment rather than
    /// only at the instant a message lands. `None` until the first word,
    /// which is taken as a snap: easing onto it would animate a whole day
    /// passing in the second the world opened.
    told: Option<f32>,
    /// An hour held still by the socket's `hold` — see [`Sky::hold`] — outranking the
    /// real one for as long as it is set. Local by construction: it changes
    /// what this machine draws and nothing about what time it is in the
    /// world. The clock keeps running underneath, so letting go returns to
    /// the true hour rather than to the one it was when the order was given.
    /// Set through [`Sky::hold`] alone, which is the one thing entitled to
    /// decide there is an hour worth freezing.
    commanded: Option<f32>,
}

impl Default for Sky {
    fn default() -> Self {
        Self {
            phase: ASSUMED,
            told: None,
            commanded: None,
        }
    }
}

impl Sky {
    /// The hour to draw: the ordered one while something holds it, otherwise
    /// the one the world is actually at.
    pub fn phase(&self) -> f32 {
        self.commanded.unwrap_or(self.phase)
    }

    /// What the server last said the time was. Where a client sets this and
    /// why the first word snaps is [`crate::net::receive`].
    pub fn told(&mut self, phase: f32) {
        if self.told.is_none() {
            self.phase = phase;
        }
        self.told = Some(phase);
    }

    /// Whether a server has ever said what hour it is.
    ///
    /// Not the same question as [`Sky::caught_up`], and the difference is
    /// what lets `hold` tell a wait from a refusal: a run that has heard a
    /// word and not yet arrived at it is on its way, and one that has heard
    /// none may have no world to hear from at all.
    pub fn heard_the_hour(&self) -> bool {
        self.told.is_some()
    }

    /// Whether the drawn hour has closed on the hour the server last named.
    ///
    /// What `hold` waits for. The gap is eased across rather than jumped —
    /// see [`advance_the_day`] — so on the frame a `time` is answered the sky
    /// is still leaving the hour it was at, and freezing it there would pin
    /// the hour the driver had just moved off. `false` before the first word,
    /// there being no hour yet to have caught up with.
    pub fn caught_up(&self) -> bool {
        self.told.is_some_and(|told| {
            let ahead = (told - self.phase).rem_euclid(1.0);
            ahead.min(1.0 - ahead) <= CAUGHT_UP
        })
    }

    /// Holds the sky still at the hour it stands at, once there is a true
    /// one to hold — see [`crate::control`]'s `hold`, which is the whole
    /// reason this exists. Before the first word from the server there is nothing worth
    /// freezing, so this does nothing and is asked again next frame.
    pub fn hold(&mut self) {
        if self.told.is_some() {
            self.commanded = Some(self.phase());
        }
    }

    /// Whether it is night as drawn — which is what decides whether there is
    /// a night to offer to wait out, and, in [`crate::clouds`], whether the
    /// light is carrying any weather. One answer for both: the clouds may
    /// only go on and off at the moment the light changes bodies, so a second
    /// spelling of this would be a second opinion about when that is.
    pub fn is_night(&self) -> bool {
        protocol::is_night(self.phase())
    }
}

/// Whichever of the two bodies is above the horizon.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Aloft {
    Sun,
    Moon,
}

/// Which body is up at an hour, and how far it is through its own crossing of
/// the sky: `0.0` as it rises, `1.0` as it sets.
///
/// The half-turn either side of [`towards_the_sun`]'s own horizon — the sun is
/// up from 0.25 to 0.75, and the moon, being opposite it, holds the other
/// half. What [`crate::instruments`] draws the day's arc from.
///
/// Not a second opinion about [`Sky::is_night`], which asks a different
/// question: that one is about which body is *lighting the world*, and the two
/// deliberately part company for the last tenths of an hour either side of the
/// swap — see [`light_from`] for what that buys.
pub fn aloft(phase: f32) -> (Aloft, f32) {
    let risen = (phase - 0.25).rem_euclid(1.0);
    if risen < 0.5 {
        (Aloft::Sun, risen / 0.5)
    } else {
        (Aloft::Moon, (risen - 0.5) / 0.5)
    }
}

/// Marks the one light in the sky, so the systems that aim and colour it can
/// find it again. It is the sun for most of the day and the moon for the rest
/// — see the module doc for why that is one entity and not two. Public
/// because [`crate::clouds`] hangs its shadows on this same light.
#[derive(Component)]
pub struct SkyLight;

/// Marks the line of text the night offers itself with.
#[derive(Component)]
struct NightOffer;

/// Marks the text the offer is written into: its one child, and the only
/// [`Text`] this module has any business writing to. Without it the query
/// that puts the words there asks for mutable access to every piece of text
/// in the world — the debug overlay's counters included — to write one.
#[derive(Component)]
struct OfferLine;

pub struct SkyPlugin;

impl Plugin for SkyPlugin {
    fn build(&self, app: &mut App) {
        let day = light_at(NO_HOUR);
        app
            // The clouds are a mask on the light this module hangs, and are
            // drawn by putting that light in the right place — so they come
            // with the sky rather than standing beside it in the binary.
            .add_plugins(CloudsPlugin)
            .init_resource::<Sky>()
            .insert_resource(ClearColor(day.sky))
            .insert_resource(GlobalAmbientLight {
                color: day.fill,
                brightness: day.brightness,
                ..default()
            })
            .add_systems(OnEnter(AppState::InWorld), (hang_the_light, offer_spawn))
            .add_systems(OnExit(AppState::InWorld), leave_the_world)
            .add_systems(
                Update,
                (advance_the_day, light_the_world, offer_the_night)
                    .chain()
                    // After the clouds have moved, since where this puts the
                    // light is where they are: the two read and write one
                    // resource in one schedule, and which frame's weather the
                    // light stands in is not the executor's to choose.
                    .after(crate::clouds::drift_downwind)
                    .run_if(in_state(AppState::InWorld)),
            )
            // Only with the helm: a paused game is a player who is not
            // holding anything down, whatever the keyboard says.
            .add_systems(
                Update,
                ask_for_dawn.run_if(in_state(Helm::Sailing).and_then(resource_exists::<Online>)),
            );
    }
}

/// Which way the sun lies from the ground at an hour, as a unit vector: `x`
/// east, `y` up, `z` south — [`protocol::ground::NORTH`] being `-z`.
///
/// The sun rises due east at 0.25, stands at its highest at noon and sets due
/// west at 0.75; below the horizon the same vector goes on round, and negated
/// it is where the moon is. The arc is tilted southward by [`TILT`] rather
/// than passing overhead, so `y` is never quite 1 and the light is never
/// quite straight down.
fn towards_the_sun(phase: f32) -> Vec3 {
    let (up, east) = ((phase - 0.25) * TAU).sin_cos();
    Vec3::new(east, up * TILT.cos(), up * TILT.sin())
}

/// Which way the world's light comes from at an hour: the sun through the
/// day, the moon through the night, and neither of them from underneath.
///
/// The bodies change places at the night's own edges — [`protocol::NIGHTFALL`]
/// and [`protocol::DAYBREAK`] — rather than as the sun crosses the horizon,
/// and that is what buys the sunset. Swapping at the horizon would have meant
/// keying the light almost out at exactly the moment it should be at its most
/// coloured, since the two bodies are opposite each other and a bright flip
/// is a visible one. Swapping in the dark instead leaves the sun lighting the
/// world for the last few tenths of an hour of its day, and by the time the
/// moon takes it on the light is a fifth of noon's and the sky is violet.
///
/// What it costs is that a body just past its horizon would light the ground
/// from below — every overhang lit and every top dark, which reads as nothing
/// in nature. So the direction is held at [`GRAZE`] rather than allowed under
/// it: the last of the sun lies along the water instead of coming up out of it.
fn light_from(phase: f32) -> Vec3 {
    let sun = towards_the_sun(phase);
    let body = if protocol::is_night(phase) { -sun } else { sun };
    Vec3::new(body.x, body.y.max(GRAZE), body.z).normalize()
}

/// The light and colours of an hour, read off [`HOURS`].
fn light_at(phase: f32) -> Hour {
    let phase = phase.rem_euclid(1.0);
    // The last keyframe at or before this hour, and the one after it — which
    // for the small hours after the last keyframe is the first one again, a
    // day on. HOURS[0] sits at 0.0, so there is always one at or before.
    let i = HOURS
        .iter()
        .rposition(|hour| hour.at <= phase)
        .unwrap_or(HOURS.len() - 1);
    let (from, to) = (HOURS[i], HOURS[(i + 1) % HOURS.len()]);
    let span = (to.at - from.at).rem_euclid(1.0);
    let t = ((phase - from.at).rem_euclid(1.0) / span).clamp(0.0, 1.0);

    Hour {
        at: phase,
        light: from.light.mix(&to.light, t),
        lux: from.lux.lerp(to.lux, t),
        fill: from.fill.mix(&to.fill, t),
        brightness: from.brightness.lerp(to.brightness, t),
        sky: from.sky.mix(&to.sky, t),
    }
}

/// Hangs the one light the world is lit by. Aimed and coloured from the next
/// frame on by [`light_the_world`]; what is set here is only what does not
/// change with the hour.
fn hang_the_light(mut commands: Commands) {
    // Shadow map resolution. The first cascade spreads its texels over the
    // whole frustum slice out to its far bound — about a hundred metres of
    // diagonal at the default zoom — so at Bevy's default 2048 a texel is
    // around 5 cm of world. The terrain never notices: its facets are metres
    // across and their shadows are broad shapes. The mast does. It is the
    // thinnest caster in the world, and at 16 cm its shadow is a stripe three
    // texels wide, whose edges snap from texel to texel as the boat moves —
    // a visible flicker along the whole stripe. Doubling the resolution
    // halves the texel and the stripe stops seething. The cost is GPU memory
    // (each cascade is one square layer of this size), which is why it stops
    // at 4096 rather than going further.
    commands.insert_resource(DirectionalLightShadowMap { size: 4096 });

    commands.spawn((
        Name::new("Sky light"),
        SkyLight,
        DespawnOnExit(AppState::InWorld),
        DirectionalLight {
            shadow_maps_enabled: true,
            // The default biases cause bad self-shadowing acne on a heightfield
            // this large — dark speckle all over the hillsides.
            shadow_depth_bias: 0.06,
            shadow_normal_bias: 2.2,
            ..default()
        },
        crate::terrain::cascades(crate::HAZE_END),
        Transform::default(),
    ));
}

/// Runs the clock, and closes whatever gap the server's last word left.
///
/// Reachable from [`crate::control`]'s tests, which drive a `hold` against
/// the real ease rather than against a phase moved by hand — the gap this
/// closes is the whole of what that line waits for.
///
/// Both halves matter. Running it here is what makes the sun move smoothly
/// between tellings a second apart; easing onto the telling is what keeps
/// this machine's day the same day as everyone else's — and it is the whole
/// of how a night being run off at sixty times the pace reaches the picture,
/// since nothing on this side knows that is happening.
///
/// The gap is taken as the shorter way round, which is safe because the server
/// never *jumps* its clock: a client that has stopped hearing from one mid-night
/// falls at most 0.15 of a day behind — the server's `WAIT_LAPSE` at
/// `NIGHT_PACE` — against the half day this decides the way round on. Raising
/// either constant spends that margin.
///
/// The ease trails the server through a run-off rather than sitting on it, so
/// this machine is still in the night for about a second after the server has
/// reached daybreak and goes on sending `WantDawn`. Harmless: the server tests
/// `is_night` before it runs anything off, and on screen it reads as the tail
/// of the night it is.
pub(crate) fn advance_the_day(time: Res<Time>, mut sky: ResMut<Sky>) {
    let step = time.delta_secs() / protocol::DAY_SECONDS;
    sky.phase = (sky.phase + step).rem_euclid(1.0);

    let Some(told) = sky.told else { return };
    // The word ages with our own clock, so that "where the server said the
    // sun was, a second ago" keeps meaning where the sun is now. Without
    // this the gap would grow by a second of day every second and the ease
    // would be forever pulling the sun backwards into the last message.
    let told = (told + step).rem_euclid(1.0);
    sky.told = Some(told);

    let ahead = (told - sky.phase).rem_euclid(1.0);
    let gap = if ahead <= 0.5 { ahead } else { ahead - 1.0 };
    sky.phase = (sky.phase + gap * eased(CATCH_UP, time.delta_secs())).rem_euclid(1.0);
}

/// Puts the hour on the scene: where the light comes from, what colour it is,
/// and what the sky behind it does.
///
/// The light stands in whichever body is above the horizon — the sun while
/// `y` is up, the moon opposite it while it is not — and the sky's own light
/// carries the rest. Ambient is deliberately strong against the light all
/// day: this look wants a shadow to read as a second flat tone rather than as
/// darkness, and at night it is most of what there is to see by.
fn light_the_world(
    sky: Res<Sky>,
    clouds: Res<Clouds>,
    mut lights: Query<(&mut Transform, &mut DirectionalLight), With<SkyLight>>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut clear: ResMut<ClearColor>,
    mut haze: Query<&mut DistanceFog>,
) {
    let hour = light_at(sky.phase());
    let from = light_from(hour.at);

    for (mut transform, mut light) in &mut lights {
        // Facing down the direction the light comes from, and standing
        // wherever the clouds want it to. A directional light shines the same
        // way from anywhere, so only its facing is this module's business —
        // and that is exactly what leaves its position and scale free for
        // [`crate::clouds`] to hang the cloud shadows off.
        *transform = clouds.stand_the_light(from);
        light.color = hour.light;
        light.illuminance = hour.lux;
    }

    ambient.color = hour.fill;
    ambient.brightness = hour.brightness;
    clear.0 = hour.sky;
    // The haze has to be the sky's own colour at every hour, or the far
    // ground fades into a daylight horizon under a night sky.
    for mut fog in &mut haze {
        fog.color = hour.sky;
    }
}

/// Tells the server this player would like the night over with, while they
/// are in a position to ask: at anchor — no way on the hull — with a night to
/// wait out and the key held down.
///
/// Sent on a beat rather than once, because the wish is a standing one that
/// lapses: letting go of the key is a client that has simply stopped asking,
/// and needs no message of its own. What the asking gets is the server's
/// business — the night runs only while everyone in the world is asking.
fn ask_for_dawn(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    bindings: Res<KeyBindings>,
    sky: Res<Sky>,
    online: Res<Online>,
    boats: Query<&Boat>,
    mut asked: Local<f32>,
) {
    if !waiting_out_the_night(&keys, &bindings, &sky, &boats) {
        return;
    }
    let now = time.elapsed_secs();
    if now - *asked < ASK_EVERY {
        return;
    }
    *asked = now;
    online.connection.want_dawn();
}

/// Whether the player is, right now, holding the boat at anchor through the
/// night — which is both what [`ask_for_dawn`] sends on and what
/// [`offer_the_night`] says out loud.
fn waiting_out_the_night(
    keys: &ButtonInput<KeyCode>,
    bindings: &KeyBindings,
    sky: &Sky,
    boats: &Query<&Boat>,
) -> bool {
    sky.is_night() && at_anchor(boats) && keys.pressed(bindings.key(Action::WaitOutNight))
}

/// Whether the boat is lying still. A hull with way on is one being sailed,
/// and a player sailing has not turned in for the night — the same reading
/// `player::embark_or_land` takes before it lets anybody step off a deck,
/// forgiveness and all: a glide's last imperceptible tail must not withhold
/// the night any more than it may refuse the shore.
///
/// There is one boat in a world, so this is a question about *the* boat: no
/// boat at all is a world still being entered, which has no night to offer
/// yet.
fn at_anchor(boats: &Query<&Boat>) -> bool {
    boats.single().is_ok_and(Boat::reads_as_stopped)
}

/// The line the night puts on the screen, spawned hidden and left to
/// [`offer_the_night`].
fn offer_spawn(mut commands: Commands) {
    commands.spawn((
        NightOffer,
        DespawnOnExit(AppState::InWorld),
        Visibility::Hidden,
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(OFFER_MARGIN),
            left: Val::Px(0.0),
            right: Val::Px(0.0),
            justify_content: JustifyContent::Center,
            ..default()
        },
        children![(
            OfferLine,
            Text::default(),
            TextFont {
                font: FontSource::Serif,
                font_size: FontSize::Px(OFFER_SIZE),
                ..default()
            },
            TextColor(OFFER_INK),
        )],
    ));
}

/// Offers the night, and says when it is being waited out.
///
/// Only while there is something to offer: a night, and a boat lying still to
/// wait it out from — ashore counts, the boat a walker left on the beach
/// being as still as one anybody is standing on. Under way, or in daylight,
/// the line is not there at all: a control that cannot be used is furniture.
///
/// It says what to do rather than what has happened, and names no hour: the
/// night is plainly a night to look at, and a readout of the time would be
/// this world telling the player something the sky has already said.
fn offer_the_night(
    keys: Res<ButtonInput<KeyCode>>,
    bindings: Res<KeyBindings>,
    sky: Res<Sky>,
    boats: Query<&Boat>,
    helm: Option<Res<State<Helm>>>,
    offer: Single<(&mut Visibility, &Children), With<NightOffer>>,
    mut lines: Query<&mut Text, With<OfferLine>>,
) {
    let (mut visibility, children) = offer.into_inner();
    let sailing = helm.is_none_or(|helm| *helm.get() == Helm::Sailing);
    let offered = sailing && sky.is_night() && at_anchor(&boats);
    *visibility = if offered {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    if !offered {
        return;
    }

    let waiting = waiting_out_the_night(&keys, &bindings, &sky, &boats);
    // The key is named by what it typed when it was chosen, so a rebound or
    // foreign keyboard is offered its own key rather than a US one — see
    // [`KeyBindings::name`].
    let wanted = if waiting {
        "Waiting for dawn…".to_string()
    } else {
        format!(
            "Hold {} to wait out the night",
            bindings.name(Action::WaitOutNight)
        )
    };

    for child in children.iter() {
        if let Ok(mut line) = lines.get_mut(child) {
            // Written only when it turns over, which is at most twice a
            // night. Assigning the same words again marks the text changed
            // and puts the line back through layout, every frame, all night,
            // to say what it already said. Judged against the text itself
            // rather than remembered, because what is on screen is a fresh
            // entity in every world entered and a memory of the last one
            // would leave the second world's line blank.
            if line.0 != wanted {
                line.0 = wanted.clone();
            }
        }
    }
}

/// Puts the daylight back on the way out.
///
/// The clear colour, the ambient light and the camera's haze are the app's,
/// not the world's — the camera outlives the world it was looking at — so a
/// world left at midnight would otherwise hand the menus a black screen, no
/// light to draw by, and a haze still keyed to a violet sky. All three go
/// back together, exactly as [`light_the_world`] moves them together, or the
/// next world's first frame fades its ground into last night. The hour goes
/// back to being unknown for the same reason it snaps on the first word: the
/// next world entered is a different world, and its time is not this one's.
fn leave_the_world(
    mut sky: ResMut<Sky>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut clear: ResMut<ClearColor>,
    mut haze: Query<&mut DistanceFog>,
) {
    *sky = Sky::default();
    let day = light_at(NO_HOUR);
    ambient.color = day.fill;
    ambient.brightness = day.brightness;
    clear.0 = day.sky;
    for mut fog in &mut haze {
        fog.color = day.sky;
    }
}

#[cfg(test)]
mod tests {
    use bevy::asset::AssetPlugin;
    use bevy::state::app::StatesPlugin;
    use bevy::time::{TimePlugin, TimeUpdateStrategy};

    use server::WorldConfig;

    use super::*;
    use crate::boat::BoatPlugin;
    use crate::camera::View;
    use crate::net::{Online, Reach, Session};
    use crate::testing::{hold, run_frames, run_until, set_wind, FRAME};

    /// A headless app in a world with the sky running, and a boat in it to
    /// lie at anchor. No terrain and no renderer: what these tests are about
    /// is a clock, a direction and a colour, none of which needs a GPU.
    fn sky_app() -> App {
        let mut app = sky_app_ashore_of_entry();
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::InWorld);
        app.update();
        app
    }

    /// The same app, stopped short of entering the world — for the one test
    /// that joins a real session first, the way a real run does: whether a
    /// boat is launched or told depends on the session being in place
    /// before the threshold is crossed.
    fn sky_app_ashore_of_entry() -> App {
        let mut app = App::new();
        app.add_plugins((
            TaskPoolPlugin::default(),
            AssetPlugin::default(),
            TimePlugin,
            StatesPlugin,
            // The boat's plugin readies the rowboat's clips, and clips and
            // the graph they hang in are assets of this plugin's.
            bevy::animation::AnimationPlugin,
            BoatPlugin,
            SkyPlugin,
        ))
        .insert_resource(TimeUpdateStrategy::ManualDuration(FRAME))
        .init_state::<AppState>()
        .add_sub_state::<Helm>()
        .init_resource::<View>()
        .init_resource::<KeyBindings>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_asset::<Mesh>()
        // What the clouds' mask is put into, the sky bringing them with it.
        .init_asset::<Image>()
        .init_resource::<Assets<StandardMaterial>>();
        app.update();
        app
    }

    fn phase(app: &App) -> f32 {
        app.world().resource::<Sky>().phase()
    }

    fn set_phase(app: &mut App, phase: f32) {
        app.world_mut().resource_mut::<Sky>().phase = phase;
    }

    /// The line the night offers itself with, or `None` while it is not being
    /// offered at all.
    fn offer(app: &mut App) -> Option<String> {
        let shown = app
            .world_mut()
            .query_filtered::<&Visibility, With<NightOffer>>()
            .single(app.world())
            .is_ok_and(|visibility| *visibility != Visibility::Hidden);
        shown.then(|| {
            app.world_mut()
                .query::<&Text>()
                .iter(app.world())
                .map(|text| text.0.clone())
                .next()
                .expect("the offer has a line to say")
        })
    }

    #[test]
    fn the_day_turns_at_the_pace_the_wire_names() {
        // Ten minutes to the day, and the client keeps that clock itself
        // between the server's tellings — so a minute of frames is a tenth of
        // a day, wherever it started.
        let mut app = sky_app();
        set_phase(&mut app, 0.4);
        let before = phase(&app);

        run_frames(&mut app, 60);
        let turned = phase(&app) - before;
        let expected = 60.0 * FRAME.as_secs_f32() / protocol::DAY_SECONDS;
        assert!(
            (turned - expected).abs() < expected * 0.05,
            "a second of frames turned the day by {turned} rather than {expected}"
        );
    }

    #[test]
    fn the_first_word_from_the_server_is_taken_whole_and_later_ones_are_eased_onto() {
        // The first is a snap: a client draws an assumed morning until it is
        // told, and easing off that would animate a day that never happened.
        let mut app = sky_app();
        app.world_mut().resource_mut::<Sky>().told(0.8);
        assert!(
            (phase(&app) - 0.8).abs() < 1e-6,
            "the first word was eased onto rather than taken"
        );

        // The second is not. It is a correction to a clock already running,
        // and the sun may not jump from one telling to the next.
        app.world_mut().resource_mut::<Sky>().told(0.9);
        app.update();
        let after = phase(&app);
        assert!(
            after > 0.8 && after < 0.9,
            "a later word moved the sun straight to {after}"
        );
    }

    #[test]
    fn a_night_run_off_is_caught_up_with_forwards_through_midnight() {
        // What the server does with a night everybody is waiting out is run
        // its clock fast, so its word arrives ahead of ours — and the way
        // round to it has to be forwards even when the shorter way crosses
        // midnight. A sun that answered by going backwards would be a day
        // undoing itself.
        let mut app = sky_app();
        set_phase(&mut app, 0.97);
        app.world_mut().resource_mut::<Sky>().told(0.97);
        app.world_mut().resource_mut::<Sky>().told(0.04);

        let mut been = vec![phase(&app)];
        run_until(
            &mut app,
            "the clock has caught up with the small hours",
            |app| {
                been.push(phase(app));
                (phase(app) - 0.04).abs() < 0.005
            },
        );

        // Forwards the whole way: every step either went on round the day or
        // wrapped past midnight, and none of them went back.
        for pair in been.windows(2) {
            let step = (pair[1] - pair[0]).rem_euclid(1.0);
            assert!(
                step < 0.5,
                "the day went backwards from {} to {}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn the_clouds_move_the_light_without_moving_where_it_shines_from() {
        // The cloud shadows are carried by walking the light itself downwind —
        // see [`crate::clouds`] — which is only safe because a directional
        // light's position means nothing. What would undo that is aiming it at
        // the middle of the world instead of down its own bearing: the sun
        // would then swing round as the weather blew past, by more the longer
        // anybody played.
        let mut app = sky_app();
        app.world_mut().resource_mut::<crate::sea::Forecast>().wind = Some(Vec2::new(30.0, -18.0));
        run_frames(&mut app, 60);

        let phase = phase(&app);
        let light = app
            .world_mut()
            .query_filtered::<&Transform, With<SkyLight>>()
            .single(app.world())
            .expect("a world has one light in it")
            .to_owned();
        assert!(
            light.translation.xz().length() > 1.0,
            "the clouds have not gone anywhere: {}",
            light.translation
        );
        assert!(
            light.forward().dot(-light_from(phase)) > 0.9999,
            "the drift has aimed the light at {} rather than down {}",
            light.forward().as_vec3(),
            -light_from(phase)
        );
    }

    /// What [`crate::instruments`] draws the day's arc from: the two bodies
    /// take the sky in turn, each crossing it once, and neither is ever half
    /// way through a crossing it has not begun.
    #[test]
    fn the_bodies_take_the_sky_in_turn() {
        assert_eq!(
            aloft(0.25),
            (Aloft::Sun, 0.0),
            "the sun did not rise at 0.25"
        );
        assert_eq!(aloft(0.5), (Aloft::Sun, 0.5), "noon was not mid-crossing");
        assert_eq!(
            aloft(0.75),
            (Aloft::Moon, 0.0),
            "the moon did not rise at 0.75"
        );
        assert_eq!(
            aloft(0.0),
            (Aloft::Moon, 0.5),
            "midnight was not mid-crossing"
        );

        // The wrap round midnight, which is where an arithmetic that forgot to
        // fold would hand back a crossing outside its own arc.
        for hundredth in 0..100 {
            let (_, through) = aloft(hundredth as f32 / 100.0);
            assert!(
                (0.0..1.0).contains(&through),
                "the hour {hundredth} was {through} through a crossing"
            );
        }

        // And the swap is the horizon, not the night: the light is still the
        // sun's a little past sunset — see `light_from` — so these two are
        // entitled to disagree, and the arc must follow the horizon.
        assert_eq!(aloft(0.77).0, Aloft::Moon);
        assert!(!protocol::is_night(0.77), "the test's premise has moved");
    }

    #[test]
    fn the_sun_rises_in_the_east_stands_south_of_overhead_and_sets_in_the_west() {
        // The world's east is +x and its north is -z — see
        // `protocol::ground::NORTH` — so this is the whole of what a day
        // looks like from the ground.
        let sunrise = towards_the_sun(0.25);
        assert!(sunrise.x > 0.99, "the sun rose at {sunrise}");
        assert!(sunrise.y.abs() < 1e-6, "the sun rose {} up", sunrise.y);

        let noon = towards_the_sun(0.5);
        assert!(noon.y > 0.9, "noon stands {} up", noon.y);
        assert!(noon.z > 0.0, "noon is not south of overhead: {noon}");
        assert!(noon.y < 1.0, "noon is straight overhead, which flattens it");

        let sunset = towards_the_sun(0.75);
        assert!(sunset.x < -0.99, "the sun set at {sunset}");

        let midnight = towards_the_sun(0.0);
        assert!(midnight.y < -0.9, "the sun at midnight is {midnight}");
    }

    #[test]
    fn the_light_comes_from_the_sun_by_day_the_moon_by_night_and_never_from_below() {
        // Through the day it stands where the sun is; through the night it is
        // opposite, which is where a full moon is. And at no hour at all does
        // it light the world from underneath.
        for i in 0..1_000 {
            let phase = i as f32 / 1_000.0;
            let light = light_from(phase);
            // Not quite `>= GRAZE`: holding the direction up and then
            // making it a unit vector again shortens it by a hair, which is
            // nothing to a light and everything to an exact comparison.
            assert!(
                light.y > GRAZE * 0.99,
                "the light at {phase} comes from {} up",
                light.y
            );
            assert!(
                (light.length() - 1.0).abs() < 1e-4,
                "the light at {phase} is not a direction: {light}"
            );

            let sun = towards_the_sun(phase);
            let side = if protocol::is_night(phase) { -sun } else { sun };
            assert!(
                light.xz().dot(side.xz()) > 0.0,
                "the light at {phase} comes from {light}, not from {side}"
            );
        }
    }

    #[test]
    fn noon_is_the_brightest_hour_and_the_small_hours_the_darkest() {
        let noon = light_at(0.5);
        let midnight = light_at(0.0);
        let dusk = light_at(0.79);

        assert!(
            midnight.lux * 10.0 < noon.lux && midnight.brightness * 10.0 < noon.brightness,
            "the night is not a night: {} lux and {} of fill against noon's {} and {}",
            midnight.lux,
            midnight.brightness,
            noon.lux,
            noon.brightness
        );
        // And dusk lies between them rather than beside either: a night that
        // arrived at nightfall would be a light switch.
        assert!(dusk.brightness > midnight.brightness && dusk.brightness < noon.brightness);
    }

    #[test]
    fn the_light_changes_hand_without_the_picture_jumping() {
        // The one light stands in two bodies, and the moment it changes is
        // the moment this whole table is arranged around: walked minute by
        // minute, no step of the day may move the light, the fill or the sky
        // by more than a nudge. What this catches is a keyframe put where the
        // swap is bright, which reads as the sun blinking to the other side
        // of the world.
        let step = 1.0 / (24.0 * 60.0);
        let mut before = light_at(0.0);
        for i in 1..(24 * 60) {
            let at = i as f32 * step;
            let now = light_at(at);
            let jump = (now.lux - before.lux).abs();
            assert!(jump < 400.0, "the light jumps {jump} lux at {at}",);
            let colour = (now.light.to_linear().to_vec3() - before.light.to_linear().to_vec3())
                .length()
                + (now.sky.to_linear().to_vec3() - before.sky.to_linear().to_vec3()).length();
            assert!(colour < 0.05, "the colour jumps by {colour} at {at}");
            before = now;
        }
    }

    #[test]
    fn the_night_is_offered_to_a_boat_lying_still_and_to_nobody_else() {
        let mut app = sky_app();

        // By day there is nothing to wait out, however still the boat lies.
        app.world_mut().resource_mut::<Sky>().commanded = Some(0.5);
        app.update();
        assert_eq!(offer(&mut app), None, "the day offered a night");

        // By night, at anchor, it says which key waits it out.
        app.world_mut().resource_mut::<Sky>().commanded = Some(0.9);
        app.update();
        let offered = offer(&mut app).expect("the night was not offered");
        assert!(
            offered.contains('R'),
            "the offer does not name its key: {offered}"
        );

        // And with the key down it says what is happening instead.
        hold(&mut app, KeyCode::KeyR);
        app.update();
        let waiting = offer(&mut app).expect("the offer went away when it was taken up");
        assert_ne!(waiting, offered, "holding the key changed nothing");
    }

    #[test]
    fn a_boat_under_way_is_offered_no_night() {
        // Sailing on through the dark is a choice the player is allowed to
        // make; what they cannot do is skip the night while making it.
        let mut app = sky_app();
        app.world_mut().resource_mut::<Sky>().commanded = Some(0.9);
        app.update();
        assert!(offer(&mut app).is_some(), "the night was not offered");

        // Dead astern of the default heading — the assumed wind would leave
        // the boat in irons, and a boat that never made way would pass this
        // test with the offer wrongly still up.
        set_wind(&mut app, Vec2::new(-5.0, -5.0));
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 6);
        assert_eq!(offer(&mut app), None, "a boat making way was offered a bed");
    }

    #[test]
    fn a_night_waited_out_at_anchor_ends_in_the_morning() {
        // The whole of it, over a real socket: a world opened in the small
        // hours, a boat lying still in it, and the key held down. The client
        // asks, the server runs its clock, and this machine's own sky follows
        // it out of the night.
        crate::testing::quarantine_data_dir();
        let session = Session::open(WorldConfig { seed: 5 }, Reach::Alone, 0.19, false)
            .expect("a world to lie at anchor in");
        // The session in hand *before* the threshold, as a real join has it:
        // the boat lain at anchor is the one the server tells of, not one
        // the offline entry would have launched beside it.
        let mut app = sky_app_ashore_of_entry();
        app.add_plugins(crate::net::NetPlugin);
        app.insert_resource(Online::new(session.connection));
        let _hosting = session.hosting;
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::InWorld);
        app.update();

        run_until(&mut app, "the world has said what time it is", |app| {
            app.world().resource::<Sky>().told.is_some()
        });
        assert!(
            app.world().resource::<Sky>().is_night(),
            "the world did not open in the night it was given"
        );

        hold(&mut app, KeyCode::KeyR);
        run_until(&mut app, "the night has been waited out", |app| {
            !app.world().resource::<Sky>().is_night()
        });
    }
}
