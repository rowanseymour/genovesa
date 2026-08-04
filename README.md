# Genovesa

An experiment in procedural 3D terrain, built with [Bevy](https://bevy.org).
Where it goes is undecided.

![Sixteen generated islands](docs/maps.png)

*Sixteen maps of assorted shapes — from single-chunk islets 128 m across to
1.5 km continents — all drawn to one scale, rendered by `mapgen collage`.*

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

## The terrain, briefly

Every map is an island: a height field of layered Perlin noise, domain-warped so
the coastlines meander, with one world unit one metre and sea level at zero.
Noise gives the shape, and per-seed fitting gives the proportions — how much
land, how much mountain, how tall the peaks — so seeds vary in character without
any of them coming out drowned or absurd. Coasts divide into beaches, rocky
shores and cliffs, with the landform and the colours reading the same field so
that a beach is always flat and sandy and a cliff steep and grey.

The look is flat-shaded facets in a small fixed palette — no textures and no
gradients anywhere. The mesh is built in 128 m chunks, drawn coarser than the
height field is sampled, so the facets read as deliberate shapes. A map is any
number of chunks along each axis — square or not, from a single chunk up.

A bigger map means more landscape, not stretched landscape: wavelengths are
fixed in metres, so a large map holds more ranges, more coast and more inland
water rather than larger ones. And a small map only holds what fits — by the
smallest, a low green islet rather than a shrunken alp.

The full story — every constant, and why it is what it is — lives in the
comments in [`crates/world/src/terrain.rs`](crates/world/src/terrain.rs), part of
the `world` crate: the terrain without the engine, so the same maps can
be generated without Bevy along for the ride.
[`crates/world/src/noise.rs`](crates/world/src/noise.rs) is a dependency-free
Perlin implementation, kept in-tree so a seed always produces the same map.

## Looking at maps

Judging the generator means seeing many maps from above, not walking around
one. The `mapgen` binary renders them in plan straight to PNG, with no window
and no GPU:

```bash
cargo run --release --bin mapgen -- grid
```

| Command | What it draws |
| --- | --- |
| `grid` | nine seeds per map shape — squares, rectangles and the single-chunk map — all at one scale, a file each; the view a generator change gets judged on |
| `map` | one map on its own, for looking hard at a single seed |
| `collage` | the image at the top of this page |

Options: `--size` (metres, `1024` or `1536x1024`), `--seed`, `--scale` in
metres per pixel, and `--out`. For `grid` and `collage`, which draw many maps
at once, `--seed` is the seed the whole set is spread from. Run
`mapgen --help` for the details.

The collage at the top of this page is redrawn by
[`tools/readme-collage.sh`](tools/readme-collage.sh), which runs `mapgen` and
quantises the result through ffmpeg. It takes the seed, so trying a few and
keeping the one you like is just running it again:

```bash
tools/readme-collage.sh 7
```

## Development helpers

The app takes options too — `--state` to open on a given screen, `--size` and
`--seed` to set the map up, and `--focus`, `--zoom` and `--yaw` to say where
the camera starts. Run `cargo run -- --help` for the details.

`--shot` writes a PNG of the view instead of waiting to be looked at. It can be
given as many times as you like: the view options are read left to right, so
each shot is the view as the options before it have left it.

```bash
cargo run -- --seed 7 --focus 98,-317 --yaw 45 \
  --zoom 120 --shot near.png \
  --zoom 340 --shot far.png \
  --yaw 225 --shot behind.png
```

That is one process and one generated map, so the pictures are all of the same
world and worth comparing against each other. Shots are rendered off screen —
no window opens, and the run quits when the last one is written.

Four `#[ignore]`-d tests measure rather than draw, run like:

```bash
cargo test --release island_shape -- --ignored --nocapture
```

| Test | What it shows |
| --- | --- |
| `island_shape` | per-seed numbers: land and mountain shares, peak height, slopes, coastline |
| `shore_mix` | how each seed's waterline divides between beach, rocky shore and cliff |
| `generator_cost` | the one-off cost of fitting a map's generator, before any mesh is built |
| `mesh_build_cost` | generation cost per map size |

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
