# The seabird

What the shape is *for*, which a `.blend` has nowhere to say. The numbers are
in the file; these are the reasons behind the ones that are not obvious.

A seabird is only ever seen as one of a line: half a dozen of them gliding
single file a couple of metres over the shallows, undulating together. Nobody
looks at one bird; the *line* is the animal. So the model is a gliding
silhouette and nothing else — no flap, no feet, no feathers.

**Wingspan about 2.1 m, life size.** Unlike the eagle it is not scaled up: a
line flies close enough to the boat that outsized birds would crowd the
picture, and a line reads at a distance the way a single bird cannot.

**The wings droop past the wrist.** A gliding seabird holds its inner wing
flat and lets the tips fall, where the soaring eagle's rise — the two
silhouettes are opposite answers to the same question, and the droop is most
of what tells a line over the surf from a raptor that has lost its mountain.

**The bill is long and the tail is short.** A pelican's proportions, near
enough: at line-of-birds distance the long head is the only thing that says
seabird rather than pigeon.

**It faces +Y in this file**, which the +Y-up export turns into Bevy's -Z
forward, the boat's own convention. Wings and tail are thin closed sheets,
like the palm's fronds — a plane has no back, and would vanish from one side
under backface culling.

**It carries its own colours: white, with a grey mantle and dark wingtips.** The
body keeps the chalk grey the client used to hold, (0.84, 0.84, 0.80) — a
seabird is seen low against bright water, where a pale bird is the one that
reads. The upper wing is (0.55, 0.57, 0.60) and the tips (0.22, 0.22, 0.24).

The other way round from the eagle, and for the same reason: what a bird is
seen from decides which surface is worth painting. The underside stays white
throughout, and only the four up-facing facets of each wing carry anything —
which at this size is a gull, and at any smaller size is still a pale bird.

Where a line *flies* is the game's business, in `crates/game/src/wildlife.rs`
— decorative wildlife is client-side, so nothing about this model or its
placing crosses the wire.
