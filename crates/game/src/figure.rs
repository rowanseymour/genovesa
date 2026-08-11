//! What the player is drawn as: a person, modelled and rigged like anything
//! else the game draws, and walked by a clip out of the same file.
//!
//! This is the first model that *moves under its own power*, and it is the
//! reason the export carries skins and actions at all. A hull or a palm is one
//! rigid shape put somewhere; a person is a shape with a skeleton inside it and
//! a run cycle drawn on a dope sheet. Both halves belong in the master, where
//! they can be looked at: a walk is even more a thing to be watched while it is
//! moved than a hull is, and a gait built out of constants in Rust can only be
//! judged by rebuilding the game.
//!
//! So the division is: **the master owns the pose, this module owns the
//! clock.** Blender says what a running person looks like at any point in the
//! cycle. Nothing here knows the length of a leg, how far it swings, or that
//! the body dips when the legs are spread — remodel the figure with longer legs
//! and a rolling gait and no line below changes.
//!
//! What this module does own is *where in the cycle the figure is*, and it
//! drives that by **ground covered** rather than by the clock: the run clip is
//! paused, and [`animate`] seeks it to the point the player's own stride has
//! reached. A clip left running at its own speed would slide the feet whenever
//! the walk was anything other than the speed it was drawn at, and would keep
//! walking on the spot when the player stopped. Seeking it means the feet keep
//! up by construction, a player backing up runs the cycle backwards, and a
//! future rowboat or current that moves the player moves the legs with no
//! second animation and nothing here being told.
//!
//! Standing is the other clip. `idle` and `run` hang off one blend node and the
//! gait's own [`Stride::amount`] crossfades them, so a figure that stops
//! settles into standing rather than freezing mid-stride.
//!
//! Two things about the model the game cannot see for itself, both pinned by
//! tests here: the file's meshes are named for the tones they are painted in
//! (glTF materials are ignored, exactly as everywhere else — see
//! [`crate::matte`]), and the skin is rigid, every vertex bound to one bone at
//! full weight. Weight-paint it smoothly and facets bend as it walks, which is
//! the one thing a world drawn in flat tones cannot have.
//!
//! A player standing on a deck has an unchanging transform *within the boat*,
//! however fast the boat is sailing, so they stand there like a passenger.
//! Only their own movement is a step — which is what makes being carried and
//! walking two readings of one number rather than a mode flag.

use std::f32::consts::TAU;

use bevy::animation::graph::{AnimationGraph, AnimationGraphHandle, AnimationNodeIndex};
use bevy::animation::{AnimationClip, AnimationPlayer};
use bevy::asset::AssetPath;
use bevy::gltf::{GltfAssetLabel, GltfMeshName};
use bevy::prelude::*;

use crate::player::{Player, WALK_SPEED};
use crate::{eased, matte};

/// The figure, rigged and with its two clips in it.
const MODEL: &str = "models/player.glb";

/// The clips, by their position in the file — held to their names by
/// `the_model_carries_the_clips_the_game_plays`. Standing and running are the
/// whole vocabulary for now; a third would be a third node on the blend.
const IDLE: usize = 0;
const RUN: usize = 1;

/// What each mesh in the file is painted, by the name it carries there.
///
/// The model is split into meshes by *tone* rather than by body part — one
/// coat, one pair of legs, one lot of bare skin — because the split's only
/// job is to carry a colour. Which bone moves which vertex is the skin's
/// business and cuts across this freely: the coat's mesh holds the hat, which
/// rides the head bone, and nothing has to agree about that but the master.
///
/// A mesh whose name is not here keeps the file's own PBR material and arrives
/// looking like nothing else in the world, so
/// `the_model_is_the_person_the_game_paints` holds the file to exactly these.
const TONES: [(&str, Color); 3] = [
    ("coat", COAT_COLOR),
    ("canvas", CANVAS_COLOR),
    ("skin", SKIN_COLOR),
];

/// Charcoal, for the coat and the hat. A silhouette rather than a colour:
/// findable on sand, grass and deck alike, and out of the way of the hues the
/// remote players' markers are dealt from.
const COAT_COLOR: Color = Color::srgb(0.22, 0.21, 0.26);
/// Sun-bleached canvas, for the legs. Pale against the coat, so the swinging
/// half of the figure is the half that stands out.
const CANVAS_COLOR: Color = Color::srgb(0.62, 0.58, 0.50);
/// Weathered skin, for the head and the bare forearms. Warm, where nothing
/// else on the figure is, so a small head still reads as a head between the
/// hat and the coat.
const SKIN_COLOR: Color = Color::srgb(0.74, 0.55, 0.42);

/// Ground covered by one turn of the run cycle, in metres — a stride being
/// two steps, left and right.
///
/// This is the one number that has to agree with the master, and it is why it
/// is a number rather than something read out of the file: it is not *in* the
/// file. A clip knows how long it lasts in seconds, not how far the figure
/// drawn in it would travel. Draw a longer-legged run and this wants growing
/// to match, or the feet will scuff.
const STRIDE: f32 = 1.7;

/// How fast the run comes on and dies away against standing, in e-foldings
/// per second — see [`eased`]. A fraction of a second, so a step taken from
/// standing is a step rather than a limb snapping out.
const SETTLING: f32 = 9.0;

/// How much faster than a walk a frame's movement has to be before it is not
/// a step at all. Boarding a boat puts the player from wherever they stood
/// onto its deck, and a capture sweep's teleport is a jump in the same
/// transform this reads — without this, the arithmetic below would take a
/// hundred metres of either for a hundred metres of walking and spin the legs
/// through a dozen strides.
const TELEPORT: f32 = 1.5;

/// The clips, mixed and ready to play: the graph both are hung in, the node
/// each occupies, and the run's own handle, which [`animate`] needs to ask
/// how long a turn of the cycle lasts.
#[derive(Resource)]
struct Gaits {
    graph: Handle<AnimationGraph>,
    idle: AnimationNodeIndex,
    run: AnimationNodeIndex,
    cycle: Handle<AnimationClip>,
}

/// The model hung under a player, marking the whole figure — what
/// [`conduct`] looks for above an animation player to know that the player it
/// has found is a person walking rather than some other rigged thing the
/// world grows later.
#[derive(Component)]
struct Figure;

/// An animation player that is a figure's: the entity the loader put one on,
/// found to be under a [`Figure`] and handed the graph. Only these are seeked
/// by the gait, so the day an eagle arrives with a wingbeat in it, its own
/// clip is not driven by how far the player has walked.
#[derive(Component)]
struct Dancer;

/// How the player's own walk stands at the moment, carried on the player.
///
/// `phase` runs round the gait cycle in radians, advanced by the distance the
/// player covers rather than by time. `amount` is how much of the run is
/// showing against standing, eased on and off. `last` is where they were,
/// being the only way to learn what they covered — the figure asks nobody
/// what is moving them, which is what lets anything the game grows later
/// drive the gait for free.
#[derive(Component)]
struct Stride {
    phase: f32,
    amount: f32,
    last: Vec3,
}

pub struct FigurePlugin;

impl Plugin for FigurePlugin {
    fn build(&self, app: &mut App) {
        // None of this is the player's hands, so none of it stops with the
        // pause menu up: a paused player is not moving, which the gait reads
        // as standing still and settles into on its own.
        app.add_systems(Startup, rig)
            .add_systems(Update, (dress, conduct, paint, (stride, animate).chain()));
    }
}

/// Where in the model file one of its clips is.
fn clip(index: usize) -> AssetPath<'static> {
    GltfAssetLabel::Animation(index).from_asset(MODEL)
}

/// Mixes the two clips together once for the whole run: they hang off a
/// single blend node, whose children's weights [`animate`] moves between.
/// Built up front rather than when a player appears, because a graph is a
/// description of the clips and not of anybody dancing to them.
fn rig(
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
) {
    let mut graph = AnimationGraph::new();
    let root = graph.root;
    let blend = graph.add_blend(1.0, root);
    let cycle: Handle<AnimationClip> = assets.load(clip(RUN));
    // Standing at full weight and running at none: a figure that has not
    // moved yet is standing, and the first step eases the balance across.
    let idle = graph.add_clip(assets.load(clip(IDLE)), 1.0, blend);
    let run = graph.add_clip(cycle.clone(), 0.0, blend);

    commands.insert_resource(Gaits {
        graph: graphs.add(graph),
        idle,
        run,
        cycle,
    });
}

/// Hangs the figure under any player that has just appeared — whoever spawned
/// them, and without them knowing a figure exists.
///
/// The whole scene is spawned rather than the meshes being pulled out of the
/// file one at a time, which is what every other model here does. A skinned
/// mesh has to arrive as a hierarchy: the loader wires the bones, the skin and
/// the animation targets onto the nodes as it builds them, and a mesh lifted
/// out on its own would be a shape with no skeleton behind it. The cost is
/// that the file's own materials come with it, which [`paint`] undoes.
///
/// It stands with its soles at the player's origin, that being where the
/// master puts them — the point `player::walk` holds on the ground and the
/// deck holds a passenger at.
fn dress(
    mut commands: Commands,
    assets: Res<AssetServer>,
    players: Query<(Entity, &Transform), Added<Player>>,
) {
    for (player, place) in &players {
        commands.entity(player).with_child((
            Name::new("Figure"),
            Figure,
            WorldAssetRoot(assets.load(GltfAssetLabel::Scene(0).from_asset(MODEL))),
        ));
        commands.entity(player).insert(Stride {
            phase: 0.0,
            amount: 0.0,
            last: place.translation,
        });
    }
}

/// Sets a figure dancing as its scene finishes arriving: the loader puts an
/// [`AnimationPlayer`] on the root of whatever it found animated, and this
/// hands that player the graph and starts both clips.
///
/// The run is started *paused*. It is played to be seeked — see [`animate`] —
/// and a clip left to run at its own speed would walk the figure's legs while
/// they stood still.
fn conduct(
    mut commands: Commands,
    gaits: Res<Gaits>,
    hierarchy: Query<&ChildOf>,
    figures: Query<(), With<Figure>>,
    mut arrivals: Query<(Entity, &mut AnimationPlayer), Added<AnimationPlayer>>,
) {
    for (entity, mut player) in &mut arrivals {
        // Somebody else's model, if the world ever grows a rigged one: the
        // loader puts a player on anything it finds animated, and this gait
        // is the walking figure's alone.
        if !hierarchy
            .iter_ancestors(entity)
            .any(|above| figures.contains(above))
        {
            continue;
        }

        commands
            .entity(entity)
            .insert((AnimationGraphHandle(gaits.graph.clone()), Dancer));
        player.play(gaits.idle).repeat();
        player.play(gaits.run).repeat().pause();
    }
}

/// Paints the figure in the world's own tones as its meshes arrive, throwing
/// away the materials that came out of the file.
///
/// The same rule the rest of the game's models are drawn by, arriving here a
/// different way: glTF materials are PBR — a roughness, a metalness, a
/// specular response — and the look here is a small fixed palette under
/// [`matte`], so a person lit the way the file asked for would be the one
/// surface in the world with a highlight on it.
fn paint(
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut arrivals: Query<
        (&GltfMeshName, &mut MeshMaterial3d<StandardMaterial>),
        Added<GltfMeshName>,
    >,
) {
    for (mesh, mut material) in &mut arrivals {
        if let Some((_, tone)) = TONES.iter().find(|(named, _)| *named == mesh.0) {
            *material = MeshMaterial3d(materials.add(matte(*tone)));
        }
    }
}

/// Advances the gait by the ground the player covered this frame.
///
/// Distance rather than time is the whole of it: a stride is [`STRIDE`]
/// metres of ground whatever speed it is walked at, so the feet cannot slide.
/// Backing up runs the cycle backwards, which puts the legs through a
/// backwards walk without a second clip existing.
///
/// The movement read is the player's own — their transform in whatever frame
/// they are in — so a player riding a deck is still, however the boat is
/// moving beneath them. Only height is thrown away: walking up a hill is
/// walking, and the ground's rise is not extra stride.
fn stride(time: Res<Time>, mut players: Query<(&Transform, &mut Stride)>) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (place, mut stride) in &mut players {
        let covered = place.translation.xz().distance(stride.last.xz());
        let ahead = place
            .forward()
            .xz()
            .dot(place.translation.xz() - stride.last.xz());
        stride.last = place.translation;

        // A jump is not a walk: see `TELEPORT`. The gait is left exactly
        // where it was, so a player who boards mid-step is standing on the
        // deck with their legs still, not finishing the step on it.
        let speed = covered / dt;
        if speed > WALK_SPEED * TELEPORT {
            stride.amount = 0.0;
            continue;
        }

        let going = if ahead < 0.0 { -1.0 } else { 1.0 };
        stride.phase = (stride.phase + going * covered / STRIDE * TAU).rem_euclid(TAU);
        let target = (speed / WALK_SPEED).min(1.0);
        stride.amount += (target - stride.amount) * eased(SETTLING, dt);
    }
}

/// Puts the figure where the gait says: the run seeked to the phase the
/// player's own stride has reached, and the balance between running and
/// standing set to how much of a walk is on.
///
/// Nothing happens until the clip has loaded, which is a frame or two into a
/// match — the figure stands in its rest pose until then, which is what it
/// would be doing anyway.
fn animate(
    gaits: Res<Gaits>,
    clips: Res<Assets<AnimationClip>>,
    strides: Query<&Stride>,
    mut players: Query<&mut AnimationPlayer, With<Dancer>>,
) {
    let Ok(stride) = strides.single() else {
        return;
    };
    let Some(cycle) = clips.get(&gaits.cycle).map(AnimationClip::duration) else {
        return;
    };

    for mut player in &mut players {
        if let Some(run) = player.animation_mut(gaits.run) {
            run.set_weight(stride.amount);
            run.seek_to(stride.phase / TAU * cycle);
        }
        if let Some(idle) = player.animation_mut(gaits.idle) {
            idle.set_weight(1.0 - stride.amount);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::{FRAC_PI_2, PI};

    use bevy::animation::graph::AnimationNodeType;

    use super::*;
    use crate::terrain::Ground;
    use crate::testing::{
        hold, is_flat_shaded, model, run_frames, skin_weights, test_ground, triangles,
        winds_outwards, world_app, TEST_ISLAND_REACH,
    };

    /// The corners of every mesh in the model, in the file's own frame.
    fn corners() -> Vec<Vec3> {
        (0..TONES.len())
            .flat_map(|mesh| triangles(MODEL, mesh, "POSITION"))
            .flatten()
            .collect()
    }

    /// Where each mesh sits in the file, by name — the game asks by name, so
    /// the tests do too.
    fn meshes() -> Vec<String> {
        model(MODEL).0["meshes"]
            .as_array()
            .expect("the model has meshes")
            .iter()
            .map(|mesh| mesh["name"].as_str().expect("a named mesh").to_owned())
            .collect()
    }

    #[test]
    fn the_model_is_the_person_the_game_paints() {
        // Every mesh in the file is one the palette has a tone for, and every
        // tone has a mesh: one that has drifted out of the pairing arrives
        // wearing whatever Blender last gave it, which is a shine nothing
        // else in the world has.
        let mut named = meshes();
        named.sort();
        let mut wanted: Vec<String> = TONES.iter().map(|(name, _)| (*name).to_owned()).collect();
        wanted.sort();
        assert_eq!(named, wanted, "the model's meshes are not the palette's");

        // And the conditions every model here is held to — see
        // `testing::assert_model_draws`, which cannot be used directly
        // because it pins meshes to positions and these are asked for by
        // name.
        for (index, name) in meshes().iter().enumerate() {
            let faces = triangles(MODEL, index, "POSITION");
            assert!(winds_outwards(&faces), "the {name} is wound inside-out");
            assert!(
                is_flat_shaded(&faces, &triangles(MODEL, index, "NORMAL")),
                "the {name} is smooth-shaded"
            );
        }
    }

    #[test]
    fn the_skin_is_rigid_so_the_facets_stay_flat() {
        // What holds the look together once the model moves: each vertex is
        // carried by exactly one bone, so a facet is turned rather than bent.
        // Weight-paint a shoulder smoothly in Blender and the boxes round off
        // as the figure walks — gradients across facets, in a world that has
        // none anywhere.
        for mesh in 0..TONES.len() {
            for weights in skin_weights(MODEL, mesh) {
                let carrying = weights.iter().filter(|w| **w > 0.0).count();
                assert_eq!(
                    carrying,
                    1,
                    "a vertex of {} is shared between bones: {weights:?}",
                    meshes()[mesh]
                );
                assert!(
                    weights.iter().any(|w| (*w - 1.0).abs() < 1e-3),
                    "a vertex of {} is carried at {weights:?}",
                    meshes()[mesh]
                );
            }
        }
    }

    #[test]
    fn the_figure_is_a_person_standing_on_the_origin() {
        // Person-sized, and standing on its own soles: the master puts the
        // feet at the origin because that is the point the walk holds on the
        // ground and the deck holds a passenger at. A figure modelled about
        // its middle would walk knee-deep in the sand.
        let (low, high) = corners()
            .iter()
            .fold((f32::MAX, f32::MIN), |(l, h), c| (l.min(c.y), h.max(c.y)));
        assert!(low.abs() < 1e-4, "the figure's soles are at {low}, not 0");
        assert!(
            (1.6..=2.0).contains(&high),
            "the figure stands {high} m tall, which is not a person"
        );
    }

    #[test]
    fn the_hat_points_the_way_the_player_faces() {
        // The one part of the figure that says which way it is facing from
        // overhead, where a body is nearly symmetric: the brim reaches
        // further ahead — -Z, the way everything here faces — than the whole
        // rest of the coat reaches astern.
        let coat = meshes()
            .iter()
            .position(|name| name == "coat")
            .expect("coat");
        let (ahead, astern) = triangles(MODEL, coat, "POSITION")
            .into_iter()
            .flatten()
            .fold((f32::MAX, f32::MIN), |(f, a), c| (f.min(c.z), a.max(c.z)));
        assert!(
            -ahead > astern,
            "the coat reaches {} ahead and {astern} astern, so it has no point",
            -ahead
        );
    }

    #[test]
    fn the_model_carries_the_clips_the_game_plays() {
        // The game asks for its clips by position in the file, as it asks for
        // meshes elsewhere; an afternoon in Blender that renamed or reordered
        // the actions would have the figure standing to attention while it
        // ran.
        let clips: Vec<String> = model(MODEL).0["animations"]
            .as_array()
            .expect("the model has animations")
            .iter()
            .map(|clip| clip["name"].as_str().expect("a named clip").to_owned())
            .collect();
        assert_eq!(clips.get(IDLE).map(String::as_str), Some("idle"));
        assert_eq!(clips.get(RUN).map(String::as_str), Some("run"));
    }

    // --- What the game does with it ------------------------------------------

    /// A match with the player ashore on the test island, on their own feet
    /// and free to walk.
    fn ashore_app() -> App {
        let mut app = world_app();
        app.insert_resource(test_ground());

        let mut players = app
            .world_mut()
            .query_filtered::<(&mut Transform, Entity), With<Player>>();
        let (mut place, player) = players
            .single_mut(app.world_mut())
            .expect("a match should have a player in it");
        // Halfway up the island, well inside the shore, facing the middle.
        *place = Transform::from_xyz(TEST_ISLAND_REACH * 0.5, 0.0, 0.0)
            .with_rotation(Quat::from_rotation_y(FRAC_PI_2));
        app.world_mut().entity_mut(player).remove::<ChildOf>();
        run_frames(&mut app, 1);
        app
    }

    /// Stands in for the clip the file would have brought, so the seeking can
    /// be tested without a render app to load a glTF with. One second long,
    /// which makes a seek time a fraction of the cycle read directly.
    const CYCLE: f32 = 1.0;

    /// Gives the app a loaded run clip and something playing it, as a real
    /// match gets from the loader once the model has arrived.
    fn with_a_dancer(app: &mut App) -> Entity {
        let (cycle, run) = {
            let gaits = app.world().resource::<Gaits>();
            (gaits.cycle.clone(), gaits.run)
        };
        let mut clip = AnimationClip::default();
        clip.set_duration(CYCLE);
        app.world_mut()
            .resource_mut::<Assets<AnimationClip>>()
            .insert(&cycle, clip)
            .expect("the stand-in clip goes where the real one would");

        // Under the figure, where the loader would have put it: `conduct`
        // takes an animation player for the walker's only if it hangs below
        // one.
        let figure = app
            .world_mut()
            .query_filtered::<Entity, With<Figure>>()
            .single(app.world())
            .expect("the player has no figure hung under them");
        let dancer = app
            .world_mut()
            .spawn((AnimationPlayer::default(), ChildOf(figure)))
            .id();
        run_frames(app, 1);
        assert!(
            app.world()
                .entity(dancer)
                .get::<AnimationPlayer>()
                .expect("a dancer")
                .animation(run)
                .is_some(),
            "the figure was never set dancing"
        );
        dancer
    }

    fn gait_of(app: &mut App) -> (f32, f32, Vec3) {
        let stride = app
            .world_mut()
            .query::<&Stride>()
            .single(app.world())
            .expect("a dressed player carries a stride");
        (stride.phase, stride.amount, stride.last)
    }

    /// The run clip's seek time and weight, as the figure is dancing them.
    fn playing(app: &mut App, dancer: Entity) -> (f32, f32) {
        let gaits = app.world().resource::<Gaits>();
        let (run, idle) = (gaits.run, gaits.idle);
        let player = app
            .world()
            .entity(dancer)
            .get::<AnimationPlayer>()
            .expect("a dancer");
        let running = player.animation(run).expect("the run is playing");
        let standing = player.animation(idle).expect("the idle is playing");
        assert!(
            (running.weight() + standing.weight() - 1.0).abs() < 1e-4,
            "the blend does not add up: {} and {}",
            running.weight(),
            standing.weight()
        );
        (running.seek_time(), running.weight())
    }

    #[test]
    fn the_player_is_dressed_in_a_figure() {
        let mut app = world_app();
        let player = app
            .world_mut()
            .query_filtered::<Entity, With<Player>>()
            .single(app.world())
            .expect("a match should have a player in it");

        let mut figures = app
            .world_mut()
            .query_filtered::<(&ChildOf, &Transform), With<WorldAssetRoot>>();
        let (parent, place) = figures
            .single(app.world())
            .expect("the player has no figure hung under them");
        assert_eq!(parent.parent(), player);
        assert_eq!(place.translation, Vec3::ZERO, "the figure is off its feet");
    }

    #[test]
    fn something_else_that_moves_is_left_to_its_own_clock() {
        // The loader puts an animation player on anything animated it finds,
        // and the day the wildlife is rigged there will be several — none of
        // which are walking anywhere. Only the figure's is taken over.
        let mut app = world_app();
        let stranger = app.world_mut().spawn(AnimationPlayer::default()).id();
        run_frames(&mut app, 2);

        let stranger = app.world().entity(stranger);
        assert!(
            stranger.get::<Dancer>().is_none(),
            "an eagle was taken for a walker"
        );
        assert!(
            stranger.get::<AnimationGraphHandle>().is_none(),
            "the walker's clips were pressed on something else"
        );
    }

    #[test]
    fn the_run_is_paused_and_seeked_by_the_ground_covered() {
        // The whole point of driving a clip by distance: a stride is STRIDE
        // metres of ground, so the feet cannot slide under a body moving at
        // some other speed. The clip never advances on its own — pausing it
        // is what leaves the ground in charge.
        let mut app = ashore_app();
        let dancer = with_a_dancer(&mut app);
        let (was, _, from) = gait_of(&mut app);

        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 40);

        // Measured from the gait's own reads rather than from the transform,
        // which may be a frame ahead of it — the two systems are unordered,
        // and a gait a sixtieth of a second behind the feet is nothing to
        // pin.
        let (phase, _, to) = gait_of(&mut app);
        let covered = to.xz().distance(from.xz());
        assert!(covered > STRIDE, "not enough ground covered to tell");

        let expected = (was + covered / STRIDE * TAU).rem_euclid(TAU);
        assert!(
            (phase - expected).abs() < 0.01,
            "{covered:.2} m of ground left the gait at {phase:.2} rad, not {expected:.2}"
        );

        let (seek, _) = playing(&mut app, dancer);
        assert!(
            (seek - phase / TAU * CYCLE).abs() < 1e-4,
            "the cycle is at {seek} s, not at the {phase} rad the walk has reached"
        );
        assert!(
            app.world()
                .entity(dancer)
                .get::<AnimationPlayer>()
                .expect("a dancer")
                .animation(app.world().resource::<Gaits>().run)
                .expect("the run is playing")
                .is_paused(),
            "the run is running on its own clock"
        );
    }

    #[test]
    fn a_standing_player_settles_into_standing() {
        let mut app = ashore_app();
        let dancer = with_a_dancer(&mut app);

        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 40);
        let (_, running) = playing(&mut app, dancer);
        assert!(running > 0.9, "a walk at speed is only {running} of a run");

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::ArrowUp);
        run_frames(&mut app, 60);

        let (_, running) = playing(&mut app, dancer);
        assert!(
            running < 0.01,
            "a second after stopping the figure is still {running} running"
        );
    }

    #[test]
    fn a_player_riding_a_boat_stands_still_however_fast_it_sails() {
        // The gait reads the player's own transform, which aboard is the
        // deck's own frame, unchanged frame after frame — so the passenger is
        // a passenger with no mode flag saying so.
        let mut app = world_app();
        app.insert_resource(test_ground());
        let dancer = with_a_dancer(&mut app);

        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 120);

        let (_, amount, _) = gait_of(&mut app);
        assert_eq!(amount, 0.0, "the player walked the deck of a sailing boat");
        assert_eq!(playing(&mut app, dancer), (0.0, 0.0));
    }

    #[test]
    fn boarding_is_a_jump_rather_than_a_stride() {
        // Boarding lifts the player from the beach onto the deck. Read as
        // movement — tens of metres in one frame — and the legs would spin
        // through a dozen strides.
        let mut app = ashore_app();
        hold(&mut app, KeyCode::ArrowUp);
        run_frames(&mut app, 30);
        let (before, _, _) = gait_of(&mut app);

        let player = app
            .world_mut()
            .query_filtered::<Entity, With<Player>>()
            .single(app.world())
            .expect("a match should have a player in it");
        let boat = app
            .world_mut()
            .query_filtered::<Entity, With<crate::boat::Boat>>()
            .single(app.world())
            .expect("a match should have a boat in it");
        app.world_mut()
            .entity_mut(player)
            .insert((ChildOf(boat), Transform::default()));
        run_frames(&mut app, 1);

        let (after, amount, _) = gait_of(&mut app);
        assert_eq!(after, before, "the jump was walked");
        assert_eq!(amount, 0.0, "the jump left the figure mid-stride");
    }

    #[test]
    fn walking_backwards_runs_the_gait_backwards() {
        let mut app = ashore_app();
        hold(&mut app, KeyCode::ArrowDown);
        run_frames(&mut app, 30);

        let (phase, amount, _) = gait_of(&mut app);
        assert!(amount > 0.0, "backing up never started the legs");
        assert!(
            phase > PI,
            "the gait ran forwards on a player backing up: {phase}"
        );
    }

    #[test]
    fn the_figure_asks_nothing_of_the_ground() {
        // Nothing here reads the terrain — the figure stands on whatever the
        // player's origin is on, which is `player::walk`'s business — so it
        // poses just as well in a world with no ground delivered at all.
        let mut app = world_app();
        assert!(app.world().get_resource::<Ground>().is_none());
        let dancer = with_a_dancer(&mut app);
        run_frames(&mut app, 10);
        assert_eq!(playing(&mut app, dancer), (0.0, 0.0));
    }

    #[test]
    fn the_graph_holds_a_clip_at_each_of_the_two_nodes() {
        // A graph built against the wrong labels would leave the figure
        // standing to attention while it ran, and nothing at runtime would
        // say so — a node that holds no clip simply poses nothing.
        let app = world_app();
        let gaits = app.world().resource::<Gaits>();
        let graph = app
            .world()
            .resource::<Assets<AnimationGraph>>()
            .get(&gaits.graph)
            .expect("the graph was built");

        assert_ne!(gaits.idle, gaits.run, "both clips landed on one node");
        for (node, wanted) in [(gaits.idle, None), (gaits.run, Some(&gaits.cycle))] {
            let held = match &graph.get(node).expect("a node the graph knows").node_type {
                AnimationNodeType::Clip(clip) => clip.clone(),
                other => panic!("the gait hangs off a {other:?} rather than a clip"),
            };
            if let Some(wanted) = wanted {
                assert_eq!(&held, wanted, "the run node is not the run clip");
            }
        }
    }
}
