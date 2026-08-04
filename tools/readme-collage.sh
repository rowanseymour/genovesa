#!/usr/bin/env bash
# Redraws docs/maps.png, the collage at the top of the README.
#
# Takes the seed the sixteen maps are spread from, so trying a few and keeping
# the one you like is just running this again with a different number.
#
#   tools/readme-collage.sh        # the collage that is there now
#   tools/readme-collage.sh 7      # a different sixteen
#
# The quantise is what makes it worth a script: the flat palette drops to 256
# colours losslessly to the eye and about a third of the size, which is worth
# doing to a file that ships in the README.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
seed=${1:-1}
tmp=$(mktemp -d)
raw=$tmp/collage.png

trap 'rm -rf "$tmp"' EXIT

cargo run --release --quiet --manifest-path "$root/Cargo.toml" --bin mapgen -- \
    collage --seed "$seed" --out "$raw"

ffmpeg -y -loglevel error -i "$raw" -filter_complex \
    "[0:v]palettegen=max_colors=256:stats_mode=full[p];[0:v][p]paletteuse=dither=floyd_steinberg" \
    "$root/docs/maps.png"

echo "seed $seed: docs/maps.png ($(du -h "$raw" | cut -f1) -> $(du -h "$root/docs/maps.png" | cut -f1))"
