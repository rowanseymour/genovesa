//! The ground tackle: the anchor on the bottom and the cable up to the bow,
//! drawn for every hull with its hook down — see [`Anchored`] — and the
//! painter from a ship's taffrail to the stem of the boat it tows, drawn for
//! every hull on one — see [`Towed`].
//!
//! What a player wants to see is *where the hook is*. A hull at anchor looks
//! exactly like one adrift until the wind shifts and it fails to go with it,
//! and where the hook lies is what says how far it may swing and which way.
//! So the anchor itself is drawn at the hook, on the bottom, and the cable is
//! drawn all the way down to it rather than stopping at the water: the sea is
//! clear over the ground an anchor holds in — [`crate::sea`]'s murk goes
//! blind at exactly that depth, on purpose — and a cable seen going down and
//! along the bottom points at the hook from the deck.
//!
//! The cable hangs the way a chain does. Under a light pull a chain lies
//! along the bottom from the anchor and lifts into a catenary to the bow, and
//! the whole shape is one number: the catenary's parameter, the horizontal
//! pull over the chain's weight per metre. It is held at [`SLACK`] for a hull
//! lying easy, which lays a run of chain on the bottom whenever the hull has
//! swung out far enough for one; nearer the hook there is not the run for it,
//! and the parameter is found instead as the one that has the chain leaving
//! the ring exactly along the bottom. Either way the chain never pulls *up*
//! on the anchor, which is what an anchor that holds looks like, and the
//! cable is a curve down into the water rather than a bar.
//!
//! The painter is rope, and rope floats: slack, it sags off the straight line
//! between its ends by as much as the slack allows and no deeper than the
//! water, and under way it is the straight line, which is what a rope doing
//! its work looks like. Its length is [`boat::PAINTER`]'s; the solver holds
//! the boat to that, so what is drawn is the rope the tender is actually on.
//!
//! No piece is a child of its hull. The anchor stays where it was dropped
//! while the hull swings round it, the cable is between the two, and the
//! painter is between two hulls; a child would ride one hull's transform and
//! go with it. They belong to a hull through a relationship of their own
//! instead — [`GearOf`] and [`Tackle`] — so a hull that goes takes its gear
//! with it exactly as it would its children, without the transform coming
//! along.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::PrimitiveTopology;
use bevy::prelude::*;

use crate::boat::{self, Anchored, Rigged, Towed, Vessel};
use crate::sea::SeaConditions;
use crate::terrain::Ground;
use crate::{matte, AppState};

/// The catenary parameter of a cable under the light pull a hull lying easy
/// puts on it, in metres — the horizontal pull over the chain's weight per
/// metre. Small, so that the chain drops steeply from the bow and a hull
/// swung out to the end of its scope has a visible run of cable lying on
/// the bottom before the hook; a bigger number is a tighter, flatter cable,
/// which is a hull in a blow rather than one at rest.
const SLACK: f32 = 3.0;

/// The least the parameter is let fall to, for a hull straight over its
/// hook: the chain hangs all but vertically from the ring, and at nothing
/// at all the curve has no shape to take.
const TAUTEST: f32 = 1e-3;

/// How many segments of the cable lie along the bottom, and how many are
/// lifted. The lifted ones carry the curve and get most of the count; the
/// run along the bottom is straight, and its segments are only there to
/// follow a bottom that is not level.
const ALONG: usize = 2;
const LIFTED: usize = 8;

/// The points along the cable, from the ring to the stemhead — and along
/// the painter, which is drawn in as many.
const POINTS: usize = ALONG + LIFTED + 1;

/// The anchor's shank as a share of its hull's length. Big for the boat —
/// a metre on the ship — because it is looked at from forty metres up
/// through a fathom or two of water, and an anchor to scale would be a dot.
const ANCHOR_SHARE: f32 = 0.16;

/// The cable's radius as a share of its hull's length: a hand's breadth
/// across on the ship, for the same reason the anchor is oversized. At the
/// default zoom that is a few pixels, which is a line the eye can follow.
const CABLE_SHARE: f32 = 0.01;

/// The bar the anchor is forged from, as a share of its shank.
const BAR: f32 = 0.07;

/// Wrought iron, wet.
const IRON: Color = Color::srgb(0.17, 0.16, 0.15);

/// Hemp, likewise.
const HEMP: Color = Color::srgb(0.6, 0.5, 0.36);

pub struct TacklePlugin;

impl Plugin for TacklePlugin {
    fn build(&self, app: &mut App) {
        // After the hull has been put where this frame leaves it, for the
        // same reason the wake is: the cable is hung from the stemhead, and
        // read a system earlier it would be hung from where the bow was.
        app.add_systems(
            Update,
            (rig_the_tackle, lay_the_cable)
                .chain()
                .after(boat::float)
                .run_if(in_state(AppState::InWorld)),
        );
    }
}

/// The hull a piece of gear belongs to — on the anchor and on the cable.
#[derive(Component)]
#[relationship(relationship_target = Tackle)]
pub struct GearOf(pub Entity);

/// The gear a hull has out, on the hull: the anchor and its cable, the
/// painter it rides on, or both. Each piece comes and goes with the fact it
/// answers to — [`Anchored`], [`Towed`] — a frame behind it, and all of it
/// goes with the hull.
#[derive(Component)]
#[relationship_target(relationship = GearOf, linked_spawn)]
pub struct Tackle(Vec<Entity>);

/// What a piece of gear is, and so which fact on its hull it answers to.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum Piece {
    /// The anchor itself, lying at the hook.
    Hook,
    /// The cable from the anchor's ring to the stemhead.
    Cable,
    /// The painter from the towing ship's taffrail to the stemhead.
    Painter,
}

/// A rope's mesh, re-hung every frame. World-space vertices on an identity
/// transform: the two ends move on different terms, and neither frame is
/// the right one for the middle.
#[derive(Component)]
struct Hung(Handle<Mesh>);

/// The pieces every hull's gear shares, made once and cloned per hull.
struct Kit {
    anchor: Handle<Mesh>,
    iron: Handle<StandardMaterial>,
    hemp: Handle<StandardMaterial>,
}

/// Puts gear out for a hull that has a fact to answer for — the hook gone
/// over, a painter made fast — and takes in whatever a hull no longer has
/// the fact for.
#[allow(clippy::type_complexity)]
fn rig_the_tackle(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut kit: Local<Option<Kit>>,
    hulls: Query<(Entity, &Rigged, Has<Anchored>, Has<Towed>, Option<&Tackle>), With<Vessel>>,
    pieces: Query<&Piece>,
) {
    for (hull, rigged, anchored, towed, tackle) in &hulls {
        let (mut has_anchor, mut has_painter) = (false, false);
        for piece in tackle.into_iter().flat_map(|tackle| tackle.iter()) {
            let Ok(what) = pieces.get(piece) else {
                continue;
            };
            let wanted = match what {
                Piece::Hook | Piece::Cable => anchored,
                Piece::Painter => towed,
            };
            if !wanted {
                commands.entity(piece).despawn();
            } else if *what == Piece::Painter {
                has_painter = true;
            } else {
                has_anchor = true;
            }
        }
        if (anchored && !has_anchor) || (towed && !has_painter) {
            let kit = kit.get_or_insert_with(|| Kit {
                anchor: meshes.add(anchor_mesh()),
                iron: materials.add(matte(IRON)),
                hemp: materials.add(matte(HEMP)),
            });
            // Hidden until [`lay_the_cable`] has somewhere to put them: a
            // bottom not yet arrived is no place to lay an anchor, and a rope
            // hung before it is laid is a knot at the origin.
            let rope = |meshes: &mut Assets<Mesh>| {
                Hung(meshes.add(cable_mesh(&[Vec3::ZERO; POINTS], rope_radius(rigged))))
            };
            if anchored && !has_anchor {
                commands.spawn((
                    Name::new("Anchor"),
                    Piece::Hook,
                    GearOf(hull),
                    DespawnOnExit(AppState::InWorld),
                    Mesh3d(kit.anchor.clone()),
                    MeshMaterial3d(kit.iron.clone()),
                    Transform::from_scale(Vec3::splat(rigged.length() * ANCHOR_SHARE)),
                    Visibility::Hidden,
                ));
                let cable = rope(&mut meshes);
                commands.spawn((
                    Name::new("Cable"),
                    Piece::Cable,
                    GearOf(hull),
                    DespawnOnExit(AppState::InWorld),
                    Mesh3d(cable.0.clone()),
                    cable,
                    MeshMaterial3d(kit.iron.clone()),
                    Visibility::Hidden,
                ));
            }
            if towed && !has_painter {
                let painter = rope(&mut meshes);
                commands.spawn((
                    Name::new("Painter"),
                    Piece::Painter,
                    GearOf(hull),
                    DespawnOnExit(AppState::InWorld),
                    Mesh3d(painter.0.clone()),
                    painter,
                    MeshMaterial3d(kit.hemp.clone()),
                    Visibility::Hidden,
                ));
            }
        }
    }
}

/// How thick a hull's ropes are drawn — see [`CABLE_SHARE`].
fn rope_radius(rigged: &Rigged) -> f32 {
    rigged.length() * CABLE_SHARE
}

/// Lays every anchor on the bottom at its hook and hangs its cable from
/// there to the stemhead, and hangs every painter from the towing ship's
/// taffrail to its boat's stem — off the poses the hulls arrived at this
/// frame.
#[allow(clippy::type_complexity)]
fn lay_the_cable(
    ground: Option<Res<Ground>>,
    sea: Res<SeaConditions>,
    time: Res<Time>,
    mut meshes: ResMut<Assets<Mesh>>,
    hulls: Query<
        (
            &Transform,
            &Rigged,
            Option<&Anchored>,
            Option<&Towed>,
            &Tackle,
        ),
        Without<GearOf>,
    >,
    ships: Query<(&Transform, &Rigged), (With<Vessel>, Without<GearOf>)>,
    mut gear: Query<(&Piece, Option<&Hung>, &mut Transform, &mut Visibility), With<GearOf>>,
) {
    let ground = ground.as_deref();
    for (hull, rigged, anchored, towed, tackle) in &hulls {
        let laid = anchored.and_then(|anchored| lay(ground, hull, *rigged, anchored.0));
        let painter = towed.and_then(|towed| {
            let (ship, ship_rigged) = ships.get(towed.by()).ok()?;
            let from = ship.transform_point(ship_rigged.taffrail());
            let to = hull.transform_point(rigged.stemhead());
            let radius = rope_radius(rigged);
            // The joint holds the hulls [`boat::PAINTER`] apart on the plane;
            // the rope drawn between the rails has the drop between them to
            // cover as well, or a taut tow would be drawn as one stretched.
            let length = (boat::PAINTER.powi(2) + (from.y - to.y).powi(2)).sqrt();
            Some(painter_run(from, to, length).map(|at| {
                let afloat = sea.water_over(ground, at.xz(), time.elapsed_secs_wrapped());
                at.with_y(at.y.max(afloat + radius))
            }))
        });
        let shown = |seen: bool| {
            if seen {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            }
        };
        for piece in tackle.iter() {
            let Ok((what, hung, mut pose, mut visibility)) = gear.get_mut(piece) else {
                continue;
            };
            match what {
                Piece::Hook => {
                    if let Some(laid) = &laid {
                        *pose = laid.anchor;
                    }
                    visibility.set_if_neq(shown(laid.is_some()));
                }
                Piece::Cable => {
                    if let (Some(laid), Some(hung)) = (&laid, hung) {
                        rehang(&mut meshes, hung, &laid.cable, rope_radius(rigged));
                    }
                    visibility.set_if_neq(shown(laid.is_some()));
                }
                Piece::Painter => {
                    if let (Some(run), Some(hung)) = (&painter, hung) {
                        rehang(&mut meshes, hung, run, rope_radius(rigged));
                    }
                    visibility.set_if_neq(shown(painter.is_some()));
                }
            }
        }
    }
}

/// Puts a rope's mesh where its run now is.
fn rehang(meshes: &mut Assets<Mesh>, hung: &Hung, run: &[Vec3; POINTS], radius: f32) {
    if let Some(mut mesh) = meshes.get_mut(&hung.0) {
        *mesh = cable_mesh(run, radius);
    }
}

/// Where a hull's gear lies this frame: the anchor's pose, and the cable's
/// run from its ring to the stemhead, in world metres.
struct Laid {
    anchor: Transform,
    cable: [Vec3; POINTS],
}

/// Lays the gear for a hull with its hook at `hook`, or nothing where the
/// bottom there has not arrived — there is then no depth to lay it at, and
/// the gear waits hidden rather than guessing one.
///
/// The anchor lies with its shank along the bottom pointing at the bow, the
/// way the pull leaves it, and the cable leaves the ring at the shank's end.
/// A hull straight over its hook has no pull to point the shank by, and
/// the anchor takes the hull's own heading instead.
fn lay(ground: Option<&Ground>, hull: &Transform, rigged: Rigged, hook: Vec2) -> Option<Laid> {
    let ground = ground?;
    let bed = ground.height(hook.x, hook.y)?;
    let size = rigged.length() * ANCHOR_SHARE;
    let radius = rigged.length() * CABLE_SHARE;
    let hawse = hull.transform_point(rigged.stemhead());
    let across = (hawse.xz() - hook).normalize_or(hull.forward().xz().normalize_or(Vec2::NEG_Y));
    let ring = Vec3::new(
        hook.x + across.x * size,
        bed + radius,
        hook.y + across.y * size,
    );
    let run = hawse.xz().distance(ring.xz());
    let rise = (hawse.y - ring.y).max(radius);
    let cable = cable_run(run, rise).map(|point| {
        let at = ring + Vec3::new(across.x * point.x, point.y, across.y * point.x);
        // Over a bottom that rises between the hook and the hull the run
        // along it rises too, rather than cutting through the ground; over
        // one that falls away the chain hangs straight across the hollow,
        // which is what a chain under any pull at all does.
        match ground.height(at.x, at.z) {
            Some(height) => at.with_y(at.y.max(height + radius)),
            None => at,
        }
    });
    Some(Laid {
        anchor: Transform {
            translation: Vec3::new(hook.x, bed, hook.y),
            rotation: Quat::from_rotation_y(across.x.atan2(across.y)),
            scale: Vec3::splat(size),
        },
        cable,
    })
}

/// The cable's run in the vertical plane through the ring and the stemhead:
/// from the ring at the origin, along the bottom and up to the stemhead
/// `run` metres across and `rise` metres up — see the module doc for the
/// shape, and [`SLACK`] for the number it turns on.
///
/// For a lifted length of chain hung from a point `rise` above where it
/// leaves the bottom tangentially, the catenary with parameter `a` spans
/// `a·acosh(1 + rise/a)` across; that span grows with `a`, so where the
/// slack chain's span outreaches `run` the parameter is bisected down to the
/// one whose span is `run` exactly, and the run along the bottom is nothing.
fn cable_run(run: f32, rise: f32) -> [Vec2; POINTS] {
    let span = |a: f32| a * (1.0 + rise / a).acosh();
    let a = if span(SLACK) <= run {
        SLACK
    } else if span(TAUTEST) >= run {
        TAUTEST
    } else {
        let (mut taut, mut slack) = (TAUTEST, SLACK);
        for _ in 0..32 {
            let between = 0.5 * (taut + slack);
            if span(between) > run {
                slack = between;
            } else {
                taut = between;
            }
        }
        taut
    };
    let touchdown = (run - span(a)).max(0.0);
    // The lifted length, from the catenary's own identity s² = y² + 2ay.
    let hung = (rise * rise + 2.0 * a * rise).sqrt();
    // One, except for the hull straight over its hook, whose chain even the
    // tautest parameter spans a hair wider than the run: that hang is
    // squeezed across to fit, and the hull sees a chain going straight down.
    let squeeze = (run - touchdown) / span(a);
    let mut points = [Vec2::ZERO; POINTS];
    for (i, point) in points.iter_mut().enumerate() {
        *point = if i <= ALONG {
            Vec2::new(touchdown * i as f32 / ALONG as f32, 0.0)
        } else {
            let s = hung * (i - ALONG) as f32 / LIFTED as f32;
            Vec2::new(
                touchdown + squeeze * a * (s / a).asinh(),
                (s * s + a * a).sqrt() - a,
            )
        };
    }
    // On the stemhead exactly, whatever the arithmetic above rounded to.
    points[POINTS - 1] = Vec2::new(run, rise);
    points
}

/// A painter `length` long between two made-fast ends: the straight line
/// where the rope is taut or stretched, and where it is slack a sag off
/// that line deep enough to take up the slack — the parabola's own
/// arc-length rule, `length ≈ chord + 8·sag²/3·chord`, turned round. Deeper
/// than the water is the caller's to refuse: rope floats.
fn painter_run(from: Vec3, to: Vec3, length: f32) -> [Vec3; POINTS] {
    let chord = from.distance(to);
    let slack = (length - chord).max(0.0);
    let sag = (3.0 * chord * slack / 8.0).sqrt();
    std::array::from_fn(|i| {
        let t = i as f32 / (POINTS - 1) as f32;
        from.lerp(to, t) - Vec3::Y * sag * (t * std::f32::consts::PI).sin()
    })
}

/// The cable as a shape: a three-sided tube of the given radius round its
/// run, unindexed so that each facet carries its own flat normal. Three
/// sides because that is the fewest that reads as round from every angle
/// and the fewest the flat shading has to light.
fn cable_mesh(run: &[Vec3; POINTS], radius: f32) -> Mesh {
    const SIDES: usize = 3;
    let rings: Vec<[Vec3; SIDES]> = (0..POINTS)
        .map(|i| {
            // Along the cable at this point: the chord between its
            // neighbours, so that the tube turns the corner rather than
            // kinking; at either end, the one segment there is.
            let along = run[(i + 1).min(POINTS - 1)] - run[i.saturating_sub(1)];
            let along = along.normalize_or(Vec3::Y);
            let out = along.cross(Vec3::Y);
            let out = if out.length_squared() > 1e-6 {
                out.normalize()
            } else {
                along.cross(Vec3::X).normalize()
            };
            let round = along.cross(out);
            std::array::from_fn(|k| {
                let turn = k as f32 * std::f32::consts::TAU / SIDES as f32;
                run[i] + (out * turn.cos() + round * turn.sin()) * radius
            })
        })
        .collect();
    let mut positions = Vec::with_capacity((POINTS - 1) * SIDES * 6);
    for pair in rings.windows(2) {
        let (near, far) = (pair[0], pair[1]);
        for k in 0..SIDES {
            let next = (k + 1) % SIDES;
            positions.extend([near[k], near[next], far[next], near[k], far[next], far[k]]);
        }
    }
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_computed_flat_normals()
}

/// A fisherman's anchor a shank long, lying as one lies when it holds:
/// the crown at the origin, the shank along the bottom to the ring at
/// `+Z`, the arms standing up and down from the crown with a fluke on
/// each — the lower one in the ground — and the stock across the shank's
/// end, flat on the bottom. Boxes, merged: it is looked at through water,
/// and the outline is the whole of what it has to say.
fn anchor_mesh() -> Mesh {
    let bar = |size: Vec3, at: Vec3| {
        Mesh::from(Cuboid::from_size(size)).transformed_by(Transform::from_translation(at))
    };
    let lift = BAR / 2.0;
    let arm = 0.45;
    let mut anchor = bar(Vec3::new(BAR, BAR, 1.0), Vec3::new(0.0, lift, 0.5));
    for piece in [
        // The arms, and a fluke at the end of each: a slab facing the shank.
        bar(Vec3::new(BAR, 2.0 * arm, BAR), Vec3::new(0.0, lift, 0.0)),
        bar(Vec3::new(0.03, 0.2, 0.3), Vec3::new(0.0, lift + arm, 0.15)),
        bar(Vec3::new(0.03, 0.2, 0.3), Vec3::new(0.0, lift - arm, 0.15)),
        // The stock, and the ring the cable is bent to.
        bar(Vec3::new(2.0 * arm, BAR, BAR), Vec3::new(0.0, lift, 0.88)),
        bar(Vec3::new(0.16, 0.16, 0.05), Vec3::new(0.0, lift, 1.0)),
    ] {
        anchor
            .merge(&piece)
            .expect("every piece is a cuboid, cut with the same attributes");
    }
    anchor
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::Player;
    use crate::testing::{run_frames, test_shore, world_app, SHORE_WATERLINE};
    use bevy::ecs::system::RunSystemOnce;
    use protocol::BoatKind;

    /// The hull the player is aboard.
    fn helmed(app: &mut App) -> Entity {
        app.world_mut()
            .query_filtered::<&ChildOf, With<Player>>()
            .single(app.world())
            .expect("the player is aboard something")
            .parent()
    }

    fn hooks(app: &mut App) -> Vec<Transform> {
        app.world_mut()
            .query::<(&Piece, &Transform)>()
            .iter(app.world())
            .filter(|(what, _)| **what == Piece::Hook)
            .map(|(_, pose)| *pose)
            .collect()
    }

    /// Every vertex of every rope of a kind in the world.
    fn ropes(app: &mut App, what: Piece) -> Vec<Vec<Vec3>> {
        let handles: Vec<Handle<Mesh>> = app
            .world_mut()
            .query::<(&Piece, &Hung)>()
            .iter(app.world())
            .filter(|(piece, _)| **piece == what)
            .map(|(_, hung)| hung.0.clone())
            .collect();
        let meshes = app.world().resource::<Assets<Mesh>>();
        handles
            .iter()
            .map(|handle| {
                meshes
                    .get(handle)
                    .expect("a cable's mesh outlives the cable")
                    .attribute(Mesh::ATTRIBUTE_POSITION)
                    .and_then(|positions| positions.as_float3())
                    .expect("a cable is positions")
                    .iter()
                    .map(|&p| Vec3::from(p))
                    .collect()
            })
            .collect()
    }

    fn cables(app: &mut App) -> Vec<Vec<Vec3>> {
        ropes(app, Piece::Cable)
    }

    fn depth_at(app: &App, at: Vec2) -> f32 {
        app.world()
            .resource::<Ground>()
            .height(at.x, at.y)
            .expect("the shore has arrived")
    }

    /// The shore world with the player's ship lying over the apron in five
    /// metres of water, its hook down two boat-lengths inshore of it.
    fn anchored_app() -> (App, Entity, Vec2) {
        let mut app = world_app();
        app.insert_resource(test_shore());
        let ship = helmed(&mut app);
        let lying = Vec2::new(SHORE_WATERLINE + 10.0, 0.0);
        let hook = Vec2::new(SHORE_WATERLINE + 4.0, 0.0);
        let mut pose = app.world_mut().get_mut::<Transform>(ship).unwrap();
        pose.translation = Vec3::new(lying.x, 0.0, lying.y);
        pose.rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        app.world_mut().entity_mut(ship).insert(Anchored(hook));
        run_frames(&mut app, 3);
        (app, ship, hook)
    }

    #[test]
    fn an_anchored_hull_shows_its_anchor_on_the_bottom_and_its_cable_to_the_stemhead() {
        let (mut app, ship, hook) = anchored_app();
        let bed = depth_at(&app, hook);
        assert!(
            bed < -1.0 && bed > -8.0,
            "the hook lies in {bed} m of water"
        );

        let anchors = hooks(&mut app);
        assert_eq!(anchors.len(), 1, "one hull at anchor is one anchor");
        let anchor = anchors[0];
        assert!(
            anchor.translation.xz().distance(hook) < 1e-3
                && (anchor.translation.y - bed).abs() < 1e-3,
            "the anchor lies at {} rather than on the bottom at {hook} ({bed} m)",
            anchor.translation
        );
        // Pointed at the hull, which lies out to +X of it: the shank runs
        // along the mesh's +Z, so the rotation has to carry +Z onto +X.
        let shank = anchor.rotation * Vec3::Z;
        assert!(
            shank.dot(Vec3::X) > 0.99,
            "the shank points {shank}, not at the hull"
        );

        let all = cables(&mut app);
        assert_eq!(all.len(), 1, "one hull at anchor is one cable");
        let cable = &all[0];
        assert_eq!(cable.len(), (POINTS - 1) * 3 * 6);
        let ship_pose = *app.world().get::<Transform>(ship).unwrap();
        let rigged = *app.world().get::<Rigged>(ship).unwrap();
        let stemhead = ship_pose.transform_point(rigged.stemhead());
        let radius = rigged.length() * CABLE_SHARE;
        let nearest_the_stemhead = cable
            .iter()
            .map(|v| v.distance(stemhead))
            .fold(f32::MAX, f32::min);
        assert!(
            nearest_the_stemhead <= radius + 1e-3,
            "the cable comes no nearer the stemhead than {nearest_the_stemhead} m"
        );
        let lowest = cable.iter().map(|v| v.y).fold(f32::MAX, f32::min);
        assert!(
            (lowest - bed).abs() < 3.0 * radius,
            "the cable's low point is {lowest}, not on the bottom at {bed}"
        );
        for vertex in cable {
            let under = depth_at(&app, vertex.xz());
            assert!(
                vertex.y >= under - 1e-3,
                "the cable passes through the ground at {vertex} (bottom {under})"
            );
            assert!(
                vertex.x >= hook.x - radius && vertex.x <= stemhead.x + radius,
                "the cable strays to {vertex}, outside the run from hook to stemhead"
            );
        }

        // Weighed: the gear goes, and the hull is left holding none.
        app.world_mut().entity_mut(ship).remove::<Anchored>();
        run_frames(&mut app, 2);
        assert!(hooks(&mut app).is_empty() && cables(&mut app).is_empty());
        assert!(app.world().get::<Tackle>(ship).is_none());
    }

    #[test]
    fn the_gear_waits_hidden_where_no_bottom_has_arrived() {
        // A world with no ground handed to it: the hull is anchored on the
        // player's word, but there is no depth to lay the anchor at, so the
        // gear exists and is not shown — and is shown the moment there is.
        let mut app = world_app();
        let ship = helmed(&mut app);
        let hook = Vec2::new(SHORE_WATERLINE + 4.0, 0.0);
        let mut pose = app.world_mut().get_mut::<Transform>(ship).unwrap();
        pose.translation = Vec3::new(SHORE_WATERLINE + 10.0, 0.0, 0.0);
        app.world_mut().entity_mut(ship).insert(Anchored(hook));
        run_frames(&mut app, 3);
        let seen = |app: &mut App| -> Vec<Visibility> {
            app.world_mut()
                .query_filtered::<&Visibility, With<GearOf>>()
                .iter(app.world())
                .copied()
                .collect()
        };
        assert_eq!(seen(&mut app), vec![Visibility::Hidden; 2]);
        app.insert_resource(test_shore());
        run_frames(&mut app, 2);
        assert_eq!(seen(&mut app), vec![Visibility::Inherited; 2]);
    }

    #[test]
    fn a_hull_that_goes_takes_its_gear_with_it() {
        let mut app = world_app();
        app.insert_resource(test_shore());
        let lying = Vec2::new(SHORE_WATERLINE + 8.0, 0.0);
        let dinghy = app
            .world_mut()
            .run_system_once(move |mut commands: Commands, mut kit: boat::HullKit| {
                boat::spawn_hull(
                    &mut commands,
                    &mut kit,
                    BoatKind::Rowboat,
                    Transform::from_xyz(lying.x, 0.0, lying.y),
                    None,
                )
            })
            .expect("a hull can be spawned");
        app.world_mut()
            .entity_mut(dinghy)
            .insert(Anchored(lying + Vec2::new(-2.0, 0.0)));
        run_frames(&mut app, 3);
        assert_eq!(hooks(&mut app).len(), 1);
        app.world_mut().despawn(dinghy);
        run_frames(&mut app, 1);
        assert!(
            hooks(&mut app).is_empty() && cables(&mut app).is_empty(),
            "the gear outlived its hull"
        );
    }

    #[test]
    fn a_boat_on_the_painter_shows_the_rope_from_the_taffrail_to_its_stem() {
        let mut app = world_app();
        app.insert_resource(test_shore());
        let ship = helmed(&mut app);
        let ship_pose = *app.world().get::<Transform>(ship).unwrap();
        let alongside = ship_pose.translation + ship_pose.right() * 4.0;
        let dinghy = app
            .world_mut()
            .run_system_once(move |mut commands: Commands, mut kit: boat::HullKit| {
                boat::spawn_hull(
                    &mut commands,
                    &mut kit,
                    BoatKind::Rowboat,
                    Transform::from_translation(alongside.with_y(0.0)),
                    None,
                )
            })
            .expect("a hull can be spawned");
        app.world_mut()
            .entity_mut(dinghy)
            .insert(Towed::behind(ship));
        run_frames(&mut app, 30);

        assert!(hooks(&mut app).is_empty(), "a tow is not an anchor");
        let all = ropes(&mut app, Piece::Painter);
        assert_eq!(all.len(), 1, "one boat in tow is one painter");
        let rope = &all[0];
        let ship_pose = *app.world().get::<Transform>(ship).unwrap();
        let ship_rigged = *app.world().get::<Rigged>(ship).unwrap();
        let taffrail = ship_pose.transform_point(ship_rigged.taffrail());
        let dinghy_pose = *app.world().get::<Transform>(dinghy).unwrap();
        let dinghy_rigged = *app.world().get::<Rigged>(dinghy).unwrap();
        let stem = dinghy_pose.transform_point(dinghy_rigged.stemhead());
        let radius = rope_radius(&dinghy_rigged);
        for (end, name) in [(taffrail, "taffrail"), (stem, "stem")] {
            let nearest = rope
                .iter()
                .map(|v| v.distance(end))
                .fold(f32::MAX, f32::min);
            assert!(
                nearest <= radius + 1e-3,
                "the painter comes no nearer the {name} than {nearest} m"
            );
        }
        let lowest = rope.iter().map(|v| v.y).fold(f32::MAX, f32::min);
        assert!(
            lowest > -0.5,
            "the painter sinks to {lowest} m: rope floats"
        );
        let riding = taffrail.xz().distance(stem.xz());
        assert!(
            riding <= boat::PAINTER + 0.3,
            "the boat rides {riding} m off, past the painter's length"
        );

        // Cast off: the rope goes with the tow.
        app.world_mut().entity_mut(dinghy).remove::<Towed>();
        run_frames(&mut app, 2);
        assert!(ropes(&mut app, Piece::Painter).is_empty());
    }

    #[test]
    fn a_slack_painter_sags_and_a_taut_one_runs_straight() {
        let from = Vec3::new(0.0, 1.2, 0.0);
        let to = Vec3::new(0.0, 0.4, 3.0);
        let taut = painter_run(from, to, from.distance(to));
        for (i, point) in taut.iter().enumerate() {
            let along = from.lerp(to, i as f32 / (POINTS - 1) as f32);
            assert!(
                point.distance(along) < 1e-4,
                "a taut painter bellies to {point}"
            );
        }
        let slack = painter_run(from, to, from.distance(to) + 1.0);
        assert!(slack[0].distance(from) < 1e-5 && slack[POINTS - 1].distance(to) < 1e-5);
        let middle = slack[POINTS / 2];
        let straight = from.lerp(to, 0.5);
        assert!(
            middle.y < straight.y - 0.5 && middle.xz().distance(straight.xz()) < 1e-4,
            "a metre of slack hangs the middle at {middle} against {straight}"
        );
    }

    /// The run's points come out in order along the cable and never back
    /// down: the cable is one curve from the ring up to the stemhead.
    fn assert_climbs(points: &[Vec2; POINTS]) {
        for pair in points.windows(2) {
            assert!(
                pair[1].x >= pair[0].x - 1e-4 && pair[1].y >= pair[0].y - 1e-4,
                "the cable turns back on itself between {} and {}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn the_cable_lies_along_the_bottom_before_it_lifts() {
        // A hull swung out to the end of its scope in deep-ish water: more
        // run than the slack chain spans, so a length of it lies on the
        // bottom and the rest curves up gently from where it leaves.
        let (run, rise) = (12.0, 9.0);
        let points = cable_run(run, rise);
        assert_climbs(&points);
        for point in &points[..=ALONG] {
            assert_eq!(point.y, 0.0, "{point} is not on the bottom");
        }
        let touchdown = points[ALONG].x;
        assert!(touchdown > 1.0, "only {touchdown} m of chain on the bottom");
        let leaving = points[ALONG + 1] - points[ALONG];
        assert!(
            leaving.y < leaving.x,
            "the chain leaves the bottom at {leaving}, steeper than it lies"
        );
        assert!(points[POINTS - 1].distance(Vec2::new(run, rise)) < 1e-3);
    }

    #[test]
    fn a_hull_close_over_its_hook_hangs_its_cable_from_the_ring() {
        // Too little run for any chain to lie along the bottom: it leaves
        // the ring at once and hangs nearly straight down from the bow.
        let (run, rise) = (1.0, 9.0);
        let points = cable_run(run, rise);
        assert_climbs(&points);
        assert!(
            points[ALONG].x < 1e-3,
            "chain on the bottom at {}",
            points[ALONG]
        );
        let leaving = points[ALONG + 1] - points[ALONG];
        assert!(
            leaving.y > leaving.x,
            "the chain leaves the ring at {leaving}, lying rather than hanging"
        );
        assert!(points[POINTS - 1].distance(Vec2::new(run, rise)) < 1e-3);

        // And straight over the hook, the degenerate end of the same case.
        let points = cable_run(0.0, 9.0);
        assert_climbs(&points);
        assert!(points[POINTS - 1] == Vec2::new(0.0, 9.0));
    }

    #[test]
    fn the_cable_is_a_tube_round_its_run_with_its_faces_outward() {
        let run: [Vec3; POINTS] = std::array::from_fn(|i| {
            let t = i as f32 / (POINTS - 1) as f32;
            Vec3::new(t * 10.0, t * t * 6.0, (t * 3.0).sin())
        });
        let radius = 0.07;
        let mesh = cable_mesh(&run, radius);
        let positions: Vec<Vec3> = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(|p| p.as_float3())
            .unwrap()
            .iter()
            .map(|&p| Vec3::from(p))
            .collect();
        assert_eq!(positions.len(), (POINTS - 1) * 3 * 6);
        let nearest = |v: Vec3| {
            run.iter()
                .copied()
                .min_by(|a, b| a.distance(v).total_cmp(&b.distance(v)))
                .unwrap()
        };
        for triangle in positions.chunks_exact(3) {
            let (a, b, c) = (triangle[0], triangle[1], triangle[2]);
            let normal = (b - a).cross(c - a);
            let middle = (a + b + c) / 3.0;
            let outward = middle - nearest(middle);
            assert!(
                normal.dot(outward) > 0.0,
                "a facet at {middle} faces into the cable"
            );
            for vertex in [a, b, c] {
                assert!(
                    vertex.distance(nearest(vertex)) <= radius + 1e-4,
                    "{vertex} stands off the run"
                );
            }
        }
    }

    #[test]
    fn the_anchor_lies_a_shank_long_with_its_flukes_standing() {
        let mesh = anchor_mesh();
        let positions = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(|p| p.as_float3())
            .unwrap();
        let (mut least, mut most) = (Vec3::MAX, Vec3::MIN);
        for &p in positions {
            least = least.min(Vec3::from(p));
            most = most.max(Vec3::from(p));
        }
        assert!(
            most.z >= 1.0,
            "the ring is short of a shank's length at {}",
            most.z
        );
        assert!(
            least.z >= -0.05,
            "the crown lies behind the origin at {}",
            least.z
        );
        assert!(
            most.y > 0.5 && least.y < -0.4,
            "the arms stand {least} to {most}"
        );
        assert!(
            (least.x + most.x).abs() < 1e-4,
            "the anchor lies off its own axis"
        );
    }
}
