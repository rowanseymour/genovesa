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
rise and width each carry their own nudge, one frond is an old one hanging well
below the others, and every blade curves the same way about its own axis so the
crown has a handedness. Seven fronds at exactly a seventh of a turn apart look
identical from seven directions — and the server only ever turns a palm about
the vertical, so a regular crown would tile: a beach of them would read as one
tree stamped repeatedly however carefully the bearings were scattered. The
irregularity is what gives a palm a front and a back, and
`the_crown_maps_onto_itself_at_no_rotation` holds the file to having them.

**Each frond is a closed blade, not a plane.** A leaf is usually a flat quad at
half the triangles, but a plane has no back — it would vanish from one side
under backface culling, or need a two-sided material nothing else in this world
uses. Giving fronds a little thickness keeps one rule for the whole game, which
is the rule the tests check.

The trunk's foot sits at the model's origin, and that is not free to change:
the client puts a palm's origin on the height field at its own position, so a
trunk starting a metre up would hover over its own shadow. There is a test.

Where palms *stand* is not decided here or anywhere in this directory — it is
the world's business, in `crates/world/src/palms.rs`, and travels to clients
with the ground.
