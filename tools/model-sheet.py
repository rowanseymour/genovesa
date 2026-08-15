"""Draws every model in the game onto one page, to be looked at.

    tools/model-sheet.sh              # screenshots/models.html
    tools/model-sheet.sh /tmp/m.html  # somewhere else

Not documentation. Nothing has to keep it up to date and nothing reads it but
a person wondering how the models are looking — run it when that is the
question, throw the answer away afterwards. It goes to `screenshots/`, which
git ignores, and it is one self-contained file so it can be sent to somebody.

Three views of each: from the tilt the game's own camera holds, from the side,
and from below, which is where a countershaded belly is. Beside them, what the
shipped `.glb` actually contains — triangles, size in metres, and the colours
the model carries, since a swatch says more than a triple does.
"""

import base64
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
OUT = argv[0] if argv else os.path.join(HERE, 'screenshots', 'models.html')

#: The game's own downward tilt — `camera::PITCH`. Duplicated rather than read,
#: because this is a picture rather than a promise: if it drifts, the page is a
#: few degrees off and nobody is misled.
PITCH = math.radians(51.75)
SHOTS = (('from the camera', PITCH), ('from the side', math.radians(8)),
         ('from below', math.radians(-40)))


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
    scene.render.film_transparent = True
    scene.view_settings.view_transform = 'Standard'
    scene.render.image_settings.file_format = 'PNG'
    scene.render.image_settings.color_mode = 'RGBA'
    scene.render.resolution_x = scene.render.resolution_y = 560

    world = bpy.data.worlds.new('sky')
    world.node_tree.nodes['Background'].inputs[1].default_value = 0.45
    scene.world = world

    key = bpy.data.objects.new('key', bpy.data.lights.new('key', 'SUN'))
    key.data.energy = 3.0
    key.data.angle = math.radians(3)
    key.rotation_euler = Euler((math.radians(50), 0, math.radians(40)))
    scene.collection.objects.link(key)
    # A second, weaker sun from underneath, or every from-below shot is a
    # silhouette and the belly the shot is *for* is a black shape.
    fill = bpy.data.objects.new('fill', bpy.data.lights.new('fill', 'SUN'))
    fill.data.energy = 1.4
    fill.rotation_euler = Euler((math.radians(-125), 0, math.radians(-30)))
    scene.collection.objects.link(fill)
    return scene


def render(scene, name, elevation):
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

    path = os.path.join(bpy.app.tempdir, f'{name}.png')
    scene.render.filepath = path
    bpy.ops.render.render(write_still=True)
    bpy.data.objects.remove(cam, do_unlink=True)
    with open(path, 'rb') as f:
        return base64.b64encode(f.read()).decode()


PAGE = """<!doctype html>
<meta charset="utf-8"><title>models</title>
<style>
 body {{ background: #1b1f23; color: #d8d4cc; font: 15px/1.5 system-ui, sans-serif;
        margin: 0 auto; padding: 3rem 2rem; max-width: 74rem; }}
 h1 {{ font-weight: 600; letter-spacing: .02em; margin: 0 0 .3rem; }}
 .note {{ color: #8b8b86; margin-bottom: 3rem; }}
 section {{ display: grid; grid-template-columns: 1fr 1fr 1fr 15rem; gap: 1rem;
           align-items: center; border-top: 1px solid #33383d; padding: 1.6rem 0; }}
 figure {{ margin: 0; text-align: center; }}
 figure img {{ width: 100%; background:
   repeating-conic-gradient(#23272c 0 25%, #1e2226 0 50%) 0 0/22px 22px; border-radius: 6px; }}
 figcaption {{ color: #71736f; font-size: 12px; padding-top: .3rem; }}
 h2 {{ margin: 0 0 .5rem; font-size: 1.15rem; }}
 dl {{ display: grid; grid-template-columns: auto 1fr; gap: .1rem .7rem; margin: 0; font-size: 13px; }}
 dt {{ color: #71736f; }} dd {{ margin: 0; }}
 .swatches {{ display: flex; flex-wrap: wrap; gap: .3rem; margin-top: .7rem; }}
 .swatch {{ width: 1.9rem; height: 1.9rem; border-radius: 4px; border: 1px solid #0006; }}
 .none {{ color: #a4746b; font-size: 13px; margin-top: .7rem; }}
</style>
<h1>Models</h1>
<p class="note">Every master under <code>assets-src/models/</code>, drawn from the
game's own camera tilt, from the side, and from below. Generated by
<code>tools/model-sheet.sh</code> — a look at how things are, not a document
anything has to keep true.</p>
{sections}
"""

SECTION = """<section>
  {figures}
  <div>
    <h2>{name}</h2>
    <dl>
      <dt>size</dt><dd>{size} m</dd>
      <dt>triangles</dt><dd>{tris}</dd>
      <dt>vertices</dt><dd>{verts}</dd>
      <dt>meshes</dt><dd>{meshes}</dd>
      {clips}
      <dt>file</dt><dd>{kb} kB</dd>
    </dl>
    {palette}
  </div>
</section>
"""

names = sorted(d for d in os.listdir(MASTERS)
               if os.path.isdir(os.path.join(MASTERS, d)))
scene = stage()
sections = []
for name in names:
    facts = shipped(name)
    figures = ''.join(
        f'<figure><img src="data:image/png;base64,{render(scene, name, tilt)}">'
        f'<figcaption>{label}</figcaption></figure>'
        for label, tilt in SHOTS)
    if facts['colours']:
        swatches = ''.join(
            '<div class="swatch" style="background: rgb({}, {}, {})" '
            'title="{} facets"></div>'.format(
                *[round(c * 255) for c in rgb], count)
            for rgb, count in facts['colours'])
        palette = f'<div class="swatches">{swatches}</div>'
    else:
        palette = '<p class="none">no colours of its own — the client paints it</p>'
    sections.append(SECTION.format(
        figures=figures,
        name=name,
        size=' × '.join(f'{s:g}' for s in facts['size']),
        tris=f"{facts['tris']:,}",
        verts=f"{facts['verts']:,}",
        meshes=', '.join(facts['meshes']),
        clips=f"<dt>clips</dt><dd>{', '.join(facts['clips'])}</dd>" if facts['clips'] else '',
        kb=facts['bytes'] // 1024,
        palette=palette,
    ))
    print(f'{name}: {facts["tris"]} tris, {len(facts["colours"])} colours')

os.makedirs(os.path.dirname(OUT), exist_ok=True)
with open(OUT, 'w') as f:
    f.write(PAGE.format(sections='\n'.join(sections)))
print(f'wrote {OUT} ({os.path.getsize(OUT) // 1024} kB)')
