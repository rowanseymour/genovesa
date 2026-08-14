# The icon

What the picture is *for*, which `build.py` has nowhere to say. The numbers are
in the script; these are the reasons behind the ones that are not obvious.

An icon is seen at two sizes and neither of them is the one it is drawn at.
Large, it sits on a store page or a README and can afford detail. Small — 32
pixels in a dock, 16 in a window list — it is four or five shapes and a colour,
and that is the size that decides whether anyone finds the thing twice. Every
choice below is the small size winning an argument.

**A compass rose, and not a picture of the game.** The obvious icon is what the
game already draws: an island in plan, which is a strong silhouette in a blue
square and unmistakably *this* game. It was drawn and rejected. An island's
whole character is its coastline and its mottled interior, and both are
high-frequency detail — at 64 pixels it is a green smudge with a grey dot on
it, and at 32 it is a green smudge. A rose is four big shapes that stay four
big shapes all the way down. The islands are still here, in the corners, doing
the job of saying which kind of sea this is.

**The ground's palette, not the interface's.** `game`'s own compass — the card
in the corner of the screen, in `compass.rs` — is drawn in the menu furniture:
off-white ink on a dark translucent face, deliberately quiet so it can sit over
the picture without shouting. Quiet is exactly wrong for an icon, which is
competing with every other icon on a dock. So the rose is painted out of
`protocol`'s ground palette instead: sand on sea water, the two most separated
colours the world contains. The two instruments are the same idea in different
rooms, and it is not worth making them match at the cost of one of them.

**One tone per face, lit from the north-west.** Which is the ground's own rule,
applied to something that is not ground: no gradients anywhere, and the light
where the mountains in a `mapgen` plan have it. The whole north-west half of
the rose is sand and the south-east half is sand with the sun off it, and that
single split is what stops a flat star reading as a sticker. It is also the
first thing that still works at 32 pixels, when the ink lines between the faces
have gone.

**A notch at north.** The rose has fourfold symmetry and the light does not, so
the drawing has a way up but only just. The notch outside the rim states it.
It is the one piece of furniture that reads *better* small, because it survives
as a bump on the ring long after it has stopped being a triangle.

**Two islands, not one.** A single island in a corner reads as a decoration
placed there for balance. Two read as an archipelago the rose is laid over,
which is the whole subject of the game in the space left around a compass.

**Their beaches are out of proportion, on purpose.** A sand rim in the
proportion the generator actually produces is a hairline at this size, and the
islands come out as green blobs with no coast. `island()` cuts the grass back
much further than a real beach reaches, so that each has a visible shore.

Nothing in the game reads `assets/icon.png`. Setting a window's icon is
`winit`'s `set_window_icon`, which is unsupported on macOS — there the icon
comes from a bundle's `.icns` and there is no bundle here yet — so wiring it
would buy nothing on the machine this is developed on, and cost `game` a direct
dependency on `winit` at a version that has to keep agreeing with Bevy's. The
picture is worth having before that is worth doing.
