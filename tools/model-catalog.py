"""Draws every model in the game onto one page, to be looked at.

    tools/model-catalog.sh            # docs/models.md
    tools/model-catalog.sh /tmp/m.md  # somewhere else

Three views of each: from the tilt the game's own camera holds, from the side,
and from below, which is where a countershaded belly is. Beside them, what the
shipped `.glb` actually contains — triangles, size in metres, and the colours
the model carries, since a swatch says more than a triple does.

The page is committed, as `docs/maps.png` is, and re-run when the models
change. It is nobody's contract: a stale one shows an old model rather than
telling a lie about a current one, which is the whole reason a picture is
worth keeping where a paragraph is not.

The renders go beside it as files, so that regenerating after changing one
model writes one small blob into the history rather than the whole page again.
`model-catalog.sh` quantises them afterwards, the way the README's collage is.

Markdown rather than a page of my own, because the only place anybody reads
this is GitHub, which renders `.md` and shows `.html` as source. That rules out
CSS, so the layout is a table, and the colours are a strip of PNG — there is no
way to draw a swatch in Markdown. It also rules out transparency: the renders
carry their own background, or a near-black shark is invisible on a dark theme
and the white seabird on a light one.
"""

import json
import math
import os
import struct
import sys

import bpy
from mathutils import Euler, Vector

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MASTERS = os.path.join(HERE, 'assets-src', 'models')
SHIPPED = os.path.join(HERE, 'assets', 'models')

argv = sys.argv[sys.argv.index('--') + 1:]
OUT = argv[0] if argv else os.path.join(HERE, 'docs', 'models.md')
SHOTS_DIR = os.path.splitext(OUT)[0]

#: The game's own downward tilt — `camera::PITCH`. Duplicated rather than read,
#: because this is a picture rather than a promise: if it drifts, the page is a
#: few degrees off and nobody is misled.
PITCH = math.radians(51.75)
#: Steeply from below rather than a little: at a shallow angle an animal shows
#: its flank, which is painted as its back, and the belly the shot is for never
#: appears.
SHOTS = (('from the camera', PITCH), ('from the side', math.radians(8)),
         ('from below', math.radians(-72)))


def linear_to_srgb(c):
    return c * 12.92 if c <= 0.0031308 else 1.055 * (c ** (1 / 2.4)) - 0.055


def shipped(name):
    """What the exported file holds: triangles, extent, and its palette."""
    path = os.path.join(SHIPPED, f'{name}.glb')
    with open(path, 'rb') as f:
        data = f.read()
    i, chunks = 12, []
    while i < len(data):
        length, _kind = struct.unpack_from('<II', data, i)
        chunks.append(data[i + 8:i + 8 + length])
        i += 8 + length
    js, blob = json.loads(chunks[0]), chunks[1]

    tris, verts, colours, low, high = 0, 0, {}, [1e9] * 3, [-1e9] * 3
    for mesh in js['meshes']:
        for prim in mesh['primitives']:
            tris += js['accessors'][prim['indices']]['count'] // 3
            pos = js['accessors'][prim['attributes']['POSITION']]
            verts += pos['count']
            low = [min(a, b) for a, b in zip(low, pos['min'])]
            high = [max(a, b) for a, b in zip(high, pos['max'])]

            paint = prim['attributes'].get('COLOR_0')
            if paint is None:
                continue
            acc = js['accessors'][paint]
            wide = {'VEC3': 3, 'VEC4': 4}[acc['type']]
            at = js['bufferViews'][acc['bufferView']].get('byteOffset', 0) \
                + acc.get('byteOffset', 0)
            for n in range(acc['count']):
                rgb = tuple(round(linear_to_srgb(
                    struct.unpack_from('<f', blob, at + n * 4 * wide + 4 * c)[0]), 3)
                    for c in range(3))
                colours[rgb] = colours.get(rgb, 0) + 1

    return {
        'meshes': [m['name'] for m in js['meshes']],
        'clips': [a.get('name', '?') for a in js.get('animations', [])],
        'tris': tris,
        'verts': verts,
        'size': [round(h - l, 2) for l, h in zip(low, high)],
        'colours': sorted(colours.items(), key=lambda kv: -kv[1]),
        'bytes': len(data),
    }


def stage():
    bpy.ops.wm.read_factory_settings(use_empty=True)
    scene = bpy.context.scene
    scene.render.engine = 'BLENDER_EEVEE'
    scene.render.film_transparent = False
    scene.view_settings.view_transform = 'Standard'
    scene.render.image_settings.file_format = 'PNG'
    scene.render.image_settings.color_mode = 'RGBA'
    # Twice what the page shows them at, which is what a dense screen wants.
    scene.render.resolution_x = scene.render.resolution_y = 460
    # Blender dithers its output by default, which on flat tones is noise in
    # every pixel of a background that should be one colour — and noise is the
    # one thing PNG cannot pack. It is worth about two thirds of the file.
    scene.render.dither_intensity = 0.0

    # Lit like a specimen rather than like the game. The sun goes with the
    # camera — see `render` — because a fixed one leaves every underside in
    # shadow, and an underside is the whole reason the third column is there.
    # The ambient carries the rest, so a facet turned away is still its own
    # colour rather than black.
    # A mid slate to sit each model on. Both themes GitHub renders in are at
    # one end or the other, so anything nearer white or black loses a model
    # painted the same way — the shark on the dark one, the seabird on the
    # light. This is also what the renders are lit by.
    world = bpy.data.worlds.new('sky')
    world.node_tree.nodes['Background'].inputs[0].default_value = (0.20, 0.23, 0.26, 1)
    world.node_tree.nodes['Background'].inputs[1].default_value = 0.85
    scene.world = world

    sun = bpy.data.objects.new('sun', bpy.data.lights.new('sun', 'SUN'))
    sun.data.energy = 2.2
    sun.data.angle = math.radians(4)
    scene.collection.objects.link(sun)
    return scene


def render(scene, name, label, elevation):
    """One view of the master, framed to whatever it happens to measure."""
    for old in [o for o in bpy.data.objects if o.type == 'MESH']:
        bpy.data.objects.remove(old, do_unlink=True)

    with bpy.data.libraries.load(os.path.join(MASTERS, name, f'{name}.blend')) as (src, dst):
        dst.objects = list(src.objects)
    drawn = []
    for ob in dst.objects:
        if ob is None or ob.type != 'MESH':
            continue
        copy = ob.copy()
        copy.data = ob.data.copy()
        scene.collection.objects.link(copy)
        drawn.append(copy)

    points = [copy.matrix_world @ Vector(c) for copy in drawn for c in copy.bound_box]
    low = Vector((min(p.x for p in points), min(p.y for p in points), min(p.z for p in points)))
    high = Vector((max(p.x for p in points), max(p.y for p in points), max(p.z for p in points)))
    middle = (low + high) * 0.5
    span = max(high - low)

    cam = bpy.data.objects.new('cam', bpy.data.cameras.new('cam'))
    cam.data.lens = 60
    distance = span * 2.6
    bearing = math.radians(-65)
    eye = middle + Vector((
        math.cos(bearing) * math.cos(elevation) * distance,
        math.sin(bearing) * math.cos(elevation) * distance,
        math.sin(elevation) * distance,
    ))
    cam.location = eye
    cam.rotation_euler = (middle - eye).normalized().to_track_quat('-Z', 'Y').to_euler()
    scene.collection.objects.link(cam)
    scene.camera = cam

    # The sun looks from where the camera does, turned a little off its
    # shoulder so that facets still shade differently from one another. Dead
    # along the view they would flatten into one tone.
    sun = bpy.data.objects['sun']
    over = Euler((0, 0, math.radians(28))).to_matrix() @ (middle - eye).normalized()
    sun.rotation_euler = over.to_track_quat('-Z', 'Y').to_euler()

    path = os.path.join(SHOTS_DIR, f'{name}-{label}.png')
    scene.render.filepath = path
    bpy.ops.render.render(write_still=True)
    bpy.data.objects.remove(cam, do_unlink=True)
    return os.path.join(os.path.basename(SHOTS_DIR), f'{name}-{label}.png')


def palette_strip(path, colours, block=44, height=24):
    """A row of colour blocks, as a PNG.

    Hand-rolled because Markdown has no way to draw a swatch and this is the
    one thing on the page that a reader wants to *see* rather than read — a
    triple tells you nothing about whether two greens are alike.
    """
    import zlib

    width = block * len(colours)
    rows = bytearray()
    for _ in range(height):
        rows.append(0)                                  # no filter on this row
        for rgb in colours:
            rows.extend(bytes(round(c * 255) for c in rgb) * block)

    def chunk(kind, body):
        head = kind + body
        return (struct.pack('>I', len(body)) + head
                + struct.pack('>I', zlib.crc32(head) & 0xFFFFFFFF))

    with open(path, 'wb') as f:
        f.write(b'\x89PNG\r\n\x1a\n')
        f.write(chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 2, 0, 0, 0)))
        f.write(chunk(b'IDAT', zlib.compress(bytes(rows), 9)))
        f.write(chunk(b'IEND', b''))


PAGE = """# Models

Every master under `assets-src/models/`, drawn from the game's own camera tilt,
from the side, and from below, beside what its shipped `.glb` holds. Generated
by `tools/model-catalog.sh` — re-run it when the models change.

| | from the camera | from the side | from below |
| --- | --- | --- | --- |
{rows}
"""

ROW = ("| **{name}**<br>{size} m<br>{tris} triangles<br>{verts} vertices"
       "<br>{meshes}{clips}<br>{kb} kB{palette} "
       "| {shots} |\n")

names = sorted(d for d in os.listdir(MASTERS)
               if os.path.isdir(os.path.join(MASTERS, d)))
os.makedirs(SHOTS_DIR, exist_ok=True)
scene = stage()
rows = []
for name in names:
    facts = shipped(name)
    shots = ' | '.join(
        f'<img src="{render(scene, name, label.split()[-1], tilt)}" '
        f'alt="{name}, {label}" width="230">'
        for label, tilt in SHOTS)

    if facts['colours']:
        strip = os.path.join(SHOTS_DIR, f'{name}-palette.png')
        palette_strip(strip, [rgb for rgb, _ in facts['colours']])
        where = os.path.join(os.path.basename(SHOTS_DIR), f'{name}-palette.png')
        palette = (f'<br><img src="{where}" alt="its colours" '
                   f'height="18">')
    else:
        palette = '<br>*painted by the client*'

    rows.append(ROW.format(
        name=name,
        size=' × '.join(f'{s:g}' for s in facts['size']),
        tris=f"{facts['tris']:,}",
        verts=f"{facts['verts']:,}",
        meshes=', '.join(f'`{m}`' for m in facts['meshes']),
        clips='<br>' + ', '.join(f'`{c}`' for c in facts['clips']) if facts['clips'] else '',
        kb=facts['bytes'] // 1024,
        palette=palette,
        shots=shots,
    ))
    print(f'{name}: {facts["tris"]} tris, {len(facts["colours"])} colours')

os.makedirs(os.path.dirname(OUT), exist_ok=True)
with open(OUT, 'w') as f:
    f.write(PAGE.format(rows=''.join(rows)))
print(f'wrote {OUT} ({os.path.getsize(OUT) // 1024} kB)')
