//! The swell: the waves the sea wears, and everything that has to agree on
//! them.
//!
//! The sea is drawn as low-poly waves — real geometry, displaced in the vertex
//! shader and shaded flat, so a wave is a run of tilting facets like everything
//! else in the world rather than a normal-mapped shimmer. On a surface as
//! matte as this water a normal map would barely read anyway: there is no
//! specular for it to perturb, and what sells the motion instead is facets
//! changing tone as they tilt, and the waterline creeping up and down every
//! beach as the surface rises and falls through the shore.
//!
//! Three parties have to agree on where the water stands at a moment: the
//! shader displacing the sea mesh, the boat riding on it, and the markers
//! other players stand as. The parameters live once, in [`components`], and
//! reach the shader through a uniform so they cannot drift from the Rust
//! side; the *formula* — a sum of sines — is written twice, here in [`swell`]
//! and once in `assets/shaders/sea.wgsl`, and the two must be kept the same.
//! Time is the other half of the agreement: the shader reads `globals.time`,
//! which Bevy fills from `Time::elapsed_secs_wrapped`, so that is what every
//! Rust caller of [`swell`] must pass.
//!
//! The camera deliberately does *not* ride the swell. Its focus stays on the
//! flat waterline, so the world bobs around a steady eye rather than the
//! whole picture heaving with the boat.

use std::f32::consts::TAU;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;

/// Displaces the sea's vertices and shades the result — see the module doc,
/// and the file itself, which carries the other copy of [`swell`].
const SHADER: &str = "shaders/sea.wgsl";

/// The swell, as components: heading, wavelength in metres, amplitude in
/// metres. Three of them, because one sine reads as a marching pattern and
/// two as a grid; three, crossing at odd angles, is the fewest that reads as
/// water. Headings are deliberately unrelated to the axes of the chunk grid
/// and to each other.
///
/// The wavelengths stay well above twice [`SPACING`], or a wave would fall
/// between the mesh's vertices and alias into shimmer. The amplitudes are
/// gentle — a calm day — both because the look wants a sea, not a storm, and
/// because the boat rides this height with no easing: what the water does,
/// the hull does.
const WAVES: [(Vec2, f32, f32); 3] = [
    (Vec2::new(0.966, 0.259), 43.0, 0.22),
    (Vec2::new(-0.643, 0.766), 24.0, 0.12),
    (Vec2::new(-0.259, -0.966), 14.0, 0.065),
];

/// Gravity, for the dispersion relation: deep-water waves travel at
/// `ω = sqrt(g·k)`, so the long swell outruns the short chop and the surface
/// never repeats itself the way a single-speed pattern would.
const GRAVITY: f32 = 9.81;

/// How much more steeply the facets are *lit* than the water actually
/// slopes. The shading and the geometry pull in opposite directions: a swell
/// gentle enough for the boat to ride and the beaches to keep their
/// waterlines tilts its facets a few degrees, which under this sky is almost
/// no tone change at all — honest amplitudes were tried first and the sea
/// read as flat; amplitudes big enough to read lit honestly were a storm,
/// with the hull heaving metres and the waterline marching up the beaches.
/// So the fragment shader exaggerates only the slope the *light* sees, and
/// the surface everything rides stays calm.
const SHADING_TILT: f32 = 4.0;

/// Metres between the sea mesh's vertices, over the region that waves. The
/// terrain's facets are 2 m; the sea's are coarser because its shapes are
/// longer — at 4 m the shortest wave in [`WAVES`] still gets three facets per
/// crest. A quarter of a million vertices, which sounds like a lot and is a
/// single static buffer the vertex shader walks; halving it was tried first,
/// and took the shortest legible wavelength — and most of the sea's visible
/// slope — with it.
pub const SPACING: f32 = 4.0;

/// Half-width of the region that actually waves, in metres — vertices at
/// [`SPACING`] reach this far out from the mesh's centre, and a single ring
/// of enormous flat cells carries on from there to the horizon. A whole
/// number of spacings, so the fine grid meets the ring exactly.
const REACH: f32 = 1024.0;

/// Where the swell starts fading with distance from the mesh's centre, and
/// where it has fully gone, in metres. The far edge stays inside [`REACH`],
/// so every vertex the flat outer ring shares an edge with has stopped
/// moving and the two regions meet without cracks. All of it is deep inside
/// the haze — the fade exists so the mesh can end, not to be seen.
const FADE: (f32, f32) = (512.0, 960.0);

/// The sea's water, waves and all: the standard water surface underneath,
/// with the swell displacing its vertices on top.
pub type SeaMaterial = ExtendedMaterial<StandardMaterial, SeaExtension>;

/// What the sea shader needs beyond the standard material: the swell's
/// components, packed for the sum the shader runs per vertex.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct SeaExtension {
    /// One wave per row: `xy` is the heading scaled by the wavenumber, `z`
    /// the angular frequency, `w` the amplitude — exactly the terms of
    /// [`swell`], so the shader adds them up rather than deriving anything.
    #[uniform(100)]
    waves: [Vec4; WAVES.len()],
    /// `x` and `y` are [`FADE`], `z` is [`SHADING_TILT`]; `w` is padding.
    #[uniform(100)]
    fade: Vec4,
}

impl Default for SeaExtension {
    fn default() -> Self {
        Self {
            waves: components(),
            fade: Vec4::new(FADE.0, FADE.1, SHADING_TILT, 0.0),
        }
    }
}

impl MaterialExtension for SeaExtension {
    fn vertex_shader() -> ShaderRef {
        SHADER.into()
    }

    fn fragment_shader() -> ShaderRef {
        SHADER.into()
    }
}

/// [`WAVES`], worked into the terms both copies of the formula run on: `xy`
/// heading times wavenumber, `z` angular frequency, `w` amplitude.
fn components() -> [Vec4; WAVES.len()] {
    WAVES.map(|(heading, wavelength, amplitude)| {
        let wavenumber = TAU / wavelength;
        let frequency = (GRAVITY * wavenumber).sqrt();
        let direction = heading.normalize() * wavenumber;
        Vec4::new(direction.x, direction.y, frequency, amplitude)
    })
}

/// Height of the swell above the flat waterline at a point, in metres —
/// negative in a trough. `elapsed` is `Time::elapsed_secs_wrapped`, the same
/// clock the shader's `globals.time` runs on.
///
/// This is the Rust copy of the formula in `assets/shaders/sea.wgsl`; the
/// two must agree or the boat stops sitting on the water it is drawn in.
pub fn swell(at: Vec2, elapsed: f32) -> f32 {
    components()
        .iter()
        .map(|wave| wave.w * (wave.xy().dot(at) - wave.z * elapsed).sin())
        .sum()
}

/// Snaps a coordinate onto the sea mesh's own lattice.
///
/// The mesh travels with the camera, and its vertices sample the swell at
/// whatever world points they land on. Moved continuously, every vertex
/// resamples the field every frame and the facets swim against the waves
/// they are drawing; moved in whole steps of [`SPACING`], each vertex sits
/// exactly where one sat before, and the surface holds still while the mesh
/// slides underneath it.
pub fn snap(coordinate: f32) -> f32 {
    (coordinate / SPACING).round() * SPACING
}

/// The sea's mesh: a grid of [`SPACING`] cells out to [`REACH`], with one
/// outer ring of cells carrying on to `extent / 2` — the same tensor grid
/// throughout, so the fine middle and the enormous rim share their border
/// vertices and cannot crack apart. The rim's cells are kilometres across,
/// which is fine, because past [`FADE`] the surface they draw is flat.
///
/// Vertices are shared, unlike the terrain's: the facet look comes from the
/// fragment shader deriving each facet's normal from its own slope, so
/// nothing here needs duplicating per triangle. The quads' diagonals
/// alternate in a checkerboard, or the shared diagonal direction reads as a
/// grain running across the water.
pub fn surface_mesh(extent: f32) -> Mesh {
    let half = extent / 2.0;
    let steps = (2.0 * REACH / SPACING) as usize;

    let mut stations = Vec::with_capacity(steps + 3);
    stations.push(-half);
    stations.extend((0..=steps).map(|i| -REACH + i as f32 * SPACING));
    stations.push(half);

    let across = stations.len();
    let mut positions = Vec::with_capacity(across * across);
    for &z in &stations {
        for &x in &stations {
            positions.push(Vec3::new(x, 0.0, z));
        }
    }
    // Every normal is up. The shader replaces them per fragment; these exist
    // because the standard pipeline expects the attribute.
    let normals = vec![[0.0f32, 1.0, 0.0]; positions.len()];

    let mut indices = Vec::with_capacity((across - 1) * (across - 1) * 6);
    for z in 0..across - 1 {
        for x in 0..across - 1 {
            let a = (z * across + x) as u32;
            let b = a + 1;
            let c = a + across as u32;
            let d = c + 1;
            // Wound counter-clockwise seen from above, the same way round as
            // the terrain's facets.
            if (x + z) % 2 == 0 {
                indices.extend([a, c, d, a, d, b]);
            } else {
                indices.extend([a, c, b, b, c, d]);
            }
        }
    }

    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_indices(Indices::U32(indices))
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The most the surface can ever stand off the waterline: every wave at
    /// its crest at once.
    fn ceiling() -> f32 {
        WAVES.iter().map(|(_, _, amplitude)| amplitude).sum()
    }

    #[test]
    fn the_swell_stays_within_its_amplitudes() {
        // The boat and the shore both live within centimetres of the
        // waterline, so the swell being bounded is not decoration — it is
        // what keeps a calm day calm everywhere and forever.
        let limit = ceiling();
        for i in 0..1000 {
            let at = Vec2::new((i * 37 % 997) as f32 * 3.1, (i * 61 % 991) as f32 * -2.7);
            let height = swell(at, i as f32 * 0.37);
            assert!(
                height.abs() <= limit,
                "the swell reaches {height} m at {at}, past every crest combined ({limit} m)"
            );
        }
    }

    #[test]
    fn the_swell_moves() {
        // Anywhere at all, the surface a few seconds later is a different
        // surface — the whole point of it.
        let at = Vec2::new(12.0, -34.0);
        assert_ne!(swell(at, 0.0), swell(at, 2.0));
    }

    #[test]
    fn the_wavelengths_clear_the_mesh() {
        // A wave shorter than two spacings falls between the vertices and
        // aliases; three per crest is where it stops looking like shimmer.
        for (_, wavelength, _) in WAVES {
            assert!(
                wavelength >= 3.0 * SPACING,
                "a {wavelength} m wave is under-sampled at {SPACING} m spacing"
            );
        }
    }

    #[test]
    fn the_fade_ends_inside_the_fine_grid() {
        // The flat rim shares vertices with the fine grid's border; those
        // border vertices must have stopped moving or the two crack apart.
        assert!(FADE.1 < REACH);
    }

    #[test]
    fn the_mesh_seams_cannot_crack() {
        // The reach is a whole number of spacings — the last fine cell ends
        // exactly at REACH, where the rim begins.
        assert_eq!(REACH % SPACING, 0.0);
    }
}
