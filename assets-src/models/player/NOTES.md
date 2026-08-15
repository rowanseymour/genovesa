# The player

Why the shape is what it is; the numbers are in the file and the rules are in
the tests. It is the one thing always on screen, and the first model that moves
under its own power — `export.sh` carries skins and animations for its sake.

- **A person, standing on the origin.** Not the far taller beacon a remote
  player is marked with, which has to be found from hundreds of metres up. The
  soles are at zero because that is the point the game holds on the ground, and
  at deck height aboard a boat; modelled about its middle it would walk
  knee-deep in the sand.
- **Boxes, and one triangle.** A person built of rectangular solids is no more
  of a compromise than a beach built of facets. Nothing here needs a bevel.
- **One mesh, carrying its own colours on its facets.** It was three meshes
  once, one per tone, when the client painted by name; with the colour on the
  corners the split had no job left. The game repaints every arriving mesh
  with one white matte — `crates/game/src/models.rs` — to keep the file's PBR
  material out of the world.
- **Charcoal coat and hat.** A silhouette rather than a colour: findable on
  sand, grass and deck alike, and out of the way of the hues the remote
  players' markers are dealt from.
- **Sun-bleached canvas legs.** Pale against the coat, so the swinging half of
  the figure is the half that stands out.
- **Weathered skin for the head and bare forearms.** Warm, where nothing else
  on the figure is, so a small head still reads as a head between the hat and
  the coat.
- **Every vertex weighted to exactly one bone.** The easiest rule to break with
  a weight-paint brush: a shared vertex *bends* its facet as the figure walks,
  and a gradient across a facet is the one thing this look cannot have.
- **The hat is a tricorn, and it is what says which way the player faces.** From
  overhead a body is nearly symmetric and shoulders say little; a triangle with
  its point forward says it at a glance.
- **The legs are thick and the stance wide.** A limb that reads from the
  camera's height has to be a couple of facets wide rather than a wire, and the
  daylight between them at the top of a stride is most of what says *running*
  from above.

## The two actions

`idle` is one keyframe of the rest pose; `run` is a full cycle. The game
crossfades by how much of a walk is on, so they are two ends of one dial.

**The run is not played at its own speed.** The game pauses it and *seeks* it
to wherever the player's stride has reached, advancing by ground covered rather
than by the clock — which is what stops the feet sliding at any speed other
than the one it was drawn at, and makes backing up run the cycle backwards for
free. So the cycle must loop, first frame matching last, or the walk jumps once
a stride. And the game's `STRIDE` is how far that cycle carries the figure,
which is not in this file: draw a longer-legged run and that constant wants
growing to match, or the feet scuff.

**The body dips when the legs are spread.** These legs do not bend, so a leg
swung out ahead reaches less far down than one standing straight, and a body at
a fixed height would leave the feet skating at every full stride. The `root`
bone drops by exactly what the swing costs. Give the rig knees and this stops
being the right rule.

Where the figure *walks* is `crates/game/src/figure.rs` for the gait and
`player.rs` for the person being moved.
