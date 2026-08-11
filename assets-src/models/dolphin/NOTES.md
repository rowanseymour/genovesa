# The dolphin

What the shape is *for*, which a `.blend` has nowhere to say. The numbers are
in the file; these are the reasons behind the ones that are not obvious.

A dolphin is seen for a second at a time: an arc through the surface fifty
metres or more from the camera, most of the body under water the rest of the
while. The model is whatever makes that one second read — a smooth dark back,
a dorsal fin, flukes — and nothing that only a stopped, close-up dolphin
would show.

**About 2.4 m — life size.** Unlike the palm and the eagle it is not scaled
up: a pod leaps close enough to the boat that an outsized dolphin would read
as a whale, and a whale is a different promise to the player.

**The flukes are horizontal and that is the whole species.** A vertical tail
reads as a shark from every angle that matters, and the surface-piercing
angles are all this model has. The dorsal fin sweeps back for the same
reason: a triangle standing straight up is a shark's, a raked one is a
dolphin's.

**The girth peaks forward of the middle and the spine rises aft.** Lofted
hex rings, six corners being the fewest that read as round rather than as a
box when the back breaks the surface. The centre-line lifting toward the
tail gives the leap its curve even though the mesh itself is straight.

**Origin at the middle of the body.** The game pitches the model about its
origin to follow the arc of a leap, so the origin has to sit where a real
dolphin bends — amidships — or the nose would sweep and the tail would hang.

**It faces +Y in this file**, which the +Y-up export turns into Bevy's -Z
forward, the boat's own convention.

Where a pod *swims* is the game's business, in `crates/game/src/wildlife.rs` —
decorative wildlife is client-side, so nothing about this model or its placing
crosses the wire.
