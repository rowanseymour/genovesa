#!/usr/bin/env python3
# Builds assets/icon.png, the application's icon.
#
#   assets-src/icon/build.py             # the shipped 1024px icon
#   assets-src/icon/build.py --size 64   # somewhere else to look at it
#
# The icon is *drawn* rather than screenshotted, and that is the whole reason
# this file exists. A render of the real thing — a frame of the game, or a
# `mapgen` plan of a seed somebody liked — carries a hundred facets of noise
# that survive to 512px and turn to mud at 32, which is the size that decides
# whether an icon works. What reads small is a handful of big shapes in the
# game's own colours, and those have to be laid out by hand.
#
# Nothing here ships and nothing in the game reads it; it emits a picture and
# stops. The numbers are all in this file, and NOTES.md beside it says why the
# picture is this picture.
#
# Rendering the SVG needs one of two things. `rsvg-convert` (librsvg) is the
# portable one and is what a Linux machine will have; `qlmanage` is on every
# Mac and draws through the same WebKit that previews the file in Finder. They
# agree on everything this drawing uses, which is flat fills and strokes.

import argparse
import math
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

# --- The palette ------------------------------------------------------------
#
# Every colour is a tone out of `protocol`'s ground palette, because the icon
# has to look like the thing it opens. Copied rather than read: this script
# does not build, and a tone that moved would be caught by the icon looking
# wrong long before anything else noticed.
SEA = "#1a6b9e"  # Tone::SeaWater
SEA_LIT = "#1d70a4"  # the sea, one facet brighter
SEA_DIM = "#176599"  # and one darker
SHELF = "#3e86a8"  # the water shelving up to a coast
SHALLOW = "#75ad9e"  # Tone::Shallow
SAND = "#dbc785"  # Tone::Sand
SAND_DARK = "#8a7b4f"  # sand with the sun off it
GRASS = "#70a842"  # Tone::Grass
INK = "#14313c"  # the waterline: darker than any tone there is

# The drawing is done on a 1024 grid whatever it is rendered at.
S = 1024
# Corner radius. Apple rounds an icon to about 22% of its side and everyone
# else rounds less, so this sits at the generous end and looks deliberate
# rather than clipped on a platform that would have rounded it anyway.
RADIUS = 224

# --- An island, in plan -----------------------------------------------------
#
# Not any particular seed's island — a plausible one. A lobed coast with a bay
# biting into the east, a spit off the south-west and a headland north: the
# shapes `mapgen grid` keeps turning up. Only two appear in the icon and both
# are small, so the coast is doing all the work and there is no point drawing
# what is inside it.
COAST = (
    "M 470 124 "
    "C 592 114 690 166 740 248 "
    "L 796 296 "
    "C 842 354 852 432 826 494 "
    "C 804 550 748 570 700 554 "
    "C 640 534 594 566 586 622 "
    "C 578 678 622 718 692 732 "
    "C 766 748 808 802 780 848 "
    "C 750 896 658 908 588 880 "
    "C 518 852 470 806 420 800 "
    "C 350 792 300 832 246 812 "
    "C 176 786 150 720 168 654 "
    "L 126 584 "
    "C 150 518 138 468 154 408 "
    "C 176 328 236 234 320 176 "
    "C 366 144 414 130 470 124 Z"
)
ICX, ICY = 490, 500  # about the middle of it, which is what it is scaled about


def about(s, cx=ICX, cy=ICY):
    """A scale about a point, since SVG only scales about the origin."""
    return f"translate({cx},{cy}) scale({s}) translate({-cx},{-cy})"


def island(scale, cx, cy, rim=0.86, ink=6):
    """One island: two steps of shelving water, a rim of sand, then grass.

    The rim is the coast path inset rather than a stroke, so the sand narrows
    into an inlet and widens on a blunt headland the way a real beach does.
    `rim` is how far in the grass starts, and both islands here are small
    enough that it has to be cut wider than a beach really is — a sand rim in
    true proportion is a hairline at this size, and the island comes out a
    green blob with no coast at all.
    """
    body = [
        f'  <path d="{COAST}" fill="{SHELF}" transform="{about(1.13)}"/>',
        f'  <path d="{COAST}" fill="{SHALLOW}" transform="{about(1.05)}"/>',
        # The ink is divided by the scale so a small island keeps a hairline
        # waterline instead of a black band: it is the one line in the drawing
        # that must not grow with what it is drawn around.
        f'  <path d="{COAST}" fill="{SAND}" stroke="{INK}"'
        f' stroke-width="{ink / scale:.1f}" stroke-linejoin="round"/>',
        f'  <path d="{COAST}" fill="{GRASS}" transform="{about(rim)}"/>',
    ]
    return (
        f'<g transform="translate({cx - ICX},{cy - ICY}) {about(scale)}">\n'
        + "\n".join(body)
        + "\n</g>"
    )


# --- The rose ---------------------------------------------------------------
#
# Four long points and four short, and every face its own flat tone — which is
# the ground's own rule (one colour per triangle, no gradient anywhere) applied
# to something that is not ground. The light comes from the north-west, as it
# does over the mountains in a `mapgen` plan, so the whole north-west half of
# the rose is sand and the south-east half is sand with the sun off it. That
# one split is what makes a flat shape read as a solid at 32 pixels.
LIGHT = -135  # degrees: where the sun is, measured as SVG measures angles


def rose(cx, cy, R, waist=0.46):
    """The rose, tip outwards, starting at north and going clockwise."""
    out = []
    # The intercardinal rays first, so the points are drawn over them. They are
    # a chart's furniture rather than part of the star, and they are the first
    # thing to disappear as the icon gets smaller, which is correct.
    for k in range(4):
        a = math.radians(45 + 90 * k - 90)
        out.append(
            f'<path d="M {cx} {cy} L {cx + R * 0.92 * math.cos(a):.1f}'
            f' {cy + R * 0.92 * math.sin(a):.1f}" stroke="{SAND}"'
            f' stroke-width="{R * 0.035:.1f}" stroke-linecap="round"/>'
        )
    for k in range(8):
        a0 = math.pi * k / 4 - math.pi / 2
        a1 = math.pi * (k + 1) / 4 - math.pi / 2
        # Long point, short point, long point: the waist is where the short
        # ones reach to, and is what decides whether the star is a compass or
        # a throwing knife.
        r0, r1 = (R, R * waist) if k % 2 == 0 else (R * waist, R)
        x0, y0 = cx + r0 * math.cos(a0), cy + r0 * math.sin(a0)
        x1, y1 = cx + r1 * math.cos(a1), cy + r1 * math.sin(a1)
        facing = math.atan2((y0 + y1) / 2 - cy, (x0 + x1) / 2 - cx)
        lit = math.cos(facing - math.radians(LIGHT)) > 0.2
        out.append(
            f'<path d="M {cx} {cy} L {x0:.1f} {y0:.1f} L {x1:.1f} {y1:.1f} Z"'
            f' fill="{SAND if lit else SAND_DARK}" stroke="{INK}"'
            f' stroke-width="{R * 0.03:.1f}" stroke-linejoin="round"/>'
        )
    return "\n".join(out)


def ring(cx, cy, R, width, dash=None):
    dash = f' stroke-dasharray="{dash}"' if dash else ""
    return (
        f'<circle cx="{cx}" cy="{cy}" r="{R}" fill="none" stroke="{SAND}"'
        f' stroke-width="{width}"{dash}/>'
    )


def draw(px):
    """The whole icon, as an SVG document rendered at `px` pixels square."""
    R = 392  # the rim, which sets the scale of everything inside it
    body = [
        # The sea, and two facets of it. They are a hair either side of the
        # base tone — enough that the ground behind the rose is not one dead
        # slab, not so much that it reads as a shadow under the star.
        f'<rect width="{S}" height="{S}" fill="{SEA}"/>',
        f'<path d="M 0 0 L {S} 0 L 0 {S} Z" fill="{SEA_LIT}"/>',
        f'<path d="M {S} {S} L {S} 300 L 300 {S} Z" fill="{SEA_DIM}"/>',
        ring(S // 2, S // 2, R, 13),
        ring(S // 2, S // 2, 348, 8, dash="5 44"),
        # A notch outside the rim at north. The rose is symmetric enough that
        # without it the icon has no way up, and it survives small as a bump
        # on the ring long after it has stopped being a triangle.
        f'<path d="M {S // 2} {S // 2 - R - 51} L {S // 2 + 34} {S // 2 - R}'
        f' L {S // 2 - 34} {S // 2 - R} Z" fill="{SAND}"/>',
        rose(S // 2, S // 2, 316),
        # Land last, so the rim passes behind it rather than slicing it. Two
        # islands and not one: a single island in a corner reads as a
        # decoration, two read as an archipelago the rose is laid over.
        island(0.30, 880, 886),
        island(0.21, 150, 158),
    ]
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{px}" height="{px}"'
        f' viewBox="0 0 {S} {S}">\n'
        f'<clipPath id="corners">'
        f'<rect width="{S}" height="{S}" rx="{RADIUS}" ry="{RADIUS}"/>'
        f"</clipPath>\n"
        f'<g clip-path="url(#corners)">\n' + "\n".join(body) + "\n</g>\n</svg>\n"
    )


def render(svg, out, px):
    """SVG to PNG, by whichever of the two renderers this machine has."""
    if shutil.which("rsvg-convert"):
        subprocess.run(
            ["rsvg-convert", "-w", str(px), "-h", str(px), "-o", str(out), str(svg)],
            check=True,
        )
        return
    if shutil.which("qlmanage"):
        # qlmanage names its output after the input and will not be told
        # otherwise, so it writes into a directory of its own and the result
        # is moved into place.
        with tempfile.TemporaryDirectory() as tmp:
            subprocess.run(
                ["qlmanage", "-t", "-s", str(px), str(svg), "-o", tmp],
                check=True,
                capture_output=True,
            )
            written = Path(tmp) / f"{svg.name}.png"
            if not written.exists():
                sys.exit(f"{sys.argv[0]}: qlmanage rendered nothing")
            # Only once it is whole, so a killed run cannot leave a torn file
            # committed.
            written.replace(out)
        return
    sys.exit(f"{sys.argv[0]}: neither rsvg-convert nor qlmanage is installed")


def main():
    parser = argparse.ArgumentParser(description="Build the application icon.")
    parser.add_argument("--size", type=int, default=1024, help="pixels square")
    parser.add_argument("--out", type=Path, help="where to write the PNG")
    args = parser.parse_args()

    out = args.out or ROOT / "assets" / "icon.png"
    with tempfile.TemporaryDirectory() as tmp:
        svg = Path(tmp) / "icon.svg"
        svg.write_text(draw(args.size))
        render(svg, out, args.size)
    print(f"{Path(sys.argv[0]).name}: {out} ({args.size}px)")


if __name__ == "__main__":
    main()
