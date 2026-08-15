//! Boats: the hulls the player gets about in, and the keys that steer the one
//! they are aboard.
//!
//! The player themself is not here. They are a person — see [`crate::player`]
//! — riding this boat as a child of it, and the boat is one of the vehicles
//! they will get about in rather than the player's own shape. What *kind* of
//! boat an entity is lives in its [`Hull`]: the dimensions and manners the
//! rules below are written against, ship-sized today ([`SHIP`]) and
//! rowboat-sized next, so a new kind of boat is a new `Hull` and a new model,
//! not a new module.
//!
//! A hull is modelled rather than drawn here: [`MODEL`] is a glTF file built
//! from a Blender master under `assets-src/`, and this module spawns its
//! meshes and steers what they hang off. The few dimensions a `Hull` names
//! are the ones the rules read — where the keel is, and how deep. Those are
//! not the model's to change quietly, so
//! `the_model_is_the_hull_the_keel_is_probed_along` holds the file to them;
//! everything else about the shape is the modeller's, and this file has no
//! opinion on it.
//!
//! A boat faces down its own -Z, so [`Transform::forward`] is the way it is
//! pointing and steering can leave the axis convention alone. Its origin is on
//! the waterline rather than at the keel or the deck, which is what lets
//! [`float`] put it down by simply setting the height of the surface it is on.

use bevy::asset::RenderAssetUsages;
use bevy::math::Vec3Swizzles;
use bevy::mesh::PrimitiveTopology;
use bevy::prelude::*;

use crate::bindings::KeyBindings;
use crate::camera::View;
use crate::player::Player;
use crate::sea;
use crate::terrain::Ground;
use crate::{eased, matte, model_mesh, AppState, Helm};

/// The ship, as a file. Built from `assets-src/models/boat/boat.blend` by
/// `assets-src/models/export.sh`, which is also where the export settings the
/// look depends on are written down.
const MODEL: &str = "models/boat.glb";

/// Which mesh in [`MODEL`] is which. glTF numbers its meshes rather than naming
/// them in a way the loader can ask for, so these are positions in the file —
/// which means reordering the objects in Blender would silently swap the hull
/// for the spar. `the_model_is_a_hull_and_a_spar_fit_to_draw` is what
/// stops that being found by looking at it.
const HULL_MESH: usize = 0;
const SPAR_MESH: usize = 1;

/// The dimensions and manners of one kind of boat — everything [`float`],
/// [`steer`] and [`grounding`] need to know to drive one. This is a boat's
/// *character*; what a particular boat is doing lives on [`Boat`]. The values
/// carry their reasoning where they are picked, on [`SHIP`].
#[derive(Clone, Copy)]
struct Hull {
    /// Length overall, in metres.
    length: f32,
    /// Beam, in metres — how far apart the water is sampled athwartships to
    /// read the roll the waves ask of the hull. Close to the model's planking
    /// but not held to it the way `length` is: the samples are reading the
    /// surface's slope, and a slope read a few centimetres wide of the hull
    /// is the same slope.
    beam: f32,
    /// Keel depth below the waterline. The sea is translucent, so this much
    /// of the hull shows through the water as a darker shape under the deck —
    /// but what makes it the game's business rather than the model's is
    /// [`Hull::grounding_draft`], which is measured from it.
    draft: f32,
    /// Deck height above the waterline — the freeboard, the model's own
    /// sheer. The game's business because somebody stands on it: a player
    /// aboard is put down here rather than at the hull's origin, which is the
    /// waterline and so is knee-deep in the bilges. Held to the model by
    /// `the_model_is_the_hull_the_keel_is_probed_along`, like the draft it is
    /// measured against.
    deck: f32,
    /// Where the keel begins and ends, in metres from amidships — negative
    /// forward, the same axis the hull is modelled on. [`grounding`] probes
    /// along these, so what runs aground is the line that is drawn.
    forefoot_station: f32,
    heel_station: f32,
    /// The masthead: metres above the waterline, and metres from amidships on
    /// the same axis as the keel's stations. The pennant is tied on here, so
    /// like the deck and the draft these are the model's numbers rather than
    /// the game's to choose — a mast re-cut in Blender and not re-measured
    /// here would fly its pennant in mid-air beside the spar, which is what
    /// `the_model_flies_a_pennant_from_its_masthead` is for.
    masthead: f32,
    masthead_station: f32,
    /// Metres per second under way.
    speed: f32,
    /// Metres per second going astern.
    astern_speed: f32,
    /// Seconds of lag between the speed the keys ask for and the speed the
    /// hull makes — the time constant of an exponential ease, so most of any
    /// change arrives within this long and it is all but done in three times
    /// it. Named as a duration rather than as the rate [`eased`] takes, a
    /// hull having a weight that is easier to think about in seconds. The
    /// ease is what gives a hull that weight: it gathers way over seconds
    /// instead of leaping to `speed` on the frame the key goes down, and
    /// carries a glide when the key comes up.
    way_response: f32,
    /// How fast the helm brings the bow round, in radians per second.
    /// Together with `speed` this fixes the turning circle.
    turn_rate: f32,
    /// How far the hull heels in a full-helm turn at full speed, in radians.
    /// It heels *outwards*, the way a keeled hull does: the water grips the
    /// keel below the waterline while the turn flings the mass above it, so
    /// the boat leans out of the corner, not into it like a bicycle.
    heel_at_full_turn: f32,
    /// Seconds of lag between the heel a turn asks for and the heel the hull
    /// shows, the same exponential shape as `way_response` and much quicker:
    /// rolling is the lightest thing a hull does. Quick enough that the lean
    /// arrives while the turn is still news, slow enough that the hull rolls
    /// rather than snaps — and the same curve is the straightening, run back
    /// down to level when the helm comes off.
    heel_response: f32,
    /// Seconds of lag between the tilt the water asks for and the tilt the
    /// hull shows — the same exponential family as `heel_response`, and a
    /// good deal slower: heeling is the hull rolling on its own keel, this is
    /// the whole hull being *lifted* by one end. The lag is also what keeps
    /// the chop out of the deck. Under way the short seas pass beneath the
    /// hull every couple of seconds, and a deck that chased each one
    /// faithfully would wag; on this curve the hull rides the long swell and
    /// lets the chop go by underneath.
    sway_response: f32,
}

impl Hull {
    /// How little water the hull is held in: ground standing higher than this
    /// far below the waterline stops it.
    ///
    /// On the coasts the generator draws this puts the hull within a metre or
    /// two of the waterline; where it holds a boat further off, it is off a
    /// shelf too thin to float one, and the shallows are painted as shallows
    /// long before they are this thin — so a boat held out is held out of
    /// water it can be seen to be held out of.
    fn grounding_draft(&self) -> f32 {
        self.draft - KEEL_BITE
    }
}

/// The ship: the boat a world is entered aboard, and [`MODEL`]'s subject.
const SHIP: Hull = Hull {
    // A small sailing boat: at the default zoom the visible ground is some
    // tens of metres across, so this length reads as a boat rather than as a
    // speck, and at the far end of the zoom range it is still a mark on the
    // water rather than gone.
    length: 7.0,
    beam: 2.4,
    draft: 0.8,
    deck: 0.9,
    // The forefoot stops short of the bow, which is what gives the stem its
    // rake; the heel runs right aft to the transom.
    forefoot_station: -7.0 * 0.5 * 0.7,
    heel_station: 7.0 * 0.5,
    // Six metres of mast, stepped forward of amidships — the model's, not a
    // choice made here.
    masthead: 6.9,
    masthead_station: -1.05,
    // Brisk beyond honesty for a seven-metre hull, but the ship is how the
    // world is crossed: at this speed the ground in view at the default zoom
    // slides by in a few seconds, and the next island is minutes away rather
    // than tens of minutes.
    speed: 10.0,
    // Enough to back off a beach or out of a cove, and slow enough that
    // nobody crosses an ocean in reverse.
    astern_speed: 4.0,
    // The hull gathers way over a few seconds and carries it for a couple of
    // lengths' glide — seven metres of timber, felt.
    way_response: 1.5,
    // With the speed above, a turning circle of about five metres — tight
    // enough to feel answerable from a camera forty metres up, wide enough
    // that coming about reads as a turn rather than a spin.
    turn_rate: 2.0,
    // Enough to swing the masthead more than a metre, which is what makes a
    // turn visible from forty metres up, and shy of anything that reads as
    // capsizing.
    heel_at_full_turn: 0.22,
    // Quick, rolling being the lightest thing seven metres of timber does.
    heel_response: 0.4,
    // And the lift of the whole hull much slower than its roll.
    sway_response: 0.9,
};

/// How much of the keel the ground is allowed to take before a hull is
/// stopped. Stopping a boat the instant the ground rises to meet the keel is
/// an invisible wall a boat's length offshore, whereas a fifth of a metre of
/// bite is a boat *beaching*: the keel is seen to touch, and then it stops.
/// Well clear of the two centimetres the heights are quantised to, so the
/// threshold cannot chatter. One constant for every hull — what it answers to
/// is the quantisation, not the boat.
const KEEL_BITE: f32 = 0.2;

/// How many points along the keel are asked about the bottom. Spread from the
/// forefoot to the heel inclusive, so the gap between them comes out under
/// the two metres the ground is sampled at — an invariant each hull's keel
/// length has to keep, and `the_keel_is_probed_as_closely_as_the_ground_is_
/// sampled` pins: no facet of the height field can lie wholly between two
/// probes, so ground that rises across a facet is read on the way up rather
/// than stepped over.
///
/// That is what the spacing buys, and it is worth being plain that it is less
/// than "nothing gets past". A crest only one lattice line wide is *not* seen:
/// the field is linear between its corners, so two probes either side of such a
/// crest read its flanks, and the hull sails through a rock standing at the
/// waterline. Coasts are safe from it by being coasts — the bottom shelves, so
/// the ground under the keel is near enough monotone, and the innermost probe
/// is reading the shallowest water and reading it honestly. What is exposed is
/// the isolated skerry, which the generator draws on purpose and draws about a
/// facet across. Seeing one reliably would mean probing at a fraction of a
/// metre rather than at two, on every frame and at both poses, to buy a rock in
/// open water — while the coasts, which are what a boat is actually stopped by,
/// need none of it. Sailing through a skerry is the smaller wrong, and the one
/// that can be paid off from the other end, by giving the skerries some width.
///
/// The sides are not probed: a hull here is a shallow V, drawing its full
/// draft on the centreline and nothing at all at the beam, so a probe out
/// there would
/// have to carry a draught of its own to say anything the keel has not said.
/// That is a standing condition on the model rather than an observation about
/// one — a hull remodelled with a flat bottom carried out to the beam would
/// need probes out there too.
const KEEL_PROBES: usize = 4;

/// Way below this, with no drive asked for, is stopped, and [`steer`] snaps
/// it to exactly zero. The ease only ever halves the remainder — left alone
/// the boat would creep forever, never quite done stopping — and a hull at
/// rest should be *at rest*: the same spot every frame, nothing moving.
const WAY_STOPPED: f32 = 0.02;

/// Within this of the heel the turn is asking for, the hull snaps to it
/// exactly — the same tail-closing that [`WAY_STOPPED`] does for the way,
/// and for the same reason: the ease only ever halves the remainder, and a
/// hull done straightening should be *level*, holding one rotation frame
/// after frame rather than forever creeping towards it. A third of a degree,
/// invisible at any zoom.
const HEEL_SETTLED: f32 = 0.005;

/// Timber. Nothing on an island or in the sea is anywhere near this hue, so the
/// boat is findable in a landscape of greens and blues without being lit any
/// differently from them.
const HULL_COLOR: Color = Color::srgb(0.62, 0.28, 0.22);
/// Bare spar, pale enough to stand off both the water and the hull.
const SPAR_COLOR: Color = Color::srgb(0.86, 0.80, 0.68);
/// The pennant. Hotter and lighter than the hull's timber, which is the only
/// other warm thing in a world of greens and blues: at the far end of the zoom
/// the boat is a mark on the water and this is the mark on the mark.
const PENNANT_COLOR: Color = Color::srgb(0.87, 0.35, 0.18);

/// The pennant, as a shape: how far it flies from the mast and how deep it is
/// at the hoist, in metres. A long thin burgee rather than a square flag —
/// the length is what carries the bearing at the distance the boat is watched
/// from, and the narrow hoist is what lets a triangle stand in for a hanging
/// flag when the wind drops (see [`pennant_mesh`]).
const PENNANT: (f32, f32) = (1.2, 0.3);

/// The apparent wind that flies the pennant out, in metres per second, and
/// how far it still sags at that wind, in radians. Both are the drawing's to
/// pick rather than the weather's: a flag that only lifted in the gales would
/// be a limp rag through most of a day, and one that ever came out perfectly
/// straight would read as a signboard rather than as cloth.
const PENNANT_FLIES: (f32, f32) = (8.0, 0.14);

/// Below this apparent wind, in metres per second, the pennant keeps the
/// bearing it had and simply hangs. The same problem the compass's arm has —
/// a dying wind's direction is noise — and the same answer, except that here
/// the hanging *is* the reading: a flag straight down is how a calm looks.
const PENNANT_CALM: f32 = 0.3;

/// The flutter: how far the pennant swings either side of its bearing at full
/// wind, in radians, and how fast, in radians per second. It is scaled by the
/// wind like everything else about the flag, so a calm hangs dead still and a
/// blow snaps — cloth being the one thing in view that says how hard it is
/// blowing without being asked.
const FLUTTER: (f32, f32) = (0.16, 7.0);

/// A boat in the world: what kind it is, and what it is doing.
///
/// `hull` is the kind — the ship for now, and the rest is the sailing state.
/// `way` is the speed the hull is actually making along its heading, in
/// metres per second, ahead positive — the state the eased throttle lives
/// in. The keys name a speed; [`steer`] brings `way` towards it.
///
/// `heel` is the roll the hull is showing *for the turn*, in radians about
/// its own forward, positive with the masthead to port. `pitch` and `roll`
/// are the tilt the water is showing on it — [`float`] easing the deck
/// towards the sea's own slope under the hull, pitch about the athwart axis
/// with the bow up positive, roll the same sign and the same rotation factor
/// as the heel. All three are kept here rather than read back off the
/// transform because the transform holds heading, pitch and the two rolls
/// multiplied together, and unpicking a quaternion every frame to learn
/// numbers these systems wrote themselves is work for nothing.
#[derive(Component)]
pub struct Boat {
    hull: Hull,
    way: f32,
    heel: f32,
    pitch: f32,
    roll: f32,
}

impl Boat {
    /// The ship a world is entered aboard, at rest.
    pub fn ship() -> Self {
        Self {
            hull: SHIP,
            way: 0.0,
            heel: 0.0,
            pitch: 0.0,
            roll: 0.0,
        }
    }

    /// Whether the hull has no way on at all. Exact equality is meaningful
    /// here because [`steer`] snaps the tail of every glide to precisely
    /// zero — see [`WAY_STOPPED`] — so a hull is either making way or it is
    /// this. What going ashore asks before it lets anybody step off a moving
    /// deck.
    pub fn at_rest(&self) -> bool {
        self.way == 0.0
    }

    /// Where somebody aboard stands, in metres above the hull's origin — see
    /// [`Hull::deck`]. What a player boarding is put down at, so that they
    /// stand on the deck rather than in it.
    pub fn deck(&self) -> f32 {
        self.hull.deck
    }
}

/// The pennant flying at the masthead, and the bearing it is streaming on —
/// its own, kept here rather than read back off the transform because the
/// transform holds the boat's rotation taken out again, and because a calm
/// has to leave the bearing where it was rather than invent a new one.
#[derive(Component)]
struct Pennant {
    bearing: f32,
}

pub struct BoatPlugin;

impl Plugin for BoatPlugin {
    fn build(&self, app: &mut App) {
        // Steering before floating, so ground gained or lost by this frame's
        // movement is under the hull the same frame rather than the next.
        // The conditions the hull floats on, here as well as in the terrain
        // plugin: resources are global and initialising one twice is free,
        // and the boat's own tests run without any terrain at all.
        app.init_resource::<sea::SeaConditions>()
            .add_systems(OnEnter(AppState::InWorld), launch)
            .add_systems(
                Update,
                // Only the steering stops when the game is paused. Floating is
                // not motion — it sets the hull to the height of the ground
                // under it — so leaving it running means a chunk arriving
                // while the pause menu is up is settled on before the player
                // looks again, rather than snapping under them on resume.
                // The pennant last, and outside the pause like the floating:
                // it flies off the hull's rotation and the wind, so reading
                // either before this frame's steering had written it would
                // leave the flag a frame behind the mast it is tied to.
                (
                    steer.run_if(in_state(Helm::Sailing)),
                    float,
                    fly_the_pennant,
                )
                    .chain()
                    .run_if(in_state(AppState::InWorld)),
            );
    }
}

/// Puts the ship in the world at the point the world is entered, pointing the
/// way the opening view looks — with the player aboard, entering a world
/// being something done afloat.
///
/// The view names where the player enters the world, so the boat goes there
/// rather than anywhere of its own choosing. Entry is the world's spawn
/// point — open water the layout keeps just off the first island's coast —
/// so the boat starts afloat with land dead ahead; a `--focus` can still put
/// it down inland, aground until the movement keys drive it back to the sea.
/// The meshes hang off the boat as children rather than on it: a mesh carries
/// one material, and the hull and the spar are two colours. Their geometry is
/// already in the boat's own frame — the modeller places the mast on the deck,
/// not the game — so the children sit at the identity and the only transform
/// anything writes is the boat's own. The player is one more child, at the
/// identity like the meshes: aboard *is* being in the hierarchy — see
/// [`crate::player`] — so they stand wherever the hull carries them and go
/// down with the ship when the world is left.
fn launch(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    assets: Res<AssetServer>,
    view: Res<View>,
) {
    // The file's own materials are ignored, and the meshes are pulled out of it
    // one at a time rather than the whole scene being spawned. glTF materials
    // are PBR — a roughness, a metalness, a specular response — and the look
    // here is a small fixed palette under `matte`, so a hull lit the way the
    // file asked for would be the one surface in the world with a highlight on
    // it. Leaving the colours in Rust also keeps them beside the ground's,
    // which is the comparison that matters when either is picked.
    let hull_material = materials.add(matte(HULL_COLOR));
    let spar_material = materials.add(matte(SPAR_COLOR));
    // Cloth is the one thing aboard with no inside, so it is the one material
    // here that is drawn from both faces. Left single-sided the pennant would
    // wink out every time the wind put its back to the camera, which happens
    // several times a minute at a flutter.
    let pennant_material = materials.add(StandardMaterial {
        double_sided: true,
        cull_mode: None,
        ..matte(PENNANT_COLOR)
    });

    commands.spawn((
        Name::new("Boat"),
        Boat::ship(),
        DespawnOnExit(AppState::InWorld),
        // A rotation of `yaw` about the vertical takes -Z to the camera's own
        // forward, so the boat starts pointing away from the viewer.
        Transform::from_xyz(view.focus.x, 0.0, view.focus.z)
            .with_rotation(Quat::from_rotation_y(view.yaw)),
        // Carried by the parent because the children inherit it: without one
        // here there is nothing for their own visibility to be computed
        // against, and a boat whose meshes are on entities of their own would
        // never be drawn.
        Visibility::default(),
        children![
            (
                Name::new("Hull"),
                Mesh3d(assets.load(model_mesh(MODEL, HULL_MESH))),
                MeshMaterial3d(hull_material),
            ),
            (
                Name::new("Spar"),
                Mesh3d(assets.load(model_mesh(MODEL, SPAR_MESH))),
                MeshMaterial3d(spar_material),
            ),
            // Tied to the masthead and pointed by [`fly_the_pennant`]. The
            // one piece of the boat that is not in the file: a flag is a
            // shape that has to be *aimed*, and aiming it means knowing where
            // its tie is, which a mesh out of Blender does not say.
            (
                Name::new("Pennant"),
                Pennant {
                    // Astern until the first frame says otherwise, which is
                    // where a flag on a boat at rest in still air would lie
                    // anyway.
                    bearing: 0.0,
                },
                Mesh3d(meshes.add(pennant_mesh())),
                MeshMaterial3d(pennant_material),
                Transform::from_xyz(0.0, SHIP.masthead, SHIP.masthead_station),
            ),
            // The figure itself is hung under this by `figure::dress`, which
            // is the player's own business rather than the boat's; what the
            // boat says is where a person aboard stands, which is on its
            // deck. The visibility is so that figure inherits cleanly from
            // its siblings.
            (
                Name::new("Player"),
                Player,
                Transform::from_xyz(0.0, SHIP.deck, 0.0),
                Visibility::default(),
            )
        ],
    ));

    // Said out loud for the same reason a run without a seed says which world
    // it picked: a placeholder nobody can find is indistinguishable from one
    // that never spawned, and `--focus` takes exactly these two numbers.
    info!("boat launched at {}, {}", view.focus.x, view.focus.z);
}

/// The pennant, as a shape: a burgee tied at the origin, so that everything
/// [`fly_the_pennant`] does is a rotation about the point the flag is actually
/// made fast at.
///
/// It flies along -Z at rest, the way the boat itself faces, so a bearing
/// becomes a rotation about the vertical with no axis convention of its own.
/// The hoist hangs *below* the tie rather than straddling it, because a flag
/// is tied at its top corner and swings from there.
///
/// The one thing it is not is flat, and that is the whole reason it is three
/// triangles instead of one. A flat pennant vanishes whenever the wind lines
/// up with the camera — which at a fixed camera bearing is several times an
/// hour, and looks exactly like the flag having been deleted. Pushing a
/// single interior point out to one side puts a shallow belly in the cloth,
/// which is both what a real flag does and enough to keep some part of it
/// facing the viewer from any direction. It costs two triangles.
///
/// Rigid cloth is still a lie in a calm — real canvas folds down the mast
/// rather than swinging round like a boom — and [`PENNANT`]'s narrow hoist is
/// what makes the lie cheap: at three tenths of a metre the missing fold is a
/// hand's width, watched from forty metres up, while the length that carries
/// the reading is the part that behaves.
fn pennant_mesh() -> Mesh {
    let (length, hoist) = PENNANT;
    let tie = Vec3::ZERO;
    let foot = Vec3::new(0.0, -hoist, 0.0);
    let fly = Vec3::new(0.0, -hoist * 0.5, -length);
    // The belly: inside the outline, nearer the hoist than the fly, and out
    // to starboard by a tenth of the flag's length. Deep enough to catch the
    // light differently from its neighbours, shallow enough that the pennant
    // still reads as one shape rather than as a paper aeroplane.
    let belly = Vec3::new(length * 0.1, -hoist * 0.5, -length * 0.4);

    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    // Unindexed, so that each facet can carry its own normal — the flat
    // shading the whole world is drawn in, and the reason the belly is worth
    // having at all.
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![tie, foot, belly, foot, fly, belly, fly, tie, belly],
    )
    .with_computed_flat_normals()
}

/// How the pennant lies under an apparent wind: the bearing it streams on and
/// how far it hangs off the horizontal, in radians. `flying` is the bearing it
/// is on now, which a wind too slack to have a direction leaves alone.
///
/// The bearing is where the air is *going*, which needs no defending here the
/// way it does on the compass: a flag is blown, and the eye reads it as blown.
/// The droop is the whole of the strength reading — flat out in a blow, dead
/// down in a calm, and everything between — so a player who never looks at
/// the corner of the screen still knows what the wind is doing.
fn pennant_pose(apparent: Vec2, flying: f32) -> (f32, f32) {
    let (full, sag) = PENNANT_FLIES;
    let hard = (apparent.length() / full).clamp(0.0, 1.0);
    let bearing = if apparent.length() > PENNANT_CALM {
        // The map's x and y are the world's x and z, and the flag is drawn
        // down -Z: the same turn a boat's own heading is read as.
        f32::atan2(-apparent.x, -apparent.y)
    } else {
        flying
    };
    (
        bearing,
        sag + (std::f32::consts::FRAC_PI_2 - sag) * (1.0 - hard),
    )
}

/// Flies the pennant on the wind the masthead feels.
///
/// Which is the *apparent* wind — the world's wind less the boat's own way
/// through it — and that is the difference between this and the compass, on
/// purpose. An instrument wants the true wind, because a bearing that changed
/// as the player accelerated would be useless for steering by; a flag has no
/// such duty and every reason to be honest, so a boat driving into a light
/// air blows its own pennant astern, and one running before a breeze at
/// nearly the speed of it flies limp. The two disagreeing is not a fault to
/// be reconciled — it is the same thing sailors get from a burgee and a
/// masthead instrument, and a player who notices has learned something true
/// about sailing.
///
/// The boat's rotation is taken back out of the flag's, so heel, pitch and
/// heading move where the pennant *is* without touching where it points: the
/// masthead swings through a turn and the cloth stays on the wind.
fn fly_the_pennant(
    time: Res<Time>,
    conditions: Res<sea::SeaConditions>,
    boats: Query<(&Boat, &Transform), Without<Pennant>>,
    mut pennants: Query<(&mut Pennant, &ChildOf, &mut Transform)>,
) {
    for (mut pennant, of, mut transform) in &mut pennants {
        let Ok((boat, hull)) = boats.get(of.parent()) else {
            continue;
        };
        let apparent = conditions.wind() - hull.forward().xz() * boat.way;
        let (bearing, droop) = pennant_pose(apparent, pennant.bearing);
        pennant.bearing = bearing;

        // The flutter rides on top of the bearing rather than replacing it,
        // and is scaled by the same wind that lifted the flag: cloth that
        // snapped as hard in an air as in a blow would be a flag with a motor
        // in it.
        let (throw, rate) = FLUTTER;
        let hard = (apparent.length() / PENNANT_FLIES.0).clamp(0.0, 1.0);
        let flutter = throw * hard * (time.elapsed_secs_wrapped() * rate).sin();

        let flying = Quat::from_rotation_y(bearing + flutter) * Quat::from_rotation_x(-droop);
        transform.rotation = hull.rotation.inverse() * flying;
    }
}

/// Keeps the boat on the surface it is over: the ground where the ground
/// stands proud of the water, and the swell where it does not — the same rule
/// the other players' markers ride. [`Ground::surface`] is this with a flat
/// sea; the swell is what the sea is actually drawn wearing, and taking the
/// higher of ground and water is what makes the changeover continuous — a
/// hull nosing onto a beach settles from bobbing to beached with no step,
/// because at the shoreline the two heights meet.
///
/// The waterline is the hull's origin, so a boat that has run aground is
/// half-buried in the hillside; that is what aground looks like, and steering
/// is what will keep it off. No easing on the water's motion either: the
/// swell is gentle, and a hull seven metres long simply is where the water
/// is.
///
/// Afloat, the hull also wears the water's *slope*: the swell is sampled off
/// the bow and the stern and out at either beam, and the deck eases towards
/// the plane those four heights describe — pitching as seas pass under it
/// fore and aft, rolling as they pass across, on the hull's own sway
/// response, which is where its weight lives. The height above is not eased
/// and the tilt is, deliberately: the hull *is* where the water is, but it is
/// metres long, and turning to a shape that long takes it time the height
/// does not need. A beached hull eases level instead — the ground is holding
/// it, and a deck still working to water sliding past a held keel would give
/// the trick away.
///
/// The tilt goes on and comes off as a factor of its own. The rotation holds
/// heading, then the water's pitch, then a single roll factor that the wave
/// roll shares with the turn's heel — so this system strips the tilt it
/// applied last frame from the right and hangs the new one on, and [`steer`],
/// multiplying its heel delta on from the right, keeps reaching the roll
/// factor it always has.
fn float(
    ground: Option<Res<Ground>>,
    time: Res<Time>,
    sea: Res<sea::SeaConditions>,
    mut boats: Query<(&mut Transform, &mut Boat)>,
) {
    let elapsed = time.elapsed_secs_wrapped();

    for (mut transform, mut boat) in &mut boats {
        let hull = boat.hull;
        let t = eased(1.0 / hull.sway_response, time.delta_secs());
        let at = transform.translation;
        let height = ground.as_ref().and_then(|g| g.height(at.x, at.z));
        let mut afloat = false;
        if let Some(height) = height {
            // The water under the hull decides whether the swell here is the
            // open sea's or the shore's — the ground is asked exactly, where
            // the shader reads its windowed picture of the same heights; they
            // differ by at most a texel of interpolation, in water where the
            // swell is smallest.
            let water = sea.water_over(ground.as_deref(), at.xz(), elapsed);
            transform.translation.y = height.max(water);
            afloat = water >= height;
        }

        let (target_pitch, target_roll) = if afloat {
            // The same swell the hull's own height rides, at a point of the
            // hull rather than its middle. Ground not yet sent counts as
            // deep — the benefit of the doubt the depth window gives the
            // shader — and ground standing dry puts the shore wave's last
            // breath there, which is next to no water and next to no tilt.
            let water_at = |offset: Vec3| {
                let point = transform.transform_point(offset);
                sea.water_over(ground.as_deref(), point.xz(), elapsed)
            };
            (
                f32::atan2(
                    water_at(Vec3::new(0.0, 0.0, -hull.length / 2.0))
                        - water_at(Vec3::new(0.0, 0.0, hull.length / 2.0)),
                    hull.length,
                ),
                f32::atan2(
                    water_at(Vec3::new(hull.beam / 2.0, 0.0, 0.0))
                        - water_at(Vec3::new(-hull.beam / 2.0, 0.0, 0.0)),
                    hull.beam,
                ),
            )
        } else {
            (0.0, 0.0)
        };

        let pitch = settled(boat.pitch + (target_pitch - boat.pitch) * t, target_pitch);
        let roll = settled(boat.roll + (target_roll - boat.roll) * t, target_roll);
        if pitch == boat.pitch && roll == boat.roll {
            // Nothing to change — which is every frame for a boat with no
            // water under it, whose rotation must stay unwritten the way an
            // idle boat's does in [`steer`].
            continue;
        }
        transform.rotation = (transform.rotation
            * Quat::from_rotation_z(-(boat.roll + boat.heel))
            * Quat::from_rotation_x(pitch - boat.pitch)
            * Quat::from_rotation_z(roll + boat.heel))
        .normalize();
        boat.pitch = pitch;
        boat.roll = roll;
    }
}

/// The tail-closing every eased angle here gets, in [`HEEL_SETTLED`]'s terms:
/// within a third of a degree of its target the angle *is* the target, so a
/// hull done settling holds one rotation frame after frame rather than
/// creeping towards it forever.
fn settled(eased: f32, target: f32) -> f32 {
    if (target - eased).abs() < HEEL_SETTLED {
        target
    } else {
        eased
    }
}

/// How far the bottom stands above the depth the hull is held at, in metres,
/// taken at the worst-placed point of the keel — negative for as long as there
/// is water enough under all of it, zero where the hull is about to be stopped.
/// Not the keel's own penetration, which is this plus [`KEEL_BITE`] — the gap
/// between the hull's draft and its [`Hull::grounding_draft`]: the rule wants
/// one number that rises as the ground does, and nothing ever reads it but
/// its sign and its ordering against itself, both of which the offset leaves
/// alone.
///
/// This is the whole of collision. The ground the client has is a height field
/// on a two-metre lattice, and the boat is a keel line above it, so "is there
/// water enough here" is a handful of lookups rather than anything to do with
/// intersecting the hull's triangles: [`Ground::height`] answers on exactly the
/// facets the mesh was built from, which is what makes the ground the boat is
/// stopped by the ground the player can see.
///
/// A probe over a chunk that has not arrived says nothing rather than
/// objecting, the same choice [`float`] makes: ground the client has not been
/// sent is not ground it may invent. Outrunning the stream would take a stalled
/// server, and if land does turn up under the hull, backing off still works —
/// see [`steer`] for why.
fn grounding(hull: &Hull, ground: Option<&Ground>, transform: &Transform) -> f32 {
    let Some(ground) = ground else {
        return f32::NEG_INFINITY;
    };

    let keel = hull.heel_station - hull.forefoot_station;
    (0..KEEL_PROBES)
        .filter_map(|i| {
            let station = hull.forefoot_station + keel * i as f32 / (KEEL_PROBES - 1) as f32;
            let at = transform.transform_point(Vec3::new(0.0, 0.0, station));
            Some(ground.height(at.x, at.z)? + hull.grounding_draft())
        })
        .fold(f32::NEG_INFINITY, f32::max)
}

/// Drives the boat the player is at the helm of, in its own frame, the way a
/// boat is driven: forward and back run the hull along its heading, and the
/// steering keys are the helm, bringing the bow round for as long as they're
/// held. The view plays no part — turning the camera changes what the keys
/// look like on screen, never what they do — which is what makes a long sail
/// a held key rather than a chase between the camera's yaw and the boat's.
///
/// Only the boat the player is *aboard* answers, which is what being at the
/// helm means here. Ashore, the same keys are the walker's — see
/// `player::walk` — and a hull left at anchor holds station rather than
/// sailing off with its absent owner's keystrokes.
///
/// The throttle is eased rather than instant: the keys name a target speed
/// and the hull's way relaxes towards it on its own way-response curve,
/// stepped exactly for however long the frame was, so the ramp is the same
/// shape at any frame rate. That covers both ends of a sail — way gathered
/// over seconds when the key goes down, and carried into a glide when it
/// comes up — from one constant, with [`WAY_STOPPED`] closing the tail the
/// exponential would otherwise never finish.
///
/// The helm answers even with no way on, which no rudder would; a boat that
/// can't point where it's told while stationary is annoying before it is
/// realistic. It answers aground as well, and for a better reason than that:
/// a turn refused alongside an advance is a hull wedged bow-first against a
/// shore with nothing left that would free it, so the bow may always come
/// round even where the hull may not go.
///
/// Land is what the hull may not go through, and the frame's advance is
/// offered to [`grounding`] before it is taken. It is allowed if the pose it
/// would reach floats — or, failing that, if it is aground no *deeper* than
/// the pose already held. That second half is not an escape hatch for a boat
/// that has got itself ashore; it is the only rule that both frees one and
/// can't be played. It is also narrower than it reads. A hull that is floating
/// can only ever be allowed a pose that floats — `here` at or under zero makes
/// the second clause imply the first — so a boat under way never reaches dry
/// ground at all: it halts still afloat, with at most the fifth of a metre
/// of [`KEEL_BITE`] in the mud, and backing off from
/// there is the *first* clause doing the work, the water astern being water.
/// What the second clause is for is the pose the boat did not sail into — a
/// `--focus` that puts it inland, and ground arriving under a hull already
/// sitting there. Out of those every way down to the sea is downhill, so it
/// goes; and every way further in is uphill, so nosing the bow over a beach to
/// unlock the island — which "aground already, let it through" would hand a
/// player on the first frame — is refused like any other climb. Which is also
/// why the comparison carries no tolerance: a hair of slack is a hair of climb
/// every frame, and a hair a frame is a metre a second up a hillside.
///
/// Only the pose at the end of the advance is judged; the path swept getting
/// there is covered by the probes of the frame before, which holds for as long
/// as a frame's advance stays under the probe spacing. At the ship's speed that is
/// seventeen centimetres at sixty frames a second, and two and a half metres
/// at the quarter second Bevy clamps a stalled frame to — so the sweep is only
/// ever missed on a frame that was already a visible break in the picture.
///
/// Turning at speed also heels the hull: the target lean is helm times way —
/// sharpness times speed, so a hard turn at full way carries the whole of
/// the hull's full-turn heel, a gentle one at half way a quarter of it, and a bow
/// swung round at rest none at all — and the shown heel relaxes towards it on
/// the hull's heel-response curve, which is both the roll into the turn and the
/// straightening out of it. Signed way keeps the geometry honest going
/// astern: the same helm turns about a centre on the other side, so the heel
/// flips with it. The roll is applied about the boat's own forward axis, and
/// the keel lies along that axis, so heeling moves nothing [`grounding`]
/// probes or [`steer`] advances along — it is wholly a thing the eye gets.
///
/// Both poses are judged with the rotation the helm has just applied, so a
/// turn only ever changes where the advance goes, never whether it is allowed.
/// Being stopped takes the way off, which is what running aground does; the
/// ease then makes coming off again the few seconds it should be. What the
/// hull is stopped *by* is the keel line and nothing else — the stem rakes out
/// over the forefoot, so a bow can overhang a cliff face by half a metre
/// before anything objects, which from a camera forty metres up is nothing.
fn steer(
    keys: Res<ButtonInput<KeyCode>>,
    bindings: Res<KeyBindings>,
    time: Res<Time>,
    ground: Option<Res<Ground>>,
    players: Query<&ChildOf, With<Player>>,
    mut boats: Query<(&mut Transform, &mut Boat)>,
) {
    // A player ashore is in no boat's query, and that is the whole of how
    // the helm goes dead when they step off.
    let Some((mut transform, mut boat)) = players
        .single()
        .ok()
        .and_then(|aboard| boats.get_mut(aboard.parent()).ok())
    else {
        return;
    };

    let (drive, helm) = bindings.driving(&keys);

    let ground = ground.as_deref();

    let hull = boat.hull;
    let speed = if drive > 0.0 {
        hull.speed
    } else {
        hull.astern_speed
    };
    let target = drive * speed;
    // A response named in seconds is a rate of its reciprocal.
    let t = eased(1.0 / hull.way_response, time.delta_secs());

    if helm != 0.0 {
        transform.rotate_y(helm * hull.turn_rate * time.delta_secs());
    }
    // Written only while something is happening, so an idle boat holds
    // still without being marked changed every frame.
    if target != 0.0 || boat.way != 0.0 {
        let way = boat.way + (target - boat.way) * t;
        let way = if target == 0.0 && way.abs() < WAY_STOPPED {
            0.0
        } else {
            way
        };

        let advance = transform.forward() * way * time.delta_secs();
        let here = grounding(&hull, ground, &transform);
        let there = grounding(
            &hull,
            ground,
            &Transform {
                translation: transform.translation + advance,
                ..*transform
            },
        );

        if there <= 0.0 || there <= here {
            boat.way = way;
            transform.translation += advance;
        } else {
            boat.way = 0.0;
        }
    }

    // Heel last, against the way this frame settled on, so running
    // aground starts the straightening the same frame it takes the way
    // off. Port helm is a positive turn and an outward lean is to
    // starboard, which about the forward axis is a negative roll — hence
    // the sign. The transform holds heading, then the water's pitch,
    // then one roll factor the heel shares with the wave roll — see
    // [`float`] — and the helm above multiplies heading on from the
    // left, so rolling on from the right reaches that roll factor alone
    // and the guard keeps an idle boat's rotation unwritten.
    let target_heel = -hull.heel_at_full_turn * helm * boat.way / hull.speed;
    if boat.heel != target_heel {
        let heel_t = eased(1.0 / hull.heel_response, time.delta_secs());
        let heel = settled(boat.heel + (target_heel - boat.heel) * heel_t, target_heel);
        transform.rotation *= Quat::from_rotation_z(heel - boat.heel);
        boat.heel = heel;
    }
}

#[cfg(test)]
mod tests {
    use protocol::ground::FACET_METRES;

    use super::*;
    use crate::bindings::Action;
    use crate::testing::{
        assert_model_draws, elapsed, hold, rebind, run_frames, test_ground, triangles, world_app,
        TEST_ISLAND_REACH,
    };

    /// Frames enough for the ease to be indistinguishable from settled —
    /// over eight time constants, a remainder of a few parts in ten thousand.
    const SETTLED: usize = 800;

    /// A headless app with the boat systems running, already in a match —
    /// the shared [`world_app`], under this module's older name.
    fn test_app() -> App {
        world_app()
    }

    fn boat(app: &mut App) -> Transform {
        *app.world_mut()
            .query_filtered::<&Transform, With<Boat>>()
            .single(app.world())
            .expect("a match should have a boat in it")
    }

    /// The bow's bearing, in the same terms as a camera yaw.
    fn heading_yaw(app: &mut App) -> f32 {
        let forward = boat(app).forward();
        f32::atan2(-forward.x, -forward.z)
    }

    /// Which way the pennant is flying, in the world and in the same terms as
    /// a heading — the flag's own rotation carried back out through the hull's,
    /// which is the reverse of what [`fly_the_pennant`] does to get it.
    fn pennant_bearing(app: &mut App) -> f32 {
        let hull = boat(app).rotation;
        let flag = *app
            .world_mut()
            .query_filtered::<&Transform, With<Pennant>>()
            .single(app.world())
            .expect("a boat should be flying a pennant");
        let flying = hull * flag.rotation * Vec3::NEG_Z;
        f32::atan2(-flying.x, -flying.z)
    }

    /// Radians per second the bow comes round at while `key` is held. A rate
    /// rather than an angle — proportionality to how long the key was held is
    /// what makes a turn the same on any machine.
    fn turn_rate(key: KeyCode) -> f32 {
        let mut app = test_app();
        let start_yaw = heading_yaw(&mut app);
        let before = elapsed(&app);
        hold(&mut app, key);
        run_frames(&mut app, 20);
        let seconds = elapsed(&app) - before;
        assert!(seconds > 0.0, "no time passed while the key was held");
        (heading_yaw(&mut app) - start_yaw) / seconds
    }

    #[test]
    fn the_model_is_a_hull_and_a_spar_fit_to_draw() {
        // The things about the file the game cannot see for itself — see
        // `assert_model_draws`. The order is the sharp one here: a Blender
        // afternoon that left the spar first would paint the hull in
        // bare-spar cream and stand a seven-metre plank of timber where the
        // mast should be.
        assert_model_draws(MODEL, &[(HULL_MESH, "hull"), (SPAR_MESH, "spar")]);
    }

    #[test]
    fn the_model_is_the_hull_the_keel_is_probed_along() {
        // What `grounding` assumes about a shape it never looks at: the keel
        // runs at the ship's draft below the waterline, from the forefoot aft
        // to the heel, and the hull is the ship's length. Remodel the boat
        // deeper and every probe would be reading the water above its own keel
        // — the hull would sail through the shallows it should be stopped by,
        // and nothing but this would notice.
        let corners: Vec<Vec3> = triangles(MODEL, HULL_MESH, "POSITION")
            .into_iter()
            .flatten()
            .collect();
        let lowest = corners.iter().map(|c| c.y).fold(f32::MAX, f32::min);
        let highest = corners.iter().map(|c| c.y).fold(f32::MIN, f32::max);
        let (bow, transom) = corners
            .iter()
            .fold((f32::MAX, f32::MIN), |(f, a), c| (f.min(c.z), a.max(c.z)));

        assert!(
            (lowest + SHIP.draft).abs() < 1e-4,
            "the model's keel is {lowest} below the waterline, not {}",
            -SHIP.draft
        );
        // And the sheer, which is where a player aboard is stood: remodel the
        // hull with more freeboard and they would be shin-deep in the deck.
        assert!(
            (highest - SHIP.deck).abs() < 1e-4,
            "the model's deck is {highest} above the waterline, not {}",
            SHIP.deck
        );
        let half = SHIP.length * 0.5;
        assert!(
            (bow + half).abs() < 1e-4 && (transom - half).abs() < 1e-4,
            "the model runs {bow}..{transom}, not a {}m hull about amidships",
            SHIP.length
        );

        // The keel itself, not just the depth: the probes are spread between
        // these two stations, and each one has to be over hull rather than over
        // the water ahead of a forefoot that has crept aft.
        let keel: Vec<&Vec3> = corners
            .iter()
            .filter(|c| (c.y + SHIP.draft).abs() < 1e-4)
            .collect();
        let forefoot = keel.iter().map(|c| c.z).fold(f32::MAX, f32::min);
        let heel = keel.iter().map(|c| c.z).fold(f32::MIN, f32::max);
        assert!(
            (forefoot - SHIP.forefoot_station).abs() < 1e-4
                && (heel - SHIP.heel_station).abs() < 1e-4,
            "the keel runs {forefoot}..{heel}, not {}..{}",
            SHIP.forefoot_station,
            SHIP.heel_station
        );
    }

    #[test]
    fn the_model_flies_a_pennant_from_its_masthead() {
        // The spar's own top, which is where the flag is tied. A mast re-cut
        // in Blender and not re-measured here would leave the pennant flying
        // in mid-air beside it, and nothing else in the game looks at the
        // spar at all.
        let corners: Vec<Vec3> = triangles(MODEL, SPAR_MESH, "POSITION")
            .into_iter()
            .flatten()
            .collect();
        let top = corners.iter().map(|c| c.y).fold(f32::MIN, f32::max);
        assert!(
            (top - SHIP.masthead).abs() < 1e-4,
            "the model's masthead is {top} above the waterline, not {}",
            SHIP.masthead
        );

        let (forward, aft) = corners
            .iter()
            .fold((f32::MAX, f32::MIN), |(f, a), c| (f.min(c.z), a.max(c.z)));
        let stepped = (forward + aft) * 0.5;
        assert!(
            (stepped - SHIP.masthead_station).abs() < 1e-4,
            "the mast stands at {stepped}, not {}",
            SHIP.masthead_station
        );
    }

    #[test]
    fn the_pennant_blows_downwind_and_hangs_in_a_calm() {
        // Where the air is going, not where it came from, and the whole of
        // the strength reading in the droop.
        let (flat, sag) = PENNANT_FLIES;
        // A wind blowing due north — towards -Z — lays the flag along -Z,
        // which is the boat's own zero.
        let (bearing, droop) = pennant_pose(Vec2::new(0.0, -flat), 0.0);
        assert!(
            bearing.abs() < 1e-5,
            "a northward wind flew it to {bearing}"
        );
        assert!(
            (droop - sag).abs() < 1e-5,
            "a full wind left it {droop} down"
        );

        // And one blowing east swings it a quarter turn — the sign the game's
        // own headings use, so port helm and a veering wind agree.
        let (east, _) = pennant_pose(Vec2::new(flat, 0.0), 0.0);
        assert!(
            (east + std::f32::consts::FRAC_PI_2).abs() < 1e-5,
            "an eastward wind flew it to {east}"
        );

        // A calm hangs it straight down and leaves the bearing it had, rather
        // than snapping to whatever direction the last breath of air took.
        let (held, hanging) = pennant_pose(Vec2::new(0.05, -0.05), 1.234);
        assert_eq!(held, 1.234);
        // Within a degree of straight down: the breath of air still in the
        // numbers is allowed to lift it by that much and no more.
        assert!(hanging > std::f32::consts::FRAC_PI_2 - 0.02);

        // Half the wind is most of the way down: the flag lifts through the
        // airs and only the last of it is spent on the blows, which is what
        // makes a light day readable at all.
        let (_, half) = pennant_pose(Vec2::new(0.0, -flat * 0.5), 0.0);
        assert!(half > droop && half < std::f32::consts::FRAC_PI_2);
    }

    #[test]
    fn the_pennant_stays_on_the_wind_through_a_turn() {
        // The reading is the world's, not the boat's: putting the helm over
        // swings the masthead through a right angle and must leave the cloth
        // pointing where it was. Only the helm — with no way on, the apparent
        // wind is the true one throughout, so anything that moved here moved
        // for the wrong reason.
        let mut app = test_app();
        run_frames(&mut app, 2);
        let before = pennant_bearing(&mut app);
        let bow = heading_yaw(&mut app);

        hold(&mut app, KeyCode::ArrowLeft);
        run_frames(&mut app, 40);

        let turned = (heading_yaw(&mut app) - bow).abs();
        assert!(turned > 1.0, "the bow only came round {turned} radians");

        let swung = (pennant_bearing(&mut app) - before).abs();
        assert!(
            swung < 2.0 * FLUTTER.0,
            "the pennant followed the bow round by {swung} radians"
        );
    }

    #[test]
    fn the_boat_points_its_bow_the_way_it_faces() {
        // What steering will be written against: -Z is the bow, so a boat
        // turned to a heading moves along its own forward.
        let mut app = test_app();
        let heading = boat(&mut app).forward();
        let yaw = app.world().resource::<View>().yaw;

        let expected = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
        assert!(
            (*heading - expected).length() < 1e-5,
            "a boat at yaw {yaw} faces {heading:?}, not {expected:?}"
        );
    }

    /// A paused boat is a stopped boat, and the world it is in is still there
    /// to be sailed on afterwards.
    #[test]
    fn pausing_takes_the_helm_away_and_resuming_gives_it_back() {
        let mut app = test_app();
        set_helm(&mut app, Helm::Paused);

        let before = boat(&mut app).translation;
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 20);
        assert_eq!(
            boat(&mut app).translation,
            before,
            "the boat sailed on with the pause menu up"
        );

        set_helm(&mut app, Helm::Sailing);
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 20);
        assert_ne!(
            boat(&mut app).translation,
            before,
            "the helm never came back"
        );
    }

    /// Floating is not steering — see the run conditions in [`BoatPlugin`].
    /// Ground arriving while the game is paused is settled on there and then,
    /// rather than snapping the hull the frame the player resumes.
    #[test]
    fn a_paused_boat_still_rides_ground_that_arrives_under_it() {
        let mut app = test_app();
        set_helm(&mut app, Helm::Paused);
        assert_eq!(boat(&mut app).translation.y, 0.0);

        app.insert_resource(test_ground());
        run_frames(&mut app, 2);
        assert!(
            boat(&mut app).translation.y > 0.0,
            "the hull is still at sea level with an island under it"
        );
    }

    fn set_helm(app: &mut App, helm: Helm) {
        app.world_mut().resource_mut::<NextState<Helm>>().set(helm);
        app.update();
    }

    #[test]
    fn the_forward_key_drives_the_boat_the_way_the_bow_points() {
        let mut app = test_app();
        let before = boat(&mut app);
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 20);
        let moved = boat(&mut app).translation - before.translation;

        // Forward means the boat's own forward — no helm held, so the whole
        // of the movement is dead ahead.
        assert!(moved.length() > 0.0, "the boat never moved");
        assert!(
            moved.normalize().dot(*before.forward()) > 0.999,
            "the boat went {moved:?} rather than along its heading"
        );
        // And along the surface, not through it — height is `float`'s alone.
        assert_eq!(moved.y, 0.0);
    }

    /// Metres per second the boat settles to while `key` is held, signed by
    /// whether it went ahead or astern — measured after the way is gathered,
    /// so it is the speed made good and not some point on the ramp.
    fn speed_made(key: KeyCode) -> f32 {
        let mut app = test_app();
        hold(&mut app, key);
        run_frames(&mut app, SETTLED);

        let before = boat(&mut app);
        let start = elapsed(&app);
        run_frames(&mut app, 60);
        let seconds = elapsed(&app) - start;
        assert!(seconds > 0.0, "no time passed while the key was held");

        let moved = boat(&mut app).translation - before.translation;
        moved.dot(*before.forward()) / seconds
    }

    #[test]
    fn ahead_and_astern_each_make_their_own_speed() {
        let ahead = speed_made(KeyCode::ArrowUp);
        assert!(
            (ahead - SHIP.speed).abs() < SHIP.speed * 0.01,
            "the boat made {ahead} m/s ahead, not {}",
            SHIP.speed
        );

        // Backing off a beach is the whole use of astern, so it is slower and
        // it is backwards — along the heading reversed, not a turn.
        let astern = speed_made(KeyCode::ArrowDown);
        assert!(
            (astern + SHIP.astern_speed).abs() < SHIP.astern_speed * 0.01,
            "the boat made {astern} m/s astern, not -{}",
            SHIP.astern_speed
        );
    }

    #[test]
    fn the_boat_gathers_way_rather_than_leaping_to_speed() {
        // The first half second of a standing start: under way at once, but
        // nowhere near full speed — the ramp is the point of the ease.
        let mut app = test_app();
        let before = boat(&mut app).translation;
        let start = elapsed(&app);
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 30);
        let seconds = elapsed(&app) - start;

        let made = (boat(&mut app).translation - before).length() / seconds;
        assert!(made > 0.0, "the boat never began to move");
        assert!(
            made < SHIP.speed * 0.5,
            "{made} m/s inside the first half second is a leap, not gathered way"
        );
    }

    #[test]
    fn the_boat_carries_its_way_into_a_glide_and_then_stops() {
        let mut app = test_app();
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, SETTLED);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release_all();

        // Off the key at full speed: a glide of a few metres, not a dead stop.
        let going = boat(&mut app).translation;
        run_frames(&mut app, 30);
        let glide = (boat(&mut app).translation - going).length();
        assert!(
            glide > 1.0,
            "the boat stopped dead the moment the key came up"
        );

        // And the glide ends: the way snaps to stopped, and a boat at rest
        // holds the same spot exactly, frame after frame.
        run_frames(&mut app, SETTLED);
        let at_rest = boat(&mut app).translation;
        run_frames(&mut app, 10);
        assert_eq!(
            boat(&mut app).translation,
            at_rest,
            "the boat is still creeping"
        );
    }

    #[test]
    fn the_helm_brings_the_bow_round_at_the_turn_rate() {
        // Port is a positive turn about the vertical, the same way round as
        // the camera's own Q; starboard its mirror.
        let port = turn_rate(KeyCode::ArrowLeft);
        let starboard = turn_rate(KeyCode::ArrowRight);

        let tolerance = SHIP.turn_rate * 0.01;
        assert!(
            (port - SHIP.turn_rate).abs() < tolerance,
            "the bow came round at {port} rad/s to port, not {}",
            SHIP.turn_rate
        );
        assert!(
            (starboard + SHIP.turn_rate).abs() < tolerance,
            "the bow came round at {starboard} rad/s to starboard, not -{}",
            SHIP.turn_rate
        );
    }

    #[test]
    fn the_helm_answers_with_no_way_on() {
        // Turning without moving: the bow swings, the hull stays put.
        let mut app = test_app();
        let before = boat(&mut app);
        hold(&mut app, KeyCode::ArrowLeft);
        run_frames(&mut app, 20);
        let after = boat(&mut app);

        assert_eq!(after.translation, before.translation);
        assert_ne!(after.rotation, before.rotation, "the bow never swung");
    }

    /// The hull's heel, in radians, positive with the masthead to port — what
    /// is left of the pose once the heading's yaw is taken back off it. Read
    /// from the transform rather than the component, because the lean the
    /// player sees is the one the transform holds.
    fn heel_shown(app: &mut App) -> f32 {
        let transform = boat(app);
        let forward = transform.forward();
        let yaw = f32::atan2(-forward.x, -forward.z);
        let roll = Quat::from_rotation_y(yaw).inverse() * transform.rotation;
        let (axis, angle) = roll.to_axis_angle();
        angle * axis.z
    }

    #[test]
    fn a_turn_at_speed_heels_the_hull_outwards() {
        let mut app = test_app();
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, SETTLED);

        // Port helm at full way: the whole of the heel, and to starboard —
        // a keeled hull leans out of a corner, not into it like a bicycle.
        hold(&mut app, KeyCode::ArrowLeft);
        run_frames(&mut app, SETTLED);
        let heel = heel_shown(&mut app);
        assert!(
            (heel + SHIP.heel_at_full_turn).abs() < SHIP.heel_at_full_turn * 0.05,
            "a full-speed port turn heels {heel} rad, not -{}",
            SHIP.heel_at_full_turn
        );

        // The lean is the eye's alone: the bow still points along the
        // surface, so nothing the movement is built on has tilted with it.
        assert!(boat(&mut app).forward().y.abs() < 1e-6);

        // Starboard is the mirror.
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::ArrowLeft);
        hold(&mut app, KeyCode::ArrowRight);
        run_frames(&mut app, SETTLED);
        let heel = heel_shown(&mut app);
        assert!(
            (heel - SHIP.heel_at_full_turn).abs() < SHIP.heel_at_full_turn * 0.05,
            "a full-speed starboard turn heels {heel} rad, not {}",
            SHIP.heel_at_full_turn
        );
    }

    #[test]
    fn the_heel_rolls_on_rather_than_snapping() {
        // A few frames into a full-speed turn: leaning already, but nowhere
        // near the whole heel — the roll is eased the way the way is.
        let mut app = test_app();
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, SETTLED);
        hold(&mut app, KeyCode::ArrowLeft);
        run_frames(&mut app, 6);

        let heel = heel_shown(&mut app).abs();
        assert!(heel > 0.0, "the hull never began to lean");
        assert!(
            heel < SHIP.heel_at_full_turn * 0.5,
            "{heel} rad inside the first tenth of a second is a snap, not a roll"
        );
    }

    #[test]
    fn the_helm_alone_heels_nothing() {
        // The lean is sharpness times speed, and a bow swung round at rest
        // has no speed: the boat pivots bolt upright.
        let mut app = test_app();
        hold(&mut app, KeyCode::ArrowLeft);
        run_frames(&mut app, SETTLED);
        assert_eq!(heel_shown(&mut app), 0.0);
    }

    #[test]
    fn the_hull_comes_level_when_the_turn_ends() {
        let mut app = test_app();
        hold(&mut app, KeyCode::ArrowUp);
        hold(&mut app, KeyCode::ArrowLeft);
        run_frames(&mut app, SETTLED);
        assert_ne!(heel_shown(&mut app), 0.0, "the turn never heeled the hull");

        // Helm amidships, way still on: the heel runs back down its own
        // curve and *ends* — exactly level, not forever creeping towards it.
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::ArrowLeft);
        run_frames(&mut app, SETTLED);
        let level = heel_shown(&mut app);
        assert!(
            level.abs() < 1e-6,
            "the hull is still heeled {level} rad with the helm amidships"
        );
    }

    #[test]
    fn opposed_keys_hold_the_boat_still() {
        // All four at once: ahead cancels astern outright — not by the faster
        // gear's margin — and port cancels starboard.
        let mut app = test_app();
        let before = boat(&mut app);
        for key in [
            KeyCode::ArrowUp,
            KeyCode::ArrowDown,
            KeyCode::ArrowLeft,
            KeyCode::ArrowRight,
        ] {
            hold(&mut app, key);
        }
        run_frames(&mut app, 20);
        let after = boat(&mut app);
        assert_eq!(after.translation, before.translation);
        assert_eq!(after.rotation, before.rotation);
    }

    #[test]
    fn a_rebound_key_steers_and_the_key_it_replaced_stops() {
        let mut app = test_app();
        rebind(&mut app, Action::MoveForward, KeyCode::KeyJ);

        let before = boat(&mut app).translation;
        hold(&mut app, KeyCode::KeyJ);
        run_frames(&mut app, 20);
        assert_ne!(
            boat(&mut app).translation,
            before,
            "the newly bound key did not steer"
        );

        // J was nobody's key, so nothing was traded for it and W is now bound
        // to nothing at all. Holding it has to leave the boat where it lies.
        let mut app = test_app();
        rebind(&mut app, Action::MoveForward, KeyCode::KeyJ);
        let before = boat(&mut app).translation;
        hold(&mut app, KeyCode::KeyW);
        run_frames(&mut app, 20);
        assert_eq!(
            boat(&mut app).translation,
            before,
            "W still steers after being rebound away"
        );
    }

    #[test]
    fn the_arrow_keys_steer_whatever_the_bindings_say() {
        let mut app = test_app();
        // Hand every movement action to keys nowhere near the arrows.
        rebind(&mut app, Action::MoveForward, KeyCode::KeyI);
        rebind(&mut app, Action::MoveBack, KeyCode::KeyK);
        rebind(&mut app, Action::SteerLeft, KeyCode::KeyJ);
        rebind(&mut app, Action::SteerRight, KeyCode::KeyL);

        let before = boat(&mut app).translation;
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 20);
        assert_ne!(
            boat(&mut app).translation,
            before,
            "the arrow keys stopped steering once the letters moved"
        );
    }

    #[test]
    fn a_match_launches_the_boat_where_the_world_is_entered() {
        let mut app = test_app();
        app.insert_resource(View {
            focus: Vec3::new(98.0, 0.0, -317.0),
            ..default()
        });
        // Back to the menu and in again: entering is what launches a boat, and
        // the one from the last world went with it.
        for state in [AppState::MainMenu, AppState::InWorld] {
            app.world_mut()
                .resource_mut::<NextState<AppState>>()
                .set(state);
            app.update();
        }

        let at = boat(&mut app).translation;
        assert_eq!(Vec2::new(at.x, at.z), Vec2::new(98.0, -317.0));
    }

    #[test]
    fn the_player_enters_the_world_aboard_the_boat() {
        // Entering a world is done afloat: one player, riding the boat as a
        // child of it — which is what the camera and the position reports
        // resolve through, so a player spawned loose or not at all would
        // leave both staring at nothing.
        let mut app = test_app();
        let boat = app
            .world_mut()
            .query_filtered::<Entity, With<Boat>>()
            .single(app.world())
            .expect("a match should have a boat in it");
        let aboard = app
            .world_mut()
            .query_filtered::<&ChildOf, With<Player>>()
            .single(app.world())
            .expect("a match should have a player in it")
            .parent();
        assert_eq!(aboard, boat, "the player is not aboard the boat");
    }

    #[test]
    fn the_boat_rides_the_surface_it_is_over() {
        let mut app = island_app();

        /// Where the boat comes to rest when it is put down at a spot.
        fn put_down(app: &mut App, spot: Vec2) -> f32 {
            let mut transform = app
                .world_mut()
                .query_filtered::<&mut Transform, With<Boat>>()
                .single_mut(app.world_mut())
                .expect("a match should have a boat in it");
            transform.translation.x = spot.x;
            transform.translation.z = spot.y;
            app.update();
            boat(app).translation.y
        }

        /// What the ground the boat is riding says about a spot.
        fn ground(app: &App, spot: Vec2) -> f32 {
            app.world()
                .resource::<Ground>()
                .height(spot.x, spot.y)
                .expect("the test ground has arrived")
        }

        // Dry land: the highest ground on the island, where the answer is
        // furthest from the waterline.
        let mut peak = (Vec2::ZERO, f32::MIN);
        for iz in 0..24 {
            for ix in 0..24 {
                let spot = Vec2::new(ix as f32 / 23.0 * 2.0 - 1.0, iz as f32 / 23.0 * 2.0 - 1.0)
                    * TEST_ISLAND_REACH;
                let height = ground(&app, spot);
                if height > peak.1 {
                    peak = (spot, height);
                }
            }
        }
        assert!(peak.1 > 0.0, "the island is entirely under water");
        assert_eq!(
            put_down(&mut app, peak.0),
            peak.1,
            "the boat is not sitting on the ground it is over"
        );

        // And open water past the coast: the swell exactly, at the moment the
        // frame settled, however deep the seabed under it. The same function
        // the game floats with, because what is being pinned is the
        // agreement — the hull at the height the water is drawn at.
        let offshore = Vec2::new(TEST_ISLAND_REACH * 1.5, 0.0);
        assert!(
            ground(&app, offshore) < 0.0,
            "the point picked to be open water is dry land"
        );
        let floated = put_down(&mut app, offshore);
        // The default conditions, because no forecast has reached this app —
        // exactly what `float` is riding on.
        let water = crate::sea::SeaConditions::default().swell(
            offshore,
            elapsed(&app),
            -ground(&app, offshore),
        );
        assert_eq!(
            floated, water,
            "the boat floats at {floated} m, the swell there stands at {water} m"
        );
    }

    #[test]
    fn an_anchored_boat_bobs_on_the_swell() {
        // Not sailing, not steering — just afloat, and still never quite
        // still: the water moves, so the hull does. A run of frames has to
        // find it at more than one height, where the flat sea held it at
        // exactly one forever.
        let mut app = island_app();
        place(&mut app, Vec2::new(TEST_ISLAND_REACH * 1.5, 0.0), Vec2::X);

        let mut heights = Vec::new();
        for _ in 0..60 {
            run_frames(&mut app, 1);
            heights.push(boat(&mut app).translation.y);
        }
        assert!(
            heights.iter().any(|h| h != &heights[0]),
            "a second of frames never moved the hull off {} m",
            heights[0]
        );
    }

    #[test]
    fn an_anchored_hull_sways_with_the_swell() {
        // The other half of bobbing: the water's slope moves the deck, not
        // just its height. At anchor over open water the hull is never quite
        // level and never quite still — a degree or two of pitch and roll as
        // the swell passes under it, and nowhere near the lean of a
        // full-helm turn, which must stay the biggest thing the hull does.
        let mut app = island_app();
        place(&mut app, Vec2::new(TEST_ISLAND_REACH * 1.5, 0.0), Vec2::X);
        run_frames(&mut app, 300);

        let mut tilts = Vec::new();
        for _ in 0..400 {
            run_frames(&mut app, 1);
            tilts.push(boat(&mut app).up().as_vec3().angle_between(Vec3::Y));
        }
        let most = tilts.iter().fold(0.0f32, |a, &b| a.max(b));
        assert!(
            most > 0.005,
            "the deck never left level ({most} rad at most)"
        );
        assert!(
            most < SHIP.heel_at_full_turn,
            "the swell alone tilts the hull {most} rad, past a full-helm heel"
        );
        assert!(tilts.iter().any(|t| *t != tilts[0]), "the deck froze");
    }

    #[test]
    fn the_deck_leans_with_the_water_not_against_it() {
        // The sign, pinned end to end: the bow rises where the water under
        // it stands higher than under the stern, and the masthead goes to
        // port when the starboard beam is the lifted side. Correlated over a
        // few swell periods rather than matched frame by frame, because the
        // tilt is eased and trails the water it is following.
        let mut app = island_app();
        place(&mut app, Vec2::new(TEST_ISLAND_REACH * 1.5, 0.0), Vec2::X);
        run_frames(&mut app, 300);

        let (mut fore_aft, mut athwart) = (0.0, 0.0);
        for _ in 0..600 {
            run_frames(&mut app, 1);
            let transform = boat(&mut app);
            let water_at = |offset: Vec3| {
                let point = transform.transform_point(offset);
                let depth = -app
                    .world()
                    .resource::<Ground>()
                    .height(point.x, point.z)
                    .expect("the test ground has arrived");
                app.world().resource::<sea::SeaConditions>().swell(
                    Vec2::new(point.x, point.z),
                    elapsed(&app),
                    depth,
                )
            };
            let asks_pitch = water_at(Vec3::new(0.0, 0.0, -SHIP.length / 2.0))
                - water_at(Vec3::new(0.0, 0.0, SHIP.length / 2.0));
            let asks_roll = water_at(Vec3::new(SHIP.beam / 2.0, 0.0, 0.0))
                - water_at(Vec3::new(-SHIP.beam / 2.0, 0.0, 0.0));
            // The bow's lift is the pitch's sine; the starboard rail's is
            // the roll's, masthead to port as it rises.
            fore_aft += asks_pitch * transform.forward().y;
            athwart += asks_roll * transform.right().y;
        }
        assert!(fore_aft > 0.0, "the bow dips as the water under it rises");
        assert!(athwart > 0.0, "the hull rolls away from the lifted beam");
    }

    #[test]
    fn a_beached_hull_sits_level() {
        // The ground holds what it has taken: put down on the island's high
        // ground, the pose is not the water's to touch however the sea moves
        // — the rotation stays exactly as placed, frame after frame, the
        // same never-written stillness an idle boat holds.
        let mut app = island_app();
        place(&mut app, Vec2::ZERO, Vec2::X);
        app.update();
        let posed = boat(&mut app).rotation;
        run_frames(&mut app, 120);
        assert_eq!(boat(&mut app).rotation, posed);
    }

    /// The same app with a hand of ground already delivered — the island the
    /// collision tests run aground on, and the surface the float test rides.
    /// The tests that take a bare [`test_app`] instead are the other case worth
    /// having: a client that has been sent nothing must still be able to move.
    fn island_app() -> App {
        let mut app = test_app();
        app.insert_resource(test_ground());
        app
    }

    /// Puts the boat down at a spot, pointing a way — a `--focus` in little.
    fn place(app: &mut App, at: Vec2, facing: Vec2) {
        let mut transform = app
            .world_mut()
            .query_filtered::<&mut Transform, With<Boat>>()
            .single_mut(app.world_mut())
            .expect("a match should have a boat in it");
        transform.translation = Vec3::new(at.x, 0.0, at.y);
        transform.rotation = Quat::from_rotation_y(f32::atan2(-facing.x, -facing.y));
    }

    /// How far the boat is from the middle of the test island, which is round:
    /// this is its distance off the coast plus [`TEST_ISLAND_REACH`].
    fn from_the_island(app: &mut App) -> f32 {
        let at = boat(app).translation;
        Vec2::new(at.x, at.z).length()
    }

    /// What [`grounding`] makes of the pose the boat is lying in: how far the
    /// bottom there stands above the depth the hull is held at, so positive is
    /// aground and more positive is further in.
    fn bite(app: &mut App) -> f32 {
        let transform = boat(app);
        grounding(&SHIP, Some(app.world().resource::<Ground>()), &transform)
    }

    #[test]
    fn the_keel_is_probed_as_closely_as_the_ground_is_sampled() {
        // What a handful of points along the keel does buy: no facet of the
        // height field fits between two probes, so ground rising across a facet
        // is read on the way up. Not the same as seeing everything the field can
        // draw — a crest narrower than a facet is read off its flanks and
        // missed, which no spacing at this scale fixes; [`KEEL_PROBES`] carries
        // the argument for wearing that rather than probing the keel to death.
        let spacing = (SHIP.heel_station - SHIP.forefoot_station) / (KEEL_PROBES - 1) as f32;
        assert!(
            spacing <= FACET_METRES,
            "{spacing} m between probes leaves room for a {FACET_METRES} m facet to hide in"
        );
    }

    #[test]
    fn a_boat_driven_at_a_coast_grounds_short_of_it() {
        let mut app = island_app();
        place(
            &mut app,
            Vec2::new(TEST_ISLAND_REACH + 60.0, 0.0),
            Vec2::new(-1.0, 0.0),
        );

        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, SETTLED);

        // Held off the island rather than stopped out in the open: the test
        // island's rim is near vertical, so its coast is where its reach says.
        let reached = from_the_island(&mut app);
        assert!(
            reached > TEST_ISLAND_REACH,
            "the boat is {reached} m out, which is inside a coast at {TEST_ISLAND_REACH} m"
        );
        assert!(
            reached < TEST_ISLAND_REACH + 10.0,
            "the boat stopped {reached} m out, nowhere near the coast it was driven at"
        );

        // And stopped is stopped, not grinding: the way came off, so the hull
        // holds exactly the same spot with the key still down. The same spot
        // *on the map* — it still bobs, the water under it being water.
        let aground = boat(&mut app).translation;
        run_frames(&mut app, 10);
        let held = boat(&mut app).translation;
        assert_eq!(
            Vec2::new(held.x, held.z),
            Vec2::new(aground.x, aground.z),
            "the boat is still creeping ashore"
        );
    }

    #[test]
    fn astern_backs_a_grounded_boat_off() {
        let mut app = island_app();
        place(
            &mut app,
            Vec2::new(TEST_ISLAND_REACH + 60.0, 0.0),
            Vec2::new(-1.0, 0.0),
        );
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, SETTLED);
        let aground = from_the_island(&mut app);
        // Phase one has to have been *stopped* for phase two to say anything:
        // a run that sailed clean over the island would back off it exactly as
        // far, and the test would pass with no collision in the build at all.
        // So the same check the coast test makes, made again here.
        assert!(
            aground > TEST_ISLAND_REACH && aground < TEST_ISLAND_REACH + 10.0,
            "the boat is {aground} m out, which is not held at a coast at {TEST_ISLAND_REACH} m"
        );

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release_all();
        hold(&mut app, KeyCode::ArrowDown);
        run_frames(&mut app, SETTLED);

        // Backing off a beach is what astern is for, and it needs no rule of
        // its own: away from the ground is the deeper water the hull came in
        // over, and deeper is always allowed.
        let backed = from_the_island(&mut app);
        assert!(
            backed > aground + SHIP.length,
            "the boat came off {} m, less than its own length",
            backed - aground
        );
    }

    #[test]
    fn open_water_is_sailed_at_full_speed() {
        // Nothing under the keel, nothing in the way: the ground the boat has
        // been sent must cost it no speed at all where there is water enough.
        let mut app = island_app();
        place(
            &mut app,
            Vec2::new(TEST_ISLAND_REACH + 100.0, 0.0),
            Vec2::new(1.0, 0.0),
        );
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, SETTLED);

        let before = boat(&mut app).translation;
        let start = elapsed(&app);
        run_frames(&mut app, 60);
        let seconds = elapsed(&app) - start;

        let made = (boat(&mut app).translation - before).length() / seconds;
        assert!(
            (made - SHIP.speed).abs() < SHIP.speed * 0.01,
            "the boat made {made} m/s over open water, not {}",
            SHIP.speed
        );
    }

    #[test]
    fn a_boat_put_down_inland_drives_back_to_the_sea() {
        // What `--focus` on an island leaves behind, and the case that says
        // the ground holds a boat without ever trapping one.
        let mut app = island_app();
        let ashore = Vec2::new(TEST_ISLAND_REACH * 0.5, 0.0);
        place(&mut app, ashore, Vec2::new(1.0, 0.0));
        assert!(
            bite(&mut app) > 0.0,
            "the spot picked to be dry land has water over it"
        );

        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, SETTLED);

        // Back at sea means back on the water: riding the swell exactly,
        // rather than holding any height the hillside gave it.
        let at = boat(&mut app).translation;
        let depth = -app
            .world()
            .resource::<Ground>()
            .height(at.x, at.z)
            .expect("the boat sailed off the ground it was given");
        assert_eq!(
            at.y,
            crate::sea::SeaConditions::default().swell(Vec2::new(at.x, at.z), elapsed(&app), depth),
            "the boat never made it back to the water"
        );
        let afloat = from_the_island(&mut app);
        assert!(
            afloat > TEST_ISLAND_REACH,
            "the boat is still {afloat} m from the middle, inside the coast"
        );
    }

    #[test]
    fn a_grounded_boat_is_never_driven_further_aground() {
        // The pose a rule of "aground already, let it through" would hand the
        // whole island to: bow over the beach, and then whatever the player
        // likes. Driven straight at it, and with no helm — the helm was held
        // here once, on the thought that a hull swept round every heading
        // tries the rule from more angles than one heading does. It tries
        // nothing at all. The turning circle at full way is five metres, so
        // the bow comes round and the boat sails off the shore under any rule
        // whatever, including both of the ones this test exists to catch; what
        // it was watching was a boat that had floated away. A heading held is
        // also what makes the two bites comparable, `grounding` being a
        // reading of the ground under a particular pose and not a property of
        // the spot. Coming round while aground has a test of its own.
        let mut app = island_app();
        place(
            &mut app,
            Vec2::new(TEST_ISLAND_REACH - 1.0, 0.0),
            Vec2::new(-1.0, 0.0),
        );
        let before = bite(&mut app);
        let out = from_the_island(&mut app);
        assert!(before > 0.0, "the boat was meant to start aground");

        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, SETTLED);

        let after = bite(&mut app);
        assert!(
            after <= before,
            "the boat worked its way {} m further into the ground",
            after - before
        );
        // And the same thing said in the terms a player sees it in: bow at the
        // island, key down for eight hundred frames, and not a metre of the
        // island gained.
        let ended = from_the_island(&mut app);
        assert!(
            ended >= out,
            "the boat made {} m towards the middle of the island",
            out - ended
        );
    }

    #[test]
    fn the_helm_answers_while_aground() {
        // A refused turn on top of a refused advance is a hull wedged against
        // a shore for good, so the bow comes round whatever is under it.
        let mut app = island_app();
        place(
            &mut app,
            Vec2::new(TEST_ISLAND_REACH - 1.0, 0.0),
            Vec2::new(-1.0, 0.0),
        );
        let before = boat(&mut app).rotation;

        hold(&mut app, KeyCode::ArrowUp);
        hold(&mut app, KeyCode::ArrowLeft);
        run_frames(&mut app, 20);

        assert_ne!(
            boat(&mut app).rotation,
            before,
            "a boat the ground has stopped cannot come round"
        );
    }
}
