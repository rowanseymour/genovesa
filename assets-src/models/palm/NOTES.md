# The palm

Why the shape is what it is; the numbers are in the file and the rules are in
the tests.

- **Tall enough to throw a shadow across the sand it stands on**, which is most
  of what stops it reading as a green dot.
- **The trunk bends near the top, and leans.** The lean is what stops a stand
  of them looking like fence posts, since the server turns each to its own
  bearing.
- **Few sides to the trunk, not many** — a facet has to be big enough to read
  as a facet. Round, it is a smooth grey stick with no shape to catch the light.
- **The droop is the whole thing.** Fronds leave the crown rising and fall well
  below it, so the head has a shoulder; held straight out they read as a
  starfish on a stick.
- **No two fronds are alike, and that is load-bearing.** The server only turns
  a palm about the vertical, so a regular crown would tile — a beach reading as
  one tree stamped repeatedly. What carries that is the spread of bearings and
  reaches, not the one low frond, which is only a shoulder on the crown;
  `the_crown_maps_onto_itself_at_no_rotation` holds the file to it. An earlier
  version of that test measured the crown's centre of mass off the trunk and
  read 14 mm on a visibly lopsided crown: fronds radiate, so averaging their
  corners hides exactly what is being asked about.
- **Timber browner and darker than its sand**, so the trunk reads against it at
  any zoom, and deliberately not the boat's — a hull is meant to be findable in
  a landscape where a tree is meant to belong to one. **Frond green** sits
  between the grass behind a beach and the forest above it.
- **The foot is at the origin**, which is where the client puts it on the
  height field.

Where palms *stand* is `crates/world/src/palms.rs`, and travels with the ground.
