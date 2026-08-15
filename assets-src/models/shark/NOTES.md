# The shark

Why the shape is what it is; the numbers are in the file and the rules are in
the tests.

- **A fin, with enough shark under it.** What the player is shown for most of
  its life is the dorsal fin cutting the water; the body pays off the moment
  the water goes clear or the camera swings low.
- **A touch over life size.** A reef shark, not a monster: big enough to read
  from the camera's height, small enough that *this coast has teeth* is a
  promise the game can keep.
- **The dorsal fin is taller and more upright than a real one's.** It is the
  part that breaks the surface and has to read at a hundred metres — and raked
  would read as a dolphin, which is the cut that model's notes make from the
  other side.
- **The tail is vertical, upper lobe long**: horizontal flukes read as a
  dolphin from every angle that matters.
- **Six-corner rings with a vertex on the ridge line**, so the back carries an
  edge for the fin to sit on.
- **A dark back and a pale belly.** The back was sand-grey once, on the
  argument that a shark over sunlit sand reads best pale. That was backwards:
  pale over pale sand is camouflage, and what should be moving in the shallows
  is a shadow. Dark enough to read as one even through the water it is seen
  through, which lightens it further. The belly was argued away too — the
  camera sees a shark from above, so an underside is a view that barely
  exists — but banking in clear water is exactly when one is looked at.
- **The mesh name `hide` is load-bearing**: the game paints by name, through
  `TONES` in `crates/game/src/beasts.rs`.
- **Origin amidships between the pectorals**, where a shark turns, since the
  game yaws the whole body about it.

## The rig

Three bones — `body`, `tail`, `caudal` — with every vertex on exactly one at
full weight. A shared vertex bends the facet it sits on, and a gradient across
a facet is the one thing this look cannot have. Only the propelling end
articulates; the fins ride `body`.

`swim` is the whole dope sheet: a lateral wave nose to tail, each joint lagging
the one ahead, which is thunniform swimming and reads as such at any distance.
Unlike the player's run it is not seeked by ground covered — the game plays it
on its own clock and scales the rate with the water going by, so a shark pushed
faster swishes faster rather than gliding like a submarine.
