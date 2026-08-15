# The palm

What the shape is *for*, which a `.blend` has nowhere to say. The numbers are
in the file; these are the reasons behind the ones that are not obvious.

A palm is seen from the same place everything else is — a camera some forty
metres up looking down at 50° — and almost always in the middle distance,
standing on a strip of sand a few facets wide with the sea on one side of it.
It is scenery, not something the player goes and looks at, so what matters is
that a dozen of them along a coast read as *a beach with palms on it* at a
glance. Everything below serves the silhouette.

**About six metres, and it leans.** Tall enough to throw a shadow across the
sand it stands on, which is most of what stops it reading as a green dot. The
trunk leaves the ground upright and does its bending near the top, the way one
carrying a head of fronds does — and the lean is what stops a stand of them
looking like fence posts, since the server turns each to its own bearing.

**Four sides to the trunk, not more.** The same argument the ground is drawn
on: a facet has to be big enough to read as a facet. A round trunk at this
distance is a smooth grey stick with no shape to catch the light.

**Seven fronds, and the droop is the whole thing.** They leave the crown rising
and fall well below it, so the head has a shoulder rather than being a flat
disc. Fronds held straight out read as a starfish on a stick.

**No two fronds are alike, and that is load-bearing.** Bearing, length, droop,
rise and width each carry their own nudge, one frond is an older one hanging
lower than the rest, and every blade curves the same way about its own axis so
the crown has a handedness. Seven fronds at exactly a seventh of a turn apart
look identical from seven directions — and the server only ever turns a palm
about the vertical, so a regular crown would tile: a beach of them would read as
one tree stamped repeatedly however carefully the bearings were scattered. The
irregularity is what gives a palm a front and a back, and
`the_crown_maps_onto_itself_at_no_rotation` holds the file to having them.

What carries that, measurably, is the bearings and the reaches: the seven sit at
gaps of 28° to 76° against a regular 51.4°, and reach 2.2 m to 2.7 m out. The
old frond used to hang to 2.0 m, half again the length of any other and a tongue
down the trunk from any angle; pulling it back to 4.1 m, still the lowest in a
crown whose next is at 4.6 m, moved the test's margin from 1.86 m to 1.45 m
against a floor of 0.25. So the low frond is a shoulder on the crown, not the
thing keeping a beach from tiling — that was already carried elsewhere.

**Each frond is a closed blade, not a plane.** A leaf is usually a flat quad at
half the triangles, but a plane has no back — it would vanish from one side
under backface culling, or need a two-sided material nothing else in this world
uses. Giving fronds a little thickness keeps one rule for the whole game, which
is the rule the tests check.

**One mesh, and it carries its own colours.** The tree is painted per vertex
and drawn with a white material, so a palm is one mesh, one material and one
entity — it was two of each only for as long as the crown and the trunk were
two colours the *client* held. The colours themselves:

- **Timber**, a shade browner and darker than the sand a palm stands on, so
  the trunk reads against it at any zoom. Deliberately not the boat's — a hull
  is meant to be findable in a landscape and a tree is meant to belong to one.
- **Frond green**, darker and yellower than the grass behind a beach and
  lighter than the forest above it, so a stand of palms is its own band of
  colour rather than an outcrop of whatever it is standing in front of.

They are stored **scene-linear**, which is what a glTF `COLOR_0` means and what
the game multiplies into a white base colour. The sRGB triples those two are
named by — (0.42, 0.33, 0.24) and (0.31, 0.50, 0.20) — are what a screen is
told, not what the file holds; writing them in as though they were linear would
draw the tree a good deal paler than any of this describes. The material has to
*read* the colour attribute through a Color Attribute node, and not merely have
one alongside it: the exporter maps an attribute to `COLOR_0` only where a
material uses it. Wiring it is also what makes Blender show the tree in the
colours the game will draw it in.

The trunk's foot sits at the model's origin, and that is not free to change:
the client puts a palm's origin on the height field at its own position, so a
trunk starting a metre up would hover over its own shadow. There is a test.

Where palms *stand* is not decided here or anywhere in this directory — it is
the world's business, in `crates/world/src/palms.rs`, and travels to clients
with the ground.
