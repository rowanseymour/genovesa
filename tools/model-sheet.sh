#!/usr/bin/env bash
# Draws every model onto one page, to be looked at — see model-sheet.py, which
# this only finds Blender for.
#
#   tools/model-sheet.sh              # screenshots/models.html
#   tools/model-sheet.sh /tmp/m.html  # somewhere else
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

"$blender" --background --factory-startup --python "$here/model-sheet.py" -- "$@" \
    | grep -vE '^(Blender|Read blend|Fra:|Saved:|$)'
