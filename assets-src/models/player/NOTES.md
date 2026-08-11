# The player

What the shape is *for*, which a `.blend` has nowhere to say. The numbers are
in the file; these are the reasons behind the ones that are not obvious.

The player is the one thing in the world that is always on screen, and it is
seen from the camera some tens of metres up and behind, looking down at 50°.
That is the one view worth modelling for, and most of what is below follows
from it. It is also the first model here that **moves under its own power**,
and the first with an armature and actions in it — `export.sh` carries skins
and animations for this file's sake.

**A person, 1.8 m, standing on the origin.** Not the four-metre beacon a
remote player stands as: a marker has to be found from hundreds of metres up,
where this is only ever looked at from the camera following it. The soles are
at Z zero because that is the point the game holds on the ground — and holds
at deck height aboard a boat — so a figure modelled about its middle would
walk knee-deep in the sand.

**Boxes, and one triangle.** The world is drawn in flat facets a couple of
metres across in a small fixed palette, so a person built of rectangular
solids is no more of a compromise than a beach built of facets. Nothing here
needs a bevel; anything that reads at this distance is a silhouette.

**Three meshes, one per tone: `coat`, `canvas`, `skin`.** The game ignores the
file's materials and paints these itself, matching them *by name* — see
`TONES` in `crates/game/src/figure.rs`. So the split is by colour and not by
body part: the coat mesh carries the hat, and which bone moves which vertex
cuts across it freely. Rename or add a mesh and the game has no tone for it,
which a test fails on rather than letting it into the world wearing whatever
Blender last gave it.

**Every vertex is weighted to exactly one bone, at 1.0.** This is the rule the
look depends on and the easiest one to break with a weight-paint brush. A
vertex shared between two bones *bends* the facet it belongs to as the figure
walks, and a gradient across a facet is the one thing a world of flat tones
cannot have. Pinned by a test.

**The hat is a tricorn, and it is what says which way the player faces.** From
overhead a body is nearly symmetric and shoulders say very little; a triangle
with its point forward says it at a glance, and sweeps round when the player
turns on the spot. Modelled as a plain triangular plate — the outline is all
that survives at this distance, and the outline of a tricorn is a point.

**The legs are thick and the stance is wide.** They are the moving part, and a
limb that reads at forty metres has to be a couple of facets wide rather than
a wire. The gap between them is what lets daylight through at the top of a
stride, which is most of what says *running* from above.

**It faces +Y in this file**, which the +Y-up export turns into Bevy's -Z
forward — the boat's own convention, and the seabird's.

## The two actions

`idle` is one keyframe of the rest pose, and `run` is a full cycle at 24 fps.
The game crossfades between them by how much of a walk is on, so they are two
ends of one dial rather than two states.

**The run is not played at its own speed.** The game pauses it and *seeks* it
to wherever the player's own stride has reached, advancing by ground covered
rather than by the clock. That is what stops the feet sliding at any speed
other than the one the cycle was drawn at, and it makes a player backing up
run the cycle backwards for free. Two things follow for anyone editing it:

- **The cycle must loop**, first frame matching last, or the walk will jump
  once a stride.
- **A stride is 1.7 m of ground**, which is the game's `STRIDE` and is not in
  this file — a clip knows how long it lasts in seconds, not how far the
  figure drawn in it would travel. Draw a longer-legged run and that constant
  wants growing to match, or the feet will scuff.

**The body dips when the legs are spread.** These legs do not bend, so a leg
swung out ahead reaches less far down than one standing straight, and a body
held at a fixed height would leave the feet skating in the air at every full
stride. The `root` bone drops by exactly what the swing costs — `LEG - LEG
cos(swing)` — which is why a stiff-limbed walk bobs at all. Give the rig knees
and this stops being the right rule.

Where the figure *walks*, and what makes it walk at all, is the game's
business: `crates/game/src/figure.rs` for the gait and
`crates/game/src/player.rs` for the person being moved.
