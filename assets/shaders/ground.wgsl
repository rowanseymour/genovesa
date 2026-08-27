// The ground under its own baked shadows.
//
// This extends the standard PBR material rather than replacing it — the
// fragment runs the ordinary standard-material path once, whole, and then
// blends the result toward the same fragment with the sun's share removed,
// by how deep into its own shadow the ground stands at this hour.
//
// Whether it stands in shadow was decided when the terrain was made: each
// vertex carries, in the UV channel nothing else uses, the first and last
// phase of the day at which the sun clears the terrain around it — see
// `ChunkPayload::lit` in the protocol crate, which owns what the pair means.
// The thresholds interpolate across each cell like any attribute, so the
// shadow's edge lands inside cells and sweeps over the ground as the hour
// turns, with no shadow map drawn by anybody.
//
// The hour itself arrives through the uniform below, packed by the Rust side
// (`terrain.rs`), which is the single authority on it — including the swap
// onto the moon's half of the day at night.
//
// The same uniform carries the grain: a speckle over the palette colour, as
// though the ground were carved out of a block of noise. It is a function of
// world position alone — hashed here, per fragment, from nothing but the
// coordinates — so it cannot tile, cannot seam at a chunk border, and draws
// identically at every level of mesh detail. The cells are cubes rather than
// columns so that a cliff face is grained like the ground at its foot instead
// of wearing the top's texels stretched down it.

#import bevy_pbr::{
    ambient,
    forward_io::{VertexOutput, FragmentOutput},
    mesh_view_bindings::view,
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
}

struct Daylight {
    // x: the phase of the day to hold the lit intervals against. y: half the
    // width of the terminator, in phase. z: the grain's full swing as a
    // fraction of the palette colour, zero for surfaces that go without.
    // w: metres to a grain cell's edge. The reasoning for all of them lives
    // on `terrain::Daylight`.
    hour: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> daylight: Daylight;

// One grain cell's value in [0, 1). Integer mixing (PCG3D's) rather than the
// usual fract(sin(...)) because the input is a world coordinate: sine hashes
// decay into visible pattern as the numbers grow, and the world is wide.
fn grain(cell: vec3<i32>) -> f32 {
    var v = bitcast<vec3<u32>>(cell) * 1664525u + 1013904223u;
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v ^= v >> vec3<u32>(16u);
    v.x += v.y * v.z;
    return f32(v.x) / 4294967296.0;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    // The grain goes on before lighting, so both the sunlit result and the
    // shadowed one below are built from the speckled colour. The branch is on
    // a uniform, so water skips the whole thing coherently.
    let swing = daylight.hour.z;
    if swing > 0.0 {
        let cell = vec3<i32>(floor(in.world_position.xyz / daylight.hour.w));
        pbr_input.material.base_color = vec4(
            pbr_input.material.base_color.rgb * (1.0 + swing * (grain(cell) - 0.5)),
            pbr_input.material.base_color.a,
        );
    }

    pbr_input.material.base_color =
        alpha_discard(pbr_input.material, pbr_input.material.base_color);

    let full = apply_pbr_lighting(pbr_input);

    // How much of the sun reaches this fragment: inside its interval all of
    // it, outside none, with the crossing softened so the terminator sweeps
    // rather than snaps.
    let lit = in.uv;
    let edge = daylight.hour.y;
    let hour = daylight.hour.x;
    let sun = smoothstep(lit.x - edge, lit.x + edge, hour)
        * (1.0 - smoothstep(lit.y - edge, lit.y + edge, hour));

    // The same fragment with the sun's share gone: the sky's own fill, which
    // is the ambient term the full path also used — bevy's own formula, so
    // shadowed ground is exactly the indirect light it would have had, and a
    // shadow reads as a second flat tone rather than as darkness. The ground
    // is matte: no metal, no reflectance, so the specular arguments are
    // nothing and the diffuse colour is the base colour whole.
    let ndotv = max(dot(pbr_input.N, pbr_input.V), 0.0001);
    let shaded = ambient::ambient_light(
        pbr_input.world_position,
        pbr_input.N,
        pbr_input.V,
        ndotv,
        pbr_input.material.base_color.rgb,
        vec3(0.0),
        1.0,
        pbr_input.diffuse_occlusion,
    ) * view.exposure;

    var out: FragmentOutput;
    out.color = mix(vec4(shaded, full.a), full, sun);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
