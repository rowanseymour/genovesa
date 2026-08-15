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

**It carries its own colours, and they are countershaded.** The back keeps the
wet slate the client used to hold, (0.42, 0.50, 0.55) — lighter than the deep
sea it breaks out of and darker than the spray-white a leap suggests, so the
arc reads against the water at the distances pods keep. The belly is
(0.80, 0.81, 0.82).

Which facets are belly is decided by the way each one faces rather than by how
high it sits. The porpoising is what it is for: a pod leaves the water on its
side as often as level, and a dolphin that was one colour all over lost the
whole shape of the leap.

Where a pod *swims* is the server's word and the wire carries it — a pod is
a *beast* now, one creature every player can point at — while what it looks
like doing so stays the client's, in `crates/game/src/beasts.rs`: the member
count, the stations and the porpoising are all drawn there from the pod's id.
