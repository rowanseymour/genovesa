# The masters

One `.blend` per model, exported by `export.sh`. Each has its own NOTES for
what is peculiar to it — these are the rules all of them obey, and the reason
each is worth obeying.

- **Modelled for one view.** Everything is seen from the game's fixed camera
  tilt, in the middle distance, against ground drawn in flat facets a couple of
  metres across. Silhouette is the whole job: detail that only reads close up
  is triangles spent where nobody is looking.
- **+Y is forward.** The +Y-up export lands that on Bevy's -Z, so the game can
  point a model along its heading knowing nothing about the file.
- **Closed solids, never planes.** A plane has no back — it vanishes from one
  side under backface culling, or needs a two-sided material nothing here uses.
  Leaves, wings and tails are thin closed sheets.
- **Flat facets and no gradients**, which is why a colour belongs to a facet
  rather than to a vertex. `export.sh` has the mechanics.
- **A model carries its own colours** and is drawn with a white material, so
  what a thing is painted is settled here rather than in the client.
