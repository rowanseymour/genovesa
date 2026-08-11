#!/usr/bin/env bash
# Builds assets/models/<model>.glb from the master under assets-src/models/<model>/.
#
#   assets-src/models/export.sh boat
#   assets-src/models/export.sh palm
#
# One script for every model rather than one per directory: the master differs,
# the export does not, and the settings below are the part worth writing down.
# A copy per model would be the same forty lines drifting apart.
#
# Each model still gets a directory of its own, holding the master and whatever
# else belongs to it. The directory carries the model's name and not the .glb —
# an asset is a name here, and which extension it ships under is this script's
# business rather than part of what the thing is called. Nothing under
# assets-src/ ships — the game reads assets/, and the masters are kept so the
# shapes can be taken further.
#
# Open the master in Blender, change it, run this, and a game already running
# with `--features dev` picks the new shape up without restarting. That is the
# whole loop, and it is why the masters are .blend files rather than geometry
# in code: a shape is a thing to be looked at while it is moved.
#
# What is worth a script here is the export settings, every one of which the
# game's look or its collision depends on:
#
#   Flat shading. The exporter writes one normal per vertex, and vertices are
#   only split where the mesh says the face is flat — so a face left smooth
#   comes through as a facet with a gradient across it, which is the one thing
#   a look built out of flat tones cannot have. The masters have every face
#   flat already; this exports what is there rather than re-doing it.
#
#   +Y up. glTF's own convention and Bevy's, so the metres in the .blend are
#   the metres the keel is probed at. Blender is Z-up and the exporter rotates
#   on the way out; turn this off and the models arrive on their side.
#
#   Applied modifiers. What is exported has to be what is drawn, or the hull
#   the player runs aground is not the hull the file was checked against.
#
#   Names kept. The game asks for meshes by their position in the file, and
#   tests check that order against the names. Dropping them would leave
#   nothing to check against.
#
# No cameras, no lights, no animation: the game lights its own world, and every
# object is one static shape. Keeping them out means a stray light left in a
# master cannot follow a model into the game.
set -euo pipefail

me=$(basename "$0")
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)

if (( $# != 1 )); then
    echo "usage: $me <model>" >&2
    echo "       models: $(cd "$here" && ls -d ./*/ 2>/dev/null | sed 's|^\./||; s|/$||' | tr '\n' ' ')" >&2
    exit 2
fi
model=$1
master=$here/$model/$model.blend

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
    filepath='$tmp/$model.glb',
    export_format='GLB',
    export_yup=True,
    export_apply=True,
    export_normals=True,
    export_cameras=False,
    export_lights=False,
    export_animations=False,
    export_extras=False,
)
" >/dev/null

# Only once it is whole, so a killed run cannot leave the committed file torn.
mv "$tmp/$model.glb" "$root/assets/models/$model.glb"

echo "$me: assets/models/$model.glb ($(du -h "$root/assets/models/$model.glb" | cut -f1))"
