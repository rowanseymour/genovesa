# Working in this repo

[README.md](README.md) says what this is and how to run it.

## Backwards compatibility is not a goal

Nothing here has users to keep faith with. Make the change that leaves the code
as though the old shape had never existed: rename freely, change signatures in
place, delete rather than gate off. No aliases, no shims, no `#[deprecated]`.
If something is worth changing, change it everywhere in the same commit.

## But machines must agree with each other

A seed is a world: terrain never crosses the wire, so every machine has to
regenerate the same ocean bit for bit. Two pins protect that, and neither is
legacy baggage to be filed off:

- **The digest tests** in `world`. If you meant to change the generator,
  re-record them — `--nocapture` prints the new values. If you didn't touch it
  and they go red, a platform has stopped agreeing about what a seed means,
  which is a real bug. Never answer a red digest by loosening the assertion.
- **`PROTOCOL_VERSION`**. Changing an encoding means bumping it, not
  re-recording `the_wire_is_a_format`.

So inside `world`: no `f32::powf` (use `terrain::pow`), and nothing may depend
on time, addresses or `HashMap` order.

## Odds and ends

- Only `game` may see Bevy. After touching `world`, check it still builds
  without it (README has the command).
- Comments here explain *why*, including approaches that were tried and
  failed. Match that rather than the density of the surrounding language.
- Judge a change to the generator on nine seeds (`mapgen grid`), never on one
  favourite map.
