//! The cairn: what a claim leaves standing on the ground.
//!
//! Claiming an island is the server's to grant — it asks its own survey
//! whether this player has been the whole way round this coast and is standing
//! inside it, and nothing a client says about that is believed. What lands on
//! this side is the answer, [`protocol::ToClient::Cairn`], told to whoever
//! comes near one. This module is the half the server has no opinion on: what
//! being claimed *looks like* from the ground.
//!
//! It is a thing and not a widget. A label floating over an island says *the
//! game* is telling you something, where stone standing on a headland says
//! *somebody was here*, which is the whole of what a claim means.
//!
//! And it is the size of a person. It was a beacon first — twenty metres of
//! staff and seven of banner — which read from a mile and read as *civic*: a
//! mast and a flag that size are what a shipyard puts up, not what one person
//! carrying rock does in an afternoon. So it is a survey mark instead, a
//! pillar of dry stone a little over head height, stacked in [`COURSES`] that
//! step in as they rise. The taper is the whole of what says *built*, and what
//! it costs is the mile — see [`STONE`] for what is bought with it.
//!
//! # Standing it on the ground
//!
//! A cairn arrives as a point on the plane and the height it stands at is this
//! side's own business, the server having no camera. The telling and the
//! chunks are separate answers travelling at their own speeds, so a cairn
//! waits [`Unfooted`] until there is ground under it, exactly as an arriving
//! player does. `server::CAIRN_SIGHT` is inside the stream radius, so the wait
//! is usually a few frames — but the rule guards against a chunk that has not
//! arrived, and *when* it has not arrived is not this side's to promise.
//!
//! It waits *unseen*, because a cairn drawn at its told point before its
//! ground arrives stands on the open sea for the whole approach and then jumps
//! onto the headland as the shore streams in.
use std::collections::HashMap;

use bevy::prelude::*;

use crate::terrain::Ground;
use crate::{between, matte, signed, unit, AppState};

/// How tall the pillar stands, in metres — a little over head height on the
/// person who stacked it, which is as high as anybody piles rock by hand
/// without building steps to do it.
///
/// Head height is the point of it rather than an accident of the number: a
/// mark you can see over is landscape, and a mark you cannot is a thing
/// somebody put there. Below about a metre and a half it reads as a stone
/// wall's end; above two it starts wanting scaffolding to be believed.
const PILLAR: f32 = 1.7;

/// How wide the pillar is at the foot and at the crown, in metres.
///
/// The taper is the shape a stack of loose rock has to take to stand up, and
/// so the shape that says a person stacked it. Straight-sided it is a bollard;
/// tapered much harder than this it is back to being a heap.
const PILLAR_WIDTH: (f32, f32) = (1.0, 0.45);

/// How many courses the stones are stacked in — few enough that each is a
/// visible step in the taper, which is what carries the drystone read at the
/// distance the thing is actually looked at from.
const COURSES: usize = 4;

/// How far off its own axis a course may sit, in metres, and how far round it
/// may be turned, in radians.
///
/// Nothing structural — a few centimetres and a few degrees. A pillar stacked
/// plumb and square is a machined object, and the whole argument for the shape
/// is that hands made it. The wobble is dealt from the island's own id, so
/// every machine stacks one island's pillar identically and no two islands get
/// the same pillar.
const LEAN: f32 = 0.05;
const SKEW: f32 = 0.5;

/// The stone, in the shades one course may be dealt.
///
/// Bleached coral rag: an island in this ocean has it, one person can carry
/// it, and it is nearly white, which is the part that does the work. Nothing
/// the ground is drawn in goes above sand at `0.86` — see
/// [`protocol::ground::PALETTE`] — so a pillar this pale is the lightest thing
/// on any island, and light in a way no ground here is. That is what replaces
/// the banner: at the range this is meant to be found at, the eye is looking
/// for something that is not the palette, and a white mark against green is
/// exactly that.
///
/// Three shades rather than one because a single flat grey over four courses
/// reads as one moulded object. Dealt per course, so the stack has stones of
/// different rock in it, as a stack picked up off a hillside does.
const STONE: [Color; 3] = [
    Color::srgb(0.93, 0.92, 0.88),
    Color::srgb(0.87, 0.86, 0.81),
    Color::srgb(0.80, 0.79, 0.75),
];

/// How many sides a course is cut with. Five, because it is odd: a drum with
/// an even count shows two parallel faces and a flat silhouette from half the
/// angles a player walks round it at, and five never does.
const FACES: u32 = 5;

/// How near the middle of a pillar a walker may come, in metres — the stone's
/// own half-width at the foot, plus a body's breadth.
///
/// Here rather than in `player`, because it is a fact about how big the thing
/// is: change [`PILLAR_WIDTH`] and this is what has to move with it. What
/// *does* the refusing is `player::walk`, which owns every rule about where a
/// walker may put their feet.
pub const BERTH: f32 = PILLAR_WIDTH.0 / 2.0 + 0.4;

/// Salts for the courses' wobble — one question each, off the same bits. See
/// [`crate::unit`].
const SALT_LEAN: u32 = 0x0CA1_2117;
const SALT_SKEW: u32 = 0x5EA5_1DE0;
const SALT_STONE: u32 = 0xC0_2A11E5;

/// The cairns this client has been told of, by the entity standing for each.
///
/// Keyed by the island — the identity a cairn is told under, and so the same
/// pair of numbers on every machine — see [`protocol::ToClient::Cairn`]. Kept
/// for the reason [`crate::beasts::Beasts`] keeps its own: entities spawn
/// through `Commands`, so two words about one cairn in a single frame's drain
/// would otherwise go looking for an entity that is still a queued command.
#[derive(Resource, Default)]
pub struct Cairns {
    standing: HashMap<IVec2, Entity>,
}

impl Cairns {
    /// A word about a cairn: the first one builds it, and there is nothing in
    /// the world for a later one to change. A cairn is told again when it is
    /// christened, and a name is the *sheet's* to letter — see
    /// [`crate::chart`], which hears the same word. Where the stones stand
    /// never changes, a cairn being a pile of rock rather than a thing that
    /// moves, and nothing about how they are drawn depends on whose they are.
    /// So a second telling is quietly nothing here.
    ///
    /// The stones go up bare and are [`dress`]ed a moment later, which is the
    /// beasts' arrangement and for their reason: this spawns from a word off
    /// the wire and holds no asset store, so what a cairn is *made of* is a
    /// system of its own.
    pub fn told(&mut self, commands: &mut Commands, island: IVec2, at: Vec2) {
        if self.standing.contains_key(&island) {
            return;
        }
        let cairn = commands
            .spawn((
                Name::new(format!("Cairn {}, {}", island.x, island.y)),
                Cairn { island },
                Unfooted,
                DespawnOnExit(AppState::InWorld),
                Transform::from_xyz(at.x, 0.0, at.y),
                // Not shown until it is standing on something — see the module
                // doc, and [`stand_the_cairns`], which is what reveals it.
                Visibility::Hidden,
            ))
            .id();
        self.standing.insert(island, cairn);
    }
}

/// One cairn, as this client draws it.
///
/// Whose it is is not on it. Nothing in the world is drawn differently for a
/// claim being the player's own — a stranger's cairn is exactly as much of a
/// mark as your own, and one that announced itself by its colour would be a
/// claim nobody had to sail up to. It is the *sheet* that cares: see
/// [`crate::chart`], which is told the same word and keeps `yours` on it.
#[derive(Component)]
pub struct Cairn {
    /// The island it speaks for.
    pub island: IVec2,
}

/// A cairn waiting for ground to stand on — see the module doc.
#[derive(Component)]
struct Unfooted;

/// The cairns still waiting for ground, as a query.
type Waiting<'w, 's> = Query<
    'w,
    's,
    (Entity, &'static mut Transform, &'static mut Visibility),
    (With<Cairn>, With<Unfooted>),
>;

/// What building a cairn needs in hand — the two asset stores and the pieces
/// cut from them — bundled so that [`dress`] is one system parameter rather
/// than three. Nothing else builds a cairn: [`raise_the_cairns`] takes the
/// telling and puts the bare stones up, holding no assets at all, which is
/// why what a cairn is *made of* is a system of its own.
#[derive(bevy::ecs::system::SystemParam)]
struct CairnKit<'w, 's> {
    meshes: ResMut<'w, Assets<Mesh>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
    /// The pieces every cairn is built from, made once and dealt from per
    /// cairn: an archipelago somebody has worked their way through is four
    /// course meshes, not forty pillars' worth.
    stonework: Local<'s, Option<Stonework>>,
}

/// The handles [`dress`] deals from — see [`CairnKit::stonework`]. One mesh
/// per course, each narrower than the one under it, and the shades a course
/// may be cut from.
#[derive(Clone)]
struct Stonework {
    shades: [Handle<StandardMaterial>; STONE.len()],
    courses: [Handle<Mesh>; COURSES],
}

/// How wide a course is, and how high it sits, in metres — the taper worked
/// out in one place so that the mesh and nothing else decides it.
///
/// A course is a drum of constant width, and the width is the pillar's at the
/// middle of the band it fills, so the taper comes out as steps rather than as
/// a smooth cone. That is what a stacked pillar does: each course is whatever
/// the stones in it are, and it is the *stack* that narrows.
fn course(nth: usize) -> (f32, f32) {
    let deep = PILLAR / COURSES as f32;
    let (foot, crown) = PILLAR_WIDTH;
    let up = (nth as f32 + 0.5) / COURSES as f32;
    (foot + (crown - foot) * up, deep * (nth as f32 + 0.5))
}

/// Stacks the courses on a cairn that has just been heard of — see
/// [`Cairns::told`], which puts up the bare thing.
fn dress(mut commands: Commands, mut kit: CairnKit, raised: Query<(Entity, &Cairn), Added<Cairn>>) {
    if raised.is_empty() {
        return;
    }
    let stonework = kit
        .stonework
        .get_or_insert_with(|| Stonework {
            shades: std::array::from_fn(|nth| kit.materials.add(matte(STONE[nth]))),
            // Flat-shaded, which is the world's own language, and unwelded
            // first: flat normals cannot be computed over shared vertices,
            // which is a panic rather than a warning and does not show up
            // until something actually builds the mesh.
            courses: std::array::from_fn(|nth| {
                let (across, _) = course(nth);
                kit.meshes.add(
                    Cylinder::new(across / 2.0, PILLAR / COURSES as f32)
                        .mesh()
                        .resolution(FACES)
                        .build()
                        .with_duplicated_vertices()
                        .with_computed_flat_normals(),
                )
            }),
        })
        .clone();

    for (cairn, island) in &raised {
        commands.entity(cairn).with_children(|children| {
            for nth in 0..COURSES {
                let (_, up) = course(nth);
                // The island's own id and the course's place in the stack, so
                // that one island is stacked the same way on every machine and
                // two islands are not stacked alike — see [`LEAN`].
                let bits = (island.island.x as u32)
                    ^ (island.island.y as u32).rotate_left(16)
                    ^ (nth as u32).wrapping_mul(0x9E37_79B9);
                let off = Vec2::new(signed(bits, SALT_LEAN), signed(bits, SALT_LEAN ^ 1)) * LEAN;
                children.spawn((
                    Name::new(format!("Course {nth}")),
                    Mesh3d(stonework.courses[nth].clone()),
                    MeshMaterial3d(
                        stonework.shades[between(bits, SALT_STONE, (0, STONE.len() - 1))].clone(),
                    ),
                    Transform::from_xyz(off.x, up, off.y)
                        .with_rotation(Quat::from_rotation_y(unit(bits, SALT_SKEW) * SKEW)),
                ));
            }
        });
    }
}

/// Stands the cairns on the ground once there is ground to stand them on, and
/// shows them the moment they are standing on it.
///
/// The arrangement an arriving walker is put down by — see [`crate::player`]'s
/// own settling — and for the same reason: the point the world named is on the
/// plane, and what height that is depends on ground this client may not have
/// yet. The showing is the same frame as the settling and not a moment later —
/// the whole reason a cairn is hidden is that it would otherwise be drawn
/// somewhere it is not.
fn stand_the_cairns(mut commands: Commands, ground: Option<Res<Ground>>, mut waiting: Waiting) {
    let Some(ground) = ground else {
        return;
    };
    for (cairn, mut place, mut shown) in &mut waiting {
        if let Some(height) = ground.height(place.translation.x, place.translation.z) {
            place.translation.y = height;
            *shown = Visibility::Inherited;
            commands.entity(cairn).remove::<Unfooted>();
        }
    }
}

/// Forgets every cairn: the world is over. The entities take themselves out,
/// being `DespawnOnExit`; what is cleared here is this side of the
/// bookkeeping, which would otherwise hand out the entity ids of a world
/// nobody is in any more.
pub(crate) fn strike(mut cairns: ResMut<Cairns>) {
    cairns.standing.clear();
}

/// Stands a stone wherever the world says one is.
///
/// The other half of the same word is [`crate::chart`], which letters the
/// sheet with what the island is called: a name is something the world
/// carries, so the two hear one sentence and neither had to be told about the
/// other.
pub(crate) fn raise_the_cairns(
    mut commands: Commands,
    mut cairns: ResMut<Cairns>,
    mut seen: MessageReader<crate::net::CairnSeen>,
) {
    for cairn in seen.read() {
        cairns.told(&mut commands, cairn.island, cairn.at);
    }
}

pub struct CairnPlugin;

impl Plugin for CairnPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<crate::net::CairnSeen>()
            .init_resource::<Cairns>()
            .add_systems(
                Update,
                // Before the two below, so a stone told this frame is dressed
                // and stood on its ground in the same one.
                raise_the_cairns.in_set(crate::net::Wire::Read),
            )
            .add_systems(
                Update,
                // Neither of these is the player's hands, so neither pauses: a
                // cairn whose ground arrived while the game was paused should
                // be standing on it when they look back.
                (dress, stand_the_cairns)
                    .after(raise_the_cairns)
                    .run_if(in_state(AppState::InWorld)),
            )
            .add_systems(OnExit(AppState::InWorld), strike);
    }
}

#[cfg(test)]
mod tests {
    use bevy::asset::AssetPlugin;
    use bevy::ecs::system::SystemState;
    use bevy::state::app::StatesPlugin;
    use bevy::time::{TimePlugin, TimeUpdateStrategy};

    use super::*;
    use crate::testing::{test_ground, FRAME};

    /// A headless app with the cairn systems and nothing else: no ground until
    /// a test hands some over, which is the state a cairn told from further
    /// off than terrain streams actually arrives in.
    fn cairn_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            TaskPoolPlugin::default(),
            AssetPlugin::default(),
            TimePlugin,
            StatesPlugin,
            CairnPlugin,
        ))
        .insert_resource(TimeUpdateStrategy::ManualDuration(FRAME))
        .init_state::<AppState>()
        .init_asset::<Mesh>()
        .init_resource::<Assets<StandardMaterial>>();
        app.update();
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::InWorld);
        app.update();
        app
    }

    /// A word about a cairn, exactly as the session's drain delivers one —
    /// through `Commands`, which is why [`Cairns`] keeps its own map at all.
    fn tell(app: &mut App, island: IVec2, at: Vec2) {
        let mut state: SystemState<(Commands, ResMut<Cairns>)> = SystemState::new(app.world_mut());
        let (mut commands, mut cairns) = state.get_mut(app.world_mut()).expect("the drain's hands");
        cairns.told(&mut commands, island, at);
        state.apply(app.world_mut());
    }

    /// Every cairn standing, with how many courses are stacked on it and
    /// whether it is shown.
    fn standing(app: &mut App) -> Vec<(IVec2, usize, Visibility, Vec3)> {
        app.world_mut()
            .query::<(&Cairn, &Children, &Visibility, &Transform)>()
            .iter(app.world())
            .map(|(cairn, children, shown, place)| {
                (cairn.island, children.len(), *shown, place.translation)
            })
            .collect()
    }

    /// Where each course of one cairn sits, and how wide it is — read off the
    /// transforms and the meshes, which is what the drawing actually uses.
    fn courses(app: &mut App) -> Vec<Transform> {
        let mut found: Vec<_> = app
            .world_mut()
            .query_filtered::<(&Name, &Transform), With<Mesh3d>>()
            .iter(app.world())
            .map(|(name, place)| (name.to_string(), *place))
            .collect();
        found.sort_by(|(a, _), (b, _)| a.cmp(b));
        found.into_iter().map(|(_, place)| place).collect()
    }

    #[test]
    fn a_second_word_about_one_cairn_builds_nothing_new() {
        // A cairn is told again when it is christened, and a christening is a
        // word about the *sheet*: the stones are where they were, and dressing
        // them twice would leave one cairn wearing eight courses.
        let mut app = cairn_app();
        let island = IVec2::new(76, 255);
        tell(&mut app, island, Vec2::new(120.0, -40.0));
        app.update();
        assert_eq!(
            standing(&mut app),
            vec![(
                island,
                COURSES,
                Visibility::Hidden,
                Vec3::new(120.0, 0.0, -40.0)
            )],
            "the courses did not go up as one pillar"
        );

        tell(&mut app, island, Vec2::new(120.0, -40.0));
        app.update();
        let after = standing(&mut app);
        assert_eq!(after.len(), 1, "a christening raised a second cairn");
        assert_eq!(
            after[0].1, COURSES,
            "a christening stacked the pillar again"
        );
    }

    #[test]
    fn the_courses_stack_up_the_pillar_and_step_in_as_they_go() {
        // The one piece of arithmetic in the module. A course sits at the
        // middle of the band it fills and is as wide as the pillar is there,
        // so the stack rises without a gap and narrows without a jump — and
        // no course may be so far off its axis that it overhangs the one
        // under it, which is a pillar that has fallen over.
        let mut app = cairn_app();
        tell(&mut app, IVec2::new(-9, 4), Vec2::ZERO);
        app.update();

        let deep = PILLAR / COURSES as f32;
        let stacked = courses(&mut app);
        assert_eq!(stacked.len(), COURSES, "the pillar is not COURSES high");
        for (nth, place) in stacked.iter().enumerate() {
            let (across, up) = course(nth);
            assert!(
                (place.translation.y - up).abs() < 1e-5,
                "course {nth} sits at {}, not {up}",
                place.translation.y
            );
            assert!(
                place.translation.xz().length() < LEAN * 1.5,
                "course {nth} leans {} metres off the pillar",
                place.translation.xz().length()
            );
            if nth > 0 {
                let (under, _) = course(nth - 1);
                assert!(
                    across < under,
                    "course {nth} is no narrower than the one under it"
                );
            }
        }
        assert!(
            (stacked.last().expect("a course").translation.y + deep / 2.0 - PILLAR).abs() < 1e-5,
            "the crown does not come out at PILLAR"
        );
    }

    #[test]
    fn two_islands_are_not_stacked_the_same_way() {
        // The wobble is dealt from the island's own id, so one island is the
        // same pillar on every machine and two islands are different ones.
        // Dealt from a constant it would be one pillar repeated across the
        // archipelago, which is what a single flat cone looked like.
        let mut app = cairn_app();
        tell(&mut app, IVec2::new(1, 1), Vec2::ZERO);
        tell(&mut app, IVec2::new(-40, 17), Vec2::new(500.0, 0.0));
        app.update();

        let leans: Vec<_> = app
            .world_mut()
            .query_filtered::<&Transform, With<Mesh3d>>()
            .iter(app.world())
            .map(|place| (place.translation.xz(), place.rotation))
            .collect();
        assert_eq!(
            leans.len(),
            COURSES * 2,
            "two pillars, COURSES courses each"
        );
        assert!(
            leans[..COURSES] != leans[COURSES..],
            "both islands stacked their stones identically"
        );
    }

    #[test]
    fn a_cairn_waits_unseen_until_there_is_ground_to_stand_on() {
        // Told first and drawn later: a cairn whose chunk has not landed yet
        // is not a cairn standing on the open sea — see the module doc.
        let mut app = cairn_app();
        let island = IVec2::new(3, -2);
        let at = Vec2::new(50.0, 0.0);
        tell(&mut app, island, at);
        app.update();
        assert_eq!(
            standing(&mut app),
            vec![(
                island,
                COURSES,
                Visibility::Hidden,
                Vec3::new(at.x, 0.0, at.y)
            )],
            "a cairn was drawn standing on the sea"
        );

        // The ground turns up: the same frame settles it and shows it.
        let ground = test_ground();
        let height = ground.height(at.x, at.y).expect("the test island");
        app.insert_resource(ground);
        app.update();
        assert_eq!(
            standing(&mut app),
            vec![(
                island,
                COURSES,
                Visibility::Inherited,
                Vec3::new(at.x, height, at.y)
            )],
            "the cairn never took its footing"
        );
        assert!(
            app.world_mut()
                .query_filtered::<(), With<Unfooted>>()
                .iter(app.world())
                .next()
                .is_none(),
            "a footed cairn is still queued for ground"
        );
    }

    #[test]
    fn leaving_the_world_forgets_the_cairns() {
        // The entities go with the state, being `DespawnOnExit`; what has to
        // be said here is that the bookkeeping goes too, or the next world's
        // first telling would find an entity id from the last one.
        let mut app = cairn_app();
        let island = IVec2::new(1, 1);
        tell(&mut app, island, Vec2::ZERO);
        app.update();

        for state in [AppState::MainMenu, AppState::InWorld] {
            app.world_mut()
                .resource_mut::<NextState<AppState>>()
                .set(state);
            app.update();
        }
        assert!(standing(&mut app).is_empty(), "a cairn outlived its world");

        tell(&mut app, island, Vec2::new(9.0, 9.0));
        app.update();
        assert_eq!(
            standing(&mut app),
            vec![(
                island,
                COURSES,
                Visibility::Hidden,
                Vec3::new(9.0, 0.0, 9.0)
            )],
            "the next world's cairn went looking for the last one's entity"
        );
    }
}
