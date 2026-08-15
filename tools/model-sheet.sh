#!/usr/bin/env bash
# Redraws docs/models.html, the page every model is drawn on — see
# model-sheet.py, which this finds Blender for and then quantises after.
#
#   tools/model-sheet.sh              # docs/models.html
#   tools/model-sheet.sh /tmp/m.html  # somewhere else
#
# The quantise is the same trick readme-collage.sh uses and is worth more here:
# the renders are flat tones over transparency, which 256 colours hold exactly,
# and these are files that get rewritten every time a model moves. Without it
# the page and its pictures are megabytes of history per regeneration.
set -euo pipefail

me=$(basename "$0")
here=$(cd "$(dirname "$0")" && pwd)

if (( $# > 1 )); then
    echo "usage: $me [output.html]" >&2
    exit 2
fi

blender=blender
if ! command -v "$blender" >/dev/null 2>&1; then
    # macOS installs the .app rather than anything on PATH.
    blender=/Applications/Blender.app/Contents/MacOS/Blender
fi
if [[ ! -x $blender ]]; then
    echo "$me: blender is not installed" >&2
    exit 1
fi

out=${1:-$(cd "$here/.." && pwd)/docs/models.html}

"$blender" --background --factory-startup --python "$here/model-sheet.py" -- "$out" \
    | grep -vE '^(Blender|Read blend|Fra:|Saved:|$)'

shots=${out%.*}
before=$(du -ch "$shots"/*.png | tail -1 | cut -f1)
for raw in "$shots"/*.png; do
    # `reserve_transparent` keeps the alpha the renders are cut out with; a
    # palette without it fills the background with whatever colour is nearest.
    ffmpeg -y -loglevel error -i "$raw" -filter_complex \
        "[0:v]palettegen=max_colors=255:reserve_transparent=1[p];[0:v][p]paletteuse" \
        "$raw.tmp.png"
    mv "$raw.tmp.png" "$raw"
done

echo "$me: $(basename "$out") and $(ls "$shots"/*.png | wc -l | tr -d ' ') renders" \
     "($before -> $(du -ch "$shots"/*.png | tail -1 | cut -f1))"
