// The sea's surface: the swell as vertex displacement, and flat-shaded
// facets on the result.
//
// This extends the standard PBR material rather than replacing it — the
// fragment half runs the ordinary standard-material path (colour, alpha,
// lighting, fog) with exactly one change: the normal it lights is the
// facet's own, derived from the displaced surface's slope, so every
// triangle takes a single flat tone the way the terrain's do.
//
// The wave parameters arrive through the uniform below, packed by the Rust
// side (`sea.rs`), which is the single authority on them. The formula in
// `swell` here is the twin of `sea::swell` in Rust — the boat rides what
// this draws, so the two must be kept the same.

#import bevy_pbr::{
    forward_io::{Vertex, VertexOutput, FragmentOutput},
    mesh_functions,
    mesh_view_bindings::globals,
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    view_transformations::position_world_to_clip,
}

struct SeaParams {
    // Per wave: xy is heading times wavenumber, z angular frequency,
    // w amplitude.
    waves: array<vec4<f32>, 3>,
    // x is where the swell starts fading with distance from the mesh's
    // centre, y where it has fully gone; z is how much the lit slope is
    // exaggerated over the real one (`sea::SHADING_TILT`); w padding.
    fade: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> sea: SeaParams;

// Height of the swell above the flat waterline — the twin of `sea::swell`.
// `globals.time` is `Time::elapsed_secs_wrapped`, the clock the Rust side
// samples too.
fn swell(at: vec2<f32>, time: f32) -> f32 {
    var height = 0.0;
    for (var i = 0; i < 3; i++) {
        let wave = sea.waves[i];
        height += wave.w * sin(dot(wave.xy, at) - wave.z * time);
    }
    return height;
}

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;

    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    var world_position =
        mesh_functions::mesh_position_local_to_world(world_from_local, vec4(vertex.position, 1.0));

    // Displaced in world space, so the waves stand still while the mesh
    // travels with the camera. The fade is measured in mesh-local space —
    // distance from the mesh's centre is distance from the camera, and past
    // the fade the giant rim cells beyond the fine grid stay flat.
    let fade = 1.0 - smoothstep(sea.fade.x, sea.fade.y, length(vertex.position.xz));
    world_position.y += swell(world_position.xz, globals.time) * fade;

    out.world_position = world_position;
    out.position = position_world_to_clip(world_position.xyz);
    // A placeholder: the fragment half rederives the true facet normal from
    // the displaced surface itself.
    out.world_normal = vec3(0.0, 1.0, 0.0);
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif

    return out;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    // The facet's own normal, from how the displaced surface slopes across
    // this triangle. Screen-space derivatives are constant across a
    // triangle, so this is flat shading without duplicating any vertex —
    // the terrain builds the same look into its buffers instead. The cross
    // product's handedness depends on the screen's, so rather than reason
    // about it the normal is simply pointed up, which for a sea it always is.
    var faceted = in;
    let slope = cross(dpdy(in.world_position.xyz), dpdx(in.world_position.xyz));
    var normal = normalize(slope) * sign(slope.y);
    // Lit more steeply than the water really slopes — see `sea::SHADING_TILT`
    // for why the honest tilt cannot be seen. Scaling the horizontal
    // components of a unit normal scales the slope it encodes.
    normal = normalize(vec3(normal.x * sea.fade.z, normal.y, normal.z * sea.fade.z));
    faceted.world_normal = normal;

    // From here on, exactly what the standard material would do.
    var pbr_input = pbr_input_from_standard_material(faceted, is_front);
    pbr_input.material.base_color =
        alpha_discard(pbr_input.material, pbr_input.material.base_color);

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
