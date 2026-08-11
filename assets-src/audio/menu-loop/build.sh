#!/usr/bin/env bash
# Builds assets/audio/menu-loop.ogg, the sea heard behind the menu.
#
# This directory carries the asset's name and holds everything that goes into
# it: the recording, and this. The name is the asset's and not the file's — the
# .ogg is this script's business, Vorbis being the one compressed format Bevy
# decodes untold. Nothing here ships — the game reads assets/, and the master is
# kept only so the loop can be cut again differently.
#
#   assets-src/audio/menu-loop/build.sh          # from the master beside it
#   assets-src/audio/menu-loop/build.sh other    # from a recording of your own
#
# The master is committed as FLAC rather than as the WAV Freesound serves, which
# is lossless and on water this quiet about a fifth of the size. It is worth
# carrying at all because Freesound hands the master out only to an account, so
# a fresh clone could not fetch it back.
#
# What is worth a script here is the numbers. The master runs 75 seconds and
# quietens markedly over its last third, so looping the whole of it gives a sea
# that calms and then abruptly picks up again; measured in five second blocks it
# spans 12dB end to end. WINDOW is the stretch that does not do that. The
# crossfade is what closes the loop: the seconds after the window are laid over
# the seconds at its start, each fading past the other, so the end of the result
# runs into its own beginning. Without it the two ends meet at a step.
set -euo pipefail

me=$(basename "$0")
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)

master=$here/852108__myloop__sailboat-bow.flac
if (( $# > 1 )); then
    echo "usage: $me [master]" >&2
    exit 2
elif (( $# == 1 )); then
    master=$1
fi

# Where the loop is cut from, in seconds: the water is at a steady state from
# START, and stays that way well past the end of the window.
START=25
# How long the loop runs for. Chosen against the sea it plays under rather than
# to fill the window: long enough not to be recognisable, and no longer.
WINDOW=23
# How much of the recording either end of the join is spent fading. Long enough
# to hide the join in water that is never twice the same, short enough that the
# fade is not itself audible as a dip.
FADE=2

for tool in ffmpeg sox; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "$me: $tool is not installed" >&2
        exit 1
    fi
done

if [[ ! -f $master ]]; then
    echo "$me: no master at $master — see assets/CREDITS.md" >&2
    exit 1
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# The three pieces: the seconds that follow the window, the seconds it opens
# with, and everything in between.
ffmpeg -v error -y -i "$master" -ss $((START + WINDOW)) -t "$FADE" \
    -ac 2 -ar 48000 "$tmp/tail.wav"
ffmpeg -v error -y -i "$master" -ss "$START" -t "$FADE" \
    -ac 2 -ar 48000 "$tmp/head.wav"
ffmpeg -v error -y -i "$master" -ss $((START + FADE)) -t $((WINDOW - FADE)) \
    -ac 2 -ar 48000 "$tmp/mid.wav"

# Triangular on both sides so the two halves sum flat through the join; the
# equal-power curves leave a bump in the middle of a fade between two takes of
# the same water.
ffmpeg -v error -y -i "$tmp/tail.wav" -i "$tmp/head.wav" \
    -filter_complex "[0][1]acrossfade=d=${FADE}:c1=tri:c2=tri" "$tmp/join.wav"
ffmpeg -v error -y -i "$tmp/join.wav" -i "$tmp/mid.wav" \
    -filter_complex "[0][1]concat=n=2:v=0:a=1" "$tmp/loop.wav"

# sox rather than ffmpeg for the last step: Homebrew's ffmpeg is built without
# libvorbis, and Vorbis is the one compressed format Bevy decodes untold.
sox "$tmp/loop.wav" -C 6 "$tmp/menu-loop.ogg"

# Only once it is whole, so a killed run cannot leave the committed file torn.
mv "$tmp/menu-loop.ogg" "$root/assets/audio/menu-loop.ogg"

echo "$me: assets/audio/menu-loop.ogg (${WINDOW}s from ${START}s, ${FADE}s crossfade, $(du -h "$root/assets/audio/menu-loop.ogg" | cut -f1))"
