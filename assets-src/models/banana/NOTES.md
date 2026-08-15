# The banana

What the shape is *for*, which a `.blend` has nowhere to say. The numbers are
in the file; these are the reasons behind the ones that are not obvious.

A banana is seen from where everything else is — a camera some forty metres up
looking down at 52° — and stands on a valley floor or a lake margin, on ground
the world paints somewhere between forest and grass. It is scenery, and what
matters is that a few of them in a hollow read as *somewhere wetter and more
sheltered than the hillside above*. Everything below serves that.

**It is a clump, and that is the whole silhouette.** Three pseudostems of
different ages, the tallest 2.4 m and the youngest a metre sucker at its foot.
One stem is a palm with the wrong crown; three of stepped heights is a thing
that spreads by suckering, which is what a banana is. The clump is four metres
across and about four tall, so it reads as broader than it is high — the
opposite proportion to the palm, and that alone tells the two apart at any zoom
where both are a few pixels.

**Leaves too big to be believed, and bigger still than that.** Two to three
metres long and near a metre wide, on a plant four metres tall. Life is not far
off this, and the picture needs more: a banana at this distance is *only* its
leaves, and the first pass at half the width read as a spray of grass.

**Young leaves stand, old ones have flopped.** Each leaf leaves the stem at a
rise that falls with its age and ends drooping well below horizontal. A clump
of leaves all held at one angle reads as a fan or a shuttlecock; the spread of
rises is what gives it a top and a skirt.

**The section is a raised midrib with the edges falling away** — the opposite
fold to the palm's keeled frond, and worth the trouble because it is what the
eye knows a banana leaf by. Like every leaf in this game it is a closed blade
rather than a plane: a plane has no back and would vanish from one side under
backface culling.

**The bunch is a pixel, and it stays.** A drooping stalk off the tallest stem,
two hands on it, and the dull red-purple bud under them. Nothing else in the
world is that colour, so at any zoom that shows it at all the clump is
unmistakable — and at every other zoom it costs twenty triangles nobody sees.

**One mesh, painted per vertex, drawn with a white material.** Four colours,
named in the sRGB the game's palette is written in and stored scene-linear,
which is what a glTF `COLOR_0` means:

- **Leaf**, (0.52, 0.68, 0.22) — lighter and markedly yellower than the ground
  it stands on, which runs from forest (0.21, 0.42, 0.22) to grass
  (0.44, 0.66, 0.26). A green that merely joined in with the grass left a clump
  reading as a bush-shaped patch of it.
- **Stem**, (0.46, 0.48, 0.28) — paler and greyer than its own leaves, so the
  clump has a pale core against dark ground rather than being one mass of green.
- **Fruit**, (0.62, 0.58, 0.20), and the **bud**, (0.45, 0.22, 0.28).

The material reads the colours through a Color Attribute node, and that is not
decoration: the exporter maps an attribute to `COLOR_0` only where a material
uses it, and writes a dummy white set otherwise.

**Triangles, not quads.** The stems and leaves are swept rings, and a swept
ring twists — most of the quads it would otherwise be built from are not
planar, which gets one normal in Blender and two differently-lit triangles at
export. There is a test for the flat shading and this is how it is kept.

The foot of the tallest stem sits at the model's origin, which is not free to
change: the client puts a plant's origin on the height field at its own
position, so a clump starting a hand's breadth up would hover over its own
shadow. There is a test for that too.

Where bananas *grow* is not decided here or anywhere in this directory — it is
the world's business, in `crates/world/src/bananas.rs`, and travels to clients
with the ground.
