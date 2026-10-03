# Working in this repo

[README.md](README.md) says what this is.

## Nobody is playing this yet

One client runs over the loopback; the client/server split is a bet on company
later. So:

- **No backwards compatibility.** Rename freely, change signatures in place,
  delete rather than gate off — no aliases, shims or `#[deprecated]`.
- **Load is not an argument.** Justify a shape by what it lets a client do or
  what it stops the two ends disagreeing about, never by bytes.
- **Leave `PROTOCOL_VERSION` alone** unless you want old builds turned away.

What must hold is the shape of the split: the server owns the world, the client
draws what it is told, and nothing consequential is decided at the client.

## Machines must agree with each other

- **A seed must survive being re-hosted** — pinned by the digest tests in
  `world`. Changed the generator on purpose? Re-record (`--nocapture` prints
  the values). Didn't, and they're red? A platform disagrees about a seed; fix
  that, never loosen the assertion.
- **The wire is a format** — pinned by `the_wire_is_a_format`. It includes the
  chunk grid, height quantisation and palette. Re-record only when you meant it.

So inside `world`: no `f32::powf` (use `terrain::pow`), and nothing may depend
on time, addresses or `HashMap` order.

## The client stays thin

`game` generates nothing: it asks for chunks and draws the answers. It must not
depend on `world`; what a client needs to *draw* belongs in `protocol`. Only
`game` may see Bevy. After moving things between crates:

```bash
cargo tree --workspace --invert bevy
cargo tree -p game --depth 1
grep -rn '^pub use world' crates/server/src/
```

The head of [`crates/server/src/lib.rs`](crates/server/src/lib.rs) covers the
half no command catches.

## Running and debugging

- `cargo run --features dev` hot-reloads `assets/`.
- `--debug <port>` takes console lines on a socket; send `help` for the verbs.
  Each line answers only once its work is done, so a pipe is a script, not a
  race:

  ```bash
  cargo run -- --seed 7 --debug 7777 --headless
  printf 'goto 98 -317\nshot near.png\nquit\n' | nc 127.0.0.1 7777
  ```

- Measuring tests are `#[ignore]`d, e.g.
  `cargo test --release island_shape -- --ignored --nocapture`.
- Judge a generator change on nine seeds, never one favourite map:
  `cargo run --release --bin mapgen -- grid` (`--help` for other views).
- Models are exported from `assets-src/` by `assets-src/models/export.sh <name>`.
  They carry flat per-facet vertex colours (glTF materials are ignored), and
  skins must be rigid — one bone per vertex.

## Docs and comments

- Nothing in `docs/` is hand-written; `--help` and the socket's `help` are
  built from the code. Don't keep a list of options or verbs anywhere else.
- Commands are rows of a table that `help` and completion fold over. Prose in a
  `help` line, and hand-kept indexes like `MenuButton::EVERY`, need a test
  holding them to what they advertise.
- Comments say why, not what. A fact is written once, at its definition;
  link to it elsewhere. Long rationale goes in the module's `//!` header; an
  item's doc stays under about ten lines.

## The build

A `target/` is ~2.7G per worktree.

- The dev profile is tuned for size (`Cargo.toml` says why) — keep it.
- Never share a `CARGO_TARGET_DIR` between worktrees: they collide on the
  binary, and one worktree's `mapgen` silently draws another's world.
- Retire branches; removing a worktree takes its `target/` with it.
