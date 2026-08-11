# The boat

What the shape is *for*, which a `.blend` has nowhere to say. The dimensions
themselves are in the file; these are the reasons behind the ones that are not
obvious, and they were const docs in `crates/game/src/boat.rs` until the hull
became a model.

The boat is nearly always seen from a camera some forty metres up, looking down
at 50°, against ground drawn in flat facets a couple of metres across. That is
the one view worth modelling for, and most of what is below follows from it.

**Length overall, 7 m.** At the default zoom the visible ground is some tens of
metres across, so this reads as a boat rather than as a speck — and at the far
end of the zoom range it is still a mark on the water rather than gone.

**The widest point is aft of amidships.** The taper to the bow is nearly twice
the length of the one to the transom. A fine entry and a full stern is what
tells one end from the other when the camera is looking straight down.

**The mast is tall out of proportion to the hull.** From overhead a mast is most
of what says which way the boat is leaning and where it is against the ground
behind it, and its shadow is what pins it to the water. A mast in proportion
would be a dot. It stands about a third of the way back from the bow, which is
where a boat this shape would carry one.

**The bottom is a shallow V, not flat.** The hull falls from the deck to a keel
line, so it reads as a boat from the side as well as from above — and the game
probes only that line for the ground beneath. A hull remodelled with a flat
bottom carried out to the beam would need `KEEL_PROBES` out there too.

Two things are not free to change here. The keel's depth and the stations it
runs between are what the grounding rule is written against, and the model is
held to them by a test — see the module documentation in `boat.rs`. Everything
else about the shape is this file's own business.
