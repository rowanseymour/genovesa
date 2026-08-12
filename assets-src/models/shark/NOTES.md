# The shark

What the shape is *for*, which a `.blend` has nowhere to say. The numbers are
in the file; these are the reasons behind the ones that are not obvious.

A shark is seen as a fin. It lives in the shallows off a coast, body a metre
under the surface, and what the player is shown for most of its life is the
dorsal fin cutting the water — so this model is a fin with enough shark under
it to pay off the moment the water goes clear or the camera swings low. It is
also the first *beast*: a creature the server owns and the wire carries,
because it will one day act on a player, where the wildlife only decorates.
Where it swims is `crates/server/src/beasts.rs`; what it looks like doing so
is `crates/game/src/beasts.rs`.

**A touch over life size, 2.6 m nose to tail.** A reef shark, not a monster:
big enough to read from the camera's height, small enough that the promise it
makes — this coast has teeth — is one the game can keep with a shark-sized
shark.

**One mesh, `hide`, one tone.** The camera sees a shark from above, through
water; a pale belly would be modelling for a view that does not exist. The
game paints the mesh by name — see `TONES` in `crates/game/src/beasts.rs` —
so the name is load-bearing, exactly as the player's `coat` is.

**The dorsal fin is tall and nearly upright.** The tip stands 0.65 m over the
spine, which is deliberately more fin than a real reef shark carries: it is
the one part that breaks the surface, and it has to read as a shark's fin at
a hundred metres. Upright rather than raked, because a raked fin is a
dolphin's — that model's notes make the same cut from the other side.

**The tail is vertical, upper lobe long.** The heterocercal caudal fin is the
other half of the species: horizontal flukes read as a dolphin from every
angle that matters, so a vertical tail says shark even as a silhouette in a
wave.

**Girth peaks forward of the middle and the peduncle pinches.** Hex-ring
loft like the dolphin's, six corners being the fewest that read as round,
with a vertex on the ridge line so the back carries an edge for the fin to
sit on.

**Origin amidships, spine at Z zero.** The game holds the origin at cruising
depth and yaws the whole body about it, so the origin sits where a shark
turns — between the pectorals, not at the nose.

**It faces +Y in this file**, which the +Y-up export turns into Bevy's -Z
forward, the boat's own convention.

## The rig and the one action

Three bones — `body`, `tail`, `caudal` — and every vertex on exactly one of
them at full weight, the rigid-skin rule every rigged model here obeys: a
shared vertex bends the facet it sits on, and a gradient across a facet is
the one thing the flat-toned look cannot have. The dorsal and pectoral fins
ride `body`; only the propelling end articulates.

`swim` is the whole dope sheet: one second, keyed at the quarters, first
frame matching last so it loops. A lateral wave travels nose to tail — the
body works a few degrees, the tail more, the caudal fin most, each lagging
the one ahead — which is thunniform swimming and reads as such from any
distance. Unlike the player's run it is *not* seeked by ground covered: the
game plays it on its own clock and scales the playback rate with how fast
the water is going by, so a shark pushed faster one day swishes faster
rather than gliding like a submarine.

`build.py` beside this file is the script the master was first raised by,
kept as the record of the numbers. The `.blend` is still the master: if it
has been edited by hand since, the script is history rather than truth, so
do not re-run it over the file.
