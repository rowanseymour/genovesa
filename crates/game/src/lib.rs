//! The app that walks around in the terrain.
//!
//! Nothing here generates any of it. The ground arrives over a connection a
//! chunk at a time and [`terrain`] draws the answers; what a chunk *is* comes
//! from the `protocol` crate, which is the whole of what this crate knows
//! about the world. This crate is a library under a thin `game` binary, so
//! that the app's own parts — the camera, the menu, the command line — are
//! reachable from tests rather than sealed inside a `main`.

pub mod ambience;
pub mod backdrop;
pub mod beasts;
pub mod bindings;
pub mod boat;
pub mod cairn;
pub mod camera;
pub mod chart;
pub mod cli;
pub mod clouds;
pub mod compass;
pub mod console;
pub mod control;
pub mod debug;
pub mod figure;
pub mod glyph;
pub mod instruments;
pub mod logbook;
pub mod menu;
pub mod models;
pub mod net;
pub mod player;
pub mod sea;
pub mod settings;
pub mod sky;
pub mod stopping;
pub mod terrain;
pub mod told;
pub mod trees;
pub mod wake;
pub mod wildlife;

#[cfg(test)]
mod testing;

use bevy::asset::AssetPath;
use bevy::gltf::GltfAssetLabel;
use bevy::prelude::*;

/// Colour of the sky above the horizon. The camera's distance fog fades to the
/// same colour, so the two meet seamlessly.
pub const SKY: Color = Color::srgb(0.63, 0.80, 0.93);

/// Distance from the eye at which aerial haze starts to take the ground over,
/// in metres.
///
/// The same number the survey records coast out to, and deliberately one
/// number rather than two that happen to agree: the edge of what is worth
/// recording and the edge of what can be made out are the same edge — coast
/// beyond it is already dissolving into the air, so there is nothing there a
/// player could claim to have seen. It lives on the wire's crate rather than
/// here because the recording half of it is something both ends must agree on
/// (see [`protocol::survey::SIGHT_RADIUS`]), and the haze follows it.
pub const HAZE_START: f32 = protocol::survey::SIGHT_RADIUS;
/// Distance at which the haze has fully replaced the ground with [`SKY`]. This
/// is the edge of what the camera can see at all, whatever it is pointed at,
/// so anything the picture depends on has to reach at least this far.
pub const HAZE_END: f32 = 900.0;

/// Size of the window the game is played in, in pixels. Captured shots are
/// sized by `client resolution` instead, having no window to take it from.
pub const WINDOW: UVec2 = UVec2::new(1280, 720);

/// The ink the instruments over the world are drawn in — the compass, and the
/// lead and the day's arc beside it — and the same the menus use, so
/// everything laid over the picture reads as one chart's furniture.
///
/// Four colours and no more: one face to sit on, one edge to be bounded by,
/// one ink to read, and one dimmed ink for what is on the card without being
/// the reading. An instrument wanting a fifth is usually one that has two
/// readings where it should have one.
pub const FACE: Color = Color::srgba(0.09, 0.11, 0.10, 0.60);
pub const EDGE: Color = Color::srgb(0.70, 0.69, 0.62);
pub const INK: Color = Color::srgb(0.88, 0.87, 0.80);
pub const INK_DIM: Color = Color::srgb(0.60, 0.60, 0.55);

/// A surface in `base_color` with nothing polished about it.
///
/// Everything the game draws is lit this way — ground, water, hulls, the
/// markers other players stand as. A specular highlight is a gradient across
/// a facet, and a gradient is the one thing a look built out of flat tones
/// cannot have: it would read as the facet being curved, which is exactly
/// what the shading says it is not.
///
/// A surface wanting one thing about it different spreads the rest of this
/// over its own: `StandardMaterial { alpha_mode, ..matte(colour) }`.
pub fn matte(base_color: Color) -> StandardMaterial {
    StandardMaterial {
        base_color,
        perceptual_roughness: 1.0,
        metallic: 0.0,
        reflectance: 0.0,
        ..default()
    }
}

/// Where in a glTF file under `assets/` to find one of its meshes.
///
/// Every model the game draws is spawned this way — mesh by mesh rather than
/// as a whole scene, because the files' own PBR materials are ignored in
/// favour of [`matte`]. glTF numbers meshes rather than naming them in a way
/// the loader can ask for, so `mesh` is a position in the file, and each
/// module holds its own file to that order with a test.
///
/// `primitive: 0` because each object in a master carries one material and so
/// exports as a mesh of a single primitive; an object split across two
/// materials would arrive as two, and would want spawning as two children.
pub fn model_mesh(file: &str, mesh: usize) -> AssetPath<'static> {
    GltfAssetLabel::Primitive { mesh, primitive: 0 }.from_asset(file.to_owned())
}

/// How far of the way to a target an exponential ease travels in `dt`
/// seconds, as a fraction to interpolate by: `value.lerp(target, eased(..))`.
///
/// `rate` is how fast the gap closes, in e-foldings per second — its
/// reciprocal is the time constant, so most of any change arrives within
/// `1/rate` seconds and it is all but done in three times that.
///
/// The exponential is what makes the ease frame-rate independent. Taking a
/// fixed fraction of the gap each frame would close it at whatever speed the
/// machine happened to render at; this is the exact solution over the frame's
/// own length, so the same movement takes the same time everywhere.
///
/// Note that this never quite arrives — it only ever closes a fraction of
/// what is left. Anything that has to *stop* needs a threshold of its own to
/// snap the tail, as the boat's way does.
pub fn eased(rate: f32, dt: f32) -> f32 {
    1.0 - (-rate * dt).exp()
}

/// Stirs bits until they stop resembling what they were — SplitMix's mixing
/// rounds, without its sequence.
///
/// Everything this crate invents out of thin air comes through here, and
/// deliberately one function rather than one per module. Most callers only want
/// a number that does not look patterned. The rest want *agreement*: they feed
/// in bits every machine was dealt alike — a chunk's coordinates, a
/// [`protocol::BeastId`] — so what is dealt comes out the same on every client
/// without a byte crossing the wire. That only holds while there is one mixer,
/// two copies being two ways for two builds to disagree with nothing to say
/// so.
pub fn scramble(mut x: u32) -> u32 {
    x = x.wrapping_add(0x9E37_79B9);
    x ^= x >> 16;
    x = x.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 13;
    x = x.wrapping_mul(0xC2B2_AE35);
    x ^ (x >> 16)
}

/// A number in `0.0..1.0` from some bits and a salt — the one way this crate
/// turns entropy into a quantity, so a bearing, a jitter and a speed are all
/// drawn the same way. The salt is what lets one seed answer several
/// questions without the answers being the same answer.
pub fn unit(bits: u32, salt: u32) -> f32 {
    scramble(bits ^ salt) as f32 / u32::MAX as f32
}

/// The same, in `-1.0..1.0`: a wobble either way about whatever it is added
/// to.
pub fn signed(bits: u32, salt: u32) -> f32 {
    unit(bits, salt) * 2.0 - 1.0
}

/// One of `range.0..=range.1`, evenly.
pub fn between(bits: u32, salt: u32, range: (usize, usize)) -> usize {
    range.0 + scramble(bits ^ salt) as usize % (range.1 + 1 - range.0)
}

/// The sizes one kind of animal comes in, and what its model measures — the
/// pair being what turns a size in world metres into the scale the model is
/// hung at.
///
/// Every animal is drawn at a size dealt from bits, the way palms are: one model
/// stamped at one size reads as one animal repeated. The bits are ones every
/// machine was dealt alike, so every client draws the same animal at the same
/// size without a byte crossing the wire.
///
/// Written in *world metres* rather than as a multiplier because that is the
/// thing worth arguing about — a shark is three to four metres long, and what
/// multiple of the file that happens to be is arithmetic.
pub struct Size {
    /// What the file measures along the axis the range is quoted on — nose to
    /// tail for a swimmer, wingtip to wingtip for a bird.
    ///
    /// Held to the model by each kind's own test, because nothing at runtime
    /// can tell: a remodel that comes through half the size it was, with this
    /// left alone, draws every animal at half the size asked for and looks
    /// exactly like a modelling decision.
    pub model: f32,
    /// The smallest and largest one is drawn at, in world metres.
    pub range: (f32, f32),
}

impl Size {
    /// The scale to hang one at, dealt from bits every machine was dealt
    /// alike — see [`scramble`] on why that is the whole trick.
    pub fn dealt(&self, bits: u32, salt: u32) -> f32 {
        self.drawn(bits, salt) / self.model
    }

    /// The size that scale draws, in world metres. What the tests measure,
    /// and the only place the range is read.
    pub fn drawn(&self, bits: u32, salt: u32) -> f32 {
        self.range.0 + unit(bits, salt) * (self.range.1 - self.range.0)
    }
}

/// Top-level screen the app is on.
#[derive(States, Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum AppState {
    #[default]
    MainMenu,
    /// Choosing which kept world to return to, and whether to share it this
    /// time.
    SetSail,
    /// Choosing the seed of the world about to be entered, and whether to
    /// share it with anyone else.
    NewWorld,
    /// Naming a server to play in.
    JoinWorld,
    /// The way to the two screens below, and nothing else — see
    /// [`crate::menu`].
    Options,
    /// Choosing how the game is drawn: how much screen it takes and how many
    /// pixels it draws. See [`crate::settings`].
    Display,
    /// Choosing which key does what.
    Controls,
    InWorld,
}

/// What the player is doing while [`AppState::InWorld`] — which is a state
/// under that one rather than beside it, and that is the whole point of it.
///
/// A world lives exactly as long as `AppState::InWorld` does: the ground, the
/// boat, the connection and the served world behind it all hang off entering
/// and leaving it, so pausing must not be a way of leaving it. Escape used to
/// set [`AppState::MainMenu`] outright, which for a shared world meant one
/// mispress evicted everyone else sailing in it. Standing the pause menu up
/// *inside* `InWorld` costs nothing but the player's own hands.
///
/// Which is why only the systems that read the player's input are held while
/// paused. Everything that merely keeps the world true to itself — floating
/// the hull, easing the camera, streaming ground, answering the server — runs
/// straight through.
#[derive(SubStates, Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[source(AppState = AppState::InWorld)]
pub enum Helm {
    /// The player has the helm.
    #[default]
    Sailing,
    /// The pause menu is up over the world.
    Paused,
    /// The options screen, opened from the pause menu.
    ///
    /// Every screen from here down is doubled — one of these and one
    /// [`AppState`] beside it — and that doubling is the point rather than an
    /// oversight. The same screen reached from the main menu has no world
    /// behind it; reached from the pause menu it must not take one down, and
    /// leaving `AppState::InWorld` is exactly what would. One builder each, and
    /// the only difference is which screen Back returns to.
    Options,
    /// The display screen, opened from the options screen — see
    /// [`AppState::Display`].
    Display,
    /// The controls screen, likewise — see [`AppState::Controls`].
    Controls,
    /// The debug console is up over the world, taking the keyboard — see
    /// [`console`]. A state here rather than a flag of the console's own
    /// because that is what stops a typed `w` also driving the boat: every
    /// system that reads the player's hands already conditions on
    /// [`Helm::Sailing`].
    Console,
    /// The chart is up over the world — see [`chart`]. A state beside the
    /// pause menu and for the same reason: reading a chart must not cost
    /// anyone else their world, so the ground keeps arriving and the other
    /// boats keep moving behind the paper. What it does hold is the player's
    /// own hands, which is why it is a state rather than a flag — the helm
    /// stops while the sheet is being read, and the boat carries its way.
    Chart,
}
