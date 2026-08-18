# The rowing boat

Why the shape is what it is; the numbers are in the file. Three metres and a
bit of open boat against the ship's seven — a dinghy, rowed rather than
sailed, and the small end of what a player gets about in.

- **An open boat, so it is two shells and not one.** The ship is a closed hull
  with a deck on top and nothing to see inside it; this one is looked *into*
  from a camera forty metres up, and the inside is most of what is looked at.
  So there is an outer skin, an inner one, and a flat gunwale capping the two
  together — which is also the only place the planking's thickness shows.
- **The sole sits below the waterline, the way a real one's does.** It was
  the other way for exactly one commit: the sea used to be one unbroken
  surface drawn straight through everything, so the floor had to stand above
  the water or the boat was drawn full of sea — and holding the sole up is
  what forced the first cut deep, and long to carry the depth: a four-metre
  skiff rather than the dinghy it started as. The sea now discards its
  surface inside an open hull's waterline footprint (see `cut_the_water` and
  the `OpenHull` it reads in `crates/game/src/boat.rs`), which is what let
  this master be recut *back* to the dinghy: shorter, shallower, and dry
  inside because the water knows to stay out rather than because the floor
  ran from it.
- **The footprint the sea is told is this file's outline.** The hole is two
  superellipse halves cut square at the transom, and its numbers on
  `ROWBOAT` in `boat.rs` were read off this hull's waterline a few
  centimetres up, where the swell stands against the planking. Re-loft the
  hull and those numbers are stale: too narrow and the sea leaks back into
  the bilges, too wide and a moat of missing water shows round the bow.
- **The widest point is a little abaft amidships**, the bow fine and the
  transom wide, so which way it is pointing reads from straight overhead. The
  sheer springs up to the stem and rises again at the transom.
- **A shallow V with a chine**, like the ship's, so the same grounding rule
  fits it: the keel line is the whole of what touches, and nothing is carried
  flat out to the beam that would need probing on its own.
- **Three thwarts.** The middle one is the rowing thwart and the two others
  are where the rest of the boat's business happens — but between them they
  are also what a shipped oar lies on, which is what fixes where they are.
- **The rowlocks are blocks on the gunwale, abaft the rowing thwart** by about
  the length of a forearm. That is not decoration: it is what puts the handles
  at the rower's chest at the finish and out at arm's length at the catch. Move
  the thwart or the rowlocks and the rower's reach goes with them.

## The colours

The ship's four, in different proportions, so the two read as one fleet — see
`../boat/NOTES.md`, where they are picked. Timber for the topsides and the
transom, the darker timber below the chine, scrubbed deck for the whole
interior, and bare spar cream for the gunwale, the thwarts and the looms.

The blades are timber rather than cream, which is the one choice made here
rather than borrowed: from overhead the stroke is two dark tips swinging past
the hull, and a blade shipped inboard is a dark shape lying on a pale one
instead of cream on cream.

## The two clips

`stowed` is one keyframe: the oars in, lying fore and aft across the stern
seat and the rowing thwart, blades flat and forward. `stroke` is one full
cycle — the catch, the drive, the blades lifted and feathered for the
recovery, squared again at the top. It closes, first frame matching last.

**The oars pivot at the rowlock**, which is why the model is cut with them
out and shipping them is the pose that moves: an oar turns about the crutch it
sits in and nowhere else, so the stroke is a rotation and nothing more. The
drive is the shorter half of the cycle, the way a stroke is.

**The stroke is seeked rather than played**, exactly as the figure's run is —
by water covered, so the blades bite at whatever speed the boat is making and
a boat backing water pulls the cycle backwards. The one number that needs and
the file cannot carry — how far one stroke drives the boat, a little more than
its own length — is `PULL` in `crates/game/src/boat.rs`, where `row` does the
seeking and settles the oars stowed at rest.

The two are **states and not a dial**. Crossfading them sweeps the looms
through the gunwale, which is what shipping the oars looks like and is fine
taken briskly, but nothing should be left standing halfway.

## What the game does with it

The world does not deal one out yet: the only way afloat is the dev switch —
`set boat rowboat` at the console, `--boat rowboat` on a command line — which
redresses the player's own hull on this client alone. The numbers the game
holds this file to are the `ROWBOAT` hull's in `crates/game/src/boat.rs` —
length, draft, the sole and the sea-hole footprint above — pinned by
`the_rowboat_model_is_the_dinghy_the_game_floats` the way the ship's are.
Still the file's to give and the game's to be pinned to; what is not settled
yet is where somebody aboard *sits*, the rower not being rigged.
