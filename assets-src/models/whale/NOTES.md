# The whale

What the shape is *for*, which a `.blend` has nowhere to say. The numbers are
in the file; these are the reasons behind the ones that are not obvious.

A whale is seen as a back: a long dark mass shouldering through the surface
for a few seconds at a time, mostly submerged even then, with the rest of the
body a shadow under the water's near-opacity. The model is built for that one
view — everything is in the top line of the silhouette.

**About eleven metres, life size.** The scale *is* the animal: it reads as a
whale because it is so much longer than the dolphins the sea has taught the
player to expect, and against the 7 m boat. Making it bigger would make it a
sea monster, which is a different promise.

**The girth peaks well forward and the spine rises aft.** A rorqual's lines:
blunt head, the back's high point forward of the middle, and a tail stock
that climbs toward the flukes — so what breaks the surface is a long arc
that is visibly *going somewhere*, not a floating log.

**The dorsal fin is small and far aft.** The rorqual proportion; a tall fin
amidships would read as an orca, and an orca circling a sailing boat is a
mood this world is not selling.

**Broad horizontal flukes, long flippers.** Both mostly seen as shadow under
the surface just before the back appears — the hint that the mass has a
shape. Horizontal flukes for the same reason as the dolphin's: vertical
means shark.

**Origin amidships**, where the surfacing arc pivots. **It faces +Y in this
file**, which the +Y-up export turns into Bevy's -Z forward, the boat's own
convention.

**It carries its own colours, and they are countershaded.** The back keeps the
deep blue-grey the client used to hold, (0.27, 0.31, 0.37) — darker than the
dolphin's, because a whale's back barely clears the water and what sells the
size is a long dark mass rather than a bright shape. The belly is
(0.62, 0.65, 0.68).

Which facets are belly is decided by the way each one *faces*, not by how high
it sits: the underside is pale because it is the side in shadow, which is what
countershading is, and on a hundred facets it puts the join along the widest
line of the animal, where an eye expects it. It matters most when a whale
rolls or sounds — a flat-coloured whale is a slab from every angle, and the
flukes coming up are the moment the shape wants reading.

Where a whale *swims* is the server's word and the wire carries it — a
whale is a *beast* now, one creature every player can point at — while what
it looks like doing so stays the client's, in `crates/game/src/beasts.rs`.
