# Working in this repo

[README.md](README.md) covers what the thing is and how to run it. This is the
part that isn't obvious from reading the code.

## Backwards compatibility is not a goal

Nothing here has users to keep faith with, so nothing is owed a migration path.
Make the change that leaves the code looking as though the old shape had never
existed:

- Rename freely. Do not leave an alias behind.
- Change a signature rather than adding a second function beside it.
- Delete a feature rather than gating it off.
- No `#[deprecated]`, no shims, no "kept for compatibility" branches, no
  version checks on our own types.

If a change is worth making, it is worth making everywhere in the same commit.

Two things look like compatibility and are not. Both are about two *machines*
agreeing with each other right now, not about today agreeing with last year:

- **The digest tests** in `world` (`a_seed_is_the_same_map_down_to_the_bit`,
  `a_seed_is_the_same_world_down_to_the_bit`) pin what a seed generates. When
  you meant to change the generator, re-record them — run with `--nocapture`
  and they print what to paste. When they fail and you did not touch the
  generator, a platform or toolchain has stopped agreeing, and that is a real
  bug. Never "fix" a red digest by loosening the assertion.
- **`PROTOCOL_VERSION`** in `protocol` guards the wire, whose exact bytes are
  pinned by `the_wire_is_a_format`. Changing any encoding means bumping the
  version, not re-recording the test — a server has to be able to refuse a
  client built from another checkout.

## A seed is a world, on every machine

The whole design leans on this: terrain never crosses the wire, so a server
sends a seed and the clients regenerate the same ocean bit for bit. In the
`world` crate that means arithmetic has to be identical everywhere:

- Never `f32::powf` — use `terrain::pow`, which is MUSL's, ported. The
  platform libms disagree in the last bit and that is enough to put a player
  on different ground. A polynomial is better still where one will do.
- Nothing may depend on iteration order of a `HashMap`, on time, or on
  addresses.
- Add, multiply and square root are pinned by IEEE 754 and are safe.

CI runs the engine-free crates on macOS and Windows for exactly this reason. A
test that only ever runs on one machine has never done this job.

## Crate boundaries

`game` is the only crate that may see Bevy. `world`, `protocol` and `server`
build without it, so a headless server or a wasm build never drags an engine
in. The `bevy` feature on `world` is derives and nothing else.

Ordinary `cargo test` builds `world` with that feature on, so the engine-free
build is the one thing it never compiles — check it after touching the crate
(README has the command).

## House style

- Comments explain **why**, at length where it earns it, including approaches
  that were tried and failed. The constants in `terrain.rs` are the reference:
  a number without the reasoning behind it is a number nobody can ever change.
- A doc comment belongs to the item under it. When moving code, move its doc
  with it — three of them had come adrift and were documenting the wrong
  functions.
- Commit messages are a plain sentence saying what changed, then prose on why.
  Look at `git log` before writing one.
- Player-facing text describes the place, not the machinery. No talk of
  generation or noise in on-screen copy.
- British spelling in prose and comments.

## Judging a change to the generator

Walking around one map says almost nothing. Render nine seeds at a glance
(`mapgen grid`) and look at those — it is very easy to tune a constant until
one favourite map improves and eight others quietly get worse.

Two things that look like bugs and are wanted:

- Map size is a continuous dial, and the open world draws every size, so
  behaviour must never step at a particular size. Tapers are smoothsteps for
  this reason.
- Small maps missing the snow line entirely is the intent. Snow is a reward
  for a map with room for a real mountain, not something every map gets.
