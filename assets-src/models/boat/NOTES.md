# The boat

Why the shape is what it is; the numbers are in the file and the rules are in
the tests. These were const docs in `crates/game/src/boat.rs` until the hull
became a model. The shape is after the Bermuda sloops — the single-masted
working boats of the tropics in the age of sail — as far as a handful of
facets seen from forty metres up can carry it: sprung sheer, a full stern, a
stepped-up deck aft.

- **Long enough to read as a boat** at the default zoom, where the visible
  ground is some tens of metres across — and still a mark on the water at the
  far end of the zoom range rather than gone.
- **The widest point is aft of amidships**, the entry nearly twice the taper of
  the run. A fine bow and a full stern is what tells one end from the other
  with the camera looking straight down.
- **The sheer springs up towards the bow**, a chine softens the V, and the
  stem head stands proud of the midships deck — the extra stations are spent
  where the silhouette is, not on detail that only reads close up.
- **The mast is tall out of proportion.** From overhead it is most of what says
  which way the boat is leaning and where it sits against the ground behind it,
  and its shadow is what pins it to the water. In proportion it would be a dot.
  It stands plumb where a real Bermudian rig rakes aft, because the pennant is
  tied to its top and the sail turns about its axis: the game holds the spar
  within a tenth of a metre of one vertical line, and a rake would spend that
  allowance on style the camera reads mostly foreshortened anyway.
- **The bottom is a shallow V, not flat**, so it reads as a boat from the side
  as well as from above — and the game probes only the keel line for the ground
  beneath. A flat bottom carried out to the beam would need `KEEL_PROBES` out
  there too.
- **The quarterdeck steps up abaft the boom.** It is where the helmsman
  belongs, and the game stands the player there — the step begins where the
  boom's sweep ends, so the figure at the helm never shares its air with the
  sail. The tiller rises forward from the rudder head to the grip beside them:
  from overhead it is what says *this end is steered*.
- **The companionway sits between the mast and the step**, the one way down
  into a hull that is otherwise a closed shell — a boat lived aboard rather
  than a dinghy. Its top is the darkest tone on the model, which is what an
  opening looks like from forty metres.

## The colours

On the facets, in the file, like every model's — and the boat's palette is
picked against the world's rather than its own parts, so it is written down
here:

- **Timber** for the planking. Nothing on an island or in the sea is anywhere
  near this hue, so the boat is findable in a landscape of greens and blues
  without being lit any differently from them.
- **Scrubbed deck**, warmer and paler than the topsides, so the deck plan —
  sheer, step, hatch — reads from the camera's own overhead view, which is
  where the boat is looked at most.
- **Bare spar cream** for mast and tiller, pale enough to stand off both the
  water and the hull. The sail and pennant the client cuts (see `boat.rs`)
  key their cloth to it.

The keel's depth and the stations it runs between are what the grounding rule
is written against, and a test holds the model to them — see the module
documentation in `boat.rs`. Everything else about the shape is this file's own.
