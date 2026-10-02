//! Putting the world on screen: chunks as they are sent, and the sea they
//! stand in. The light they are all lit by is [`crate::sky`]'s.
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
//!
//! # How a square metre becomes triangles
//!
//! Every drawn cell is split in two and the pair flat-shaded: both triangles
//! carry the cell's own colour and the normal of the square they came from, so
//! a cell reads as one flat lozenge rather than as two triangles that happen
//! to match. A corner therefore belongs to one cell, and the same point of
//! ground is four different vertices where four cells meet — the price of the
//! look, paid in vertices rather than in the texture and gradient machinery
//! the look exists to refuse. It also means chunks need no border samples to
//! meet cleanly, there being no shared normals to disagree about.
//!
//! *Within* a cell the two triangles agree about everything, so a cell is four
//! vertices and six indices rather than six vertices. That is a third off the
//! vertex count, and the shadow pass — which rasterises this geometry once per
//! cascade it falls into and does nothing per vertex but transform it — is the
//! half of the frame that feels it. It only became possible when the colour
//! moved from the triangle to the cell: while each triangle had its own
//! palette entry the two halves disagreed, and an index buffer would have been
//! 0, 1, 2, 3, … and saved nothing.
//!
//! # Drawing the same ground at different densities
//!
//! How finely a chunk is *drawn* is this end's alone: the wire says what a
//! square metre is made of and holds no opinion about triangles. So a
//! [`Detail`] is never a request for anything, only a decision to throw away
//! samples already in hand — nothing is fetched again and nothing new is held.
//!
//! Two rules make the coarse cuts safe to hand out.
//!
//! Corners are **decimated, not averaged**: a drawn corner is every S-th
//! corner the payload sent, so it is a height the ground actually has. Coarse
//! and fine sheets then touch exactly wherever they share a corner, the last
//! corner lands on the chunk's own edge however coarse the cut, and the error
//! between the two is bounded on *both* sides. Averaging, or taking the
//! tallest of each block, would put the whole sheet off the ground in one
//! direction — for a shadow caster that is a hillside systematically wrongly
//! lit, where a two-sided error is a hairline at the ridges that the depth
//! bias already covers.
//!
//! And [`height_at`] never sees a [`Detail`] at all. Everything physical —
//! the waterline a hull rides, the ground a walker is held to — reads the
//! payload's own grid whatever is drawn over it, or a hull would rise and
//! settle as the camera pulled back.

use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::light::{
    CascadeShadowConfig, CascadeShadowConfigBuilder, NotShadowCaster, NotShadowReceiver,
};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, Extent3d, TextureDimension, TextureFormat};
use bevy::tasks::{block_on, futures_lite::future, AsyncComputeTaskPool, Task};

use protocol::ground::{
    cell_height, chunk_at, dequantize, lit_across, material_index, shelter_across, split,
    ChunkPayload, Exposure, Material, Plant, CELLS, CELL_METRES, CHUNK_METRES, CORNERS,
    HEIGHT_STEP, LAKE_WATER, LIT_ALL_DAY, NO_WATER, OCEAN_DEPTH, SEA_WATER, SHELTER_CELLS,
    SHELTER_CORNERS, SHELTER_METRES,
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
/// draws is one of the values `HEIGHT_FLOOR + n * HEIGHT_STEP`. A plane at a
/// whole number of steps is therefore *exactly* coplanar with flat ground at
/// that height, and coplanar surfaces fight over the depth buffer: both planes
/// below travel with the camera, so the fight is re-rasterised every frame and
/// flickers.
///
/// A whole number of steps is what both clearances used to be, and 2 cm of
/// quantisation is coarse enough that real ground lands on a given step often
/// enough to see. Half a step off, no ground can ever be nearer than a
/// centimetre, which the depth buffer resolves everywhere the haze lets
/// anything be seen.
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
/// and lift the shallows, and no more — and over the sea it is only the
/// starting point; see `sea::MURK`.
const WATER_ALPHA: f32 = 0.84;

/// How far out from the camera's focus chunks are wanted, in metres.
///
/// Worth deriving rather than guessing at, being the single number that decides
/// how much ground the machine holds and a server is asked to send. At the
/// furthest zoom the eye sits about 235 m back from the focus horizontally and
/// 298 m above it; the haze closes at [`crate::HAZE_END`], 900 m through the
/// air, so the furthest visible ground is `sqrt(900² - 298²) ≈ 849 m` from the
/// eye and up to about 1084 m from the focus.
///
/// That extreme sits directly *behind* the camera, the offset only adding to
/// the reach in the one direction the view is not looking, so 1024 m covers
/// everything in shot with room over. What is left of the gap is chunk
/// granularity: [`within`] measures to a chunk's nearest corner.
///
/// Public because the chart's own reach is measured against it: a chunk the
/// survey can see has to be one this has already brought in.
pub const STREAM_RADIUS: f32 = 1024.0;

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
/// Two rather than the eight it stood at when a chunk's mesh was a quarter
/// this size: the cap bounds *bytes* handed to the GPU in one frame, and at
/// ~4.5 MB per land chunk two holds the budget eight was chosen for.
const MESHES_PER_FRAME: usize = 2;

pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            MaterialPlugin::<SeaMaterial>::default(),
            MaterialPlugin::<ShadedMaterial>::default(),
        ))
        .add_message::<crate::net::GroundArrived>()
        .add_message::<crate::net::WindChanged>()
        .init_resource::<sea::Forecast>()
        .init_resource::<sea::SeaConditions>()
        .add_systems(OnEnter(AppState::InWorld), enter_world)
        .add_systems(OnExit(AppState::InWorld), leave_world)
        .add_systems(
            Update,
            (
                // First, and in the wire's own set: a chunk that arrived this
                // frame is drawn this frame rather than a frame later.
                take_the_answers.in_set(crate::net::Wire::Read),
                ask_for_ground,
                spawn_arrivals,
                receive_chunks,
                stream_out,
                follow_camera,
                refresh_materials,
                sea::refresh_depth,
                // The wind heard, then worn: the sea this module draws is the
                // sea `sea` keeps, so its two plugins are one.
                sea::take_the_weather.in_set(crate::net::Wire::Read),
                sea::settle_conditions,
            )
                .chain()
                .run_if(in_state(AppState::InWorld).and_then(resource_exists::<Ground>)),
        )
        // After the sky has settled the drawn hour, so the shadows on the
        // ground answer to the same instant as the light over it.
        .add_systems(
            Update,
            shade_the_ground
                .after(crate::sky::advance_the_day)
                .run_if(in_state(AppState::InWorld)),
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
    materials: Arc<[Material]>,
    /// When each corner sees the sun, exactly as sent — turned into the mesh
    /// attribute the shader reads by [`chunk_mesh`].
    lit: Arc<[[u8; 2]]>,
    /// The chunk's standing water, still quantised: it is only ever compared
    /// against [`NO_WATER`] and turned into a height once per quad drawn, so
    /// there is nothing to be gained by dequantising a whole grid of it the
    /// way the corner heights are.
    water: Option<Vec<u16>>,
    /// What grows on it. Carried through untouched and handed on — what a
    /// plant of a given kind looks like is `trees`' business, not this
    /// module's.
    plants: Vec<Plant>,
}

/// The plants a chunk arrived carrying, waiting to be stood up.
///
/// A component rather than an argument because the two halves belong to
/// different modules: this one knows when a chunk's ground exists, and
/// `trees` knows what a tree is. The component is how the first tells the
/// second, and `trees` takes it off again once it has planted them.
#[derive(Component)]
pub struct PendingPlants(pub Vec<Plant>);

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
    /// nothing here to draw *with* — but there may still be a wind to
    /// report, since the water an island's headlands shelter is mostly this:
    /// bare bed with no mesh worth building. `None` is water open to the
    /// wind, which is most of the sea.
    Ocean { shelter: Option<Arc<[Exposure]>> },
    /// Ground, with the corner heights it was sent — the same grid its mesh is
    /// built from, so anything standing on it stands on what can be seen.
    Land {
        heights: Arc<[f32]>,
        /// When each corner sees the sun, kept as sent — see
        /// [`protocol::ground::ChunkPayload::lit`].
        ///
        /// The mesh has its own copy in a vertex attribute, and this is not
        /// that: the *sea* is one plane that never met a chunk's corners, so
        /// what shades it reads the intervals back out of here by world
        /// point — see [`crate::sea::refresh_depth`]. Two bytes a corner
        /// against the heights' four, on chunks that already cost a mesh.
        lit: Arc<[[u8; 2]]>,
        /// What each cell is made of, kept as sent for the same reason `lit`
        /// is: the material window re-reads it by world point long after the
        /// mesh has baked its own copy into vertex colour — see
        /// [`refresh_materials`].
        materials: Arc<[Material]>,
        /// How exposed the chunk is to the wind from each quarter, kept as
        /// sent — see [`protocol::ToClient::Chunk`], and `None` for ground
        /// nothing shelters. Nothing bakes a copy of this into a mesh the
        /// way the light is: shelter is read by world point, by the water and
        /// by whatever is sailing on it, and both of those are asking about
        /// places no chunk's corners ever reached.
        shelter: Option<Arc<[Exposure]>>,
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
    /// it can wait: dequantising sixteen thousand corners is arithmetic,
    /// meshing is ninety-eight thousand vertices.
    pub fn deliver(
        &mut self,
        chunk: IVec2,
        shelter: Option<Vec<Exposure>>,
        payload: Option<ChunkPayload>,
    ) {
        self.outstanding.remove(&chunk);
        let shelter: Option<Arc<[Exposure]>> = shelter.map(Into::into);
        match payload {
            None => {
                self.chunks.insert(chunk, Chunk::Ocean { shelter });
            }
            Some(payload) => {
                let heights: Arc<[f32]> = payload.heights.iter().copied().map(dequantize).collect();
                let lit: Arc<[[u8; 2]]> = payload.lit.into();
                let materials: Arc<[Material]> = payload.materials.into();
                self.chunks.insert(
                    chunk,
                    Chunk::Land {
                        heights: heights.clone(),
                        lit: lit.clone(),
                        materials: materials.clone(),
                        shelter,
                        mesh: None,
                    },
                );
                self.arrived.push(Arrival {
                    chunk,
                    heights,
                    materials,
                    lit,
                    water: payload.water,
                    plants: payload.plants,
                });
            }
        }
    }

    /// The chunks waiting to be asked for, taken away.
    pub fn take_requests(&mut self) -> Vec<IVec2> {
        std::mem::take(&mut self.to_ask)
    }

    /// Whether everything asked for has arrived and been handed to a mesh
    /// builder. What the socket waits on before it answers — `shot`, the view
    /// verbs, and any line the world answered by putting the player down
    /// somewhere else — because the picture is not of the world until the
    /// world has turned up.
    pub fn settled(&self) -> bool {
        self.outstanding.is_empty() && self.to_ask.is_empty() && self.arrived.is_empty()
    }

    /// How much of the world is being held, and how much is still on its way.
    /// For the debug overlay, which is the only thing that wants the world
    /// counted rather than asked about.
    pub fn tally(&self) -> Tally {
        Tally {
            ground: self.land_held(),
            ocean: self
                .chunks
                .values()
                .filter(|chunk| matches!(chunk, Chunk::Ocean { .. }))
                .count(),
            requested: self.outstanding.len() + self.to_ask.len(),
        }
    }

    /// How many chunks of ground are held — [`Tally::ground`] on its own, for
    /// the caller that wants only that and wants it every frame. Counting the
    /// whole tally to read one field walks the map three times over.
    pub fn land_held(&self) -> usize {
        self.chunks
            .values()
            .filter(|chunk| matches!(chunk, Chunk::Land { .. }))
            .count()
    }

    /// The height of the surface at a world point: the ground where it stands
    /// above the sea, and the waterline where it does not.
    ///
    /// What everything riding the world rides — the boat, the camera centred
    /// on it, and the markers other players stand as — so that a hull crossing
    /// open ocean floats rather than walking the seabed, and one that has run
    /// aground sits in the hillside.
    ///
    /// Read off the facet grid the mesh is built from — see [`height_at`] —
    /// so a hull sits on the ground that can be seen under it.
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
            Chunk::Ocean { .. } => Some(-OCEAN_DEPTH),
            Chunk::Land { heights, .. } => {
                Some(height_at(heights, at - chunk.as_vec2() * CHUNK_METRES))
            }
        }
    }

    /// The height of one lattice corner as it was sent — what
    /// [`protocol::ground::lattice_height`] and the keel reading built on it
    /// ask of this machine, the server answering the same question of its own
    /// world. `None` where the chunk that owns the corner has not arrived.
    pub fn corner(&self, corner: IVec2) -> Option<f32> {
        let cells = IVec2::splat(CELLS as i32);
        match self.chunks.get(&corner.div_euclid(cells))? {
            Chunk::Ocean { .. } => Some(-OCEAN_DEPTH),
            Chunk::Land { heights, .. } => {
                let local = corner.rem_euclid(cells).as_uvec2();
                Some(heights[local.y as usize * CORNERS + local.x as usize])
            }
        }
    }

    /// When the surface at a world point sees the sun, read between the four
    /// corners around it — see [`protocol::ground::ChunkPayload::lit`].
    ///
    /// [`LIT_ALL_DAY`] over open water and `None` for a chunk that has not
    /// arrived, which are different answers to different questions: the sea
    /// between islands is lit because nothing stands over it, while a chunk
    /// still on its way is not yet anything. Both end up drawn as full
    /// daylight — see [`crate::sea::refresh_depth`], which is what asks —
    /// because ground that has not arrived cannot be shadowing anything the
    /// eye can see either.
    ///
    /// Read across the corner grid by [`protocol::ground::lit_across`] —
    /// which owns how a never-lit corner is weighed — rather than down the
    /// facet split [`height_at`] uses: an interval belongs to the ground
    /// around a point rather than to the triangle under it, and how a cell
    /// happens to be cut has nothing to say about it.
    pub fn lit(&self, x: f32, z: f32) -> Option<[u8; 2]> {
        let at = Vec2::new(x, z);
        let chunk = chunk_at(at);
        let lit = match self.chunks.get(&chunk)? {
            Chunk::Ocean { .. } => return Some(LIT_ALL_DAY),
            Chunk::Land { lit, .. } => lit,
        };
        let local = (at - chunk.as_vec2() * CHUNK_METRES) / CELL_METRES;
        // Inside the chunk by construction — `chunk_at` is what put it here —
        // so the far corner of the cell is the chunk's own, never the
        // neighbour's, and the two chunks either side of a boundary read the
        // same value there anyway.
        let (x0, z0) = (
            (local.x.floor() as usize).min(CELLS - 1),
            (local.y.floor() as usize).min(CELLS - 1),
        );
        let corner = |cx: usize, cz: usize| lit[cz * CORNERS + cx];
        Some(lit_across(
            [
                corner(x0, z0),
                corner(x0 + 1, z0),
                corner(x0, z0 + 1),
                corner(x0 + 1, z0 + 1),
            ],
            Vec2::new(local.x - x0 as f32, local.y - z0 as f32),
        ))
    }

    /// How much of the wind reaches a world point when it is blowing
    /// `toward` — `1.0` in the open, smaller in some island's lee, and
    /// `None` for a chunk that has not arrived.
    ///
    /// Fully exposed over open water, `None` for a chunk still on its way:
    /// the same split of answers [`Ground::lit`] makes and for the same
    /// reason. Every caller reads the `None` as open water — see
    /// [`crate::sea::SeaConditions::exposure`], which is where that is
    /// decided once for the sea and the hull alike.
    ///
    /// Read on the shelter lattice, which is far coarser than the corner grid
    /// [`Ground::lit`] walks — see [`protocol::ground::SHELTER_METRES`] — and
    /// blended by [`shelter_across`], which owns both halves of the blend.
    pub fn exposure(&self, x: f32, z: f32, toward: Vec2) -> Option<f32> {
        let at = Vec2::new(x, z);
        let chunk = chunk_at(at);
        let shelter = match self.chunks.get(&chunk)? {
            Chunk::Ocean { shelter } | Chunk::Land { shelter, .. } => shelter.as_ref(),
        };
        // No lattice is the answer "nothing shelters this", which a chunk of
        // any kind may give — see [`protocol::ToClient::Chunk`].
        let Some(shelter) = shelter else {
            return Some(1.0);
        };
        let local = (at - chunk.as_vec2() * CHUNK_METRES) / SHELTER_METRES;
        // Clamped exactly as [`Ground::lit`] clamps, and the far point of the
        // cell is the chunk's own last lattice point rather than the
        // neighbour's — which both chunks store the same value for anyway,
        // which is what [`SHELTER_CORNERS`] is for.
        let (x0, z0) = (
            (local.x.floor() as usize).min(SHELTER_CELLS - 1),
            (local.y.floor() as usize).min(SHELTER_CELLS - 1),
        );
        let point = |cx: usize, cz: usize| shelter[cz * SHELTER_CORNERS + cx];
        Some(shelter_across(
            [
                point(x0, z0),
                point(x0 + 1, z0),
                point(x0, z0 + 1),
                point(x0 + 1, z0 + 1),
            ],
            Vec2::new(local.x - x0 as f32, local.y - z0 as f32),
            toward,
        ))
    }

    /// What the ground is made of at a world point — the cell's own material,
    /// no interpolation, since a material is a name rather than a quantity.
    /// [`Material::Seabed`] over open water and `None` for a chunk that has
    /// not arrived, the same split of answers [`Ground::lit`] makes and for
    /// the same reason: the sea between islands *is* bed, while a chunk still
    /// on its way is not yet anything.
    pub fn material(&self, x: f32, z: f32) -> Option<Material> {
        let at = Vec2::new(x, z);
        let chunk = chunk_at(at);
        let materials = match self.chunks.get(&chunk)? {
            Chunk::Ocean { .. } => return Some(Material::Seabed),
            Chunk::Land { materials, .. } => materials,
        };
        let local = ((at - chunk.as_vec2() * CHUNK_METRES) / CELL_METRES).floor();
        // Clamped for the same reason [`Ground::lit`] clamps: a point a whisker
        // under the chunk's far edge can round its local coordinate up to the
        // edge itself, which is off the grid and would panic below.
        let edge = CELLS as i32 - 1;
        let index = material_index((local.x as i32).min(edge), (local.y as i32).min(edge))
            .expect("a point of the chunk is on its own material grid");
        Some(materials[index])
    }

    /// One chunk's corner heights as they were sent, or `None` for open water
    /// and for chunks that have not arrived.
    ///
    /// The whole grid rather than a height at a point, because the one caller
    /// wants to walk it: the chart traces the waterline across a chunk before
    /// streaming forgets it — see [`crate::chart`]. Cloned, which is an
    /// `Arc` bump, so the reader is not holding the world still while it works.
    pub fn heights(&self, chunk: IVec2) -> Option<Arc<[f32]>> {
        match self.chunks.get(&chunk)? {
            Chunk::Ocean { .. } => None,
            Chunk::Land { heights, .. } => Some(heights.clone()),
        }
    }

    /// The highest corner of one chunk's height grid: where it stands in the
    /// world, and how high. `None` for open water and for chunks that have
    /// not arrived. What the wildlife asks when it is looking for a summit —
    /// see `wildlife::eyrie` for how a peak on the chunk's border is read.
    pub fn peak(&self, chunk: IVec2) -> Option<(Vec2, f32)> {
        let Chunk::Land { heights, .. } = self.chunks.get(&chunk)? else {
            return None;
        };
        let (highest, height) = heights
            .iter()
            .copied()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(&b.1))?;
        let local = Vec2::new((highest % CORNERS) as f32, (highest / CORNERS) as f32) * CELL_METRES;
        Some((chunk.as_vec2() * CHUNK_METRES + local, height))
    }

    /// Whether any corner of one chunk stands above the waterline. `false` for
    /// open water, for chunks that have not arrived, and for the drowned shelf
    /// the server sends around an island.
    ///
    /// [`Ground::peak`] answers this as well, but it reads the whole grid to
    /// find the highest corner and a caller asking only whether there is land
    /// here is done at the first corner above the water.
    pub fn above_water(&self, chunk: IVec2) -> bool {
        let Some(Chunk::Land { heights, .. }) = self.chunks.get(&chunk) else {
            return false;
        };
        heights.iter().any(|height| *height > 0.0)
    }
}

/// How coarsely a chunk is drawn: a halving of the payload's own grid at each
/// level, so [`Detail::FINEST`] draws every cell the wire sent and level two
/// lays one drawn cell over sixteen of them. See the module header for what a
/// level does and does not mean.
///
/// A level rather than a stride, so that every value is one the grid can
/// actually be cut on — a stride that did not divide [`CELLS`] would leave a
/// ragged row along two sides of every chunk.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Detail(u32);

impl Detail {
    /// Every cell the payload carries, drawn. What the ground has always been
    /// drawn at, and what anything physical is still read off.
    pub const FINEST: Self = Self(0);

    /// The coarsest cut leaving an *even* number of cells to a chunk's edge,
    /// which is what [`split`]'s checkerboard needs. Two cells, far past
    /// anything worth drawing: the bound is here to make the parity a fact
    /// about the type rather than a thing to remember.
    pub const COARSEST: Self = Self(CELLS.trailing_zeros() - 1);

    /// The level `steps` halvings coarser than the finest, held inside what
    /// the grid can be cut on. Clamped rather than refused: a selector asking
    /// for more than the grid has is asking for the coarsest there is.
    pub fn new(steps: u32) -> Self {
        Self(steps.min(Self::COARSEST.0))
    }

    /// Cells of the payload's own grid that go into one drawn cell, along each
    /// edge.
    pub fn stride(self) -> usize {
        1 << self.0
    }

    /// Drawn cells along a chunk's edge.
    pub fn cells(self) -> usize {
        CELLS >> self.0
    }

    /// Metres along a drawn cell's edge.
    pub fn metres(self) -> f32 {
        self.stride() as f32 * CELL_METRES
    }
}

/// The height of one point inside a chunk, interpolated across the triangle
/// the payload's own grid puts there.
///
/// The payload's own grid whatever is drawn over it — see the module header.
/// Which triangle a point falls in comes from [`split`], which is also what
/// [`chunk_mesh`] builds from, so the two agree by construction where they
/// read the same grid. `local` is metres from the chunk's own corner.
fn height_at(heights: &[f32], local: Vec2) -> f32 {
    let cell = local / CELL_METRES;
    // Clamped rather than trusted: a point exactly on a chunk's far edge
    // belongs to the next chunk, but the arithmetic that got here is `f32` and
    // is entitled to land on the boundary itself.
    let ix = (cell.x.floor().max(0.0) as usize).min(CELLS - 1);
    let iz = (cell.y.floor().max(0.0) as usize).min(CELLS - 1);
    let corner = |cx: usize, cz: usize| heights[cz * CORNERS + cx];
    cell_height(
        IVec2::new(ix as i32, iz as i32),
        [
            corner(ix, iz),
            corner(ix + 1, iz),
            corner(ix, iz + 1),
            corner(ix + 1, iz + 1),
        ],
        cell - Vec2::new(ix as f32, iz as f32),
    )
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
struct GroundMaterial(Handle<ShadedMaterial>);

/// The one material every lake shares, so that all the standing water in a
/// view still batches into a single draw call however many chunks it crosses.
///
/// The sea has no resource of its own: it is a single plane, spawned once with
/// its material and never asked for again. This is kept because a lake arrives
/// with its chunk and has to be given the water it is made of at that moment.
#[derive(Resource)]
struct LakeMaterial(Handle<ShadedMaterial>);

/// The ocean-floor backdrop's material — [`Shading::bed`], which is why it
/// cannot share [`GroundMaterial`]. Kept only so [`shade_the_ground`] can
/// reach its hour.
#[derive(Resource)]
struct BedMaterial(Handle<ShadedMaterial>);

/// What the ground and the lakes are drawn in: the standard matte underneath,
/// with the terrain's own baked shadows applied on top by
/// `assets/shaders/ground.wgsl` — see [`Shading`].
type ShadedMaterial = ExtendedMaterial<StandardMaterial, Shading>;

/// How wide the moment of gaining or losing the sun is drawn, in phase either
/// side of a vertex's own threshold — a step and a half of the wire's 256, a
/// few seconds of the ten-minute day. Wide enough that the terminator sweeps
/// rather than snapping, and that the interval's quantisation stays
/// unreadable; narrow enough that a shadow's edge is still an edge.
const SHADE_EDGE: f32 = 1.5 / 256.0;

/// The ground's grain — the speckle `assets/shaders/ground.wgsl` hashes from
/// world position, which is what stands in for a texture: the full swing one
/// cell may pull the palette colour, as a fraction of it. The cell's width
/// lives shader-side as its `CELL`, nothing varying it per material.
const GRAIN_SWING: f32 = 0.15;

/// What the ground's shader needs beyond the standard material: the hour, to
/// hold against the lit interval every vertex carries in its UV channel —
/// see [`chunk_mesh`] for how it gets there, and
/// [`protocol::ground::ChunkPayload::lit`] for what it means — and the
/// material window it dithers boundaries out of, see [`MaterialWindow`].
///
/// The order of the uniform fields *is* the uniform's layout, and the
/// shader's `Shading` has to name them in the same order — held to it by
/// `the_shader_reads_the_uniform_in_the_order_it_is_written`.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
struct Shading {
    /// `x` is the phase to ask the intervals about: the drawn hour by day
    /// and its mirror by night, when the moon rides the same arc half a day
    /// out of phase — the same swap the sky makes of the light itself.
    /// Written every frame by [`shade_the_ground`]. `y` is [`SHADE_EDGE`];
    /// `z` is [`GRAIN_SWING`] on the ground and zero on the waters, which a
    /// swing of nothing leaves smooth; `w` is how far out, in metres of view
    /// depth, the shader draws the shadow pass's cast shadows hard —
    /// [`CASCADE_SPLIT`], easing back to the filter's own softness by there,
    /// so the hardening never re-thresholds the cascades' cross-fade — and
    /// zero on the bed, which takes no cast shadow at all: it lies under
    /// metres of water, with nothing over it to have cast one.
    #[uniform(100)]
    hour: Vec4,
    /// The material window's place in the world: `xy` the world coordinates
    /// of its corner texel's corner, `z` its width in metres — a texel is a
    /// metre, so the two are one number — and `w` [`WINDOW_MARGIN`].
    /// Rewritten whenever the window scrolls; all zeroes on surfaces that
    /// never look, which a width of nothing keeps out of the dither.
    #[uniform(100)]
    window: Vec4,
    /// What each material is drawn as, in linear colour, indexed by the
    /// number a window texel carries less one — built by exactly the
    /// conversion the mesh's vertex colours go through, so a picked cell and
    /// the ground beside it cannot disagree by so much as a rounding step.
    #[uniform(100)]
    palette: [Vec4; Material::KINDS],
    /// The window's texels — see [`MaterialWindow`], which owns the scroll
    /// and the sweep that keep them current. Bound on every making, since a
    /// binding cannot be optional; only the ground reads it.
    #[texture(101, sample_type = "u_int")]
    materials: Handle<Image>,
}

/// The three makings a shaded surface can have, and no fourth for
/// `..default()` to reach — whoever adds a surface says which it is.
impl Shading {
    /// The ground's: noon still, the grain switched on, and the window
    /// opened around the focus.
    fn ground(materials: Handle<Image>, focus: Vec2) -> Self {
        Self {
            hour: DAYLIGHT_AT_NOON.with_z(GRAIN_SWING).with_w(CASCADE_SPLIT),
            window: Self::window_uniform(MaterialWindow::origin_under(focus)),
            palette: drawn_palette(),
            materials,
        }
    }

    /// A water's: noon, and no grain — a sheet of water is one tone.
    fn water(materials: Handle<Image>) -> Self {
        Self {
            hour: DAYLIGHT_AT_NOON.with_w(CASCADE_SPLIT),
            window: Vec4::ZERO,
            palette: drawn_palette(),
            materials,
        }
    }

    /// The backdrop bed's: grained like the ground, but no window — the
    /// window says what the drawn *surface* over a column is made of, and
    /// the plane lies metres beneath it, where an arriving island's grass
    /// must not print through the water while its meshes are still being
    /// built.
    fn bed(materials: Handle<Image>) -> Self {
        Self {
            hour: DAYLIGHT_AT_NOON.with_z(GRAIN_SWING),
            window: Vec4::ZERO,
            palette: drawn_palette(),
            materials,
        }
    }

    fn window_uniform(origin: Vec2) -> Vec4 {
        Vec4::new(origin.x, origin.y, WINDOW_EXTENT, WINDOW_MARGIN)
    }
}

/// Every material's drawn colour, in the order their numbers give — see
/// [`Shading::palette`].
fn drawn_palette() -> [Vec4; Material::KINDS] {
    std::array::from_fn(|kind| {
        let srgb = Material::from_byte(kind as u8)
            .expect("a number below KINDS names a material")
            .color();
        let linear = Color::srgb(srgb.x, srgb.y, srgb.z).to_linear();
        Vec4::new(linear.red, linear.green, linear.blue, 1.0)
    })
}

/// The hour every surface opens at, before the sky has spoken: noon, which
/// is the daylight the menus are lit by — the base both [`Shading`]
/// constructors build on, and what the sea's own copy carries whole.
pub(crate) const DAYLIGHT_AT_NOON: Vec4 = Vec4::new(0.5, SHADE_EDGE, 0.0, 0.0);

impl MaterialExtension for Shading {
    fn fragment_shader() -> bevy::shader::ShaderRef {
        "shaders/ground.wgsl".into()
    }
}

/// Carries the drawn hour into every material that shades itself from the
/// baked intervals, mirrored onto the moon's half of the day when the moon is
/// the body up — see [`Shading::hour`].
///
/// The sea is written here with the ground and the lakes, though it reads its
/// intervals from a window rather than from its own vertices: what all three
/// need is the same hour, and two systems writing it would be two chances for
/// the water and the shore beside it to be at different times of day.
fn shade_the_ground(
    sky: Res<crate::sky::Sky>,
    ground: Option<Res<GroundMaterial>>,
    lake: Option<Res<LakeMaterial>>,
    bed: Option<Res<BedMaterial>>,
    window: Option<Res<sea::DepthWindow>>,
    mut materials: ResMut<Assets<ShadedMaterial>>,
    mut seas: ResMut<Assets<SeaMaterial>>,
) {
    let phase = sky.phase();
    let hour = if protocol::is_night(phase) {
        (phase + 0.5).rem_euclid(1.0)
    } else {
        phase
    };
    let handles = [
        ground.map(|it| it.0.clone()),
        lake.map(|it| it.0.clone()),
        bed.map(|it| it.0.clone()),
    ];
    for handle in handles.into_iter().flatten() {
        if let Some(mut material) = materials.get_mut(&handle) {
            material.extension.hour.x = hour;
        }
    }
    if let Some(mut sea) = window.and_then(|window| seas.get_mut(window.material())) {
        // The mirrored hour is the moon's hour, so this is the moon's bearing
        // by night — the same body the light is hung from. Never zero: the
        // arc leans off the pole (`protocol::SUN_TILT`), so the body always
        // stands somewhere on the map.
        let body = protocol::towards_the_sun(hour);
        let bearing = Vec2::new(body.x, body.z).normalize_or_zero();
        sea.extension.daylight = Vec4::new(hour, sea.extension.daylight.y, bearing.x, bearing.y);
    }
}

// ---------------------------------------------------------------------------
// The material window
// ---------------------------------------------------------------------------

/// Texels along each side of the material window, at a texel per
/// [`CELL_METRES`]. A kilometre square around the focus: the dither it feeds
/// is a boundary effect the grain's own fade has finished with long before
/// the far half of that, so the window ends well past the last place its
/// absence could be seen.
const WINDOW_TEXELS: usize = 1024;

/// Width of the material window in metres.
const WINDOW_EXTENT: f32 = WINDOW_TEXELS as f32 * CELL_METRES;

/// How far inside the window's edge the dither has fully handed back to each
/// cell's own colour, in metres. The strip a scroll exposes is stale until
/// the sweep reaches it, and the margin keeps anything read near an edge
/// deterministic — a boundary fringe that snapped between dithered and flat
/// as the window scrolled would be the window made visible.
///
/// It is also the far edge of the shader's fade-in, whose near edge is the
/// standoff of half a grain cell plus one. Below that the fade would run
/// backwards, and a `smoothstep` whose edges cross has no answer by spec —
/// so the assertion below is what keeps the ground off the same footing the
/// waters are deliberately gated onto; see `assets/shaders/ground.wgsl`.
const WINDOW_MARGIN: f32 = 64.0;

/// Half a grain cell plus one — the shader's `0.5 + CELL`, in the metres this
/// side counts in. Only this assertion reads it; the shader has its own copy
/// of `CELL`, which is the one that varies nothing.
const _: () = assert!(WINDOW_MARGIN > 0.5 + 0.5);

/// How far the camera may drift from the window's centre before the window
/// is scrolled back under it, in metres — the same decoupling the sea's
/// depth window argues at its own [`sea::DepthWindow`]-side constant.
const WINDOW_RECENTER: f32 = 128.0;

/// Rows of the window refilled from the ground each frame, round and round —
/// the whole window re-read in about a second, which is how arriving chunks,
/// forgotten chunks and scroll-exposed strips all heal with nothing keeping
/// track of what changed. The sweep never straddles the wrap; see
/// `the_sweep_divides_the_window`.
const WINDOW_SWEEP_ROWS: usize = 16;

/// A texel of the window is one byte: a material's number plus one, so that
/// zero can mean a chunk not yet arrived. The *identity* rather than the
/// colour, because odds are the kind of thing a later change keys on a
/// material's name — a dominance order, a per-material fringe — and the
/// colour is one small uniform lookup away; see [`Shading::palette`].
const WINDOW_TEXEL_BYTES: usize = 1;

/// A texel for ground the client has not been told about.
const UNKNOWN: [u8; WINDOW_TEXEL_BYTES] = [0];

/// The picture of what the ground is made of around the camera that the
/// ground shader dithers material boundaries out of: a palette colour per
/// cell, scrolled to follow the camera and perpetually re-read from
/// [`Ground`] by [`refresh_materials`] — the sea's depth window pattern
/// exactly, argued at [`sea::DepthWindow`], down to holding both handles
/// because the image and the material's idea of where it sits must move in
/// the same frame.
#[derive(Resource)]
struct MaterialWindow {
    image: Handle<Image>,
    material: Handle<ShadedMaterial>,
    /// World coordinates of the corner of texel (0, 0), a whole number of
    /// metres always, so texels stay pinned to the cells they were read from
    /// across every scroll.
    origin: Vec2,
    /// The row the round-robin sweep refills next.
    sweep: usize,
}

impl MaterialWindow {
    /// The window origin that centres the window on a focus, on the cell
    /// grid.
    fn origin_under(focus: Vec2) -> Vec2 {
        (focus / CELL_METRES).floor() * CELL_METRES - WINDOW_EXTENT / 2.0
    }
}

/// A window with nothing in it yet — see [`UNKNOWN`].
fn material_image() -> Image {
    Image::new_fill(
        Extent3d {
            width: WINDOW_TEXELS as u32,
            height: WINDOW_TEXELS as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &UNKNOWN,
        TextureFormat::R8Uint,
        RenderAssetUsages::default(),
    )
}

/// One texel's worth of what a cell is made of — see [`WINDOW_TEXEL_BYTES`].
fn material_texel(material: Option<Material>) -> [u8; WINDOW_TEXEL_BYTES] {
    match material {
        None => UNKNOWN,
        Some(material) => [material as u8 + 1],
    }
}

/// Keeps the material window under the camera and its texels agreeing with
/// the ground. The scroll and the sweep, exactly as [`sea::refresh_depth`]
/// runs them and for the reasons argued there.
fn refresh_materials(
    ground: Res<Ground>,
    cameras: Query<&MapCamera>,
    mut window: ResMut<MaterialWindow>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<ShadedMaterial>>,
) {
    let Ok(camera) = cameras.single() else {
        return;
    };
    let focus = Vec2::new(camera.focus.x, camera.focus.z);

    // The scroll.
    let drift = focus - (window.origin + WINDOW_EXTENT / 2.0);
    if drift.x.abs() > WINDOW_RECENTER || drift.y.abs() > WINDOW_RECENTER {
        let origin = MaterialWindow::origin_under(focus);
        let step = ((origin - window.origin) / CELL_METRES).round().as_ivec2();
        if let Some(mut image) = images.get_mut(&window.image) {
            if let Some(data) = image.data.as_mut() {
                scroll_texels(data, WINDOW_TEXELS, WINDOW_TEXEL_BYTES, &UNKNOWN, step);
            }
        }
        window.origin = origin;
        if let Some(mut material) = materials.get_mut(&window.material) {
            material.extension.window = Shading::window_uniform(origin);
        }
    }

    // The sweep. Read into a scratch first and compare, so a frame that
    // changed nothing marks nothing changed and re-uploads nothing.
    let mut rows = [0u8; WINDOW_TEXELS * WINDOW_SWEEP_ROWS * WINDOW_TEXEL_BYTES];
    for r in 0..WINDOW_SWEEP_ROWS {
        let row = window.sweep + r;
        for x in 0..WINDOW_TEXELS {
            let at = window.origin + Vec2::new(x as f32 + 0.5, row as f32 + 0.5) * CELL_METRES;
            let texel = material_texel(ground.material(at.x, at.y));
            let into = (r * WINDOW_TEXELS + x) * WINDOW_TEXEL_BYTES;
            rows[into..into + WINDOW_TEXEL_BYTES].copy_from_slice(&texel);
        }
    }
    let start = window.sweep * WINDOW_TEXELS * WINDOW_TEXEL_BYTES;
    let stale = |data: &[u8]| data[start..start + rows.len()] != rows[..];
    if images
        .get(&window.image)
        .and_then(|image| image.data.as_deref())
        .is_some_and(stale)
    {
        if let Some(mut image) = images.get_mut(&window.image) {
            if let Some(data) = image.data.as_mut() {
                data[start..start + rows.len()].copy_from_slice(&rows);
            }
        }
    }
    window.sweep = (window.sweep + WINDOW_SWEEP_ROWS) % WINDOW_TEXELS;
}

/// Shifts a square window's texels so that texel `(x, y)` afterwards holds
/// what texel `(x, y) + step` held before — the data moves opposite to the
/// window, which is what keeps each surviving texel over the same piece of
/// world. Texels that slide in from beyond the old window are set to
/// `blank`, and left for the caller's sweep.
///
/// Both windows scroll through here — the ground's materials and the sea's
/// depths — so the sea's `scrolling_keeps_texels_over_their_ground` pins the
/// direction for both. Row-wise `copy_within` rather than a texel loop over
/// a clone of the whole image: a scroll is a rigid shift that lands whole on
/// one frame, and both windows scroll on the *same* frame, having drifted
/// the same distance from the same focus — a megabyte cloned and a million
/// bounds-checked copies each was a felt hitch every time.
pub(crate) fn scroll_texels(
    data: &mut [u8],
    texels: usize,
    bytes: usize,
    blank: &[u8],
    step: IVec2,
) {
    let n = texels as i32;
    let row_bytes = texels * bytes;
    let blank_out = |strip: &mut [u8]| {
        for texel in strip.chunks_exact_mut(bytes) {
            texel.copy_from_slice(blank);
        }
    };
    // Rows walked so every source row is read before anything overwrites it:
    // ascending while sources lie further on, descending while they lie
    // behind. A shift within one row is `copy_within`'s own overlap to solve.
    let rows: Vec<i32> = if step.y > 0 {
        (0..n).collect()
    } else {
        (0..n).rev().collect()
    };
    for y in rows {
        let into = y as usize * row_bytes;
        let from_y = y + step.y;
        let keep = (n - step.x.abs()).max(0) as usize;
        if !(0..n).contains(&from_y) || keep == 0 {
            blank_out(&mut data[into..into + row_bytes]);
            continue;
        }
        let from = from_y as usize * row_bytes;
        let (to_x, from_x) = if step.x >= 0 {
            (0, step.x as usize)
        } else {
            ((-step.x) as usize, 0)
        };
        data.copy_within(
            from + from_x * bytes..from + (from_x + keep) * bytes,
            into + to_x * bytes,
        );
        if step.x >= 0 {
            blank_out(&mut data[into + keep * bytes..into + row_bytes]);
        } else {
            blank_out(&mut data[into..into + to_x * bytes]);
        }
    }
}

/// Marks the sea plane, which travels with the camera.
#[derive(Component)]
struct Sea;

/// Marks the ocean floor plane, likewise.
#[derive(Component)]
struct OceanFloor;

/// Builds one chunk's mesh from what the server sent about it, at the density
/// asked for: four vertices and six indices to a drawn cell, flat-shaded, with
/// corners taken from the payload by [`Detail`]'s stride. The module header
/// argues both halves of that.
fn chunk_mesh(heights: &[f32], materials: &[Material], lit: &[[u8; 2]], detail: Detail) -> Mesh {
    let (cells, stride, step) = (detail.cells(), detail.stride(), detail.metres());
    let count = cells * cells;

    let mut positions = Vec::with_capacity(count * 4);
    let mut normals = Vec::with_capacity(count * 4);
    let mut colors = Vec::with_capacity(count * 4);
    let mut uvs = Vec::with_capacity(count * 4);
    let mut indices = Vec::with_capacity(count * 6);

    // Named in the payload's own corners, so the last one lands on [`CELLS`]
    // exactly however coarse the cut: two chunks side by side agree along
    // their shared edge whenever they are drawn at the same detail, with
    // nothing to reconcile.
    let corner = |cx: usize, cz: usize| {
        let (fx, fz) = (cx * stride, cz * stride);
        Vec3::new(
            fx as f32 * CELL_METRES,
            heights[fz * CORNERS + fx],
            fz as f32 * CELL_METRES,
        )
    };
    // Off the same payload corner the height above was, by the same stride,
    // so a coarse sheet reads intervals the ground actually has at the
    // corners it draws — [`Detail`] decimates here exactly as it does the
    // heights. In the wire's own steps: what weighs them is
    // [`protocol::ground::lit_across`], which has to see the sentinel.
    let daylight = |(cx, cz): (usize, usize)| lit[(cz * stride) * CORNERS + cx * stride];

    // The payload's grid is exactly the chunk's own cells — a neighbour's
    // ground, where a drawing decision wants it, is the neighbour's own
    // delivered grid, reached by world point; see [`ChunkPayload::materials`].
    for iz in 0..cells {
        for ix in 0..cells {
            // The material nearest the middle of what this cell covers. A
            // coarse cell spans several of the payload's, which will want a
            // dominance order over the palette the day two materials have to
            // be blended across a boundary; point-sampling until then keeps
            // the colour taken from the same place the corners are, and is
            // exactly the payload's own cell at [`Detail::FINEST`].
            let (mx, mz) = (ix * stride + stride / 2, iz * stride + stride / 2);
            let material = materials[material_index(mx as i32, mz as i32)
                .expect("a cell of the chunk is on its own material grid")];
            let (sw, se) = (corner(ix, iz), corner(ix + 1, iz));
            let (nw, ne) = (corner(ix, iz + 1), corner(ix + 1, iz + 1));

            // The square's own normal, not each triangle's. The generator
            // classified the cell by this same normal — the mean slope of its
            // four corners — so lighting it this way is what makes a crag
            // look as steep as the palette says it is. It also stops a cell
            // whose diagonal folds reading as two facets of different
            // brightness.
            let along = (se.y + ne.y - sw.y - nw.y) / (2.0 * step);
            let across = (nw.y + ne.y - sw.y - se.y) / (2.0 * step);
            let normal = Vec3::new(-along, 1.0, -across).normalize();

            // Vertex colours are consumed in linear space by the PBR shader;
            // the reference palette is authored in sRGB.
            let srgb = material.color();
            let linear = Color::srgb(srgb.x, srgb.y, srgb.z).to_linear();
            let color = [linear.red, linear.green, linear.blue, 1.0];

            // The UV channel carries no texture coordinates — nothing binds
            // a texture to the ground — it carries the cell's lit interval,
            // as phases of the day: [`protocol::ground::lit_across`] at the
            // cell's middle, one value for all four vertices. Flat on
            // purpose, like the normal and the colour: a cell stands in the
            // sun or does not, whole, so a shadow's edge lands on the same
            // boundaries the material and the normal already break on —
            // instead of sweeping through cells as a gradient the texels
            // cannot follow. It is the *drawn* cell, so a coarse [`Detail`]
            // quantises the shadow onto its own wider grid along with
            // everything else; the ground is drawn at [`Detail::FINEST`],
            // where that grid is the wire's. What stays smooth is time: the
            // shader eases each cell over the hour — see
            // `assets/shaders/ground.wgsl`.
            // Pushed in [`SW`], [`SE`], [`NW`], [`NE`] order, which is what
            // [`split`]'s corner numbers index, and which is the order
            // `lit_across` weighs its corners in.
            let first = positions.len() as u32;
            let lit_pair = lit_uv(lit_across(
                [(ix, iz), (ix + 1, iz), (ix, iz + 1), (ix + 1, iz + 1)].map(daylight),
                Vec2::splat(0.5),
            ));
            for vertex in [sw, se, nw, ne] {
                positions.push([vertex.x, vertex.y, vertex.z]);
                normals.push([normal.x, normal.y, normal.z]);
                colors.push(color);
                uvs.push(lit_pair);
            }

            // Cut the way [`split`] says, which is also the way [`height_at`]
            // reads the ground underfoot.
            indices.extend(
                split(ix, iz)
                    .into_iter()
                    .flatten()
                    .map(|corner| first + corner as u32),
            );
        }
    }

    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    // Thirty-two bit, and not by preference: a chunk at [`Detail::FINEST`] is
    // 65,536 vertices, which is one past what sixteen bits can name. Coarser
    // cuts would fit in sixteen and are given the same buffer, one kind of
    // mesh being worth more than the bytes.
    .with_inserted_indices(Indices::U32(indices))
}

/// A corner's lit interval as the mesh carries it: the two phase thresholds
/// as fractions of the day, which is the domain [`Shading::hour`] is in.
fn lit_uv(pair: [u8; 2]) -> [f32; 2] {
    [
        protocol::dequantize_phase(pair[0]),
        protocol::dequantize_phase(pair[1]),
    ]
}

/// Builds the surface of one chunk's standing water, or `None` where the
/// chunk carries none — see [`ChunkPayload::water`].
///
/// One flat quad per cell, at the level the payload gives it, plus
/// [`OFF_LATTICE`] — a lake's level is quantised on the same lattice its bed is,
/// so a shore flat at exactly the lake's height would otherwise be coplanar
/// with the sheet standing on it. No colour, because water is one colour and
/// the material carries it, and no slope, because a lake is level: what varies
/// from quad to quad is only how high the sheet sits, and that changes at all
/// only where a chunk holds more than one lake.
///
/// **Where the water stops is not decided here.** A quad is drawn wherever any
/// of its corners has a level at all — which the server sends well past the
/// water's edge — so the sheet runs on *into* the bank and the opaque ground
/// mesh hides the part that has gone underground. The waterline the player sees
/// is therefore the true intersection of the two surfaces, exactly as the sea
/// already meets every coast.
fn water_mesh(water: &[u16], lit: &[[u8; 2]]) -> Option<Mesh> {
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();

    for iz in 0..CELLS {
        for ix in 0..CELLS {
            let (tl, tr) = ((ix, iz), (ix + 1, iz));
            let (bl, br) = ((ix, iz + 1), (ix + 1, iz + 1));

            // The tallest level any corner claims. A quad spanning two lakes
            // at once would need one of them to lose, and the higher is the
            // right winner: the lower sheet is the one the ground between
            // them is entitled to hide.
            let Some(level) = [tl, tr, bl, br]
                .iter()
                .map(|(cx, cz)| water[cz * CORNERS + cx])
                .filter(|level| *level != NO_WATER)
                .max()
            else {
                continue;
            };
            let y = dequantize(level) + OFF_LATTICE;

            // The sheet wears the same baked daylight as the ground — one
            // interval for the whole cell, read the way [`chunk_mesh`] reads
            // it — so a cliff's shadow falls on the water as well as on the
            // bed under it, and its edge steps on the cells the shore steps
            // on. A sheet still interpolating across its quads would meet
            // that shore with a gradient and disagree with it about where
            // the shadow stands, at exactly the hour the edge is moving.
            let daylight = lit_uv(lit_across(
                [tl, tr, bl, br].map(|(cx, cz)| lit[cz * CORNERS + cx]),
                Vec2::splat(0.5),
            ));

            for (cx, cz) in [tl, bl, tr, tr, bl, br] {
                positions.push([cx as f32 * CELL_METRES, y, cz as f32 * CELL_METRES]);
                // Dead flat, so every normal is the same one and there is
                // nothing for the light to pick out — which is what makes a
                // lake read as a sheet of water rather than as ground.
                normals.push([0.0, 1.0, 0.0]);
                uvs.push(daylight);
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
        ground: chunk_mesh(
            &arrival.heights,
            &arrival.materials,
            &arrival.lit,
            Detail::FINEST,
        ),
        water: arrival
            .water
            .as_ref()
            .and_then(|water| water_mesh(water, &arrival.lit)),
    }
}

// ---------------------------------------------------------------------------
// Entering and leaving the world
// ---------------------------------------------------------------------------

fn enter_world(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut shaded: ResMut<Assets<ShadedMaterial>>,
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
    // through unmodified — StandardMaterial multiplies the two together —
    // and the extension is what draws the baked shadows over the result. The
    // material window opens where the player enters the world, knowing
    // nothing; [`refresh_materials`] fills it in as the ground arrives.
    let window = images.add(material_image());
    let focus = Vec2::new(view.focus.x, view.focus.z);
    let ground = shaded.add(ShadedMaterial {
        base: matte(Color::WHITE),
        extension: Shading::ground(window.clone(), focus),
    });
    commands.insert_resource(MaterialWindow {
        image: window.clone(),
        material: ground.clone(),
        origin: MaterialWindow::origin_under(focus),
        sweep: 0,
    });
    commands.insert_resource(GroundMaterial(ground));

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
    // "The same material" is nearly literal: the ground's shader and grain,
    // reading colour and lit interval off the vertices — so the plane
    // carries both, seabed everywhere and lit the whole day, deep water
    // having no terrain about it to shade it. Its own instance rather than
    // [`GroundMaterial`] itself, because the ground's carries the live
    // window and the plane must not dither — see [`Shading::bed`]. (A plain
    // matte material of the same colour came before either, and stopped
    // matching the moment the ground took its grain: a flat bed beside a
    // speckled one printed the chunk rectangle straight back onto the
    // water.)
    let seabed = Material::Seabed.color();
    let linear = Color::srgb(seabed.x, seabed.y, seabed.z).to_linear();
    let mut bed = Plane3d::default()
        .mesh()
        .size(SEA_EXTENT, SEA_EXTENT)
        .build();
    let corners = bed.count_vertices();
    bed.insert_attribute(
        Mesh::ATTRIBUTE_COLOR,
        vec![[linear.red, linear.green, linear.blue, 1.0]; corners],
    );
    bed.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![lit_uv(LIT_ALL_DAY); corners]);
    let bed_material = shaded.add(ShadedMaterial {
        base: matte(Color::WHITE),
        extension: Shading::bed(window.clone()),
    });
    commands.insert_resource(BedMaterial(bed_material.clone()));
    commands.spawn((
        Name::new("Ocean floor"),
        OceanFloor,
        DespawnOnExit(AppState::InWorld),
        // Nothing is below it to catch a shadow, and nothing above it may
        // throw one onto it: it lies under metres of water, where a shadow
        // would be a dark patch seen through the surface with nothing over it
        // to have cast one.
        NotShadowCaster,
        NotShadowReceiver,
        Mesh3d(meshes.add(bed)),
        MeshMaterial3d(bed_material),
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
    commands.insert_resource(LakeMaterial(shaded.add(ShadedMaterial {
        base: still(LAKE_WATER),
        extension: Shading::water(window),
    })));

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
        // Water casts no shadow: Bevy shadows a transparent surface as though
        // it were solid, so without this the sea throws its own shadow down
        // onto its own bed. It still receives, which is what puts a hull's
        // shadow on the water beside it.
        NotShadowCaster,
        Mesh3d(meshes.add(sea::surface_mesh(SEA_EXTENT))),
        MeshMaterial3d(sea),
        Transform::from_xyz(0.0, SEA_SURFACE, 0.0),
    ));
}

/// How far the sun's shadows reach, in metres: everything the camera can be
/// looking at from its furthest zoom, and a little past it.
///
/// Far shorter than the [`crate::HAZE_END`] a landscape needed, because of
/// what is left to cast. The ground casts nothing — see
/// [`crate::sky::hang_the_light`] — so the pass is the boat, the plants, the
/// player and the beasts, all of which stand on the ground the camera is
/// centred on. What it must not be is *shorter* than the zoom: a reach that
/// stopped inside [`crate::camera::MAX_DISTANCE`] would take the boat's own
/// shadow away at the far end of the zoom, which is the one place a player
/// would be looking straight at it.
const CASTER_REACH: f32 = crate::camera::MAX_DISTANCE + 40.0;

/// Where the near cascade gives way to the far one, in metres — a little
/// past the default zoom, so the ordinary sailing view is wholly inside the
/// crisp one.
const CASCADE_SPLIT: f32 = 80.0;

/// How the sky's light slices the view up for the shadow pass that is left.
///
/// Two cascades rather than four, and out to [`CASTER_REACH`] rather than to
/// the haze: cascades exist to spend texels where the eye is, and with the
/// terrain out of the pass what is left to resolve is small models near the
/// camera. The near one carries the boat at any ordinary zoom, where the
/// mast is the thinnest thing in the world and its stripe seethes if the
/// texel grows; the far one covers the rest of the zoom at a texel nothing
/// out there is small enough to mind.
///
/// Built here rather than in [`crate::sky`], which hangs the light, because
/// what it has to reach past is the ground this module streams.
pub fn cascades() -> CascadeShadowConfig {
    CascadeShadowConfigBuilder {
        num_cascades: 2,
        first_cascade_far_bound: CASCADE_SPLIT,
        maximum_distance: CASTER_REACH,
        ..default()
    }
    .build()
}

/// Chunk entities despawn themselves on exit; the resources that tracked them
/// have to go too, or a stale world would answer the next match's queries
/// until its own first frame replaced it.
fn leave_world(mut commands: Commands) {
    commands.remove_resource::<Ground>();
    commands.remove_resource::<GroundMaterial>();
    commands.remove_resource::<LakeMaterial>();
    commands.remove_resource::<BedMaterial>();
    commands.remove_resource::<MaterialWindow>();
    commands.remove_resource::<sea::DepthWindow>();
}

// ---------------------------------------------------------------------------
// Streaming
// ---------------------------------------------------------------------------

/// Takes the ground the server has sent since last frame.
///
/// Only while a world is open, which the run condition on [`Ground`] is: a
/// chunk answered after leaving one is about a world that no longer exists
/// here, and there is nothing left for it to be part of.
///
/// Nothing is checked. What arrives is corner heights and palette entries —
/// see [`crate::net::GroundArrived`], and the note on believing where it is
/// written.
///
/// Drained rather than read, which is the one thing here that is not a
/// matter of taste. A payload is sixteen thousand corners and a palette entry
/// a square metre, and an arrival is a couple of hundred of them at once; a
/// reader hands out `&GroundArrived` and [`Ground::deliver`] wants the
/// payload itself, so reading would copy the whole burst. Draining is only
/// safe because this is the sole reader — which is the deal for exactly the
/// two words that carry a voyage's worth of anything, this and the survey.
fn take_the_answers(
    mut ground: ResMut<Ground>,
    mut arrivals: ResMut<Messages<crate::net::GroundArrived>>,
) {
    for arrival in arrivals.drain() {
        ground.deliver(arrival.chunk, arrival.shelter, arrival.ground);
    }
}

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
        let plants = arrival.plants.clone();
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
                    PendingPlants(plants),
                    ChunkBuild(task),
                    MeshMaterial3d(material.0.clone()),
                    // The ground draws its own shadows out of the baked
                    // intervals it arrived with, so putting it through the
                    // shadow pass as well would be the same shadow drawn
                    // twice by two methods that disagree at their edges — and
                    // it is the whole of what made that pass expensive. Nor
                    // does it receive through the standard path: the cast
                    // shadows land through `ground.wgsl`'s own read of the
                    // map, which owns the why.
                    NotShadowCaster,
                    NotShadowReceiver,
                    // Visible from birth: plants parent themselves here as soon
                    // as the heights land, which can be before the mesh build
                    // finishes and Mesh3d's required components would have
                    // supplied this (B0004 otherwise).
                    Visibility::default(),
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
                // as though it were solid, so a lake would otherwise throw
                // its own shadow down onto its own bed. The sea plane is kept
                // out of the pass for exactly this reason. And it receives
                // the way the ground does, through `ground.wgsl`'s own read.
                NotShadowCaster,
                NotShadowReceiver,
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
/// In whole strides of the sea mesh's own lattice rather than continuously —
/// see [`sea::snap`]: the sea is no longer featureless, and its vertices have
/// to keep sampling the same world points — and its facets keep their
/// diagonals — or the swell swims against itself. The floor needs no such
/// care, but there is nothing on it for a few metres of snap to be seen by
/// either.
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
    use protocol::ground::{quantize, CELL_COUNT};

    /// A payload of ground with a distinctive shape: a plane tilted along both
    /// axes, so that every corner has a different height and any transposed or
    /// mis-indexed read shows up as a wrong number rather than as a wrong-ish
    /// one.
    fn a_slope() -> ChunkPayload {
        ChunkPayload {
            heights: (0..CORNERS * CORNERS)
                .map(|i| {
                    let (ix, iz) = (i % CORNERS, i / CORNERS);
                    quantize(ix as f32 + 10.0 * iz as f32)
                })
                .collect(),
            materials: vec![Material::Grass; CELL_COUNT],
            lit: vec![protocol::ground::LIT_ALL_DAY; CORNERS * CORNERS],
            water: None,
            plants: Vec::new(),
        }
    }

    /// The keel reading goes through [`Ground::corner`] and the shared
    /// lattice rather than through [`Ground::height`], so the two have to be
    /// the same ground — on ground whose cells are no plane, where the cut
    /// shows, and in a chunk on the negative side of both axes, where a
    /// truncating division would read the neighbour's corners.
    #[test]
    fn the_corners_read_the_same_ground_the_height_does() {
        let crumpled = ChunkPayload {
            heights: (0..CORNERS * CORNERS)
                .map(|i| quantize(((i % CORNERS) % 3) as f32 * ((i / CORNERS) % 5) as f32))
                .collect(),
            ..a_slope()
        };
        let chunk = IVec2::new(-1, -1);
        let mut ground = Ground::default();
        ground.deliver(chunk, None, Some(crumpled));

        let origin = chunk.as_vec2() * CHUNK_METRES;
        for (x, z) in [
            (0.3, 0.6),
            (0.6, 0.3),
            (5.5, 7.25),
            (40.1, 2.9),
            (126.2, 126.7),
        ] {
            let at = origin + Vec2::new(x, z);
            let lattice = protocol::ground::lattice_height(|c| ground.corner(c), at);
            assert_eq!(lattice, ground.height(at.x, at.y), "at {at}");
        }
    }

    #[test]
    fn a_chunk_mesh_is_one_flat_lozenge_per_cell() {
        let payload = a_slope();
        let heights: Vec<f32> = payload.heights.iter().copied().map(dequantize).collect();
        let mesh = chunk_mesh(&heights, &payload.materials, &payload.lit, Detail::FINEST);

        // Four vertices to a cell and six indices, not six vertices: the two
        // triangles of a cell agree about everything, so they share corners.
        let count = CELLS * CELLS;
        assert_eq!(mesh.count_vertices(), count * 4);
        let indices = mesh.indices().expect("a cell shares its corners");
        assert_eq!(indices.len(), count * 6);

        let normals = mesh
            .attribute(Mesh::ATTRIBUTE_NORMAL)
            .expect("normals")
            .as_float3()
            .expect("three floats each");
        let colors = match mesh.attribute(Mesh::ATTRIBUTE_COLOR).expect("colours") {
            bevy::mesh::VertexAttributeValues::Float32x4(values) => values.clone(),
            other => panic!("colours came out as {other:?}"),
        };

        // All four agree — which is the whole point of the cell being the
        // unit. A cell lit as two triangles would show the diagonal the wire
        // deliberately stopped carrying.
        for cell in 0..count {
            let i = cell * 4;
            for vertex in 1..4 {
                assert_eq!(
                    normals[i],
                    normals[i + vertex],
                    "cell {cell} is lit as two triangles"
                );
                assert_eq!(
                    colors[i],
                    colors[i + vertex],
                    "cell {cell} is painted as two triangles"
                );
            }
            // A heightfield can never overhang, so every cell faces upwards.
            assert!(normals[i][1] > 0.0, "cell {cell} faces downwards");
        }

        // And every index is some cell's own corner. An index that strayed
        // into the neighbouring cell would draw ground of the wrong colour
        // and be almost impossible to see.
        for (triangle, corners) in indices
            .iter()
            .collect::<Vec<_>>()
            .as_chunks::<3>()
            .0
            .iter()
            .enumerate()
        {
            let cell = triangle / 2;
            for corner in corners {
                assert_eq!(
                    corner / 4,
                    cell,
                    "triangle {triangle} reaches out of cell {cell}"
                );
            }
        }
    }

    #[test]
    fn shelter_is_read_off_the_lattice_by_world_point() {
        use protocol::ground::{BEARINGS, EXPOSED, SHELTER_COUNT};

        let mut ground = Ground::default();
        assert_eq!(
            ground.exposure(10.0, 10.0, Vec2::NEG_Y),
            None,
            "a chunk that never came"
        );
        ground.deliver(IVec2::ZERO, None, None);
        assert_eq!(
            ground.exposure(10.0, 10.0, Vec2::NEG_Y),
            Some(1.0),
            "open water is exposed to a wind nothing stands in front of"
        );

        // A chunk sheltered from the north and open from every other quarter.
        // Both bearings are asked at the same point, so what this catches is
        // the lattice being read at the wrong slot rather than at the wrong
        // place.
        let mut lee = [EXPOSED; BEARINGS];
        lee[4] = 0;
        ground.deliver(
            IVec2::new(1, 0),
            Some(vec![lee; SHELTER_COUNT]),
            Some(a_slope()),
        );

        let at = Vec2::new(CHUNK_METRES + 40.0, 40.0);
        let southward = ground
            .exposure(at.x, at.y, Vec2::Y)
            .expect("the chunk arrived");
        let northward = ground
            .exposure(at.x, at.y, Vec2::NEG_Y)
            .expect("the chunk arrived");
        assert_eq!(southward, 0.0, "a wind blowing south read the wrong slot");
        assert_eq!(northward, 1.0, "a wind blowing north read the wrong slot");
    }

    #[test]
    fn the_shelter_lattice_meets_itself_across_a_chunk_boundary() {
        // Two chunks side by side, the western one in a lee and the eastern
        // one open. The lattice runs to and including each chunk's far edge,
        // so the shared line is stored twice with the same value in it — and
        // a reading taken a whisker either side of the boundary has to come
        // back with the same answer, or a boat crossing it feels the wind
        // step. This is what SHELTER_CORNERS being one more than the cells
        // buys, and it is the one thing about the lattice that a plain
        // grid-per-chunk would get wrong.
        use protocol::ground::{BEARINGS, EXPOSED, SHELTER_CORNERS, SHELTER_COUNT};

        let mut ground = Ground::default();
        let edge = |western: bool| {
            (0..SHELTER_COUNT)
                .map(|i| {
                    // A ramp across each chunk that meets at the seam: the
                    // west chunk climbs to half-cover at its east edge and
                    // the east chunk carries on from exactly there.
                    let ix = i % SHELTER_CORNERS;
                    let across = ix as f32 / (SHELTER_CORNERS - 1) as f32;
                    let along = if western { across } else { 1.0 + across };
                    [(along * 0.5 * EXPOSED as f32) as u8; BEARINGS]
                })
                .collect()
        };
        ground.deliver(IVec2::ZERO, Some(edge(true)), Some(a_slope()));
        ground.deliver(IVec2::new(1, 0), Some(edge(false)), Some(a_slope()));

        let just_west = ground
            .exposure(CHUNK_METRES - 0.01, 40.0, Vec2::NEG_Y)
            .expect("the western chunk");
        let just_east = ground
            .exposure(CHUNK_METRES + 0.01, 40.0, Vec2::NEG_Y)
            .expect("the eastern chunk");
        assert!(
            (just_west - just_east).abs() < 1e-3,
            "the wind steps across a chunk line: {just_west} west of it, {just_east} east"
        );
    }

    #[test]
    fn every_cell_is_split_on_the_diagonal_its_parity_calls_for() {
        // The split alternates like a checkerboard, and the indices are where
        // it shows. A cell's six indices name its four corners either way;
        // which corner is named *twice* says which diagonal was cut, and it
        // has to alternate or the ground grows a herringbone.
        //
        // Asked of a coarse cut as well, the parity there running off the
        // *drawn* cell's index and not the payload's: a herringbone is a thing
        // the eye finds in the facets it can see.
        let heights: Vec<f32> = (0..CORNERS * CORNERS)
            .map(|i| (i % CORNERS) as f32)
            .collect();

        // The order the corners are pushed in, which the indices are relative
        // to — south-west, south-east, north-west, north-east.
        const SW: usize = 0;
        const SE: usize = 1;
        const NW: usize = 2;
        const NE: usize = 3;

        for detail in [Detail::FINEST, Detail::new(2), Detail::COARSEST] {
            let mesh = chunk_mesh(
                &heights,
                &vec![Material::Grass; CELL_COUNT],
                &all_day(),
                detail,
            );
            let indices: Vec<usize> = mesh
                .indices()
                .expect("a cell shares its corners")
                .iter()
                .collect();
            let across = detail.cells();

            for (ix, iz) in [(0, 0), (1, 0), (0, 1), (1, 1), (across - 1, 0)] {
                let cell = iz * across + ix;
                let mut counts = std::collections::BTreeMap::new();
                for at in &indices[cell * 6..cell * 6 + 6] {
                    *counts.entry(at - cell * 4).or_insert(0) += 1;
                }
                assert_eq!(
                    counts.len(),
                    4,
                    "{detail:?} cell ({ix}, {iz}) used something other than its four corners"
                );
                let shared: Vec<usize> = counts
                    .iter()
                    .filter(|(_, n)| **n == 2)
                    .map(|(corner, _)| *corner)
                    .collect();
                let cut = if (ix + iz).is_multiple_of(2) {
                    // The south-west/north-east cut shares the other two.
                    vec![SE, NW]
                } else {
                    vec![SW, NE]
                };
                assert_eq!(
                    shared, cut,
                    "{detail:?} cell ({ix}, {iz}) was cut the wrong way"
                );
            }
        }
    }

    #[test]
    fn a_detail_is_a_cut_the_chunk_grid_can_actually_take() {
        // The bound is the point of the type: every level has to leave whole
        // cells, and an even number of them, or the checkerboard steps phase
        // at a chunk boundary and the ground grows the herringbone [`split`]
        // exists to break up.
        for steps in 0..=Detail::COARSEST.0 + 4 {
            let detail = Detail::new(steps);
            assert_eq!(
                CELLS % detail.stride(),
                0,
                "{detail:?} leaves a ragged row along two sides of the chunk"
            );
            assert!(
                detail.cells() >= 2 && detail.cells().is_multiple_of(2),
                "{detail:?} leaves {} cells, which cannot carry the parity",
                detail.cells()
            );
        }
        // Asked for more than the grid has, a selector gets the coarsest there
        // is rather than an error it would have to have a plan for.
        assert_eq!(Detail::new(99), Detail::COARSEST);
        assert_eq!(Detail::FINEST.cells(), CELLS);
        assert_eq!(Detail::FINEST.metres(), CELL_METRES);
    }

    #[test]
    fn a_cell_is_lit_whole_by_the_interval_at_its_middle() {
        // Every vertex of a drawn cell carries one pair — the mean of the
        // cell's own four corners, which is the interpolated value at its
        // centre — so the shadow shades whole cells on the same grid the
        // materials are painted on; see the UV note in [`chunk_mesh`]. The
        // corners are decimated with the heights and by the same stride, so
        // a coarse cut is still lit by intervals the ground actually has at
        // the corners it draws: reading the drawn cell's index instead would
        // shade the coarse sheet with the intervals of the chunk's first few
        // metres, stretched over all of it.
        let mut payload = a_slope();
        payload.lit = (0..CORNERS * CORNERS)
            .map(|i| {
                let (ix, iz) = (i % CORNERS, i / CORNERS);
                // `from` keyed on x and `until` on z, so a transpose reads as
                // some other corner's answer, and each is distinct per corner
                // so a wrong stride or an off-by-one does too. Kept well
                // formed — `from` under `until` at every corner — because a
                // pair that runs backwards means *never lit*, which
                // [`protocol::ground::lit_across`] rightly refuses to weigh:
                // a fixture full of those would be testing the sentinel path
                // while claiming to test the middle of a cell.
                [ix as u8, 128 + (iz / 2) as u8]
            })
            .collect();
        let heights: Vec<f32> = payload.heights.iter().copied().map(dequantize).collect();

        let detail = Detail::new(2);
        let stride = detail.stride();
        let mesh = chunk_mesh(&heights, &payload.materials, &payload.lit, detail);
        let uvs = match mesh
            .attribute(Mesh::ATTRIBUTE_UV_0)
            .expect("the lit intervals ride the UV channel")
        {
            bevy::mesh::VertexAttributeValues::Float32x2(uvs) => uvs,
            other => panic!("the lit intervals came back as {other:?}"),
        };

        // The payload's pattern is `[ix % 251, iz % 251]` per corner, so a
        // cell's four corners are two consecutive x values against two
        // consecutive z values and the middle of them is a half — written out
        // here rather than folded, so a wrong corner list or weight in
        // [`chunk_mesh`] cannot be copied into what checks it.
        let across = detail.cells();
        for (ix, iz) in [(0, 0), (1, 0), (0, 1), (3, 5), (across - 1, across - 1)] {
            // Both means land on whole steps for an even stride, so the
            // expectation is exact and owes nothing to how [`chunk_mesh`]
            // sums: `from` is the middle of the cell's two x corners, `until`
            // the middle of its two z ones.
            let want = [
                protocol::dequantize_phase((ix * stride + stride / 2) as u8),
                protocol::dequantize_phase(128 + (iz * stride + stride / 2) as u8 / 2),
            ];
            let base = (iz * across + ix) * 4;
            assert_eq!(
                &uvs[base..base + 4],
                [want; 4],
                "drawn cell ({ix}, {iz}) is not lit whole by the interval at its middle"
            );
        }
    }

    #[test]
    fn chunk_positions_are_local_so_the_transform_places_them() {
        let heights = vec![3.5f32; CORNERS * CORNERS];
        let materials = vec![Material::Sand; CELL_COUNT];
        let mesh = chunk_mesh(&heights, &materials, &all_day(), Detail::FINEST);

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

        ground.deliver(IVec2::ZERO, None, None);
        assert_eq!(ground.surface(10.0, 10.0), Some(0.0), "open water floats");
    }

    #[test]
    fn the_light_over_a_point_is_read_between_the_corners_around_it() {
        // What the sea's window asks, texel by texel — see
        // [`crate::sea::refresh_depth`]. A corner answers with its own
        // interval; between corners the answer is read across them, so a
        // headland's shadow reaches the water as an edge rather than as a
        // staircase of whole corners.
        let mut ground = Ground::default();
        assert_eq!(ground.lit(10.0, 10.0), None, "a chunk that never came");

        ground.deliver(IVec2::ZERO, None, None);
        assert_eq!(
            ground.lit(10.0, 10.0),
            Some(LIT_ALL_DAY),
            "open water is lit by a sun nothing stands in front of"
        );

        // A chunk whose light varies along x alone, so a read between two
        // corners has one obvious answer and a transposed one does not.
        let mut payload = a_slope();
        payload.lit = (0..CORNERS * CORNERS)
            .map(|i| {
                let ix = i % CORNERS;
                [LIT_ALL_DAY[0] + ix as u8 % 8, LIT_ALL_DAY[1]]
            })
            .collect();
        ground.deliver(IVec2::new(1, 0), None, Some(payload));

        let base = CHUNK_METRES;
        let corner = |ix: usize| {
            ground
                .lit(base + ix as f32 * CELL_METRES, 3.0)
                .expect("the chunk arrived")
        };
        assert_eq!(corner(2), [LIT_ALL_DAY[0] + 2, LIT_ALL_DAY[1]]);
        assert_eq!(corner(3), [LIT_ALL_DAY[0] + 3, LIT_ALL_DAY[1]]);
        // And halfway between them, halfway between their answers.
        let between = ground
            .lit(base + 2.5 * CELL_METRES, 3.0)
            .expect("the chunk arrived");
        assert_eq!(
            between,
            [LIT_ALL_DAY[0] + 3, LIT_ALL_DAY[1]],
            "2.5 rounds up"
        );
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
        ground.deliver(IVec2::ZERO, None, Some(payload));

        // Every corner of the grid, at its own world position.
        for iz in 0..CORNERS {
            for ix in 0..CORNERS {
                // The far edges belong to the next chunk along, which has not
                // arrived; inside the chunk, a corner is its own height.
                if ix == CELLS || iz == CELLS {
                    continue;
                }
                let at = Vec2::new(ix as f32, iz as f32) * CELL_METRES;
                let want = heights[iz * CORNERS + ix].max(0.0);
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
        let mid = Vec2::splat(CELL_METRES * 0.5);
        let mean = (heights[0] + heights[1] + heights[CORNERS] + heights[CORNERS + 1]) / 4.0;
        let got = ground.surface(mid.x, mid.y).expect("the chunk arrived");
        assert!(
            (got - mean).abs() < 1.0e-3,
            "{got} m across a facet, not {mean}"
        );
    }

    /// Ground with a saddle in every cell: the two diagonals of a quad give
    /// genuinely different heights down its middle, so anything that reads a
    /// cell across the wrong one is off by a measurable amount.
    ///
    /// `a_slope()` cannot do this job — a plane is planar within each quad
    /// either way it is cut, so it reads the same across both diagonals and
    /// would pass whatever the split rule said.
    fn a_saddle() -> Vec<f32> {
        (0..CORNERS * CORNERS)
            .map(|i| {
                let (ix, iz) = (i % CORNERS, i / CORNERS);
                // Alternating high and low corners, so each cell is a col
                // between two ridges rather than a slope.
                if (ix + iz).is_multiple_of(2) {
                    4.0
                } else {
                    -4.0
                }
            })
            .collect()
    }

    #[test]
    fn the_ground_underfoot_is_read_off_the_triangle_that_is_drawn() {
        // [`height_at`] and [`chunk_mesh`] both build from [`split`], and this
        // is what holds them to it: the height reported inside a cell has to
        // lie on the plane of whichever triangle the mesh actually drew over
        // that spot. On a saddle the two diagonals disagree by metres down the
        // middle of every cell, so a rule written twice and changed once would
        // fail here loudly.
        //
        // At [`Detail::FINEST`], which is the grid [`height_at`] always reads:
        // a coarser cut is a different sheet on purpose, and nothing has to
        // assert that here because [`height_at`] takes no [`Detail`] to be
        // given the wrong one.
        let heights = a_saddle();
        let mesh = chunk_mesh(
            &heights,
            &vec![Material::Grass; CELL_COUNT],
            &all_day(),
            Detail::FINEST,
        );
        let positions = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .expect("positions")
            .as_float3()
            .expect("three floats each");
        let indices: Vec<usize> = mesh
            .indices()
            .expect("a cell shares its corners")
            .iter()
            .collect();

        // Both parities, and not only the first cell of each.
        let cells = [
            (0, 0),
            (1, 0),
            (0, 1),
            (1, 1),
            (7, 4),
            (CELLS - 1, CELLS - 1),
        ];
        // Points spread across the cell, deliberately including both sides of
        // either diagonal and the middle, where the two disagree most.
        let spots = [
            Vec2::new(0.25, 0.25),
            Vec2::new(0.75, 0.25),
            Vec2::new(0.25, 0.75),
            Vec2::new(0.75, 0.75),
            Vec2::new(0.5, 0.5),
            Vec2::new(0.1, 0.6),
        ];

        let mut checked = 0;
        for (ix, iz) in cells {
            let cell = iz * CELLS + ix;
            for spot in spots {
                let local = (Vec2::new(ix as f32, iz as f32) + spot) * CELL_METRES;
                let got = height_at(&heights, local);

                // Whichever of the cell's two drawn triangles covers the spot,
                // read straight off the mesh's own vertices.
                let mut drawn = None;
                for triangle in 0..2 {
                    let corners: Vec<Vec2> = (0..3)
                        .map(|k| {
                            let at = positions[indices[cell * 6 + triangle * 3 + k]];
                            Vec2::new(at[0], at[2])
                        })
                        .collect();
                    let ys: Vec<f32> = (0..3)
                        .map(|k| positions[indices[cell * 6 + triangle * 3 + k]][1])
                        .collect();
                    let cross = |p: Vec2, q: Vec2| p.x * q.y - p.y * q.x;
                    let (a, b, c) = (corners[0], corners[1], corners[2]);
                    let area = cross(b - a, c - a);
                    let v = cross(local - a, c - a) / area;
                    let w = cross(b - a, local - a) / area;
                    let u = 1.0 - v - w;
                    if u >= -1.0e-6 && v >= -1.0e-6 && w >= -1.0e-6 {
                        drawn = Some(u * ys[0] + v * ys[1] + w * ys[2]);
                        break;
                    }
                }

                let drawn = drawn.expect("the cell's two triangles cover the cell");
                assert!(
                    (got - drawn).abs() < 1.0e-3,
                    "cell ({ix}, {iz}) at {spot:?}: underfoot says {got} m, \
                     the mesh draws {drawn} m"
                );
                checked += 1;
            }
        }
        assert_eq!(checked, cells.len() * spots.len());

        // And the saddle really does disagree across its diagonals, or the
        // whole test would be vacuous — the middle of a cell is one ridge or
        // the other, never the mean.
        let middle = height_at(&heights, Vec2::splat(0.5 * CELL_METRES));
        assert!(
            middle.abs() > 1.0,
            "{middle} m in the middle of a saddle cell — this ground is too \
             flat to tell the two diagonals apart"
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
        ground.deliver(IVec2::ZERO, None, None);
        ground.want(IVec2::ZERO);
        assert!(
            ground.take_requests().is_empty(),
            "asked again after hearing"
        );
    }

    /// A water grid with a lake at `level` metres over the square of corners
    /// below `edge`, and nothing anywhere else.
    /// A day nothing shadows, for the meshes whose light is not the thing
    /// under test.
    fn all_day() -> Vec<[u8; 2]> {
        vec![LIT_ALL_DAY; CORNERS * CORNERS]
    }

    fn a_lake(level: f32, edge: usize) -> Vec<u16> {
        let mut water = vec![NO_WATER; CORNERS * CORNERS];
        for iz in 0..edge {
            for ix in 0..edge {
                water[iz * CORNERS + ix] = quantize(level);
            }
        }
        water
    }

    #[test]
    fn a_lake_is_a_flat_sheet_at_the_level_it_was_sent() {
        let mesh = water_mesh(&a_lake(12.0, 8), &all_day()).expect("a lake");

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
        let mesh = water_mesh(&a_lake(12.0, 8), &all_day()).expect("a lake");
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
            8.0 * CELL_METRES,
            "the sheet stops at the last wet corner instead of running past it"
        );
    }

    #[test]
    fn ground_with_no_lake_on_it_draws_no_water() {
        assert!(water_mesh(&vec![NO_WATER; CORNERS * CORNERS], &all_day()).is_none());

        // And a payload that carries no grid at all never gets as far as
        // asking — the common case, and the one that has to cost nothing.
        let dry = Arrival {
            chunk: IVec2::ZERO,
            heights: a_slope().heights.iter().copied().map(dequantize).collect(),
            materials: a_slope().materials.into(),
            lit: all_day().into(),
            water: None,
            plants: Vec::new(),
        };
        assert!(chunk_meshes(&dry).water.is_none());
    }

    #[test]
    fn two_lakes_in_one_chunk_each_keep_their_own_level() {
        // A chunk can straddle two basins a hillside apart, which is the
        // whole reason the level travels per corner rather than per chunk.
        let mut water = vec![NO_WATER; CORNERS * CORNERS];
        for iz in 0..4 {
            for ix in 0..4 {
                water[iz * CORNERS + ix] = quantize(6.0);
                water[iz * CORNERS + ix + 20] = quantize(31.0);
            }
        }

        let mesh = water_mesh(&water, &all_day()).expect("two lakes");
        let positions = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .expect("positions")
            .as_float3()
            .expect("three floats each");

        for point in positions {
            let want = if point[0] < 10.0 * CELL_METRES {
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
            let mesh = water_mesh(&a_lake(level, 8), &all_day()).expect("a lake");
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
    fn the_grain_swing_rides_the_lane_the_shader_reads() {
        // One lane of a vec4, named on each side of the Rust/WGSL boundary
        // and compiled by neither together: swap `z` on one side only and
        // the ground draws smooth — or wrong — everywhere, blaming nothing.
        // The sea pins its own uniform the same way, in `sea.rs`. The
        // needles are assembled at runtime so this test's own source, which
        // `include_str!` sweeps up too, cannot satisfy them.
        let shader = std::fs::read_to_string(format!(
            "{}/../../assets/shaders/ground.wgsl",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("the ground's shader under assets/shaders/");
        let lane = 'z';
        assert!(
            shader.contains(&format!("shading.hour.{lane}")),
            "the shader takes the swing from `{lane}`"
        );
        assert!(
            include_str!("terrain.rs").contains(&format!(".with_{lane}(GRAIN_SWING)")),
            "the packing puts the swing in `{lane}`"
        );
    }

    #[test]
    fn the_cast_shadows_ride_the_lane_and_keep_the_switch() {
        // Two facts only the drawing machine could otherwise check: the
        // shader takes the hardening reach from the lane the packing puts
        // it in — `w`, the hazard the grain's test above explains — and it
        // still asks the light whether shadows are on, which is the whole
        // of how the console's `shadows off` reaches ground the standard
        // path no longer shades. Needles assembled at runtime, as above.
        let shader = std::fs::read_to_string(format!(
            "{}/../../assets/shaders/ground.wgsl",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("the ground's shader under assets/shaders/");
        let lane = 'w';
        assert!(
            shader.contains(&format!("shading.hour.{lane}")),
            "the shader takes the reach from `{lane}`"
        );
        assert!(
            include_str!("terrain.rs").contains(&format!(".with_{lane}(CASCADE_SPLIT)")),
            "the packing puts the reach in `{lane}`"
        );
        assert!(
            shader.contains(&format!(
                "{}_SHADOWS_ENABLED_BIT",
                "DIRECTIONAL_LIGHT_FLAGS"
            )),
            "the shader honours the light's own shadow switch"
        );
    }

    #[test]
    fn the_shader_reads_the_uniform_in_the_order_it_is_written() {
        // The order of `Shading`'s uniform fields *is* the uniform's layout,
        // and only one side of it is compiled here — the same hazard the
        // sea's test of this name holds `sea.wgsl` against. Both orders are
        // read out of the sources rather than written out a third time.
        let shader = std::fs::read_to_string(format!(
            "{}/../../assets/shaders/ground.wgsl",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("the ground's shader under assets/shaders/");
        let body = |source: &str, opens: &str| -> String {
            source
                .split_once(opens)
                .expect("the struct the uniform is packed from")
                .1
                .split_once('}')
                .expect("the end of it")
                .0
                .to_string()
        };
        let wgsl = body(&shader, "struct Shading {");
        let rust = body(include_str!("terrain.rs"), "struct Shading {");
        for fields in [&wgsl, &rust] {
            let hour = fields.find("hour:").expect("the hour field");
            let window = fields.find("window:").expect("the window field");
            let palette = fields.find("palette:").expect("the palette field");
            assert!(
                hour < window && window < palette,
                "hour, then window, then palette"
            );
        }
        // And the palette's length, which the split above cannot reach — the
        // wgsl array is sized by a literal, and a bind group refuses a
        // buffer of the wrong size only at runtime, on a machine drawing.
        assert!(
            shader.contains(&format!("array<vec4<f32>, {}>", Material::KINDS)),
            "the shader's palette holds every material"
        );
        // The shader also turns metres straight into texel indices —
        // `cell - shading.window.xy` with no divide — which is only the
        // same number while a ground cell is a metre wide.
        assert_eq!(CELL_METRES, 1.0, "a window texel is no longer a cell");
    }

    #[test]
    fn the_sweep_divides_the_window() {
        // What lets `refresh_materials` treat every sweep as one contiguous
        // run of rows: a sweep that straddled the wrap would write one slice
        // from two ends of the image.
        assert_eq!(WINDOW_TEXELS % WINDOW_SWEEP_ROWS, 0);
    }

    #[test]
    fn the_ground_answers_what_it_is_made_of() {
        let mut ground = Ground::default();
        ground.deliver(IVec2::new(1, 0), None, Some(a_slope()));
        ground.deliver(IVec2::new(-1, -1), None, None);

        // A land chunk answers with the cell's own material, anywhere in the
        // cell — a_slope is Grass throughout.
        assert_eq!(
            ground.material(128.0 + 5.5, 3.25),
            Some(Material::Grass),
            "a delivered cell knows its material"
        );
        // Open water is bed, and an unasked chunk is nothing yet.
        assert_eq!(ground.material(-5.0, -5.0), Some(Material::Seabed));
        assert_eq!(ground.material(9999.0, 0.0), None);
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
