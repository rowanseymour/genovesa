//! The cairn: what a claim leaves standing on the ground.
//!
//! Claiming an island is the server's to grant — it asks its own survey
//! whether this player has been the whole way round this coast and is standing
//! inside it, and nothing a client says about that is believed. What lands on
//! this side is the answer: [`protocol::ToClient::Cairn`], told to whoever
//! comes near one, the same way a beast or a boat is. This module is the half
//! the server has no opinion on — what being claimed *looks like* from the
//! water.
//!
//! # Why it is a thing and not a widget
//!
//! The obvious way to show an island is spoken for is a label floating over
//! it, and it is the wrong way. A player arriving at an island has their eyes
//! on the island; a marker drawn in screen space says *the game* is telling
//! you something, where a stone and a flag on a headland says *somebody was
//! here*, which is the whole of what a claim means. It also has to survive
//! being looked at from off the coast, at sea level, from a boat that is
//! moving — so it is built as a real daymark is built: a cairn of stone for
//! the mass, a staff for the height, and a banner for the movement, because at
//! any distance the eye finds the thing that *moves* long before it finds the
//! thing that is merely tall. How far it actually carries is [`STAFF`]'s
//! business, and less far than the first draft of this paragraph claimed.
//!
//! The banner streams on the true wind, on the same arithmetic a boat's
//! pennant uses — see [`crate::boat::pennant_pose`]. Cloth is cloth, and two
//! rules for how it lies would show up the first time a player anchored off a
//! cairn and watched their own masthead disagree with it. The *shape* is its
//! own ([`banner_mesh`]), cut to the convention that pose aims things in: a
//! tie at the origin, the cloth running down -Z and hanging down -Y. Built to
//! any other convention it would be aimed across the wind rather than along
//! it, which is what a rectangle from the shape library did.
//!
//! # Standing it on the ground
//!
//! A cairn arrives as a point on the plane, and the height it stands at is
//! this side's own business: the server has no camera and no need of one.
//! Ground arrives in chunks and a cairn can be told of before the ground under
//! it has come — the server tells them from further off than a client streams
//! terrain, deliberately, so a daymark is in sight before the shore it stands
//! on resolves. So a cairn waits [`Unfooted`] until there is ground to stand
//! on, exactly as an arriving player does.
//!
//! It waits *unseen*. The gap between the two distances is half a kilometre of
//! sailing, and a cairn drawn at its told point before its ground arrives is a
//! cairn standing on the open sea for the whole of the approach, which then
//! jumps onto the headland as the shore streams in. Hidden until it is footed,
//! the daymark simply appears with the island it belongs to.

use std::collections::HashMap;

use bevy::prelude::*;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::PrimitiveTopology;

use crate::boat::pennant_pose;
use crate::sea::SeaConditions;
use crate::terrain::Ground;
use crate::{matte, AppState};

/// How tall the staff stands above the stones, in metres.
///
/// A daymark is a thing to be seen from off the coast, and this is the number
/// that decides from how far. It was nine metres — a human-scale flagstaff —
/// until somebody stood one up and looked at it: against a camera that sits
/// hundreds of metres off and terrain drawn in facets tens of metres across,
/// nine metres is a pin on a golf green. Twenty is a beacon, which is what
/// this is: not a flagpole somebody planted but a mark built to be found.
///
/// It does not carry a mile. Nothing built at a size this world could believe
/// would — at a kilometre a cloth this size is a few pixels of colour, which
/// is enough to notice and not enough to read. What carries at that range is
/// the mark on the chart, which is the survey's business and not this one's.
const STAFF: f32 = 20.0;

/// How deep the staff is driven into the heap, in metres — a staff resting on
/// the stones would be a staff the first blow took away. What it costs is that
/// the head stands this much lower than [`STAFF`] above the stones, which is
/// why the height of the head is worked out once rather than written twice.
const SUNK: f32 = 0.8;

/// How far below the head of the staff the banner is tied, in metres. The
/// staff showing above the cloth is what says *staff* rather than *pole with a
/// flag glued on the end*.
const TIE_BELOW: f32 = 1.2;

/// The cairn of stones at its foot: how far across the base is, and how high
/// it is heaped, in metres.
///
/// Wide enough to read as built rather than dropped, and low enough that the
/// staff is plainly the tall part. Six metres across is a heap somebody spent
/// a day on, which is the right amount of work for a thing that says *this one
/// is mine*: at the two metres it started out as it read as a stone somebody
/// tripped over rather than as anything anybody meant.
const STONES: (f32, f32) = (6.0, 3.0);

/// The banner: how far it flies from the staff, and how deep it hangs, in
/// metres.
///
/// Big enough to be the thing the eye catches on the approach, which makes it
/// far larger — and far deeper in proportion — than the pennant at a masthead.
/// A masthead flag is a narrow streamer because it is read for its
/// *direction*, by its own crew, from ten metres. This one is read by a
/// stranger, from as far off as it carries, for being there at all: a streamer
/// at that range is a thread, so this is a flag.
const BANNER: (f32, f32) = (7.0, 3.6);

/// The stone a cairn is piled from. The world's rock, near enough: a cairn is
/// built out of whatever the island had, and an island's high ground is scree.
const STONE_COLOR: Color = Color::srgb(0.55, 0.52, 0.48);

/// The staff, which is driftwood or ship's timber — either way, wood that has
/// been in the weather.
const STAFF_COLOR: Color = Color::srgb(0.62, 0.51, 0.36);

/// The banner. Not the pennant's ochre: a pennant says *whose boat*, and this
/// says *somebody claimed this*, so they should not be mistaken for each other
/// at a distance. Deep red reads against sky, sea, sand and green alike, which
/// is what a daymark has to do.
const BANNER_COLOR: Color = Color::srgb(0.72, 0.16, 0.16);

/// How fast the banner swings onto a shifted wind, in radians a second.
///
/// Slower than a pennant, because it is bigger cloth on a fixed staff with no
/// hull swinging under it: the only motion it has is the wind's own, so the
/// wind's own is what has to read.
const SWING: f32 = 2.0;

/// The cairns this client has been told of, by the entity standing for each.
///
/// Keyed by the island, which is a ring's identity and so the same pair of
/// numbers on every machine — see [`protocol::survey::Island::id`]. Kept for
/// the reason [`crate::beasts::Beasts`] keeps its own: entities spawn through
/// `Commands`, so two words about one cairn in a single frame's drain would
/// otherwise go looking for an entity that is still a queued command.
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
    /// beasts' arrangement and for their reason: this is called from the
    /// session's drain, which already has both hands on the hulls' meshes, and
    /// two system parameters cannot each hold the asset store.
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
/// daymark as your own, and one that announced itself by its colour would be a
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

/// The banner on the staff, and the bearing it is streaming on.
///
/// Its own, rather than read back off the transform, for the reason a
/// pennant's is: a calm has to leave the cloth where the last of the wind put
/// it rather than snap it to somewhere new.
#[derive(Component)]
struct Banner {
    bearing: f32,
}

/// What building a cairn needs in hand — the two asset stores and the pieces
/// cut from them — bundled so that [`dress`] is one system parameter rather
/// than three. Nothing else builds a cairn: the telling arrives in
/// [`crate::net::receive`], which already has both hands on the hulls' kit and
/// so cannot hold these too, and putting the bare stones up is all it does.
#[derive(bevy::ecs::system::SystemParam)]
struct CairnKit<'w, 's> {
    meshes: ResMut<'w, Assets<Mesh>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
    /// The pieces every cairn is built from, made once and cloned per cairn:
    /// an archipelago somebody has worked their way through is one banner
    /// mesh, not forty.
    stonework: Local<'s, Option<Stonework>>,
}

/// The handles [`dress`] deals from — see [`CairnKit::stonework`].
#[derive(Clone)]
struct Stonework {
    stone: Handle<StandardMaterial>,
    timber: Handle<StandardMaterial>,
    cloth: Handle<StandardMaterial>,
    heap: Handle<Mesh>,
    staff: Handle<Mesh>,
    banner: Handle<Mesh>,
}

/// Builds the stones, the staff and the banner on a cairn that has just been
/// heard of — see [`Cairns::told`], which puts up the bare thing.
fn dress(mut commands: Commands, mut kit: CairnKit, raised: Query<Entity, Added<Cairn>>) {
    if raised.is_empty() {
        return;
    }
    let (across, high) = STONES;
    let (flies, hangs) = BANNER;
    let stonework = kit
        .stonework
        .get_or_insert_with(|| Stonework {
            stone: kit.materials.add(matte(STONE_COLOR)),
            timber: kit.materials.add(matte(STAFF_COLOR)),
            // Drawn from both faces, as the boat's cloth is: a banner left
            // single-sided would wink out every time the wind put its back to
            // the camera, which for a fixed staff is half of every day.
            cloth: kit.materials.add(StandardMaterial {
                double_sided: true,
                cull_mode: None,
                ..matte(BANNER_COLOR)
            }),
            // A cone rather than a dome: heaped stone stands at the angle
            // loose rock stands at, and the flat facets of a low-sided cone
            // are what a pile of rock looks like in a world with no textures
            // in it.
            // Seven sides and its own normals per facet, which is the world's
            // own language: everything here is flat-shaded, and the shape
            // library's default cone is smooth enough to read as a grey egg
            // sitting on faceted ground. The vertices are unwelded first —
            // flat normals cannot be computed over shared ones, which is a
            // panic rather than a warning and does not show up until
            // something actually builds the mesh.
            heap: kit.meshes.add(
                Cone::new(across / 2.0, high)
                    .mesh()
                    .resolution(7)
                    .build()
                    .with_duplicated_vertices()
                    .with_computed_flat_normals(),
            ),
            staff: kit.meshes.add(Cylinder::new(0.09, STAFF)),
            // The masthead's own cloth at another size, and it has to be: the
            // pose it is aimed by is the pennant's — tie at the origin, cloth
            // down -Z and hanging -Y — and a rectangle from the shape library
            // lies in the XY plane about its own middle, which is a banner
            // aimed across the wind, straddling the staff, and swinging flat
            // into the horizontal every time the wind drops.
            banner: kit.meshes.add(banner_mesh(flies, hangs)),
        })
        .clone();

    // The head of the staff, which is what the banner is tied below and the
    // one height in a cairn that is worked out rather than written down.
    let head = high + STAFF - SUNK;
    for cairn in &raised {
        commands.entity(cairn).with_children(|children| {
            children.spawn((
                Name::new("Stones"),
                Mesh3d(stonework.heap.clone()),
                MeshMaterial3d(stonework.stone.clone()),
                Transform::from_xyz(0.0, high / 2.0, 0.0),
            ));
            children.spawn((
                Name::new("Staff"),
                Mesh3d(stonework.staff.clone()),
                MeshMaterial3d(stonework.timber.clone()),
                // Standing in the heap rather than on it — see [`SUNK`].
                Transform::from_xyz(0.0, head - STAFF / 2.0, 0.0),
            ));
            children.spawn((
                Name::new("Banner"),
                Banner { bearing: 0.0 },
                Mesh3d(stonework.banner.clone()),
                MeshMaterial3d(stonework.cloth.clone()),
                // The tie, which is where the cloth is made fast and so the
                // only part of the banner that stays put: the rest of it is
                // [`fly_the_banner`]'s, and hangs and streams from here.
                Transform::from_xyz(0.0, head - TIE_BELOW, 0.0),
            ));
        });
    }
}

/// Stands the cairns on the ground once there is ground to stand them on, and
/// shows them the moment they are standing on it.
///
/// The cloth: a square-ended banner, cut to the convention
/// [`pennant_pose`] aims things in — tied at the origin, flying down -Z and
/// hanging down -Y.
///
/// Its own shape rather than the pennant's, though it borrows the pennant's
/// arithmetic and its belly. A pennant is tapered, and a tapered flag on a
/// staff is a *pennant*: on a stone heap it reads as a pin on a golf green,
/// which is a thing this world spent a screenshot finding out. A claim is a
/// flag planted, so the cloth is square-ended and deep, and reads as one.
///
/// The belly is why this is nine vertices rather than six: a flat quad edge-on
/// to the camera disappears, and a cloth with a curve in it catches the light
/// on one side. Unindexed, so each facet keeps its own normal — the flat
/// shading everything here is drawn in.
fn banner_mesh(flies: f32, hangs: f32) -> Mesh {
    let tie = Vec3::ZERO;
    let foot = Vec3::new(0.0, -hangs, 0.0);
    let head = Vec3::new(0.0, 0.0, -flies);
    let clew = Vec3::new(0.0, -hangs, -flies);
    // Out to one side, deepest around the middle of the cloth, by a tenth of
    // the fly — the pennant's proportion, on a bigger flag.
    let belly = Vec3::new(flies * 0.1, -hangs * 0.5, -flies * 0.5);

    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![
            tie, foot, belly, foot, clew, belly, clew, head, belly, head, tie, belly,
        ],
    )
    .with_computed_flat_normals()
}

/// and for the same reason: the point the world named is on the plane, and
/// what height that is depends on ground this client may not have yet. The
/// showing is the same frame as the settling and not a moment later — the
/// whole reason a cairn is hidden is that it would otherwise be drawn
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

/// Streams the banners on the wind.
///
/// The true wind, not an apparent one: a cairn has no way through the water to
/// take out of it, which is the whole difference between this and a masthead.
/// Every banner on every island lies the same way at the same moment, and a
/// player who has learned to read one has learned to read the weather.
fn fly_the_banner(
    time: Res<Time>,
    conditions: Res<SeaConditions>,
    mut banners: Query<(&mut Banner, &mut Transform)>,
) {
    let wind = conditions.wind();
    for (mut banner, mut place) in &mut banners {
        let (wanted, droop) = pennant_pose(wind, banner.bearing);
        // Swung towards the wind rather than snapped onto it — see [`SWING`].
        // Taken the short way round, or a wind crossing north would send every
        // banner in the archipelago the long way about at once.
        let swing = (wanted - banner.bearing + std::f32::consts::PI)
            .rem_euclid(std::f32::consts::TAU)
            - std::f32::consts::PI;
        banner.bearing += swing * (SWING * time.delta_secs()).min(1.0);

        place.rotation = Quat::from_rotation_y(banner.bearing) * Quat::from_rotation_x(-droop);
    }
}

/// Forgets every cairn: the world is over. The entities take themselves out,
/// being `DespawnOnExit`; what is cleared here is this side of the
/// bookkeeping, which would otherwise hand out the entity ids of a world
/// nobody is in any more.
///
/// Run from [`crate::net::NetPlugin`] as well as from this one, on the fleet's
/// terms: `receive` is what writes this resource, so an app with the net
/// plugin and not this one must still not carry one world's cairns into the
/// next. Clearing an empty map twice costs nothing.
pub(crate) fn strike(mut cairns: ResMut<Cairns>) {
    cairns.standing.clear();
}

pub struct CairnPlugin;

impl Plugin for CairnPlugin {
    fn build(&self, app: &mut App) {
        // Also initialised by `NetPlugin`, whose `receive` writes into it;
        // initialising a resource twice is free, and each plugin's tests run
        // it alone.
        app.init_resource::<Cairns>()
            .init_resource::<SeaConditions>()
            .add_systems(
                Update,
                // None of this is the player's hands, so none of it pauses:
                // the wind does not stop blowing because somebody opened a
                // menu, and a cairn whose ground arrived while the game was
                // paused should be standing on it when they look back.
                (dress, stand_the_cairns, fly_the_banner).run_if(in_state(AppState::InWorld)),
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
    use crate::testing::{set_wind, test_ground, FRAME};

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

    /// Every cairn standing, with what is built on it and whether it is shown.
    fn standing(app: &mut App) -> Vec<(IVec2, usize, Visibility, Vec3)> {
        app.world_mut()
            .query::<(&Cairn, &Children, &Visibility, &Transform)>()
            .iter(app.world())
            .map(|(cairn, children, shown, place)| {
                (cairn.island, children.len(), *shown, place.translation)
            })
            .collect()
    }

    #[test]
    fn a_second_word_about_one_cairn_builds_nothing_new() {
        // A cairn is told again when it is christened, and a christening is a
        // word about the *sheet*: the stones are where they were, and dressing
        // them twice would leave one cairn wearing two staffs and two banners.
        let mut app = cairn_app();
        let island = IVec2::new(76, 255);
        tell(&mut app, island, Vec2::new(120.0, -40.0));
        app.update();
        assert_eq!(
            standing(&mut app),
            vec![(island, 3, Visibility::Hidden, Vec3::new(120.0, 0.0, -40.0))],
            "the stones, the staff and the banner did not go up as one cairn"
        );

        tell(&mut app, island, Vec2::new(120.0, -40.0));
        app.update();
        let after = standing(&mut app);
        assert_eq!(after.len(), 1, "a christening raised a second cairn");
        assert_eq!(after[0].1, 3, "a christening dressed the cairn again");
    }

    #[test]
    fn a_cairn_waits_unseen_until_there_is_ground_to_stand_on() {
        // The half-kilometre between what the server tells and what the client
        // streams: told first, and drawn only once the shore it stands on has
        // arrived — see the module doc.
        let mut app = cairn_app();
        let island = IVec2::new(3, -2);
        let at = Vec2::new(50.0, 0.0);
        tell(&mut app, island, at);
        app.update();
        assert_eq!(
            standing(&mut app),
            vec![(island, 3, Visibility::Hidden, Vec3::new(at.x, 0.0, at.y))],
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
                3,
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
    fn a_banner_swings_the_short_way_round_a_wind_crossing_north() {
        // The only arithmetic in the module. A banner lying just west of north
        // and a wind gone just east of it are a tenth of a turn apart; taken
        // as a raw difference they are nine tenths, and every banner in the
        // archipelago sweeps the long way round at once.
        let mut app = cairn_app();
        tell(&mut app, IVec2::ZERO, Vec2::ZERO);
        app.update();

        let lying = 3.0;
        let mut banners = app.world_mut().query::<&mut Banner>();
        banners
            .single_mut(app.world_mut())
            .expect("a cairn flies one banner")
            .bearing = lying;
        // A wind whose cloth wants to lie at -3.0 radians — a tenth of a turn
        // the other side of the cut, and hard enough that the pose is a
        // bearing rather than a calm holding the old one.
        let wanted = -3.0_f32;
        let strong = 8.0;
        set_wind(
            &mut app,
            Vec2::new(-wanted.sin() * strong, -wanted.cos() * strong),
        );
        app.update();

        let swung = app
            .world_mut()
            .query::<&Banner>()
            .single(app.world())
            .expect("a cairn flies one banner")
            .bearing;
        assert!(
            swung > lying,
            "the banner went the long way about: {lying} to {swung}"
        );
        assert!(
            swung - lying < 0.05,
            "the banner swung {} radians in a frame",
            swung - lying
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
            vec![(island, 3, Visibility::Hidden, Vec3::new(9.0, 0.0, 9.0))],
            "the next world's cairn went looking for the last one's entity"
        );
    }
}
