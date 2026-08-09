// The sea's surface: the swell as vertex displacement, flat-shaded facets
// on the result, and foam where the shallows break it.
//
// This extends the standard PBR material rather than replacing it — the
// fragment half runs the ordinary standard-material path (colour, alpha,
// lighting, fog) with two changes: the normal it lights is the facet's own,
// derived from the displaced surface's slope, and a breaking crest whitens
// the water's colour before the lighting sees it.
//
// The wave parameters arrive through the uniform below, packed by the Rust
// side (`sea.rs`), which is the single authority on them; the depth of the
// water arrives as a small texture windowed around the camera, kept current
// by the same file. The formula in `swell` here is the twin of `sea::swell`
// in Rust — the boat rides what this draws, so the two must be kept the
// same.

#import bevy_pbr::{
    forward_io::{Vertex, VertexOutput, FragmentOutput},
    mesh_functions,
    mesh_view_bindings::globals,
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    view_transformations::position_world_to_clip,
}

struct SeaParams {
    // Per deep wave: xy is heading times wavenumber, z angular frequency,
    // w amplitude.
    waves: array<vec4<f32>, 3>,
    // x is where the swell starts fading with distance from the mesh's
    // centre, y where it has fully gone; z is how much the lit slope is
    // exaggerated over the real one (`sea::SHADING_TILT`); w padding.
    fade: vec4<f32>,
    // The shore wave: x its wavenumber down the depth, y its angular
    // frequency, z its unbroken amplitude, w the breaking slope.
    shore: vec4<f32>,
    // The shallows: xy the depths the crossfade spans (start, complete),
    // z the runup left of a broken wave, w the crest threshold foam starts
    // at.
    surf: vec4<f32>,
    // xy is the along-shore stagger as a wave vector; z the least grade the
    // bottom must be rising at for a breaking crest to foam; w a depth
    // texel's width in metres.
    stagger: vec4<f32>,
    // What must lie behind a breaker for it to be one: x metres to look
    // down the bottom's slope, y the depth that must be found there.
    feed: vec4<f32>,
    // The depth window: xy the world coordinates of its corner, z one over
    // its extent, w the depth a full texel encodes.
    window: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> sea: SeaParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var sea_depth: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var sea_depth_sampler: sampler;

// How much water stands under a point, in metres. Beyond the window's edge
// the sampler clamps to whatever the rim texel says — and everything out
// there has faded flat, so nobody is looking.
fn depth_at(at: vec2<f32>) -> f32 {
    let uv = (at - sea.window.xy) * sea.window.z;
    return textureSampleLevel(sea_depth, sea_depth_sampler, uv, 0.0).r * sea.window.w;
}

// How much of the swell at a depth is the shore wave rather than the open
// sea's — the twin of `sea::shore_weight`.
fn shore_weight(depth: f32) -> f32 {
    return 1.0 - smoothstep(sea.surf.y, sea.surf.x, depth);
}

// The shore wave's phase: down the depth itself, so crests are depth
// contours — parallel to every shore they approach — plus a slow drift
// along the coast that keeps the world's beaches from breaking in unison.
fn shore_phase(at: vec2<f32>, time: f32, depth: f32) -> f32 {
    return depth * sea.shore.x + dot(sea.stagger.xy, at) + sea.shore.y * time;
}

// The tallest wave this much water can carry — breaking, as a cap that
// never quite closes to zero, so the waterline itself keeps breathing.
fn shore_cap(depth: f32) -> f32 {
    return min(sea.shore.z, max(depth, 0.0) * sea.shore.w + sea.surf.z);
}

// Height of the swell above the flat waterline — the twin of `sea::swell`.
// `globals.time` is `Time::elapsed_secs_wrapped`, the clock the Rust side
// samples too.
fn swell(at: vec2<f32>, time: f32, depth: f32) -> f32 {
    var deep = 0.0;
    for (var i = 0; i < 3; i++) {
        let wave = sea.waves[i];
        deep += wave.w * sin(dot(wave.xy, at) - wave.z * time);
    }
    let shore = shore_cap(depth) * sin(shore_phase(at, time, depth));
    let w = shore_weight(depth);
    return deep * (1.0 - w) + shore * w;
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
    let depth = depth_at(world_position.xz);
    world_position.y += swell(world_position.xz, globals.time, depth) * fade;

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

    var pbr_input = pbr_input_from_standard_material(faceted, is_front);

    // Foam: white where the shore wave is breaking — its cap biting, which
    // is water shallower than the unbroken wave demands — and only on the
    // crest's face, and only where the bottom is actually rising to trip
    // it. The last condition is what keeps foam off the tidal flats: depth
    // is phase, so a flat an inch deep crests everywhere at once, and
    // without a slope gate it flashes white as a sheet. The gradient is
    // read from the depth window a texel out either way, which pins the
    // foam line to the shelf edges and beach faces where the depth is
    // moving. Every edge is a step, so the foam arrives as hard-edged
    // bands rolling shoreward, a flat tone like every other tone here.
    let at = in.world_position.xz;
    let depth = depth_at(at);
    let texel = vec2(sea.stagger.w, 0.0);
    let grade = vec2(
        depth_at(at + texel.xy) - depth_at(at - texel.xy),
        depth_at(at + texel.yx) - depth_at(at - texel.yx),
    ) / (2.0 * texel.x);
    // And a breaker must have deeper water at its back: down the slope,
    // within reach, real depth — or this face is a ripple in a lagoon
    // floor with nothing arriving to break on it.
    let steepness = length(grade);
    let downhill = grade / max(steepness, 1e-5);
    let fed = depth_at(at + downhill * sea.feed.x);
    let breaking_depth = (sea.shore.z - sea.surf.z) / sea.shore.w;
    let crest = sin(shore_phase(at, globals.time, depth));
    let foam = step(sea.surf.w, crest)
        * step(depth, breaking_depth)
        * step(sea.stagger.z, steepness)
        * step(sea.feed.y, fed)
        * shore_weight(depth);
    // Not quite white, and nearly opaque — surf hides the bed under it.
    pbr_input.material.base_color = mix(
        pbr_input.material.base_color,
        vec4(0.82, 0.87, 0.88, 0.97),
        foam,
    );

    pbr_input.material.base_color =
        alpha_discard(pbr_input.material, pbr_input.material.base_color);

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
