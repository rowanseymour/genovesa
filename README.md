# Kassiter

An experiment in procedural 3D terrain, built with [Bevy](https://bevy.org).
Where it goes is undecided.

![Sixteen generated islands](docs/maps.png)

*Sixteen maps of assorted shapes — from single-chunk islets 128 m across to
1.5 km continents — all drawn to one scale, rendered in plan by the
`readme_grid` test.*

## Running

```bash
cargo run
```

The first build compiles all of Bevy and takes several minutes. For faster
iteration afterwards:

```bash
cargo run --features dev
```

## Controls

| Input | Action |
| --- | --- |
| Arrow keys / WASD | Pan the camera |
| Q / E | Turn the view a quarter turn left / right |
| Mouse wheel / trackpad | Zoom |
| Escape | Back to the main menu |

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
comments in [`src/terrain.rs`](src/terrain.rs). [`src/noise.rs`](src/noise.rs)
is a dependency-free Perlin implementation, kept in-tree so a seed always
produces the same map.

## Development helpers

Environment variables, all optional:

| Variable | Effect |
| --- | --- |
| `KASSITER_STATE` | `newmap` or `inworld` — start on that screen |
| `KASSITER_SIZE` | Initial map size in metres — `1024` or `1536x1024` — rounded to whole 128 m chunks; also narrows `plan_grid` to that one shape |
| `KASSITER_SEED` | Initial map seed |
| `KASSITER_ZOOM` | Initial camera distance in metres |
| `KASSITER_FOCUS` | `x,z` in metres from the map centre — start the camera there |
| `KASSITER_SCREENSHOT` | Render ~270 frames, save a PNG to this path, exit |
| `KASSITER_DUMP` | Where the plan renders below write their PPM |
| `KASSITER_BATCH` | Which seeds the grid renders draw |

Six `#[ignore]`-d tests render or measure what the generator produces, run like:

```bash
KASSITER_DUMP=grid.ppm cargo test --release plan_grid -- --ignored --nocapture
```

| Test | What it shows |
| --- | --- |
| `plan_grid` | nine seeds in plan, one grid per map shape — squares, rectangles and the single-chunk map — at a constant 3 m/px; the tool for judging a generator change |
| `plan_view` | one map in plan, honouring `KASSITER_SIZE` / `KASSITER_SEED` |
| `readme_grid` | the image at the top of this page |
| `island_shape` | per-seed numbers: land and mountain shares, peak height, slopes, coastline |
| `shore_mix` | how each seed's waterline divides between beach, rocky shore and cliff |
| `mesh_build_cost` | generation cost per map size |

The renders write binary PPM; convert with
`sips -s format png <in>.ppm --out <out>.png` on macOS.

## Tests

```bash
cargo test
```

Covers terrain generation invariants, the coast, mesh chunking, the camera
maths and the menu state transitions.

## Licence

GPL-3.0 — see [LICENSE](LICENSE).
