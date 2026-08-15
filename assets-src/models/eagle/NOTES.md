# The eagle

What the shape is *for*, which a `.blend` has nowhere to say. The numbers are
in the file; these are the reasons behind the ones that are not obvious.

An eagle is only ever seen circling a summit, from a camera forty metres up
and usually a hundred or more away, against sky or against rock. It never
lands, never flaps and is never approached, so the model is a soaring
silhouette and nothing else: wings out, tail fanned, not a feather modelled.

**Wingspan about 3.7 m, twice life size.** The same argument as the palm's
six metres: at the distance a summit is watched from, an honest golden eagle
would be a flyspeck. Twice scale keeps the circling readable while staying
small enough that nobody sails under one and laughs.

**The wings kink at the wrist and rise past it.** A soaring bird holds its
wings in a shallow V with the tips swept back and raised, and that dihedral
is most of what the silhouette *is* when seen from below or edge-on. Flat
boards would read as a glider. Two panels a side is the fewest that can hold
the kink, so two is what there are.

**The tail is a fan and the head barely exists.** From the distances that
matter the bird is wings and tail; the head is a short point so the shape has
a front, and no more. Like the palm's fronds, wings and tail are thin closed
sheets rather than planes — a plane has no back, and would vanish from one
side under backface culling.

**It faces +Y in this file.** The exporter's +Y-up conversion lands that on
-Z, which is Bevy's forward and the boat's convention, so the game can point
it along its flight with a plain `looking_to`.

**It carries its own colours: dark above, pale below, white at both ends.** The
body and the upper wings keep the dark umber the client used to hold,
(0.24, 0.18, 0.13). Beneath is (0.72, 0.68, 0.58), and the head and the tail
are (0.90, 0.89, 0.85).

The white is the concession to being four pixels across. An eagle is seen from
above, where an underside never shows, so countershading alone would have left
it the dark blob it already was; white at both ends of a dark bird is the one
plumage pattern that survives being that small, and it is what a fish eagle
actually looks like. The markings are deliberately blocks rather than detail —
the old client-side note was right that real plumage colour would only muddy a
shape this size, and the answer is to be bolder rather than to stay plain.

Where an eagle *flies* is the game's business, in `crates/game/src/wildlife.rs`
— decorative wildlife is client-side, so nothing about this model or its
placing crosses the wire.
