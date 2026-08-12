# Raises shark.blend from nothing — the script the master was first built by.
#
#   /Applications/Blender.app/Contents/MacOS/Blender --background --python build.py
#
# The .blend it writes is the master, exactly as a hand-modelled one would be:
# open it, move things, re-run export.sh, and this script is out of date the
# moment that happens. It is kept because it is the honest record of every
# number in the file and the reasons live in NOTES.md — but it *builds* the
# master, it does not *define* it. If the .blend has been touched since, do
# not re-run this over it.
#
# Everything here is in Blender's own frame: Z up, the shark facing +Y, metres.
# The +Y-up export turns that into Bevy's -Z forward like every other model.

import math

import bpy
import bmesh
from mathutils import Vector

# --- The shape, in numbers -----------------------------------------------
#
# A reef shark a touch over life size, 2.6 m nose to tail tip: big enough to
# read as a shark from a camera forty metres up, small enough not to promise
# a monster. Origin amidships, spine at Z zero — the game holds the origin at
# cruising depth and yaws the whole body about it.

# Body stations, nose to peduncle: (y, radius). Girth peaks forward of the
# middle and the peduncle pinches, which is most of what says "shark" in
# profile once the fins do the rest.
STATIONS = [
    (1.28, 0.05),
    (1.02, 0.14),
    (0.58, 0.24),
    (0.12, 0.28),
    (-0.38, 0.21),
    (-0.78, 0.12),
    (-1.02, 0.055),
]
# Rings are hexagons — the fewest corners that read as round — with a vertex
# at top and bottom, so the back carries a ridge line the dorsal fin sits on.
RING_ANGLES = [30, 90, 150, 210, 270, 330]
# A shark is a little deeper than it is wide is a lie — it is the other way —
# but barely at this scale; the squash keeps the belly from reading round.
Z_SQUASH = 0.95

# The dorsal fin: the one part of this model the player will usually see, so
# it is tall — the tip stands 0.65 above the spine, which is what cuts the
# surface when the body rides just under it. Nearly upright: a raked fin is a
# dolphin's (see the dolphin's NOTES), and upright is the species.
DORSAL = [(0.40, 0.22), (0.05, 0.65), (-0.08, 0.22)]

# The caudal fin, in the same X=0 plane: vertical, upper lobe longer than the
# lower — heterocercal, and the other half of "shark" against the dolphin's
# horizontal flukes.
CAUDAL = [
    (-0.92, 0.05),
    (-1.36, 0.50),
    (-1.18, 0.02),
    (-1.30, -0.24),
    (-0.92, -0.06),
]

# Pectorals: one triangular plate a side, swept back and down off the belly
# line, port mirrored from starboard.
PECTORAL = [(0.16, 0.42, -0.14), (0.20, 0.22, -0.16), (0.60, 0.05, -0.30)]

PLATE = 0.03  # fin thickness — a facet, not a membrane

# Where the skeleton bends: the tail takes over aft of the last full ring,
# the caudal fin at the peduncle.
HIPS = -0.35
PEDUNCLE = -1.02
TAIL_TIP = -1.36
NOSE = 1.28

# The swim: a wave that travels nose to tail, each joint lagging the one
# ahead — amplitudes in radians, lags in radians of cycle phase. The body
# barely works, the caudal fin does the propelling, which is how a shark
# actually swims and reads as one from any distance.
CYCLE_FRAMES = 24  # one second at the scene's 24 fps
SWISH = {
    "body": (0.06, 0.0),
    "tail": (0.16, 1.1),
    "caudal": (0.28, 2.2),
}


def ring(y, r):
    return [
        Vector((
            r * math.cos(math.radians(a)),
            y,
            r * Z_SQUASH * math.sin(math.radians(a)),
        ))
        for a in RING_ANGLES
    ]


def build_hide():
    """One mesh, `hide`: the body lofted through the rings, capped both
    ends, with the three fin plates as thin closed prisms of their own.
    Returns the mesh object and the vertex index ranges of each part."""
    verts, faces = [], []
    parts = {}

    def plate(outline, normal):
        """A closed prism: the outline pushed half the thickness either way
        along the normal, capped both sides. Winding is left to the normal
        recalculation below — every shell here is closed, so outward is
        well defined."""
        start = len(verts)
        n = normal.normalized() * (PLATE / 2.0)
        count = len(outline)
        for p in outline:
            verts.append(p + n)
        for p in outline:
            verts.append(p - n)
        faces.append([start + i for i in range(count)])
        faces.append([start + count + i for i in reversed(range(count))])
        for i in range(count):
            j = (i + 1) % count
            faces.append([start + i, start + j, start + count + j, start + count + i])
        return range(start, len(verts))

    # The body: rings joined by quads, hexagon caps at nose and peduncle.
    body_start = len(verts)
    for y, r in STATIONS:
        verts.extend(ring(y, r))
    n = len(RING_ANGLES)
    for s in range(len(STATIONS) - 1):
        a, b = body_start + s * n, body_start + (s + 1) * n
        for i in range(n):
            j = (i + 1) % n
            faces.append([a + i, a + j, b + j, b + i])
    faces.append([body_start + i for i in range(n)])
    last = body_start + (len(STATIONS) - 1) * n
    faces.append([last + i for i in reversed(range(n))])
    parts["rings"] = [
        (y, range(body_start + s * n, body_start + (s + 1) * n))
        for s, (y, _) in enumerate(STATIONS)
    ]

    parts["dorsal"] = plate([Vector((0.0, y, z)) for y, z in DORSAL], Vector((1, 0, 0)))
    parts["caudal"] = plate([Vector((0.0, y, z)) for y, z in CAUDAL], Vector((1, 0, 0)))
    for side in (1.0, -1.0):
        outline = [Vector((side * x, y, z)) for x, y, z in PECTORAL]
        if side < 0:
            outline.reverse()
        across = (outline[1] - outline[0]).cross(outline[2] - outline[0])
        parts["pectoral_%+d" % side] = plate(outline, across)

    mesh = bpy.data.meshes.new("hide")
    mesh.from_pydata([v[:] for v in verts], [], faces)
    mesh.update()

    # Outward and flat: recalculated per closed shell, and every face left
    # faceted — the exporter writes what is here, and what is here has to be
    # the flat-toned look.
    bm = bmesh.new()
    bm.from_mesh(mesh)
    bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
    bm.to_mesh(mesh)
    bm.free()
    for polygon in mesh.polygons:
        polygon.use_smooth = False

    return bpy.data.objects.new("hide", mesh), parts


def build_rig():
    arm = bpy.data.armatures.new("shark")
    rig = bpy.data.objects.new("shark", arm)
    bpy.context.collection.objects.link(rig)
    bpy.context.view_layer.objects.active = rig
    bpy.ops.object.mode_set(mode="EDIT")

    body = arm.edit_bones.new("body")
    body.head, body.tail = Vector((0, HIPS, 0)), Vector((0, NOSE, 0))
    tail = arm.edit_bones.new("tail")
    tail.head, tail.tail = Vector((0, HIPS, 0)), Vector((0, PEDUNCLE, 0))
    tail.parent = body
    caudal = arm.edit_bones.new("caudal")
    caudal.head, caudal.tail = Vector((0, PEDUNCLE, 0)), Vector((0, TAIL_TIP, 0))
    caudal.parent = tail
    caudal.use_connect = True

    bpy.ops.object.mode_set(mode="OBJECT")
    return rig


def skin(hide, parts, rig):
    """Every vertex to exactly one bone at full weight — the rigid-skin rule
    the game's tests hold every rigged model to. The split follows the
    skeleton: full rings ahead of the hips ride the body, the tapering rear
    rides the tail, the caudal plate rides its own bone; all three fins that
    do not propel ride the body."""
    groups = {name: hide.vertex_groups.new(name=name) for name in ("body", "tail", "caudal")}
    for y, indices in parts["rings"]:
        bone = "body" if y > HIPS else "tail"
        groups[bone].add(list(indices), 1.0, "REPLACE")
    groups["caudal"].add(list(parts["caudal"]), 1.0, "REPLACE")
    groups["body"].add(list(parts["dorsal"]), 1.0, "REPLACE")
    groups["body"].add(list(parts["pectoral_+1"]), 1.0, "REPLACE")
    groups["body"].add(list(parts["pectoral_-1"]), 1.0, "REPLACE")

    hide.parent = rig
    hide.modifiers.new("Armature", "ARMATURE").object = rig


def choreograph(rig):
    """The one action, `swim`: a lateral wave keyed at the quarters of its
    cycle, first frame matching last so it loops. Sampled at the quarters
    because that is all extremes and crossings — nothing redundant for the
    exporter to drop, and nothing for an editor to misread."""
    scene = bpy.context.scene
    scene.render.fps = 24
    scene.frame_start = 1
    scene.frame_end = CYCLE_FRAMES

    bpy.context.view_layer.objects.active = rig
    bpy.ops.object.mode_set(mode="POSE")
    for name in SWISH:
        rig.pose.bones[name].rotation_mode = "XYZ"

    quarter = CYCLE_FRAMES // 4
    for frame in range(1, CYCLE_FRAMES + 2, quarter):
        theta = (frame - 1) / CYCLE_FRAMES * math.tau
        for name, (amplitude, lag) in SWISH.items():
            bone = rig.pose.bones[name]
            bone.rotation_euler.z = amplitude * math.sin(theta - lag)
            bone.keyframe_insert(data_path="rotation_euler", frame=frame)
    bpy.ops.object.mode_set(mode="OBJECT")

    rig.animation_data.action.name = "swim"


def prove_the_swish(rig):
    """A cheap self-check before saving: a quarter into the cycle the tail
    tip must stand well off the centre line, or the keys landed on an axis
    that does not swing sideways."""
    scene = bpy.context.scene
    scene.frame_set(1 + CYCLE_FRAMES // 4)
    bpy.context.view_layer.update()
    tip = rig.matrix_world @ rig.pose.bones["caudal"].tail
    assert abs(tip.x) > 0.05, f"the tail swings to x={tip.x:.3f}, which is no swish"
    print(f"swish proven: tail tip at x={tip.x:+.3f} a quarter into the cycle")
    scene.frame_set(1)


def main():
    bpy.ops.wm.read_factory_settings(use_empty=True)

    rig = build_rig()
    hide, parts = build_hide()
    bpy.context.collection.objects.link(hide)
    skin(hide, parts, rig)
    choreograph(rig)
    prove_the_swish(rig)

    import os

    here = os.path.dirname(os.path.abspath(__file__))
    bpy.ops.wm.save_as_mainfile(filepath=os.path.join(here, "shark.blend"))


main()
