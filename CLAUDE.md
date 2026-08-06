# Working in this repo

[README.md](README.md) says what this is and how to run it.

## Backwards compatibility is not a goal

Nothing here has users to keep faith with. Make the change that leaves the code
as though the old shape had never existed: rename freely, change signatures in
place, delete rather than gate off. No aliases, no shims, no `#[deprecated]`.
If something is worth changing, change it everywhere in the same commit.

## But machines must agree with each other

The ground crosses the wire now — a server generates the world and hands out
chunks — so no client is betting on its own arithmetic. Two things still have
to agree across machines, and neither pin is legacy baggage to be filed off:

- **A seed must survive being re-hosted.** Hand seed 20040112 to a different
  machine to serve and it has to raise the same islands, or a shared seed and a
  server moved between hosts quietly mean somewhere else. That is what **the
  digest tests** in `world` are for. If you meant to change the generator,
  re-record them — `--nocapture` prints the new values. If you didn't touch it
  and they go red, a platform has stopped agreeing about what a seed means,
  which is a real bug. Never answer a red digest by loosening the assertion.
- **Two builds must still be able to talk.** That is **`PROTOCOL_VERSION`**:
  changing an encoding means bumping it, not re-recording
  `the_wire_is_a_format`. The wire is wider than it looks — the chunk grid, the
  height quantisation and the palette are all part of it, because a client
  draws the ground out of them.

So inside `world`: no `f32::powf` (use `terrain::pow`), and nothing may depend
on time, addresses or `HashMap` order.

## The client is meant to be replaceable

`game` generates nothing and knows nothing about how the world is made: it asks
for chunks and draws the answers. Keep it that way — it should stay portable to
another language against `protocol`'s documentation alone. Concretely, `game`
must not depend on `world`, and anything a client needs in order to *draw*
belongs in `protocol` rather than being recomputed either side.

## Odds and ends

- Only `game` may see Bevy. After moving things between crates, check both that
  and the paragraph above (README has the two commands).
- Comments here explain *why*, including approaches that were tried and
  failed. Match that rather than the density of the surrounding language.
- Judge a change to the generator on nine seeds (`mapgen grid`), never on one
  favourite map.
