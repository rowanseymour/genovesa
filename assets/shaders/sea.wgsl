// The sea's surface: the swell as vertex displacement, flat-shaded facets
// on the result, and foam — where the shallows break a wave, and where the
// open sea breaks its own.
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
    ambient,
    forward_io::{Vertex, VertexOutput, FragmentOutput},
    mesh_functions,
    mesh_view_bindings::{globals, view},
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    view_transformations::position_world_to_clip,
}

// How many points of track a wake arrives as — the twin of `wake::TRAIL`,
// which `the_shader_walks_the_whole_track` holds this line to.
const TRAIL: i32 = 34;

// How many open hulls the sea can be cut for at once — the twin of
// `boat::HOLES`, which `the_shader_cuts_for_every_hull` holds this line to.
const HOLES: i32 = 8;

struct SeaParams {
    // Per deep wave: xy is heading times wavenumber, z angular frequency,
    // w amplitude.
    waves: array<vec4<f32>, 3>,
    // x is where the swell starts fading with distance from the mesh's
    // centre, y where it has fully gone; z is how much the lit slope is
    // exaggerated over the real one (`sea::SHADING_TILT`); w how far the
    // crests are bent off straight (`sea::BEND`).
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
    // zw is the murk: the depths across which the water's alpha climbs to
    // fully opaque, so the bed past the second is never seen.
    feed: vec4<f32>,
    // The open sea's whitecaps: x how far up the swell's leading face one
    // starts, y the height the sea must be heaping under it, z how far that
    // height wanders about; w padding.
    caps: vec4<f32>,
    // The field that wandering is read off: x the size of its coarsest cell
    // in metres, y how fast it drifts downwind; zw padding.
    breaking: vec4<f32>,
    // The depth window: xy the world coordinates of its corner, z one over
    // its extent, w the depth a full texel encodes.
    window: vec4<f32>,
    // The hour to hold the window's lit intervals against, and half the
    // width of the terminator: x and y, exactly as the ground's own shader
    // carries them. zw padding.
    daylight: vec4<f32>,
    // The wake's band: x the half-width of the water a hull turns over at its
    // stem, y how far the arms open per metre run, z how thick an arm is, w
    // how long a wake lasts.
    wash: vec4<f32>,
    // The boil and what wears it away: x how fast it widens in metres per
    // second of age, y how many seconds of it there are, z the cell of the
    // field an ageing wake breaks up on, w the least way that leaves a mark.
    boil: vec4<f32>,
    // Where the wake could possibly be: xy the least corner, zw the greatest.
    wake_bounds: vec4<f32>,
    // The hull's track, newest first: xy where its stem was, z how many
    // seconds ago, w the way it was making then.
    wake: array<vec4<f32>, TRAIL>,
    // The holes the open hulls cut in the surface, filled from the front and
    // nearest the eye first: xy the centre of one's waterline footprint —
    // its widest station — and zw the way that hull is pointing.
    hole: array<vec4<f32>, HOLES>,
    // Each footprint's reach from that centre: x forward to where the
    // outline closes at the stem, y aft to where the stern piece would
    // close, z half its width at the widest, w 1.0 in a slot that holds a
    // boat — which is what ends the list.
    hole_axes: array<vec4<f32>, HOLES>,
    // Their shapes: xy how full-bodied the bow and stern pieces are — their
    // superellipse exponents — and z metres from the centre aft to the
    // transom, where the outline is cut square.
    hole_shape: array<vec4<f32>, HOLES>,
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

// A byte of the window read back as a phase of the day. The window stores
// the wire's own 256ths of a day (`protocol::quantize_phase`), and a Unorm
// texture hands them back over 255 — so the step count comes back first and
// is then read the way the wire means it.
const PHASE_STEPS: f32 = 256.0;
fn phase_of(channel: f32) -> f32 {
    return channel * 255.0 / PHASE_STEPS;
}

// How much of the sun reaches the water at a point: none before the ground
// around it lets the sun through, none after it takes it away, and the
// crossings softened so a headland's shadow sweeps over the water rather
// than snapping across it.
//
// This is the sea's half of what `ground.wgsl` does with the same intervals.
// It reads them from the window rather than from its own vertices because
// the sea is one plane that follows the camera and never met a chunk's
// corners — see `sea::refresh_depth`, which fills the window's other two
// channels.
fn sunlight_at(at: vec2<f32>) -> f32 {
    let uv = (at - sea.window.xy) * sea.window.z;
    let lit = textureSampleLevel(sea_depth, sea_depth_sampler, uv, 0.0).gb;
    let first = phase_of(lit.x);
    let last = phase_of(lit.y);
    let hour = sea.daylight.x;
    let edge = sea.daylight.y;
    return smoothstep(first - edge, first + edge, hour)
        * (1.0 - smoothstep(last - edge, last + edge, hour));
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

// The bend the swell is read through — the twin of `sea::bend`, which is
// where the reasoning lives. Unit-ish, in metres once the amplitude out of
// the uniform has scaled it.
fn bend(at: vec2<f32>) -> vec2<f32> {
    let field = vec2(
        sin(dot(at, vec2(0.01079, -0.00917))) + 0.5 * sin(dot(at, vec2(-0.02347, 0.01768))),
        sin(dot(at, vec2(0.00774, 0.01209))) + 0.5 * sin(dot(at, vec2(0.01918, 0.02236))),
    );
    return field * sea.fade.w;
}

// The open sea's own swell: the three deep trains, summed. Written out of
// `swell` because the whitecaps want the deep water's crests on their own —
// the shore wave has its own foam and its own reasons for it, and adding the
// two before asking how tall a crest is would put caps on the shallows.
// Where in its own cycle one train stands at a point — its phase, with the
// bend in it.
//
// Everything that asks a train *anything* has to come through here, or it is
// asking about a wave that is not the one being drawn. The whitecaps learned
// that the hard way: they read the leading face off the raw phase for a
// while, and painted dead straight ribbons across a sea whose crests had long
// since stopped being straight.
fn train_phase(index: i32, at: vec2<f32>, time: f32) -> f32 {
    // The bend the whole swell is read through — the twin of `sea::bend`, and
    // see `sea::BEND` for what it is for. Scaled by the longest train's
    // wavenumber, so every train is bent by the same fraction of its own
    // wavelength rather than by the same number of metres.
    let bent = bend(at) * length(sea.waves[0].xy);
    let wave = sea.waves[index];
    return dot(wave.xy, at) + dot(normalize(wave.xy), bent) - wave.z * time;
}

fn deep(at: vec2<f32>, time: f32) -> f32 {
    var height = 0.0;
    for (var i = 0; i < 3; i++) {
        height += sea.waves[i].w * sin(train_phase(i, at, time));
    }
    return height;
}

// How high the sea has to be heaping here before it breaks — the bar, with a
// wandering field added to it.
//
// The bar alone is a constant, and a constant bar over three sines is a
// lattice: the crests beat against each other in a pattern, and thresholding
// a pattern draws it. Close up that passes for water; from a boat looking out
// over a kilometre of it, the whole ocean is stamped with rows of identical
// commas, which is worse than having no caps at all.
//
// So the bar wanders, on the one field in this file that is not made of
// sines. Sines were tried and are the reason this comment is long: any sum of
// them is periodic, so a bar built that way trades one lattice for a slower
// lattice, and the eye finds the beat about as fast either way. Noise has no
// beat to find.
//
// The swing is a fraction of the sea's own full height rather than a fixed
// number of metres, and that is what makes it work at every wind. To take
// caps off a patch of water the bar has to climb past what the swell there
// can reach, and what it can reach is the wind's business: a swing in metres
// big enough to leave bare patches in a blow would sit above the whole sea in
// a breeze and leave no caps anywhere. The floor it swings about stays
// absolute, though, which is what still keeps a calm clean — see `WHITECAP`.
fn cap_bar(at: vec2<f32>, time: f32) -> f32 {
    var ceiling = 0.0;
    for (var i = 0; i < 3; i++) {
        ceiling += sea.waves[i].w;
    }
    // Downwind, because that is what gusts do — and the first train runs with
    // the wind by construction, so its heading is the wind's without the
    // shader being told the wind at all.
    let drift = normalize(sea.waves[0].xy) * sea.breaking.y * time;
    return sea.caps.y + sea.caps.z * ceiling * gustiness(at - drift);
}

// One integer lattice point's own number, in 0..1. An ordinary integer hash:
// multiply by odd constants, fold the high bits down over the low ones, and
// what comes out has no relation to what went in that any pattern survives.
fn lattice(cell: vec2<i32>) -> f32 {
    var h = u32(cell.x) * 374761393u + u32(cell.y) * 668265263u;
    h = (h ^ (h >> 13u)) * 1274126177u;
    return f32(h ^ (h >> 16u)) * (1.0 / 4294967295.0);
}

// Value noise: the lattice's numbers, smoothly interpolated across each cell.
// The weights are the same smoothstep the fades here use, which is what makes
// the field's slope continuous across a cell boundary — with straight
// bilinear weights the seams show as creases wherever the bar crosses the
// heaping, and a grid of creases is the lattice all over again.
fn value_noise(at: vec2<f32>) -> f32 {
    let cell = floor(at);
    let corner = vec2<i32>(cell);
    let f = at - cell;
    let w = f * f * (3.0 - 2.0 * f);
    let along_bottom = mix(lattice(corner), lattice(corner + vec2(1, 0)), w.x);
    let along_top = mix(lattice(corner + vec2(0, 1)), lattice(corner + vec2(1, 1)), w.x);
    return mix(along_bottom, along_top, w.y);
}

// How prone to breaking the water here is, in -1..1: three octaves of value
// noise, the coarsest a couple of hundred metres across.
//
// The coarsest octave is the one that matters — it decides whether a stretch
// of sea is breaking at all, which is what leaves bare water between the
// patches — and the finer two are what keep the caps inside a patch from
// coming out all the same size. Three is where adding more stopped changing
// the picture, the fourth being finer than a cap.
fn gustiness(at: vec2<f32>) -> f32 {
    let cell = sea.breaking.x;
    var field = 0.45 * value_noise(at / cell);
    field += 0.33 * value_noise(at / (cell * 0.4) + 31.7);
    field += 0.22 * value_noise(at / (cell * 0.15) + 78.3);
    return field * 2.0 - 1.0;
}

// The white a boat's wake lays down here — the twin of nothing, this being
// the one piece of foam the Rust side never has to agree about, since no
// hull rides it. `sea.wake` is the hull's track, newest first; `wake.rs` owns
// every constant it is read with, and the module doc there is where the shape
// is argued.
//
// The whole of the shape comes from one question: how far is this water from
// the line the boat sailed, and how long ago was the nearest bit of that line
// laid down? Distance gives the two arms and the boil their edges, age takes
// both of them away again.
fn wake_foam(at: vec2<f32>) -> f32 {
    // Water the wake cannot reach is off in two comparisons rather than
    // thirty-two segments. The box is most of the ocean, and the branch is
    // coherent over it — whole tiles of the screen take it together.
    if (any(at < sea.wake_bounds.xy) || any(at > sea.wake_bounds.zw)) {
        return 0.0;
    }

    // The nearest point of the track, and what the track was doing there.
    // Slots past the end of a short track repeat its last point, which makes
    // a segment of no length — hence the guard on the projection rather than
    // a count of live points.
    var nearest = 1e9;
    var age = 0.0;
    var way = 0.0;
    for (var i = 1; i < TRAIL; i++) {
        let newer = sea.wake[i - 1];
        let older = sea.wake[i];
        let along = older.xy - newer.xy;
        let run = dot(along, along);
        let raw = select(0.0, dot(at - newer.xy, along) / run, run > 1e-6);
        // The clamp rounds every end off. Water off either end of a segment
        // is measured to the nearer point, so each joint of the track wears a
        // cap — the same answer the neighbouring segment gives anyway — and
        // the head wears a half-disc of white whose forward edge falls on the
        // stem, because `wake.rs` lays the head that cap's radius abaft it.
        // Both other shapes for the bow were tried: the head on the stem
        // itself put the cap's white half a beam out in front of a boat that
        // had not made it, and chopping the cap off ended the foam on a ruled
        // line across the bow — the one shape water never makes. Rounded and
        // set back, the wake opens from the bow point.
        let t = clamp(raw, 0.0, 1.0);
        let reach = distance(at, mix(newer.xy, older.xy, t));
        if (reach < nearest) {
            nearest = reach;
            age = mix(newer.z, older.z, t);
            way = mix(newer.w, older.w, t);
        }
    }

    // What age does to any of it. Not a fade: the foam is thresholded against
    // the same value noise the whitecaps' bar wanders on, so old white goes to
    // patches and then to nothing, and every edge in it stays as hard as every
    // other edge on this water. Fresh foam clears a threshold of zero
    // everywhere, which is why nothing near the hull needs a special case.
    // Both shapes are worn away by the one field, at their own rates — a
    // second field would only mean two patterns of holes crossing each other.
    let mottle = value_noise(at / sea.boil.z);

    // The band the wake opens into: the hull's shoulder at the stem, spreading
    // at a fixed angle down the track — so the arms diverge with distance
    // run, which is way times age, and a boat crawling throws a narrow one.
    let half = sea.wash.x + sea.wash.y * way * age;
    let arms = step(half - sea.wash.z, nearest) * step(nearest, half)
        * step(age / sea.wash.w, mottle);
    // The boil: the water the hull is itself turning over, filled rather than
    // outlined, widening on its own clock rather than with distance run, and
    // breaking up on a life of its own — see `wake::BOIL`. Given a hard end
    // instead it finishes on a ruled line drawn across the wake, which is the
    // one shape water never makes.
    let boil = step(nearest, sea.wash.x + sea.boil.x * age)
        * step(age / sea.boil.y, mottle);

    // Under the way it takes to stir the water, nothing at all — including
    // the stretch of a dying wake nearest the hull, which is how a wake
    // retreats down its own track as a boat glides to a stop.
    return max(arms, boil) * step(sea.boil.w, way);
}

// Height of the swell above the flat waterline — the twin of `sea::swell`.
// `globals.time` is `Time::elapsed_secs_wrapped`, the clock the Rust side
// samples too.
fn swell(at: vec2<f32>, time: f32, depth: f32) -> f32 {
    let shore = shore_cap(depth) * sin(shore_phase(at, time, depth));
    let w = shore_weight(depth);
    return deep(at, time) * (1.0 - w) + shore * w;
}

// Whether a point of the surface stands inside some open hull, and so is not
// drawn at all. An open boat is looked *into* from this camera, and the sea
// is one sheet drawn straight through everything — so without this it stands
// in the bilges of any hull whose sole is where a real one's is. The
// footprint is two superellipse halves in the hull's own frame, sized by the
// Rust side so its edge lands within the planking, where the hull's own
// timber hides the seam from every angle that matters. Per fragment rather
// than per vertex because the whole boat is smaller than one sea facet.
//
// Every hull the Rust side sent, not just the one being sailed: two boats
// lying a beam apart are both looked into, and cutting only one of them puts
// water to the thwarts of the other while its neighbour sits dry.
fn inside_a_hull(at: vec2<f32>) -> bool {
    for (var i = 0; i < HOLES; i++) {
        let axes = sea.hole_axes[i];
        // `boat::cut_the_water` fills the slots from the front, so the first
        // empty one is the end of the list — which is what keeps this loop
        // costing the ocean a single comparison on the ordinary frame with
        // one boat afloat on it.
        if (axes.w < 0.5) {
            break;
        }
        let hole = sea.hole[i];
        let shape = sea.hole_shape[i];
        // The circle is only a cheap first refusal, so it has to be a bound
        // and not a guess: no point of the footprint is further from its
        // centre than the beam plus its longer end, whichever end that is.
        // Bounding on the bow alone would be a hole with its stern quietly
        // cut off on the day some boat's transom reaches further aft than
        // its stem does forward.
        let reach = axes.z + max(axes.x, shape.z);
        if (distance(at, hole.xy) >= reach) {
            continue;
        }

        let rel = at - hole.xy;
        let ahead = hole.zw;
        let along = dot(rel, ahead);
        let athwart = abs(rel.x * ahead.y - rel.y * ahead.x);
        // Two superellipse halves sharing their beam at the widest station,
        // cut square at the transom — because one ellipse cannot be a boat:
        // fat enough for the transom's corners it wraps whole metres of
        // clear water at the bow, and fine enough for the bow it pinches at
        // the quarters and lets slivers of sea into the sternsheets. The
        // exponents say how full each end's body is; the water abaft the
        // transom is ordinary sea, however far the stern piece would reach.
        if (along <= -shape.z) {
            continue;
        }
        let bow = along > 0.0;
        let semi = select(axes.y, axes.x, bow);
        let fullness = select(shape.y, shape.x, bow);
        let u = abs(along) / semi;
        let v = athwart / axes.z;
        if (pow(u, fullness) + pow(v, fullness) < 1.0) {
            return true;
        }
    }
    return false;
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
    // The holes the open hulls cut — see `inside_a_hull`.
    if (inside_a_hull(in.world_position.xz)) {
        discard;
    }

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

    // The murk: water over a deep enough bottom is opaque — see `sea::MURK`
    // for the depths and for why the line is the anchor's. It climbs from
    // the material's own alpha rather than replacing it, so shallow water
    // keeps exactly the translucency it always had.
    let murk = smoothstep(sea.feed.z, sea.feed.w, depth);
    pbr_input.material.base_color.a = mix(pbr_input.material.base_color.a, 1.0, murk);

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

    // Whitecaps: the open sea's own foam, and two conditions rather than
    // one, because they answer different halves of what a whitecap is.
    //
    // Where — the swell has to be heaping. Nothing here is told how hard it
    // is blowing; the wind is already in the amplitudes, so a fixed height
    // in metres is a bar the sea clears more often the harder it blows, and
    // never in a calm, when the whole swell is a few centimetres. The three
    // trains beating against each other are what keep that from being a
    // pattern: the sum only clears the bar where they happen to agree, and
    // where that is drifts.
    //
    // What shape — a band down the leading face of the longest train, which
    // is the wave the eye reads the sea by. `WAVES` is ordered longest
    // first, and `the_waves_run_long_to_short` holds it that way.
    //
    // The order of the two conditions is the whole of how this looks. The
    // band is the shape and the heaping only cuts it up, so a cap comes out
    // as a sliver lying along a crest, broken where the sea is not heaping
    // enough to carry it. Done the other way about — heaping for the shape,
    // the wave to trim it — every cap is a round blob a few metres across,
    // because the heap is the smaller of the two, and a sea of round white
    // blobs reads as spots of paint rather than as water falling over.
    //
    // `-cos` is the leading face: the height's rate for one train, which
    // peaks a quarter wave ahead of the crest, so the white sits where the
    // water is climbing towards breaking rather than symmetrically on top.
    let leading = -cos(train_phase(0, at, globals.time));
    // And only where the mesh is still waving. Past the fade the surface is
    // flat however tall the sum says the swell is, and foam painted out
    // there would be white lying on glass. Measured from the camera rather
    // than from the mesh's own centre, which is the same point to within the
    // cell the mesh is snapped to, and hundreds of metres inside this fade.
    let waving = 1.0 - smoothstep(sea.fade.x, sea.fade.y, distance(at, view.world_position.xz));
    let cap = step(sea.caps.x, leading)
        * step(cap_bar(at, globals.time), deep(at, globals.time))
        * (1.0 - shore_weight(depth))
        * waving;

    // One white for all three, the shallows', the open sea's and the boat's:
    // they are the same water doing the same thing, and three whites would
    // read as three materials. Not quite white, and nearly opaque — foam
    // hides what is under it.
    pbr_input.material.base_color = mix(
        pbr_input.material.base_color,
        vec4(0.82, 0.87, 0.88, 0.97),
        max(max(foam, cap), wake_foam(at)),
    );

    pbr_input.material.base_color =
        alpha_discard(pbr_input.material, pbr_input.material.base_color);

    var out: FragmentOutput;
    let full = apply_pbr_lighting(pbr_input);

    // The same fragment with the sun's share gone — the sky's own light,
    // which is what water in a headland's shadow is lit by. Bevy's own
    // formula, so shaded water is exactly the indirect light it would have
    // had; the sea is matte enough that its specular arguments are nothing.
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

    out.color = mix(vec4(shaded, full.a), full, sunlight_at(at));
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
