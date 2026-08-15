#!/usr/bin/env bash
# Redraws docs/models.md, the page every model is drawn on — see
# model-catalog.py, which this finds Blender for and then quantises after.
#
#   tools/model-catalog.sh            # docs/models.md
#   tools/model-catalog.sh /tmp/m.md  # somewhere else
#
# The quantise is the same trick readme-collage.sh uses and is worth more here:
# the renders are flat tones, which 256 colours hold exactly, and these are
# files rewritten every time a model moves. Without it they are megabytes of
# history per regeneration.
#
# `dither=none` where the collage dithers, and that is the whole difference
# between 3.6 MB and a quarter of it: dithering scatters pixels that a flat
# tone would have shared, which is worth it on a photograph of terrain and
# ruinous on nine facets of one grey.
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

out=${1:-$(cd "$here/.." && pwd)/docs/models.md}

"$blender" --background --factory-startup --python "$here/model-catalog.py" -- "$out" \
    | grep -vE '^(Blender|Read blend|Fra:|Saved:|$)'

shots=${out%.*}
before=$(du -ch "$shots"/*.png | tail -1 | cut -f1)
for raw in "$shots"/*.png; do
    ffmpeg -y -loglevel error -i "$raw" -filter_complex \
        "[0:v]palettegen=max_colors=256[p];[0:v][p]paletteuse=dither=none" \
        "$raw.tmp.png"
    mv "$raw.tmp.png" "$raw"
done

echo "$me: $(basename "$out") and $(ls "$shots"/*.png | wc -l | tr -d ' ') renders" \
     "($before -> $(du -ch "$shots"/*.png | tail -1 | cut -f1))"
