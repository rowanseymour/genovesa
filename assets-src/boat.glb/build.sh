#!/usr/bin/env bash
# Builds assets/boat.glb, the hull and spar the player gets about in.
#
# This directory is named for the file it produces, and holds everything that
# goes into it: the Blender master, and this. Nothing here ships — the game
# reads assets/, and the master is kept so the boat can be modelled further.
#
#   assets-src/boat.glb/build.sh
#
# Open boat.blend in Blender, change the hull, run this, and a game already
# running with `--features dev` picks the new shape up without restarting.
# That is the whole loop, and it is why the master is a .blend rather than
# geometry in code: a hull is a shape to be looked at while it is moved.
#
# What is worth a script here is the export settings, every one of which the
# game's look or its collision depends on:
#
#   Flat shading. The exporter writes one normal per vertex, and vertices are
#   only split where the mesh says the face is flat — so a face left smooth
#   comes through as a facet with a gradient across it, which is the one thing
#   a look built out of flat tones cannot have. The .blend has every face flat
#   already; --python-expr below does not re-do it, it exports what is there.
#
#   +Y up. glTF's own convention and Bevy's, so the metres in the .blend are
#   the metres the keel is probed at. Blender is Z-up and the exporter rotates
#   on the way out; turn this off and the boat arrives on its side.
#
#   Applied modifiers. What is exported has to be what is drawn, or the hull
#   the player runs aground is not the hull the file was checked against.
#
# No cameras, no lights, no animation: the game lights its own world, and the
# boat is one static shape per object. Keeping them out means a stray light
# left in the .blend cannot follow the boat into the game.
set -euo pipefail

me=$(basename "$0")
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)

master=$here/boat.blend

blender=blender
if ! command -v "$blender" >/dev/null 2>&1; then
    # macOS installs the .app rather than anything on PATH.
    blender=/Applications/Blender.app/Contents/MacOS/Blender
fi
if [[ ! -x $blender ]]; then
    echo "$me: blender is not installed" >&2
    exit 1
fi

if [[ ! -f $master ]]; then
    echo "$me: no master at $master" >&2
    exit 1
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

"$blender" --background "$master" --python-expr "
import bpy
bpy.ops.export_scene.gltf(
    filepath='$tmp/boat.glb',
    export_format='GLB',
    export_yup=True,
    export_apply=True,
    export_normals=True,
    export_cameras=False,
    export_lights=False,
    export_animations=False,
    # Names are the contract: the game asks for the meshes in the order they
    # are written, and a test in boat.rs checks that order is still hull then
    # spar. Dropping the names would leave nothing for it to check.
    export_extras=False,
)
" >/dev/null

# Only once it is whole, so a killed run cannot leave the committed file torn.
mv "$tmp/boat.glb" "$root/assets/boat.glb"

echo "$me: assets/boat.glb ($(du -h "$root/assets/boat.glb" | cut -f1))"
