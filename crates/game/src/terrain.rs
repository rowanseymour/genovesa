//! Putting the world on screen: chunks as they are sent, the sea and the sun.
//!
//! Nothing here generates anything. The world arrives over the connection a
//! chunk at a time — corner heights and one palette entry per triangle, see
//! the `protocol` crate — and this module's job is to ask for the chunks near
//! the camera, turn the answers into meshes, and throw them away again as the
//! camera leaves them behind. It could be rewritten against the protocol's
//! documentation by somebody who had never seen the generator, which is the
//! point of the arrangement.
//!
//! [`Ground`] is the whole of what this machine knows about the world: which
//! chunks have been asked for, which have come back, and what the answers
//! were. Anything that has to sit on the ground asks it — the boat, the
//! camera, the markers other players stand as — and it answers `None` where
//! the ground has not arrived yet, so a rider keeps the height it had rather
//! than dropping to the waterline and climbing back out.
//!
//! The sea and the ocean floor are not chunks at all. They are two planes that
//! travel with the camera, because away from any island the world is exactly
//! those two surfaces — which is also why a chunk of open water is answered
//! with nothing rather than with eight thousand identical triangles.
//!
//! Lakes are the exception that proves it. A lake stands above sea level at a
//! height nothing local decides, so it cannot be a plane anyone draws
//! unprompted — it arrives with its chunk, as a second grid of levels, and
//! gets a second mesh, in fresh water rather than in the sea's.

use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::light::{
    CascadeShadowConfigBuilder, DirectionalLightShadowMap, NotShadowCaster, NotShadowReceiver,
};
use bevy::mesh::PrimitiveTopology;
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bevy::tasks::{block_on, futures_lite::future, AsyncComputeTaskPool, Task};

use protocol::ground::{
    chunk_at, dequantize, facets, ChunkPayload, Palm, Surface, Tone, CHUNK_METRES, FACET_METRES,
    FACET_QUADS, FACET_TRIS, FACET_VERTS, HEIGHT_STEP, LAKE_WATER, NO_WATER, OCEAN_DEPTH,
    SEA_WATER,
};

use crate::camera::{MapCamera, View};
use crate::sea::{self, SeaExtension, SeaMaterial};
use crate::{matte, AppState};

/// How far the ocean floor plane hangs below [`OCEAN_DEPTH`], in metres. It
/// only has to back the water beyond the terrain meshes, so all this has to do
/// is keep the two surfaces from being coplanar — but it has to do it a
/// kilometre out, where the depth buffer is coarse, and the step it leaves at a
/// chunk's edge is under water.
///
/// Two metres of it, plus the half step of [`OFF_LATTICE`]: two metres under
/// the bed is a height the ground can be drawn at exactly, and being clear of
/// the pinned bed is no help against a hillside that happens to pass through
/// that height flat.
const SEA_FLOOR_CLEARANCE: f32 = 2.0 + OFF_LATTICE;

/// Width of the sea plane, in metres. It travels with the camera, so it only
/// has to reach past the far plane from wherever the camera is — not across
/// any particular stretch of world.
const SEA_EXTENT: f32 = 8000.0;

/// How far a surface that is not itself ground stands off the heights ground
/// can be drawn at, in metres: half a step of the height lattice, which is as
/// far off that lattice as anything can get.
///
/// Heights arrive quantised — see [`dequantize`] — so every corner this module
/// draws is one of the values `HEIGHT_FLOOR + n * HEIGHT_STEP` and nothing in
/// between. A plane put at a whole number of steps is therefore not merely
/// close to flat ground at that height but *exactly* coplanar with it, to
/// within nanometres, and coplanar surfaces fight over the depth buffer. Both
/// planes below travel with the camera, so their half of the fight is
/// re-rasterised every frame and the result flickers: a patch of ground
/// appearing and disappearing through the water, its shape changing with a
/// fraction of a degree of camera movement.
///
/// A whole number of steps is what both clearances used to be — the sea stood
/// at 0.08 m, four of them — and 2 cm of quantisation is coarse enough that
/// real ground lands on a given step often enough to see: any shoal flat to
/// within a centimetre of the sea's height is a facet or two pinned exactly to
/// it. Half a step off, no ground can ever be nearer than a centimetre, which
/// the depth buffer resolves everywhere the haze lets anything be seen.
const OFF_LATTICE: f32 = HEIGHT_STEP / 2.0;

/// Height of the sea's surface, in metres.
///
/// Sea level is zero and the ground meets it along every coast, so the plane
/// stands a little above it rather than on it — four steps of clearance, and
/// the half step of [`OFF_LATTICE`] that keeps it off the lattice those steps
/// are counted on.
const SEA_SURFACE: f32 = 4.0 * HEIGHT_STEP + OFF_LATTICE;

/// How opaque standing water is drawn, sea and lake alike.
///
/// A drawing decision rather than a fact about the world, which is why it
/// lives here and the two colours it is applied to live in the protocol. High:
/// water is a flat tone with the bed showing faintly through it, not a pane of
/// glass over a lit bottom. What it lets through is enough to darken the deep
/// and lift the shallows, and no more.
const WATER_ALPHA: f32 = 0.84;

/// How far out from the camera's focus chunks are wanted, in metres.
///
/// Worth deriving rather than guessing at, since it is the single number that
/// decides how much ground the machine is asked to hold — and now also how
/// much a server is asked to send. At the furthest zoom the eye sits
/// `MAX_DISTANCE * cos(PITCH)` — about 235 m — back from the focus
/// horizontally, and `MAX_DISTANCE * sin(PITCH)`, about 298 m, above it. The
/// haze closes at [`crate::HAZE_END`], 900 m, and that is a distance through
/// the air rather than across the ground, so the furthest visible ground is
/// `sqrt(HAZE_END² - 298²) ≈ 849 m` from the eye and therefore up to about
/// 1084 m from the focus.
///
/// That extreme sits directly *behind* the camera, though — the 235 m only
/// adds to the reach in the direction the eye is offset in, which is the one
/// direction the view is not looking. Ahead of the camera the visible ground
/// stops at 849 m less the offset. So 1024 m covers everything in shot with
/// room over, and what is left of the gap is taken up by chunk granularity:
/// [`within`] measures to a chunk's nearest corner, so a chunk is asked for
/// whenever any of its 128 m reaches inside the radius.
const STREAM_RADIUS: f32 = 1024.0;

/// How far out a chunk has to fall before it is forgotten. The gap behind
/// [`STREAM_RADIUS`] is hysteresis: panning along a line must not shed and
/// re-request the same ring of chunks with every step. Exactly two chunks'
/// worth of it, which is several seconds of panning at the default zoom and
/// still over half a second at the fastest the camera goes — panning speed
/// scales with the zoom distance, so the hysteresis is thinnest where the view
/// is widest.
const DESPAWN_RADIUS: f32 = 1280.0;

/// Chunks turned into meshes in any one frame.
///
/// The builds themselves run on the task pool and cost the frame nothing, but
/// uploading a mesh is main-thread work, and the answers arrive in clumps
/// rather than spread out: a client entering the world asks for its whole
/// neighbourhood at once, and every chunk of an island is answered as soon as
/// the server has generated it — so an island's rectangle of chunks lands
/// within a frame or two of itself. A big island is several hundred chunks,
/// and meshing them all at once is a visible hitch exactly when the player has
/// just arrived somewhere worth looking at.
///
/// The rest keep their [`ChunkBuild`] and are picked up over the following
/// frames. No sorting by distance to go with it: the cap alone is what bounds
/// the spike, and the chunks are all within the streaming radius by
/// construction — the difference between meshing the near ones first and
/// taking them in whatever order the query yields is a few frames of a corner
/// of the view filling in.
const MESHES_PER_FRAME: usize = 8;

pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<SeaMaterial>::default())
            .add_systems(OnEnter(AppState::InWorld), enter_world)
            .add_systems(OnExit(AppState::InWorld), leave_world)
            .add_systems(
                Update,
                (
                    ask_for_ground,
                    spawn_arrivals,
                    receive_chunks,
                    stream_out,
                    follow_camera,
                    sea::refresh_depth,
                )
                    .chain()
                    .run_if(in_state(AppState::InWorld).and_then(resource_exists::<Ground>)),
            );
    }
}

/// Everything this machine has been told about the world, and everything it
/// has asked and not yet been told.
///
/// The name is literal: there is no world here beyond the chunks a server has
/// sent, and a coordinate this has never heard of is not "ocean" or "land" but
/// simply unknown — which is why [`Ground::surface`] answers `None` for it
/// rather than guessing at sea level.
#[derive(Resource, Default)]
pub struct Ground {
    /// The answers, by chunk.
    chunks: HashMap<IVec2, Chunk>,
    /// Asked for, and not yet answered. Kept so that a chunk is asked for once
    /// rather than again every frame until it arrives — a request costs eleven
    /// bytes, and a hundred frames of them is a client shouting.
    outstanding: HashSet<IVec2>,
    /// Wanted, and not yet asked for: what [`ask_for_ground`] puts on the
    /// wire. Streaming decides *what* to want without knowing there is a
    /// connection, which is what keeps this module free of the session.
    to_ask: Vec<IVec2>,
    /// Answered as ground, and not yet given an entity to be drawn by.
    arrived: Vec<Arrival>,
}

/// One chunk of ground that has come back and not yet been drawn — everything
/// [`chunk_meshes`] is about to be handed, and nothing that is kept afterwards.
struct Arrival {
    chunk: IVec2,
    heights: Arc<[f32]>,
    surfaces: Vec<Surface>,
    /// The chunk's standing water, still quantised: it is only ever compared
    /// against [`NO_WATER`] and turned into a height once per quad drawn, so
    /// there is nothing to be gained by dequantising a whole grid of it the
    /// way the corner heights are.
    water: Option<Vec<u16>>,
    /// The palms standing on it. Carried through untouched and handed on —
    /// what a palm looks like is `trees`' business, not this module's.
    palms: Vec<Palm>,
}

/// The palms a chunk arrived carrying, waiting to be stood up.
///
/// A component rather than an argument because the two halves belong to
/// different modules: this one knows when a chunk's ground exists, and
/// `trees` knows what a tree is. The component is how the first tells the
/// second, and `trees` takes it off again once it has planted them.
#[derive(Component)]
pub struct PendingPalms(pub Vec<Palm>);

/// A count of what this machine has of the world, and what it is still
/// waiting on.
pub struct Tally {
    /// Chunks that came back with ground on them — the ones that cost memory
    /// and draw calls.
    pub ground: usize,
    /// Chunks known to be open water. They hold nothing and draw nothing: the
    /// sea and floor planes already cover them, and all that is kept is the
    /// fact of having asked. Usually most of the total, since most of any
    /// neighbourhood is sea.
    pub ocean: usize,
    /// Chunks asked for, or about to be, and not yet answered — so *not* part
    /// of either count above, which are answers.
    ///
    /// The number to watch, and the only part of the readout that reflects the
    /// far end: it rises when a client has outrun what the world can generate
    /// for it, and sits at zero in a quiet corner of a warm one.
    pub requested: usize,
}

/// What one chunk turned out to be.
enum Chunk {
    /// Open water. The sea and floor planes already draw it, so there is
    /// nothing here but the fact of having asked — which is what stops
    /// [`ask_for_ground`] asking again next frame, forever.
    Ocean,
    /// Ground, with the corner heights it was sent — the same grid its mesh is
    /// built from, so anything standing on it stands on what can be seen.
    Land {
        heights: Arc<[f32]>,
        /// The entity drawing it, once [`spawn_arrivals`] has made one.
        mesh: Option<Entity>,
    },
}

impl Ground {
    /// Puts a chunk on the list to be asked for, unless it is already known or
    /// already asked for.
    fn want(&mut self, chunk: IVec2) {
        if self.chunks.contains_key(&chunk) || !self.outstanding.insert(chunk) {
            return;
        }
        self.to_ask.push(chunk);
    }

    /// Takes a server's answer about one chunk.
    ///
    /// Heights are turned back into metres here rather than in the build task,
    /// because what stands on the ground wants them this frame and what draws
    /// it can wait: dequantising four thousand corners is arithmetic, meshing
    /// is twenty-four thousand vertices.
    pub fn deliver(&mut self, chunk: IVec2, payload: Option<ChunkPayload>) {
        self.outstanding.remove(&chunk);
        match payload {
            None => {
                self.chunks.insert(chunk, Chunk::Ocean);
            }
            Some(payload) => {
                let heights: Arc<[f32]> = payload.heights.iter().copied().map(dequantize).collect();
                self.chunks.insert(
                    chunk,
                    Chunk::Land {
                        heights: heights.clone(),
                        mesh: None,
                    },
                );
                self.arrived.push(Arrival {
                    chunk,
                    heights,
                    surfaces: payload.surfaces,
                    water: payload.water,
                    palms: payload.palms,
                });
            }
        }
    }

    /// The chunks waiting to be asked for, taken away.
    pub fn take_requests(&mut self) -> Vec<IVec2> {
        std::mem::take(&mut self.to_ask)
    }

    /// Whether everything asked for has arrived and been handed to a mesh
    /// builder. What a capture run waits on before it starts its clock — the
    /// picture is not of the world until the world has turned up.
    pub fn settled(&self) -> bool {
        self.outstanding.is_empty() && self.to_ask.is_empty() && self.arrived.is_empty()
    }

    /// How much of the world is being held, and how much is still on its way.
    /// For the debug overlay, which is the only thing that wants the world
    /// counted rather than asked about.
    pub fn tally(&self) -> Tally {
        Tally {
            ground: self
                .chunks
                .values()
                .filter(|chunk| matches!(chunk, Chunk::Land { .. }))
                .count(),
            ocean: self
                .chunks
                .values()
                .filter(|chunk| matches!(chunk, Chunk::Ocean))
                .count(),
            requested: self.outstanding.len() + self.to_ask.len(),
        }
    }

    /// The height of the surface at a world point: the ground where it stands
    /// above the sea, and the waterline where it does not.
    ///
    /// What everything riding the world rides — the boat, the camera centred
    /// on it, and the markers other players stand as — so that a hull crossing
    /// open ocean floats rather than walking the seabed, and one that has run
    /// aground sits in the hillside.
    ///
    /// Read off the same facet grid the mesh is built from, and interpolated
    /// across the same triangles, so a hull sits exactly on the ground that can
    /// be seen under it rather than on a smoother field the picture only
    /// approximates.
    ///
    /// `None` where the chunk has not arrived. Absent rather than sea level,
    /// deliberately: a rider keeps the height it had for the few frames a
    /// chunk takes to come back, instead of dropping to the waterline and
    /// climbing back out as the ground lands.
    pub fn surface(&self, x: f32, z: f32) -> Option<f32> {
        Some(self.height(x, z)?.max(0.0))
    }

    /// The height of the ground itself, sea bed and all — [`Ground::surface`]
    /// without the waterline over it.
    ///
    /// What the camera keeps its eye clear of: over open water the bed is
    /// metres down and the eye is welcome to be down there with it, whereas
    /// the surface would push it up to sea level for no reason.
    pub fn height(&self, x: f32, z: f32) -> Option<f32> {
        let at = Vec2::new(x, z);
        let chunk = chunk_at(at);
        match self.chunks.get(&chunk)? {
            Chunk::Ocean => Some(-OCEAN_DEPTH),
            Chunk::Land { heights, .. } => {
                Some(height_at(heights, at - chunk.as_vec2() * CHUNK_METRES))
            }
        }
    }
}

/// The height of one point inside a chunk, interpolated across the very facet
/// the mesh draws there.
///
/// Which triangle a point falls in has to be worked out the same way the mesh
/// splits its quads — see [`facets`] — or a hull crossing a quad would step
/// where the picture slopes. `local` is metres from the chunk's own corner.
fn height_at(heights: &[f32], local: Vec2) -> f32 {
    let cell = local / FACET_METRES;
    // Clamped rather than trusted: a point exactly on a chunk's far edge
    // belongs to the next chunk, but the arithmetic that got here is `f32` and
    // is entitled to land on the boundary itself.
    let ix = (cell.x.floor().max(0.0) as usize).min(FACET_QUADS - 1);
    let iz = (cell.y.floor().max(0.0) as usize).min(FACET_QUADS - 1);
    let u = (cell.x - ix as f32).clamp(0.0, 1.0);
    let v = (cell.y - iz as f32).clamp(0.0, 1.0);

    let corner = |cx: usize, cz: usize| heights[cz * FACET_VERTS + cx];
    let (tl, tr) = (corner(ix, iz), corner(ix + 1, iz));
    let (bl, br) = (corner(ix, iz + 1), corner(ix + 1, iz + 1));

    if (ix + iz).is_multiple_of(2) {
        // Split along the anti-diagonal, from the near-far corner to the
        // far-near one.
        if u + v <= 1.0 {
            tl + u * (tr - tl) + v * (bl - tl)
        } else {
            br + (1.0 - u) * (bl - br) + (1.0 - v) * (tr - br)
        }
    } else if v >= u {
        // Split along the main diagonal.
        tl + v * (bl - tl) + u * (br - bl)
    } else {
        tl + u * (tr - tl) + v * (br - tr)
    }
}

/// Marks a terrain chunk entity, and records which world chunk it is.
#[derive(Component, Debug, Clone, Copy)]
pub struct TerrainChunk {
    /// The chunk's coordinate on the world grid. What streaming keys off, and
    /// later what level-of-detail selection and localised rebuilds will.
    pub coords: IVec2,
}

/// A chunk whose meshes are still being built off the main thread. Only the
/// assembly — the ground itself has already arrived.
#[derive(Component)]
pub(crate) struct ChunkBuild(Task<ChunkMeshes>);

/// The one material every chunk shares, so they still batch into a single draw
/// call each.
#[derive(Resource)]
struct GroundMaterial(Handle<StandardMaterial>);

/// The one material every lake shares, so that all the standing water in a
/// view still batches into a single draw call however many chunks it crosses.
///
/// The sea has no resource of its own: it is a single plane, spawned once with
/// its material and never asked for again. This is kept because a lake arrives
/// with its chunk and has to be given the water it is made of at that moment.
#[derive(Resource)]
struct LakeMaterial(Handle<StandardMaterial>);

/// Marks the sea plane, which travels with the camera.
#[derive(Component)]
struct Sea;

/// Marks the ocean floor plane, likewise.
#[derive(Component)]
struct OceanFloor;

/// Builds one chunk's mesh from what the server sent about it.
///
/// The geometry is flat shaded: every triangle carries its own normal and its
/// own single colour, so no vertex is shared between two triangles. That costs
/// three vertices per triangle instead of roughly one, and buys it back many
/// times over from drawing at [`FACET_METRES`] rather than per metre. It also
/// means chunks need no border samples to meet cleanly — there are no shared
/// normals to disagree about.
///
/// Deliberately un-indexed: with no vertex shared between triangles an index
/// buffer would be 0, 1, 2, 3, … and save nothing.
fn chunk_mesh(chunk: IVec2, heights: &[f32], surfaces: &[Surface]) -> Mesh {
    let count = FACET_TRIS * 3;
    let mut positions = Vec::with_capacity(count);
    let mut normals = Vec::with_capacity(count);
    let mut uvs = Vec::with_capacity(count);
    let mut colors = Vec::with_capacity(count);
    let base = chunk.as_vec2() * CHUNK_METRES;

    for (facet, surface) in facets().zip(surfaces) {
        let tri = facet.corners.map(|(cx, cz)| {
            Vec3::new(
                cx as f32 * FACET_METRES,
                heights[cz * FACET_VERTS + cx],
                cz as f32 * FACET_METRES,
            )
        });
        // The corners are wound counter-clockwise seen from above, which is
        // what puts the face normal upwards.
        let normal = (tri[1] - tri[0]).cross(tri[2] - tri[0]).normalize();

        // Vertex colours are consumed in linear space by the PBR shader; the
        // palette is authored in sRGB.
        let srgb = surface.color();
        let linear = Color::srgb(srgb.x, srgb.y, srgb.z).to_linear();
        let color = [linear.red, linear.green, linear.blue, 1.0];

        // UVs are in world metres over the chunk size, so a future overlay
        // lines up across chunk boundaries. One value for the whole triangle,
        // like everything else about it.
        let mid = (tri[0] + tri[1] + tri[2]) / 3.0;
        let uv = [
            (base.x + mid.x) / CHUNK_METRES,
            (base.y + mid.z) / CHUNK_METRES,
        ];

        for corner in tri {
            positions.push([corner.x, corner.y, corner.z]);
            normals.push([normal.x, normal.y, normal.z]);
            uvs.push(uv);
            colors.push(color);
        }
    }

    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
}

/// Builds the surface of one chunk's standing water, or `None` where the
/// chunk carries none — see [`ChunkPayload::water`].
///
/// One flat quad per facet quad, at the level the payload gives it, plus
/// [`OFF_LATTICE`] — a lake's level is quantised on the same lattice its bed is,
/// so a shore flat at exactly the lake's height would otherwise be coplanar
/// with the sheet standing on it. No colour, because water is one colour and
/// the material carries it, and no slope, because a lake is level: what varies
/// from quad to quad is only how high the sheet sits, and that changes at all
/// only where a chunk holds more than one lake.
///
/// **Where the water stops is not decided here.** A quad is drawn wherever
/// any of its corners has a level at all — which the server sends well past
/// the water's edge — so the sheet runs on *into* the bank and the
/// ground mesh, being opaque and higher, hides the part that has gone
/// underground. The waterline the player sees is therefore the true
/// intersection of the two surfaces, meandering at whatever precision the
/// depth buffer has, rather than the outline of the last quad that happened
/// to be under water. It is exactly how the sea already meets every coast,
/// which is the point: a lake shore should not read as a different kind of
/// edge from a sea shore.
fn water_mesh(chunk: IVec2, water: &[u16]) -> Option<Mesh> {
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let base = chunk.as_vec2() * CHUNK_METRES;

    for iz in 0..FACET_QUADS {
        for ix in 0..FACET_QUADS {
            let (tl, tr) = ((ix, iz), (ix + 1, iz));
            let (bl, br) = ((ix, iz + 1), (ix + 1, iz + 1));

            // The tallest level any corner claims. A quad spanning two lakes
            // at once would need one of them to lose, and the higher is the
            // right winner: the lower sheet is the one the ground between
            // them is entitled to hide.
            let Some(level) = [tl, tr, bl, br]
                .iter()
                .map(|(cx, cz)| water[cz * FACET_VERTS + cx])
                .filter(|level| *level != NO_WATER)
                .max()
            else {
                continue;
            };
            let y = dequantize(level) + OFF_LATTICE;

            for (cx, cz) in [tl, bl, tr, tr, bl, br] {
                positions.push([cx as f32 * FACET_METRES, y, cz as f32 * FACET_METRES]);
                // Dead flat, so every normal is the same one and there is
                // nothing for the light to pick out — which is what makes a
                // lake read as a sheet of water rather than as ground.
                normals.push([0.0, 1.0, 0.0]);
                uvs.push([
                    (base.x + cx as f32 * FACET_METRES) / CHUNK_METRES,
                    (base.y + cz as f32 * FACET_METRES) / CHUNK_METRES,
                ]);
            }
        }
    }

    (!positions.is_empty()).then(|| {
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    })
}

/// What one chunk is drawn out of: its ground, and the standing water on top
/// of it where there is any. Built together off the main thread, because they
/// come from one answer and are wanted in one frame.
struct ChunkMeshes {
    ground: Mesh,
    water: Option<Mesh>,
}

fn chunk_meshes(arrival: &Arrival) -> ChunkMeshes {
    ChunkMeshes {
        ground: chunk_mesh(arrival.chunk, &arrival.heights, &arrival.surfaces),
        water: arrival
            .water
            .as_ref()
            .and_then(|water| water_mesh(arrival.chunk, water)),
    }
}

// ---------------------------------------------------------------------------
// Entering and leaving the world
// ---------------------------------------------------------------------------

fn enter_world(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut seas: ResMut<Assets<SeaMaterial>>,
    mut images: ResMut<Assets<Image>>,
    view: Res<View>,
) {
    // Nothing is known about the world yet, and nothing is asked for until
    // there is a camera to ask around — so entering a match costs a frame
    // nothing, wherever in the world it starts.
    commands.insert_resource(Ground::default());
    info!("entered world");

    // Terrain material. Base colour is white so the vertex colours come
    // through unmodified — StandardMaterial multiplies the two together.
    commands.insert_resource(GroundMaterial(materials.add(matte(Color::WHITE))));

    // Ocean floor. The sea is translucent, so without something opaque beneath
    // it the water beyond the terrain meshes blends against the sky and reads
    // as a pale band along the coastline. Between islands this plane *is* the
    // ground — a chunk of open water is answered with nothing at all — so it
    // has to be indistinguishable from the bed the chunks build: the same
    // palette colour through the same material, matte and unlit by anything
    // the chunks aren't. Every island's skirt bed reaches exactly
    // [`OCEAN_DEPTH`] flat, so with the colours agreeing the hand-over from
    // mesh to backdrop has nothing left to show. (It used to be darker, from
    // when it only appeared beyond a lone map's edge — against streamed
    // islands that printed every island's chunk rectangle onto the water.)
    //
    // It sits [`SEA_FLOOR_CLEARANCE`] below the ground's own deepest point
    // rather than level with it. The two used to be at exactly the same
    // height, and since the height field spends whole square kilometres pinned
    // to its floor, that left two coplanar surfaces fighting over the depth
    // buffer — which read as a faint darker banding drifting across the open
    // sea as the camera moved. A couple of metres of parallax at the seam is
    // invisible with the colours matched; a shimmer is not.
    let seabed = Tone::Seabed.color();
    commands.spawn((
        Name::new("Ocean floor"),
        OceanFloor,
        DespawnOnExit(AppState::InWorld),
        // Nothing is below it to catch a shadow, so keep it out of the shadow
        // pass entirely.
        NotShadowCaster,
        // And nothing above it may throw one onto it. No honest shadow can
        // reach it — a clifftop's shadow dies within a few tens of metres of
        // its coast, and every island ends in a hundred metres of open skirt
        // — but the chunk meshes end in a two-metre step down to this plane,
        // and that step's own shadow drew a thin dark line along the western
        // edge of every island's chunk rectangle.
        NotShadowReceiver,
        Mesh3d(meshes.add(Plane3d::default().mesh().size(SEA_EXTENT, SEA_EXTENT))),
        MeshMaterial3d(materials.add(matte(Color::srgb(seabed.x, seabed.y, seabed.z)))),
        Transform::from_xyz(0.0, -OCEAN_DEPTH - SEA_FLOOR_CLEARANCE, 0.0),
    ));

    // What water is made of. Both waters are built the same way and differ
    // only in colour — see [`SEA_WATER`] and [`LAKE_WATER`], where the two are
    // named together and the difference between them is argued.
    //
    // Flat and bright rather than glassy — matte like everything else, give
    // or take the barest reflectance. Still partly transparent, so the bright
    // band running under the waterline shows through as a ring around every
    // coast: two flat tones of water, which is the whole effect. A lake has no
    // such band to show, which is most of why it does not wear the ring.
    let still = |tint: Vec3| StandardMaterial {
        reflectance: 0.02,
        alpha_mode: AlphaMode::Blend,
        ..matte(Color::srgba(tint.x, tint.y, tint.z, WATER_ALPHA))
    };
    commands.insert_resource(LakeMaterial(materials.add(still(LAKE_WATER))));

    // The sea alone wears the swell on top — a lake is sheltered water, and
    // stiller than the sea is most of what makes it read as one. The swell
    // reads its shallows from a depth window that opens where the player
    // enters the world; it opens knowing nothing — every texel deep — and
    // [`sea::refresh_depth`] fills it in as the ground itself arrives.
    let depth = images.add(sea::depth_image());
    let sea = seas.add(SeaMaterial {
        base: still(SEA_WATER),
        extension: SeaExtension::new(depth.clone(), Vec2::new(view.focus.x, view.focus.z)),
    });
    commands.insert_resource(sea::DepthWindow::new(
        depth,
        sea.clone(),
        Vec2::new(view.focus.x, view.focus.z),
    ));

    // Sea. Sized past the camera's far plane and moved along with it, so the
    // horizon is water fading into haze whichever way the view goes.
    commands.spawn((
        Name::new("Sea"),
        Sea,
        DespawnOnExit(AppState::InWorld),
        // Water casts no shadow. Bevy shadows a transparent surface as though
        // it were solid, so without this the sea throws its own shadow down
        // onto its own bed — a broad darkening of everything under water, with
        // a hard edge wherever the cascades stop resolving it, sliding about as
        // the camera moves. It still *receives* shadows, which is what puts a
        // cliff's shadow out across the water at its foot.
        NotShadowCaster,
        Mesh3d(meshes.add(sea::surface_mesh(SEA_EXTENT))),
        MeshMaterial3d(sea),
        Transform::from_xyz(0.0, SEA_SURFACE, 0.0),
    ));

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

    // Sun.
    commands.spawn((
        Name::new("Sun"),
        DespawnOnExit(AppState::InWorld),
        DirectionalLight {
            // Lower than daylight, and paired with a much stronger ambient, so
            // the gap between a lit facet and a shadowed one is a couple of
            // steps rather than a full range. Flat shading gets its readability
            // from facets differing at all, not from deep contrast.
            illuminance: 8_500.0,
            shadow_maps_enabled: true,
            // The default biases cause bad self-shadowing acne on a heightfield
            // this large — dark speckle all over the hillsides.
            shadow_depth_bias: 0.06,
            shadow_normal_bias: 2.2,
            ..default()
        },
        // Cascades are fitted to the camera's own frustum, so the far end of the
        // shadowed region travels with the camera. It has to sit past everything
        // the camera can see, or that end lands on ground that is in shot and
        // whole hillsides gain and lose their shadows as the view moves. Out at
        // the haze it can't be seen doing it.
        CascadeShadowConfigBuilder {
            first_cascade_far_bound: 60.0,
            maximum_distance: crate::HAZE_END,
            ..default()
        }
        .build(),
        // Low-ish sun: long shadows pick out the relief far better than an
        // overhead one, which flattens everything.
        Transform::from_xyz(55.0, 42.0, 28.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

/// Chunk entities despawn themselves on exit; the resources that tracked them
/// have to go too, or a stale world would answer the next match's queries
/// until its own first frame replaced it.
fn leave_world(mut commands: Commands) {
    commands.remove_resource::<Ground>();
    commands.remove_resource::<GroundMaterial>();
    commands.remove_resource::<LakeMaterial>();
    commands.remove_resource::<sea::DepthWindow>();
}

// ---------------------------------------------------------------------------
// Streaming
// ---------------------------------------------------------------------------

/// Asks for every chunk newly inside the streaming radius.
///
/// There is no layout to consult, and that is the whole difference from when
/// this machine generated the world: a client cannot know which chunks hold
/// land, so it asks about all of them and most come back as open water. That
/// costs eleven bytes a question and eleven bytes an answer, which is a
/// bargain against knowing where the islands are.
///
/// Nearest first, because they are answered roughly in the order they are
/// asked and the ground under the camera is the ground being looked at.
fn ask_for_ground(mut ground: ResMut<Ground>, cameras: Query<&MapCamera>) {
    let Ok(camera) = cameras.single() else {
        return;
    };
    let focus = Vec2::new(camera.focus.x, camera.focus.z);
    let centre = chunk_at(focus);
    let reach = (STREAM_RADIUS / CHUNK_METRES).ceil() as i32;

    let mut wanted: Vec<IVec2> = Vec::new();
    for dz in -reach..=reach {
        for dx in -reach..=reach {
            let chunk = centre + IVec2::new(dx, dz);
            if within(chunk, focus, STREAM_RADIUS) && !ground.knows_of(chunk) {
                wanted.push(chunk);
            }
        }
    }
    // Cheap when there is nothing new, which is every frame but the ones just
    // after entering the world or crossing into a fresh row of chunks.
    wanted.sort_by(|a, b| {
        nearness(*a, focus)
            .partial_cmp(&nearness(*b, focus))
            .expect("chunk distances are finite")
    });
    for chunk in wanted {
        ground.want(chunk);
    }
}

impl Ground {
    /// Whether this chunk has been answered or asked about already.
    fn knows_of(&self, chunk: IVec2) -> bool {
        self.chunks.contains_key(&chunk) || self.outstanding.contains(&chunk)
    }
}

/// Gives every chunk of ground that has arrived an entity and a mesh to build.
fn spawn_arrivals(
    mut commands: Commands,
    mut ground: ResMut<Ground>,
    material: Res<GroundMaterial>,
) {
    let pool = AsyncComputeTaskPool::get();
    for arrival in std::mem::take(&mut ground.arrived) {
        let chunk = arrival.chunk;
        let palms = arrival.palms.clone();
        // Dropped rather than drawn if the camera has already left it behind
        // — an answer can outlive the reason it was asked for.
        let Some(Chunk::Land { mesh, .. }) = ground.chunks.get_mut(&chunk) else {
            continue;
        };

        let task = pool.spawn(async move { chunk_meshes(&arrival) });
        *mesh = Some(
            commands
                .spawn((
                    Name::new(format!("Terrain chunk {},{}", chunk.x, chunk.y)),
                    TerrainChunk { coords: chunk },
                    DespawnOnExit(AppState::InWorld),
                    PendingPalms(palms),
                    ChunkBuild(task),
                    MeshMaterial3d(material.0.clone()),
                    Transform::from_translation(Vec3::new(
                        chunk.x as f32 * CHUNK_METRES,
                        0.0,
                        chunk.y as f32 * CHUNK_METRES,
                    )),
                ))
                .id(),
        );
    }
}

/// Puts finished meshes on their entities. Until this runs for a chunk, the
/// entity is a placeholder with a transform and a ticket.
///
/// At most [`MESHES_PER_FRAME`] of them, so that an island arriving all at
/// once cannot stall a frame.
fn receive_chunks(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    lake: Res<LakeMaterial>,
    mut building: Query<(Entity, &mut ChunkBuild)>,
) {
    let mut meshed = 0;
    for (entity, mut build) in &mut building {
        if meshed >= MESHES_PER_FRAME {
            break;
        }
        let Some(built) = block_on(future::poll_once(&mut build.0)) else {
            continue;
        };
        commands
            .entity(entity)
            .remove::<ChunkBuild>()
            .insert(Mesh3d(meshes.add(built.ground)));

        // A lake rides on its chunk rather than standing as an entity of its
        // own, so that streaming has one thing to forget: despawning a chunk
        // takes its water with it, and nothing has to remember that a chunk
        // had any.
        if let Some(surface) = built.water {
            commands.entity(entity).with_child((
                Name::new("Lake"),
                // Water casts no shadow — Bevy shadows a transparent surface
                // as though it were solid, so a lake would otherwise throw its
                // own shadow down onto its own bed. The sea plane is kept out
                // of the shadow pass for exactly this reason.
                NotShadowCaster,
                Mesh3d(meshes.add(surface)),
                MeshMaterial3d(lake.0.clone()),
            ));
        }
        meshed += 1;
    }
}

/// Forgets the chunks the camera has left well behind, mesh and heights alike.
///
/// Forgotten rather than kept, because the server is the one holding the
/// world: sailing back asks for the same ground again and gets the same
/// answer, out of an island the server has almost certainly still got cached.
/// Keeping every chunk a long voyage ever crossed is how a client runs out of
/// memory in a world with no edges.
fn stream_out(mut commands: Commands, mut ground: ResMut<Ground>, cameras: Query<&MapCamera>) {
    let Ok(camera) = cameras.single() else {
        return;
    };
    let focus = Vec2::new(camera.focus.x, camera.focus.z);

    ground.chunks.retain(|chunk, held| {
        if within(*chunk, focus, DESPAWN_RADIUS) {
            return true;
        }
        if let Chunk::Land {
            mesh: Some(mesh), ..
        } = held
        {
            commands.entity(*mesh).despawn();
        }
        false
    });
}

/// The transforms of the two planes that travel with the camera.
type TravellingPlanes<'w, 's> =
    Query<'w, 's, &'static mut Transform, Or<(With<Sea>, With<OceanFloor>)>>;

/// Keeps the sea and the ocean floor centred under the camera, so the water
/// simply always reaches the horizon.
///
/// In whole steps of the sea mesh's own vertex spacing rather than
/// continuously — see [`sea::snap`]: the sea is no longer featureless, and
/// its vertices have to keep sampling the same world points or the swell
/// swims against itself. The floor needs no such care, but there is nothing
/// on it for a few metres of snap to be seen by either.
fn follow_camera(cameras: Query<&MapCamera>, mut planes: TravellingPlanes) {
    let Ok(camera) = cameras.single() else {
        return;
    };
    for mut transform in &mut planes {
        transform.translation.x = sea::snap(camera.focus.x);
        transform.translation.z = sea::snap(camera.focus.z);
    }
}

/// How far a chunk's nearest point is from a focus, squared.
fn nearness(chunk: IVec2, focus: Vec2) -> f32 {
    let corner = chunk.as_vec2() * CHUNK_METRES;
    focus
        .clamp(corner, corner + CHUNK_METRES)
        .distance_squared(focus)
}

/// Whether any of a chunk lies within `radius` of `focus`.
fn within(chunk: IVec2, focus: Vec2, radius: f32) -> bool {
    nearness(chunk, focus) <= radius * radius
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::ground::{quantize, Shade};

    /// A payload of ground with a distinctive shape: a plane tilted along both
    /// axes, so that every corner has a different height and any transposed or
    /// mis-indexed read shows up as a wrong number rather than as a wrong-ish
    /// one.
    fn a_slope() -> ChunkPayload {
        ChunkPayload {
            heights: (0..FACET_VERTS * FACET_VERTS)
                .map(|i| {
                    let (ix, iz) = (i % FACET_VERTS, i / FACET_VERTS);
                    quantize(ix as f32 + 10.0 * iz as f32)
                })
                .collect(),
            surfaces: vec![Surface::new(Tone::Grass, Shade::Plain); FACET_TRIS],
            water: None,
            palms: Vec::new(),
        }
    }

    #[test]
    fn a_chunk_mesh_is_two_flat_triangles_per_quad() {
        let payload = a_slope();
        let heights: Vec<f32> = payload.heights.iter().copied().map(dequantize).collect();
        let mesh = chunk_mesh(IVec2::ZERO, &heights, &payload.surfaces);

        assert_eq!(mesh.count_vertices(), FACET_TRIS * 3);
        assert!(
            mesh.indices().is_none(),
            "flat shading needs no index buffer"
        );

        let normals = mesh
            .attribute(Mesh::ATTRIBUTE_NORMAL)
            .expect("normals")
            .as_float3()
            .expect("three floats each");
        let colors = match mesh.attribute(Mesh::ATTRIBUTE_COLOR).expect("colours") {
            bevy::mesh::VertexAttributeValues::Float32x4(values) => values.clone(),
            other => panic!("colours came out as {other:?}"),
        };

        for tri in 0..normals.len() / 3 {
            let i = tri * 3;
            for corner in 1..3 {
                assert_eq!(
                    normals[i],
                    normals[i + corner],
                    "triangle {tri} has a varying normal"
                );
                assert_eq!(
                    colors[i],
                    colors[i + corner],
                    "triangle {tri} has a varying colour"
                );
            }
            // A heightfield can never overhang, so every facet faces upwards.
            assert!(normals[i][1] > 0.0, "triangle {tri} faces downwards");
        }
    }

    #[test]
    fn chunk_positions_are_local_so_the_transform_places_them() {
        let heights = vec![3.5f32; FACET_VERTS * FACET_VERTS];
        let surfaces = vec![Surface::plain(Tone::Sand); FACET_TRIS];
        let mesh = chunk_mesh(IVec2::new(4, -2), &heights, &surfaces);

        let positions = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .expect("positions")
            .as_float3()
            .expect("three floats each");
        for point in positions {
            assert!(
                (0.0..=CHUNK_METRES).contains(&point[0])
                    && (0.0..=CHUNK_METRES).contains(&point[2]),
                "{point:?} is outside its own chunk, so the transform would double the offset"
            );
            assert_eq!(point[1], 3.5);
        }
    }

    #[test]
    fn a_chunk_that_never_arrived_has_no_height_to_give() {
        // The whole reason this answers `None`: a rider over ground that has
        // not come back keeps the height it had rather than dropping to the
        // waterline and climbing out again as the answer lands.
        let mut ground = Ground::default();
        assert_eq!(ground.surface(10.0, 10.0), None);

        ground.deliver(IVec2::ZERO, None);
        assert_eq!(ground.surface(10.0, 10.0), Some(0.0), "open water floats");
    }

    #[test]
    fn the_ground_underfoot_is_the_ground_on_screen() {
        // Heights are read off the same grid the mesh is built from and
        // interpolated across the same triangles, so a corner is exactly its
        // own height and a point between corners is on the facet drawn there
        // — not on a smoother field the picture merely approximates.
        let mut ground = Ground::default();
        let payload = a_slope();
        let heights: Vec<f32> = payload.heights.iter().copied().map(dequantize).collect();
        ground.deliver(IVec2::ZERO, Some(payload));

        // Every corner of the grid, at its own world position.
        for iz in 0..FACET_VERTS {
            for ix in 0..FACET_VERTS {
                // The far edges belong to the next chunk along, which has not
                // arrived; inside the chunk, a corner is its own height.
                if ix == FACET_QUADS || iz == FACET_QUADS {
                    continue;
                }
                let at = Vec2::new(ix as f32, iz as f32) * FACET_METRES;
                let want = heights[iz * FACET_VERTS + ix].max(0.0);
                let got = ground.surface(at.x, at.y).expect("the chunk arrived");
                assert!(
                    (got - want).abs() < 1.0e-3,
                    "corner {ix},{iz} reads {got} m, but is drawn at {want} m"
                );
            }
        }

        // And the middle of a facet is on the plane of that facet: this slope
        // is planar within each quad either way it is split, so the midpoint of
        // a quad is the mean of its four corners.
        let mid = Vec2::splat(FACET_METRES * 0.5);
        let mean =
            (heights[0] + heights[1] + heights[FACET_VERTS] + heights[FACET_VERTS + 1]) / 4.0;
        let got = ground.surface(mid.x, mid.y).expect("the chunk arrived");
        assert!(
            (got - mean).abs() < 1.0e-3,
            "{got} m across a facet, not {mean}"
        );
    }

    #[test]
    fn a_chunk_is_asked_for_once_and_only_once() {
        let mut ground = Ground::default();
        ground.want(IVec2::ZERO);
        ground.want(IVec2::ZERO);
        assert_eq!(ground.take_requests(), [IVec2::ZERO], "asked twice");

        // Still outstanding, so still not asked again — the request is on the
        // wire, not forgotten.
        ground.want(IVec2::ZERO);
        assert!(
            ground.take_requests().is_empty(),
            "asked again while waiting"
        );

        // Answered, so likewise.
        ground.deliver(IVec2::ZERO, None);
        ground.want(IVec2::ZERO);
        assert!(
            ground.take_requests().is_empty(),
            "asked again after hearing"
        );
    }

    /// A water grid with a lake at `level` metres over the square of corners
    /// below `edge`, and nothing anywhere else.
    fn a_lake(level: f32, edge: usize) -> Vec<u16> {
        let mut water = vec![NO_WATER; FACET_VERTS * FACET_VERTS];
        for iz in 0..edge {
            for ix in 0..edge {
                water[iz * FACET_VERTS + ix] = quantize(level);
            }
        }
        water
    }

    #[test]
    fn a_lake_is_a_flat_sheet_at_the_level_it_was_sent() {
        let mesh = water_mesh(IVec2::ZERO, &a_lake(12.0, 8)).expect("a lake");

        let positions = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .expect("positions")
            .as_float3()
            .expect("three floats each");
        let normals = mesh
            .attribute(Mesh::ATTRIBUTE_NORMAL)
            .expect("normals")
            .as_float3()
            .expect("three floats each");

        assert!(!positions.is_empty());
        assert_eq!(positions.len() % 3, 0, "not whole triangles");
        for (point, normal) in positions.iter().zip(normals) {
            // Dead level, at the height the payload named — a lake that
            // sloped, or that sat at the height of its bed, would show here.
            assert!(
                (point[1] - (12.0 + OFF_LATTICE)).abs() < 1.0e-3,
                "{point:?} is not on the lake's surface"
            );
            assert_eq!(*normal, [0.0, 1.0, 0.0]);
        }
        // No colours: water is one colour and the material carries it.
        assert!(mesh.attribute(Mesh::ATTRIBUTE_COLOR).is_none());
    }

    #[test]
    fn a_lake_runs_on_under_its_own_bank() {
        // The quads drawn are every quad with a level on *any* corner, not
        // only those with water over all four — which is what lets the ground
        // mesh cut the waterline instead of the quad grid cutting it. A lake
        // over the corners below 8 therefore reaches the quad from 7 to 8,
        // whose far corners are dry.
        let mesh = water_mesh(IVec2::ZERO, &a_lake(12.0, 8)).expect("a lake");
        let positions = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .expect("positions")
            .as_float3()
            .expect("three floats each");

        let reach = positions
            .iter()
            .map(|point| point[0].max(point[2]))
            .fold(0.0f32, f32::max);
        assert_eq!(
            reach,
            8.0 * FACET_METRES,
            "the sheet stops at the last wet corner instead of running past it"
        );
    }

    #[test]
    fn ground_with_no_lake_on_it_draws_no_water() {
        assert!(water_mesh(IVec2::ZERO, &vec![NO_WATER; FACET_VERTS * FACET_VERTS]).is_none());

        // And a payload that carries no grid at all never gets as far as
        // asking — the common case, and the one that has to cost nothing.
        let dry = Arrival {
            chunk: IVec2::ZERO,
            heights: a_slope().heights.iter().copied().map(dequantize).collect(),
            surfaces: a_slope().surfaces,
            water: None,
            palms: Vec::new(),
        };
        assert!(chunk_meshes(&dry).water.is_none());
    }

    #[test]
    fn two_lakes_in_one_chunk_each_keep_their_own_level() {
        // A chunk can straddle two basins a hillside apart, which is the
        // whole reason the level travels per corner rather than per chunk.
        let mut water = vec![NO_WATER; FACET_VERTS * FACET_VERTS];
        for iz in 0..4 {
            for ix in 0..4 {
                water[iz * FACET_VERTS + ix] = quantize(6.0);
                water[iz * FACET_VERTS + ix + 20] = quantize(31.0);
            }
        }

        let mesh = water_mesh(IVec2::ZERO, &water).expect("two lakes");
        let positions = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .expect("positions")
            .as_float3()
            .expect("three floats each");

        for point in positions {
            let want = if point[0] < 10.0 * FACET_METRES {
                6.0 + OFF_LATTICE
            } else {
                31.0 + OFF_LATTICE
            };
            assert!(
                (point[1] - want).abs() < 1.0e-3,
                "{point:?} should stand at {want} m"
            );
        }
    }

    /// How far a height is from the nearest one a payload can say, in metres.
    /// Zero for anything on the lattice, [`OFF_LATTICE`] for anything as far
    /// off it as a height can be.
    fn off_the_lattice(y: f32) -> f32 {
        let steps = (y - protocol::ground::HEIGHT_FLOOR) / HEIGHT_STEP;
        (dequantize(steps.round() as u16) - y).abs()
    }

    #[test]
    fn no_sheet_of_water_lies_on_the_ground_s_own_lattice() {
        // Ground is drawn at quantised heights and nowhere in between, so
        // water standing at one of them is exactly coplanar with any flat
        // patch at that height rather than merely close to it — and coplanar
        // surfaces fight for the depth buffer. The sea's plane travels with
        // the camera, which re-rolls the fight every frame: a sandbar drawn at
        // the plane's own height flickers through the water. Half a step off
        // is the furthest from the lattice anything can stand.
        //
        // Held to half the lift rather than to the lift itself: what matters
        // is clearance from the lattice, and `dequantize` is arithmetic on
        // heights of a few hundred metres, so the exact gap at the top of the
        // range is a micron or two off the nominal one.
        for (what, y) in [
            ("the sea", SEA_SURFACE),
            ("the ocean floor", -OCEAN_DEPTH - SEA_FLOOR_CLEARANCE),
        ] {
            assert!(
                off_the_lattice(y) > OFF_LATTICE / 2.0,
                "{what} stands {} m from a height the ground can be drawn at",
                off_the_lattice(y)
            );
        }

        // And a lake, whose level is quantised on the very same lattice as the
        // bed it stands on.
        for level in [0.0, 0.5, 12.0, 137.5, 402.0] {
            let mesh = water_mesh(IVec2::ZERO, &a_lake(level, 8)).expect("a lake");
            let positions = mesh
                .attribute(Mesh::ATTRIBUTE_POSITION)
                .expect("positions")
                .as_float3()
                .expect("three floats each");
            for point in positions {
                assert!(
                    off_the_lattice(point[1]) > OFF_LATTICE / 2.0,
                    "a lake at {level} m stands {} m off ground its bed could be drawn at",
                    off_the_lattice(point[1])
                );
            }
        }
    }

    #[test]
    fn a_chunk_is_within_reach_by_its_nearest_point() {
        // Chunk (0,0) spans 0..128 on both axes.
        let chunk = IVec2::ZERO;
        assert!(within(chunk, Vec2::new(64.0, 64.0), 1.0), "inside is near");
        assert!(
            within(chunk, Vec2::new(200.0, 64.0), 80.0),
            "80 m off the east edge"
        );
        assert!(
            !within(chunk, Vec2::new(200.0, 64.0), 60.0),
            "60 m short of the east edge"
        );
    }
}
