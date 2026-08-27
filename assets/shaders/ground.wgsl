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
// The shadow pass still exists for what baking cannot answer — the boat, the
// palms, whatever moves — and the ground reads its map here itself rather
// than letting the standard path apply it: the mesh is `NotShadowReceiver`
// (see the chunk spawn in `terrain.rs`), and the filtered value the map
// gives back is cut down to an edge before it darkens anything. Filtering
// first and hardening after is the point: the Gaussian's answer slides
// smoothly as a caster moves, so the hardened edge sweeps instead of
// crawling texel to texel — which is what reading the map raw does, and why
// hard filtering on the camera was tried and thrown out.
//
// The hour itself arrives through the uniform below, packed by the Rust side
// (`terrain.rs`), which is the single authority on it — including the swap
// onto the moon's half of the day at night.
//
// The same uniform carries the grain's swing: a speckle over the palette
// colour, as though the ground were carved out of a block of noise. It is a
// function of world position alone — hashed here, per fragment, from nothing
// but the coordinates — so it cannot tile, cannot seam at a chunk border, and
// draws identically at every level of mesh detail. The cells are cubes rather
// than columns so that a cliff face is grained like the ground at its foot
// instead of wearing the top's texels stretched down it, and the speckle
// bows out where a pixel outgrows a cell, because past that line it could
// only seethe.
//
// The grain is also what softens a material boundary. The mesh gives every
// cell one flat palette colour, so a coast crosses the ground as a
// ruler-straight cell edge; near the eye, each grain texel instead *picks*
// one of the four cells it stands amongst, with the odds sliding from one
// side's material to the other's across the boundary — so the line frays
// into a fringe of interleaved texels, and every pixel is still a pure
// palette colour rather than a blend of two. The cells' materials are read
// from the window texture below; far away — and off the window — the pick
// collapses to the cell the fragment stands in, which is the mesh's own
// colour, so the dither hands back to exactly what the vertices already say.

#import bevy_pbr::{
    ambient,
    forward_io::{VertexOutput, FragmentOutput},
    mesh_view_bindings::{view, lights},
    mesh_view_types,
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    shadows,
    view_transformations,
}

struct Shading {
    // x: the phase of the day to hold the lit intervals against. y: half the
    // width of the terminator, in phase. z: the grain's full swing as a
    // fraction of the palette colour, zero for surfaces that go without.
    // w: how far out cast shadows are drawn hard, in metres of view depth,
    // zero for surfaces that take none. The reasoning for all of them lives
    // on `terrain::Shading`.
    hour: vec4<f32>,
    // The material window's place in the world — the lanes are read here and
    // owned, like the hour's, by `terrain::Shading`. All zeroes on surfaces
    // that never look, which a width of nothing keeps out of the dither.
    window: vec4<f32>,
    // What each material is drawn as, indexed by a window texel's number
    // less one — `Material::KINDS` entries; the length is held to the Rust
    // side's by the bind group, which refuses a buffer of any other size.
    palette: array<vec4<f32>, 18>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> shading: Shading;

// A material's number plus one per metre cell around the camera, zero for a
// cell the client has not been told about — see `terrain::MaterialWindow`,
// which owns the scroll and the sweep that keep it current.
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var materials: texture_2d<u32>;

// Metres to a grain cell's edge. A constant here rather than a lane of the
// uniform: nothing varies it per material, and a divisor that lives in a
// buffer is a divisor that can arrive zero.
const CELL: f32 = 0.5;

// How far the grain lattice is slid off the world's own, on every axis. The
// wire's heights are 0.02 m steps and the mesh's corners whole metres, so an
// unslid lattice would lay cell faces exactly along flat ground and standing
// edges — ground pinned dead on a face flickers between the cells either side
// as interpolation rounds, the hazard `terrain::OFF_LATTICE` names for the
// water sheet. An eighth of a metre is exact in f32 and shares no multiple
// with either grid.
const OFF_LATTICE: f32 = 0.125;

// How much of the shadow map's filtered gradient a cast shadow keeps, either
// side of a half. The full gradient is the soft blur this file exists to keep
// out of the picture; none at all is a stair-step the screen has no MSAA
// against, since a shadow is shading rather than an edge. A tenth leaves
// about a pixel of easing at the map's resolution.
const CAST_EDGE: f32 = 0.1;

// One grain cell's two independent values in [0, 1) — the speckle draws on
// the first and the material pick on the second, so a texel's brightness
// says nothing about which side of a boundary it took. Integer mixing
// (PCG3D's) rather than the usual fract(sin(...)) because the input is a
// world coordinate: sine hashes decay into visible pattern as the numbers
// grow, and the world is wide. The top 24 bits alone go to float, where they
// are exact — a whole u32 rounds, and the largest round *up*, closing the
// interval.
fn grains(cell: vec3<i32>) -> vec2<f32> {
    var v = bitcast<vec3<u32>>(cell) * 1664525u + 1013904223u;
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v ^= v >> vec3<u32>(16u);
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    return vec2(f32(v.x >> 8u), f32(v.y >> 8u)) / 16777216.0;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    // The grain goes on before lighting, so both the sunlit result and the
    // shadowed one below are built from the speckled colour.
    //
    // The footprint — how much ground this pixel spans — is taken outside the
    // branch because derivatives want uniform control flow; the fade it buys
    // takes the grain to nothing by the point a pixel swallows half a cell,
    // which is where a texel stops being drawable and starts being one
    // arbitrary cell per pixel, re-rolled every time the camera moves. MSAA
    // smooths edges, not shading, so nothing else stands in the way of that.
    let at = in.world_position.xyz + vec3(OFF_LATTICE);
    let footprint = fwidth(at);
    let sharp = 1.0 - smoothstep(0.25, 0.5, max(footprint.x, max(footprint.y, footprint.z)) / CELL);
    let swing = shading.hour.z * sharp;
    if swing > 0.0 {
        let cube = vec3<i32>(floor(at / CELL));
        let r = grains(cube);

        // The dither. Each texel stakes the cells around it against each
        // other, odds by nearness — and slid toward certainty on its own
        // cell as the grain fades or the window's edge nears, so what the
        // dither hands back to is the mesh's own colour.
        //
        // The whole wager is decided once per texel — hashed, weighed and
        // anchored at the texel's own centre, never at the fragment — so a
        // boundary steps texel by texel instead of curving smoothly through
        // them, which is the entire low-res look. And the texel here is the
        // plan-view column rather than the speckle's cube: the material
        // grid has no height axis for the odds to follow, and a texel
        // sliced into y-layers re-rolls the pick along every contour of a
        // slope, which reads as fine noise rather than as texels.
        let p = in.world_position.xz;
        let tex = p - shading.window.xy;
        let inside = min(min(tex.x, tex.y), shading.window.z - max(tex.x, tex.y));
        // The standoff before the fade-in keeps all four reads on the
        // window from any fragment of the texel: half a cell of centre
        // wander, and a cell of reach either side of the centre.
        let blend = sharp * smoothstep(0.5 + CELL, shading.window.w, inside);
        if blend > 0.0 {
            let column = vec2<i32>(cube.x, cube.z);
            let pick = grains(vec3(column.x, column.y, 0)).y;
            let centre = (vec2<f32>(column) + 0.5) * CELL - OFF_LATTICE;
            let base = floor(centre - 0.5);
            let f = centre - 0.5 - base;
            let own = floor(centre);
            var ids: array<u32, 4>;
            var weights: array<f32, 4>;
            var own_id = 0u;
            var total = 0.0;
            for (var i = 0u; i < 4u; i++) {
                let corner = vec2(f32(i & 1u), f32(i >> 1u));
                let cell = base + corner;
                let id = textureLoad(materials, vec2<i32>(cell - shading.window.xy), 0).r;
                let nearness = mix(1.0 - f, f, corner);
                let anchored = select(0.0, 1.0, all(cell == own));
                if anchored > 0.5 {
                    own_id = id;
                }
                ids[i] = id;
                // A cell the client has not been told about wagers nothing.
                weights[i] = mix(anchored, nearness.x * nearness.y, blend) * f32(min(id, 1u));
                total += weights[i];
            }
            // Only a texel whose own cell is known wagers at all: with the
            // anchor's cell missing, sliding toward certainty on it has
            // nowhere to land, and the mesh's colour is the honest answer —
            // which is also what covers a scroll-exposed strip until the
            // sweep reaches it. Starting from the own cell also means the
            // walk falling out the bottom — the stake rounding up to the
            // whole of `total` — leaves the texel its own material rather
            // than nothing.
            if own_id != 0u {
                var chosen = own_id;
                let stake = pick * total;
                var cum = 0.0;
                for (var i = 0u; i < 4u; i++) {
                    cum += weights[i];
                    if stake < cum {
                        chosen = ids[i];
                        break;
                    }
                }
                pbr_input.material.base_color = vec4(
                    shading.palette[chosen - 1u].rgb,
                    pbr_input.material.base_color.a,
                );
            }
        }

        // The speckle, over whichever colour the dither settled on.
        pbr_input.material.base_color = vec4(
            pbr_input.material.base_color.rgb * (1.0 + swing * (r.x - 0.5)),
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
    let edge = shading.hour.y;
    let hour = shading.hour.x;
    var sun = smoothstep(lit.x - edge, lit.x + edge, hour)
        * (1.0 - smoothstep(lit.y - edge, lit.y + edge, hour));

    // The cast shadows, folded in by min, not product — a fragment is in
    // shadow for either reason, not twice as dark for both. Skipped whole
    // where the baked shadow has already settled it, and on surfaces whose
    // reach is nothing — see the `w` lane above.
    let reach = shading.hour.w;
    if reach > 0.0 && sun > 0.0 {
        let view_z = view_transformations::position_world_to_view(in.world_position.xyz).z;
        // Light 0 is the sky's, sun or moon by turns, and the world hangs no
        // other: point and spot lights would need their own reads. An empty
        // slot's flags are zero, so no light at all fails the same test the
        // console's `shadows off` does.
        if (lights.directional_lights[0].flags
            & mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u {
            let raw = shadows::fetch_directional_shadow(
                0u, in.world_position, in.world_normal, view_z, in.position.xy,
            );
            // Hard up close; eased back to the filter's own softness by
            // `reach`, where a shadow is small on screen, a caster thinner
            // than the far cascade's kernel would be thresholded away, and
            // the cascades' cross-fade must not be cut into a step.
            let hard = smoothstep(0.5 - CAST_EDGE, 0.5 + CAST_EDGE, raw);
            let ease = smoothstep(0.75 * reach, reach, -view_z);
            sun = min(sun, mix(hard, raw, ease));
        }
    }

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
