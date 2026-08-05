# Genovesa

An experiment in procedural 3D terrain, built with [Bevy](https://bevy.org):
an endless ocean scattered with generated islands. Where it goes is
undecided.

![Sixteen generated islands](docs/maps.png)

*Sixteen islands of assorted shapes — from single-chunk islets 128 m across
to 1.5 km continents — all drawn to one scale, rendered by `mapgen collage`.*

## Running

```bash
cargo run
```

The first build compiles all of Bevy and takes several minutes. For faster
iteration afterwards:

```bash
cargo run --features dev
```

There is a second binary, `mapgen`, which renders maps from above as PNG
without opening a window — see [Looking at maps](#looking-at-maps).

## The world, briefly

The world is an infinite plane of ocean with islands scattered across it, and
a seed is a whole world: the layout of the islands and the terrain of every
one of them follow from it deterministically, wherever you sail and in
whatever order you get there. Islands come in all sizes, from lone islets to
continents kilometres across, laid out so that the next island is usually a
short sail away and a big one is an occasional event. Nothing is generated up
front — the ground near the camera streams in as it is approached, and open
ocean between islands costs nothing at all.

Each island is a height field of layered Perlin noise, domain-warped so the
coastlines meander, with one world unit one metre and sea level at zero.
Noise gives the shape, and per-island fitting gives the proportions — how
much land, how much mountain, how tall the peaks — so islands vary in
character without any of them coming out drowned or absurd. Coasts divide
into beaches, rocky shores and cliffs, with the landform and the colours
reading the same field so that a beach is always flat and sandy and a cliff
steep and grey.

The look is flat-shaded facets in a small fixed palette — no textures and no
gradients anywhere. The mesh is built in 128 m chunks, drawn coarser than the
height field is sampled, so the facets read as deliberate shapes.

A bigger island means more landscape, not stretched landscape: wavelengths
are fixed in metres, so a large island holds more ranges, more coast and more
inland water rather than larger ones. And a small one only holds what fits —
by the smallest, a low green islet rather than a shrunken alp.

The full story — every constant, and why it is what it is — lives in the
comments in the `world` crate: the terrain without the engine, so the same
worlds can be generated without Bevy along for the ride.
[`crates/world/src/terrain.rs`](crates/world/src/terrain.rs) is one island;
[`crates/world/src/archipelago.rs`](crates/world/src/archipelago.rs) is the
ocean of them;
[`crates/world/src/noise.rs`](crates/world/src/noise.rs) is a dependency-free
Perlin implementation, kept in-tree so a seed always produces the same world.

## Playing together

Determinism is what makes the world shareable: since a seed is a whole world
on every machine, a server never sends terrain — it hands out the seed and
keeps track of who is in the world and where. Clients generate the same ocean
for themselves and meet in it.

A world started from the menu can be kept or shared. Sharing it hosts it, and
the game then joins its own server over the loopback exactly as anyone else
joins it over the network — there is no second, quieter kind of session for
playing alone in a world of your own. Others get in from the menu's join
screen, or straight from the command line:

```bash
cargo run -- --join localhost
```

The same world can be hosted without anyone playing on that machine, which is
what a dedicated server is:

```bash
cargo run --bin server -- --seed 7
```

Everyone joins on the same island, and other players appear as coloured
markers standing on the ground. The wire itself is defined once, in the
`protocol` crate, and shared by both sides.

## Looking at maps

Judging the generator means seeing many islands from above, not walking
around one. The `mapgen` binary renders them in plan straight to PNG, with no
window and no GPU:

```bash
cargo run --release --bin mapgen -- grid
```

| Command | What it draws |
| --- | --- |
| `grid` | nine seeds per island shape — squares, rectangles and the single-chunk islet — all at one scale, a file each; the view a generator change gets judged on |
| `map` | one island on its own, for looking hard at a single seed |
| `collage` | the image at the top of this page |
| `world` | a region of the open world — many islands and the ocean between them; the view a *layout* change gets judged on |

Options: `--size` (metres, `1024` or `1536x1024`), `--seed`, `--scale` in
metres per pixel, and `--out`; for `world`, `--focus` and `--span` say where
and how much. For `grid` and `collage`, which draw many islands at once,
`--seed` is the seed the whole set is spread from. Run `mapgen --help` for
the details.

The collage at the top of this page is redrawn by
[`tools/readme-collage.sh`](tools/readme-collage.sh), which runs `mapgen` and
quantises the result through ffmpeg. It takes the seed, so trying a few and
keeping the one you like is just running it again:

```bash
tools/readme-collage.sh 7
```

## Development helpers

The app takes options too — `--state` to open on a given screen, `--seed` to
pick the world, and `--focus`, `--zoom` and `--yaw` to say where the camera
starts. Run `cargo run -- --help` for the details. Without `--seed` the run
picks a world of its own and prints which, so a place worth going back to can
be asked for by name.

`--shot` writes a PNG of the view instead of waiting to be looked at. It can be
given as many times as you like: the view options are read left to right, so
each shot is the view as the options before it have left it.

```bash
cargo run -- --seed 7 --focus 98,-317 --yaw 45 \
  --zoom 120 --shot near.png \
  --zoom 340 --shot far.png \
  --yaw 225 --shot behind.png
```

That is one process and one world, so the pictures are all of the same place
and worth comparing against each other. Shots are rendered off screen — no
window opens, and the run quits when the last one is written.

Four `#[ignore]`-d tests measure rather than draw, run like:

```bash
cargo test --release island_shape -- --ignored --nocapture
```

| Test | What it shows |
| --- | --- |
| `island_shape` | per-seed numbers: land and mountain shares, peak height, slopes, coastline |
| `shore_mix` | how each seed's waterline divides between beach, rocky shore and cliff |
| `generator_cost` | the one-off cost of fitting an island's generator — what streaming pays the first time an island is approached |
| `mesh_build_cost` | generation cost per island size |

## Tests

```bash
cargo test
```

Covers terrain generation invariants, the coast, mesh chunking, the camera
maths and the menu state transitions — both crates, since the workspace runs
them together.

That run always builds the world crate the way the game asks for it, with Bevy
on, so the engine-free build it exists for is the one thing it never compiles.
Check that separately after touching the world crate:

```bash
cargo check -p world --no-default-features
```

## Licence

GPL-3.0 — see [LICENSE](LICENSE).
