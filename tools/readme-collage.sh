#!/usr/bin/env bash
# Redraws docs/maps.png, the collage at the top of the README.
#
# Takes the seed the sixteen maps are spread from, so trying a few and keeping
# the one you like is just running this again with a different number. With no
# seed it draws mapgen's own default set, which is the collage that is
# committed — so a bare run should leave docs/maps.png untouched.
#
#   tools/readme-collage.sh        # the collage that is there now
#   tools/readme-collage.sh 7      # a different sixteen
#
# The quantise is what makes it worth a script: the flat palette drops to 256
# colours losslessly to the eye and about a third of the size, which is worth
# doing to a file that ships in the README.
set -euo pipefail

me=$(basename "$0")

if (( $# > 1 )); then
    echo "usage: $me [seed]" >&2
    exit 2
fi

# No seed given means no --seed passed: mapgen's default stays the one place
# the committed collage's seed is written down.
seed=()
label="default seed"
if (( $# == 1 )); then
    if [[ ! $1 =~ ^[0-9]+$ ]]; then
        echo "$me: \`$1\` is not a seed" >&2
        exit 2
    fi
    seed=(--seed "$1")
    label="seed $1"
fi

# Checked before the build, which is the slow part.
if ! command -v ffmpeg >/dev/null 2>&1; then
    echo "$me: ffmpeg is not installed" >&2
    exit 1
fi

root=$(cd "$(dirname "$0")/.." && pwd)
tmp=$(mktemp -d)
raw=$tmp/collage.png
quantised=$tmp/maps.png

trap 'rm -rf "$tmp"' EXIT

# bash 3.2 treats an empty array as unset under `set -u`, hence the guard.
cargo run --release --quiet --manifest-path "$root/Cargo.toml" \
    -p world --bin mapgen -- \
    collage ${seed[@]+"${seed[@]}"} --out "$raw" >/dev/null

ffmpeg -y -loglevel error -i "$raw" -filter_complex \
    "[0:v]palettegen=max_colors=256:stats_mode=full[p];[0:v][p]paletteuse=dither=floyd_steinberg" \
    "$quantised"

# Only once it is whole, so a killed run cannot leave the committed file torn.
mv "$quantised" "$root/docs/maps.png"

echo "$label: docs/maps.png ($(du -h "$raw" | cut -f1) -> $(du -h "$root/docs/maps.png" | cut -f1))"
