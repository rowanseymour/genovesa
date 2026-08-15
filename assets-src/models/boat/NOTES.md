# The boat

Why the shape is what it is; the numbers are in the file and the rules are in
the tests. These were const docs in `crates/game/src/boat.rs` until the hull
became a model.

- **Long enough to read as a boat** at the default zoom, where the visible
  ground is some tens of metres across — and still a mark on the water at the
  far end of the zoom range rather than gone.
- **The widest point is aft of amidships**, the entry nearly twice the taper of
  the run. A fine bow and a full stern is what tells one end from the other
  with the camera looking straight down.
- **The mast is tall out of proportion.** From overhead it is most of what says
  which way the boat is leaning and where it sits against the ground behind it,
  and its shadow is what pins it to the water. In proportion it would be a dot.
- **The bottom is a shallow V, not flat**, so it reads as a boat from the side
  as well as from above — and the game probes only the keel line for the ground
  beneath. A flat bottom carried out to the beam would need `KEEL_PROBES` out
  there too.

The keel's depth and the stations it runs between are what the grounding rule
is written against, and a test holds the model to them — see the module
documentation in `boat.rs`. Everything else about the shape is this file's own.
