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
//! being looked at from a mile out, at sea level, from a boat that is moving —
//! so it is built as a real daymark is built: a cairn of stone for the mass, a
//! staff for the height, and a banner for the movement, because at that
//! distance the eye finds the thing that *moves* long before it finds the
//! thing that is merely tall.
//!
//! The banner streams on the true wind, on the same arithmetic a boat's
//! pennant uses — see [`crate::boat::pennant_pose`]. Cloth is cloth, and two
//! rules for how it lies would show up the first time a player anchored off a
//! cairn and watched their own masthead disagree with it.
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

use std::collections::HashMap;

use bevy::prelude::*;

use crate::boat::pennant_pose;
use crate::sea::SeaConditions;
use crate::terrain::Ground;
use crate::{matte, AppState};

/// How tall the staff stands above the stones, in metres.
///
/// A daymark is a thing to be seen from off the coast, and this is the number
/// that decides from how far. Nine metres puts the banner above the palms it
/// will usually be standing among — which is the point of a daymark rather
/// than a landmark — without making it the tallest thing on a small island.
const STAFF: f32 = 9.0;

/// The cairn of stones at its foot: how far across the base is, and how high
/// it is heaped, in metres.
///
/// Wide enough to read as built rather than dropped, and low enough that the
/// staff is plainly the tall part. It is also what a player walks up to, so it
/// is on the scale of a thing somebody could have piled by hand.
const STONES: (f32, f32) = (2.4, 1.6);

/// The banner: how far it flies from the staff, and how deep it hangs, in
/// metres.
///
/// Big enough to be the thing the eye catches at a mile — see the module doc —
/// which makes it far larger, in proportion, than the pennant at a masthead.
/// A pennant is read by its own crew from ten metres; this is read by a
/// stranger from a thousand.
const BANNER: (f32, f32) = (2.6, 1.4);

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
    /// A word about a cairn: the first one builds it, and later ones are the
    /// same stones with a new word on them — a christening, or a claim that
    /// changed hands. Where it *stands* never changes, a cairn being a pile of
    /// rock rather than a thing that moves, so a later telling only rewrites
    /// what the sheet says about it.
    /// The stones go up bare here and are [`dress`]ed a moment later, which is
    /// the beasts' arrangement and for their reason: this is called from the
    /// session's drain, which already has both hands on the hulls' meshes, and
    /// two system parameters cannot each hold the asset store.
    pub fn told(&mut self, commands: &mut Commands, island: IVec2, at: Vec2, yours: bool) {
        if let Some(&standing) = self.standing.get(&island) {
            commands.entity(standing).insert(Cairn { island, yours });
            return;
        }
        let cairn = commands
            .spawn((
                Name::new(format!("Cairn {}, {}", island.x, island.y)),
                Cairn { island, yours },
                Unfooted,
                DespawnOnExit(AppState::InWorld),
                Transform::from_xyz(at.x, 0.0, at.y),
                Visibility::default(),
            ))
            .id();
        self.standing.insert(island, cairn);
    }

    /// Forgets every cairn: the world is over. The entities take themselves
    /// out, being `DespawnOnExit`; what is cleared here is this side of the
    /// bookkeeping, which would otherwise hand out the entity ids of a world
    /// nobody is in any more.
    fn forget(&mut self) {
        self.standing.clear();
    }
}

/// One cairn, as this client draws it.
#[derive(Component)]
pub struct Cairn {
    /// The island it speaks for.
    pub island: IVec2,
    /// Whether the player is the one who built it. Nothing in the world is
    /// drawn differently for it — a stranger's cairn is exactly as much of a
    /// daymark as your own, and a claim that announced itself by its colour
    /// would be a claim nobody had to sail up to. It is the *sheet* that
    /// cares: see [`crate::chart`].
    pub yours: bool,
}

/// A cairn waiting for ground to stand on — see the module doc.
#[derive(Component)]
struct Unfooted;

/// The cairns still waiting for ground, as a query.
type Waiting<'w, 's> =
    Query<'w, 's, (Entity, &'static mut Transform), (With<Cairn>, With<Unfooted>)>;

/// The banner on the staff, and the bearing it is streaming on.
///
/// Its own, rather than read back off the transform, for the reason a
/// pennant's is: a calm has to leave the cloth where the last of the wind put
/// it rather than snap it to somewhere new.
#[derive(Component)]
struct Banner {
    bearing: f32,
}

/// What building a cairn needs in hand, bundled because the telling arrives
/// inside [`crate::net::receive`], which is already holding the hulls' kit.
#[derive(bevy::ecs::system::SystemParam)]
pub struct CairnKit<'w, 's> {
    meshes: ResMut<'w, Assets<Mesh>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
    /// The pieces every cairn is built from, made once and cloned per cairn:
    /// an archipelago somebody has worked their way through is one banner
    /// mesh, not forty.
    stonework: Local<'s, Option<Stonework>>,
}

/// The handles [`raise`] deals from — see [`CairnKit::stonework`].
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
            heap: kit.meshes.add(Cone::new(across / 2.0, high)),
            staff: kit.meshes.add(Cylinder::new(0.09, STAFF)),
            banner: kit.meshes.add(Rectangle::new(flies, hangs)),
        })
        .clone();

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
                // Standing in the heap rather than on it: a staff resting on
                // the stones would be a staff the first blow took away.
                Transform::from_xyz(0.0, high + STAFF / 2.0 - 0.3, 0.0),
            ));
            children.spawn((
                Name::new("Banner"),
                Banner { bearing: 0.0 },
                Mesh3d(stonework.banner.clone()),
                MeshMaterial3d(stonework.cloth.clone()),
                // Aimed by [`fly_the_banner`]; the height is the only part of
                // this that stays put. Hung a little below the head so the
                // staff shows above it, which is what says *staff* rather
                // than *pole with a flag glued on the end*.
                Transform::from_xyz(0.0, high + STAFF - hangs, 0.0),
            ));
        });
    }
}

/// Stands the cairns on the ground once there is ground to stand them on.
///
/// The same job [`crate::player::find_footing`] does for an arriving player,
/// and for the same reason: the point the world named is on the plane, and
/// what height that is depends on ground this client may not have yet.
fn stand_the_cairns(mut commands: Commands, ground: Option<Res<Ground>>, mut waiting: Waiting) {
    let Some(ground) = ground else {
        return;
    };
    for (cairn, mut place) in &mut waiting {
        if let Some(height) = ground.height(place.translation.x, place.translation.z) {
            place.translation.y = height;
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

/// Clears the bookkeeping on the way out of a world — see [`Cairns::forget`].
fn strike(mut cairns: ResMut<Cairns>) {
    cairns.forget();
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
