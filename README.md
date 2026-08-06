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
comments in the `world` crate: the terrain without the engine, and without a
renderer to draw it.
[`crates/world/src/terrain.rs`](crates/world/src/terrain.rs) is one island;
[`crates/world/src/archipelago.rs`](crates/world/src/archipelago.rs) is the
ocean of them;
[`crates/world/src/noise.rs`](crates/world/src/noise.rs) is a dependency-free
Perlin implementation, kept in-tree so a seed always produces the same world.
Nothing in the game crate imports any of it — see below.

## Playing together

Every world is a served world. The server generates the ocean and hands it out
a chunk at a time; a client asks for the chunks near its camera and draws what
comes back, and is told nothing else — not the seed, not the layout, not which
chunks are worth asking for. An answer is either open water, which carries no
data at all, or ground, which arrives as corner heights and one palette entry
per triangle.

That leaves the client small enough to be worth rewriting in another language
against the protocol's documentation alone, which is the point of the
arrangement: it generates nothing, so there is nothing in it that has to
reproduce every noise octave and every rounding to the bit. Determinism still
matters, but only on the server — a seed handed to a different machine to host
has to raise the same islands.

A world started from the menu can be kept or shared, and both run a server:
the game hosts one and joins it over the loopback exactly as anyone else joins
it over the network, so there is no second, quieter kind of session for playing
alone. Sharing only decides who else can reach it. Others get in from the
menu's join screen, or straight from the command line:

```bash
cargo run -- --join localhost
```

The same world can be hosted without anyone playing on that machine, which is
what a dedicated server is:

```bash
cargo run --bin server -- --seed 7
```

Everyone enters the world in the same place — afloat just off the coast of
the same island — and other players appear as coloured markers. The wire is
defined once, in the [`protocol`](crates/protocol/src/lib.rs) crate, which is
the whole of what a client has to understand: the words of a session, the grid
a chunk of ground is drawn on, and the small palette it is painted from.

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
pick the world, and `--focus`, `--zoom` and `--yaw` to say where the player
is put down and how the view opens on them. Run `cargo run -- --help` for the
details. Without `--seed` the run
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
| `generator_cost` | the one-off cost of fitting an island's generator — what a server pays the first time anyone approaches an island |
| `arrival_cost` | what one client's arrival costs a server: the chunks within a streaming radius of where a world is entered, split into ground and open water, with the megabytes and the milliseconds |

## Tests

```bash
cargo test
```

Covers terrain generation invariants, the coast, mesh chunking, the camera
maths and the menu state transitions — both crates, since the workspace runs
them together.

Two things the tests cannot see are worth checking after moving anything
between crates. Bevy must appear nowhere below the game, which is what lets a
server run headless:

```bash
cargo tree --workspace --invert bevy
```

And the game must not depend on the world crate at all — it generates nothing,
and reaches the world only through the server it may be hosting. Its direct
dependencies should be `args`, `bevy`, `protocol` and `server`:

```bash
cargo tree -p game --depth 1
```

## Licence

GPL-3.0 — see [LICENSE](LICENSE).
