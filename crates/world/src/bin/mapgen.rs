//! Renders maps in plan, straight to PNG, without opening a window.
//!
//! The generator is the part of the app worth looking at hardest, and looking
//! at it means seeing many maps from above rather than walking around one. This
//! binary is that view: no window, no GPU, no app state — just the height field
//! and the same colour function the mesh uses. It ships with the generator
//! rather than with the game so that none of the engine is even linked in.
//!
//! ```sh
//! cargo run --release --bin mapgen -- grid
//! cargo run --release --bin mapgen -- map --size 1536x1024 --seed 99
//! cargo run --release --bin mapgen -- collage --out collage.png
//! cargo run --release --bin mapgen -- world --seed 7 --span 8192
//! cargo run --release --bin mapgen -- map --seed 7 --layer country
//! ```
//!
//! What a map is painted *with* is a layer — see [`Layer`]. The default draws
//! the world as it looks; the rest draw decisions the generator makes and
//! nothing else could show, a country and a hollow being shapes rather than
//! numbers a test could hold.
//!
//! The collage that ships in the README is not written by hand from here:
//! `tools/readme-collage.sh` runs this binary and quantises the result on the
//! way to `docs/maps.png`.

use std::process::ExitCode;

use glam::{UVec2, Vec2};

use world::archipelago::{Archipelago, WorldConfig};
use world::plan::{self, Layer};
use world::terrain::{MapConfig, CHUNK_TILES};

/// The seed the grid and collage spread their set from when none is given.
/// The collage in the README is this one, so leaving it alone redraws the
/// picture that is already there.
const DEFAULT_SET_SEED: u32 = 1;

/// Built rather than written out so the two seed defaults it quotes are read
/// from the constants themselves and cannot drift.
fn usage() -> String {
    let map_seed = MapConfig::default().seed;
    // Folded over the table rather than written out, so a layer added there
    // is one this already advertises. The column is as wide as the longest
    // word plus a gutter, measured rather than counted by hand.
    let width = Layer::EVERY
        .iter()
        .map(|l| l.word().len())
        .max()
        .expect("a layer");
    let default_layer = Layer::Ground.word();
    let layers = Layer::EVERY
        .iter()
        .map(|l| format!("\n                     {:width$}  {}", l.word(), l.help()))
        .collect::<String>();
    format!(
        "\
Renders Genovesa maps in plan, as PNG.

Usage: mapgen <command> [options]

Commands:
  map        one map on its own, for looking hard at a single seed
  grid       nine seeds in a 3x3 grid, one file per map shape — the view a
             change to the generator gets judged on
  collage    the sixteen-map collage at the top of the README
  world      a region of the open world, for judging the island layout

Options:
  --size <W|WxD>   map size in metres, rounded to whole 128 m chunks
                   (map: the map's size; grid: render only this shape)
  --seed <n>       the seed to draw [default: {map_seed}]; for grid and
                   collage, which draw many maps, the seed the set of them is
                   spread from [default: {DEFAULT_SET_SEED}]
  --scale <m>      metres of ground per pixel (map, grid, world)
  --focus <x,z>    world point a `world` render is centred on [default: 0,0]
  --span <W|WxD>   metres of world a `world` render covers [default: 8192]
  --layer <name>   what to paint [default: {default_layer}]{layers}
  --out <path>     where to write; grids get one file per shape, each
                   suffixed with its size unless --size named just one
"
    )
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("mapgen: {message}");
            ExitCode::FAILURE
        }
    }
}

/// What the command line asked for.
struct Args {
    command: String,
    size: Option<UVec2>,
    /// One map's seed, or the seed a whole set is spread from.
    seed: Option<u32>,
    scale: Option<f32>,
    focus: Option<Vec2>,
    span: Option<Vec2>,
    layer: Layer,
    out: Option<String>,
}

fn run() -> Result<(), String> {
    let args = parse(std::env::args().skip(1).collect())?;
    match args.command.as_str() {
        "map" => map(&args),
        "grid" => grid(&args),
        "collage" => collage(&args),
        "world" => world(&args),
        other => Err(format!("unknown command `{other}`\n\n{}", usage())),
    }
}

fn parse(argv: Vec<String>) -> Result<Args, String> {
    if argv.iter().any(|a| a == "-h" || a == "--help") {
        print!("{}", usage());
        std::process::exit(0);
    }
    let command = argv
        .first()
        .ok_or_else(|| format!("no command given\n\n{}", usage()))?
        .clone();

    let mut parsed = Args {
        command,
        size: None,
        seed: None,
        scale: None,
        focus: None,
        span: None,
        layer: Layer::Ground,
        out: None,
    };

    // Every option here takes a value, so an option in the last position is
    // always a missing value rather than a flag that stands on its own.
    let mut rest = argv[1..].iter();
    while let Some(flag) = rest.next() {
        let value = rest
            .next()
            .ok_or_else(|| format!("`{flag}` needs a value"))?;
        match flag.as_str() {
            "--size" => {
                parsed.size = Some(MapConfig::parse_size(value).ok_or_else(|| {
                    format!("`{value}` is not a size in metres, e.g. 1024 or 1536x1024")
                })?);
            }
            "--seed" => {
                parsed.seed = Some(
                    value
                        .parse()
                        .map_err(|_| format!("`{value}` is not a seed"))?,
                )
            }
            "--scale" => {
                let scale: f32 = value
                    .parse()
                    .map_err(|_| format!("`{value}` is not a scale"))?;
                if !scale.is_finite() || scale <= 0.0 {
                    return Err("--scale must be more than zero metres per pixel".into());
                }
                parsed.scale = Some(scale);
            }
            "--layer" => {
                parsed.layer = Layer::from_word(value).ok_or_else(|| {
                    let words: Vec<_> = Layer::EVERY.iter().map(|l| l.word()).collect();
                    format!("`{value}` is not a layer — try {}", words.join(", "))
                })?
            }
            "--focus" => parsed.focus = Some(focus(value)?),
            "--span" => parsed.span = Some(span(value)?),
            "--out" => parsed.out = Some(value.clone()),
            other => return Err(format!("unknown option `{other}`\n\n{}", usage())),
        }
    }
    Ok(parsed)
}

/// A number of metres that is somewhere.
///
/// `inf` and `nan` both parse happily as floats and neither is a place: a
/// render walks out from the point it is given, so a non-finite one samples
/// the world at non-finite coordinates and writes a picture of nothing,
/// having said nothing about it.
fn metres(value: &str) -> Option<f32> {
    value.parse::<f32>().ok().filter(|m| m.is_finite())
}

/// Reads a two-part option as both halves parsed the same way, or one error
/// naming what it should have looked like. If either half is nonsense the
/// whole option is — an option that took half of what it was given would be
/// worse than one that is refused.
fn halves(
    value: &str,
    first: &str,
    second: &str,
    axis: impl Fn(&str) -> Option<f32>,
    expected: &str,
) -> Result<(f32, f32), String> {
    let bad = || format!("`{value}` is not {expected}");
    Ok((
        axis(first.trim()).ok_or_else(bad)?,
        axis(second.trim()).ok_or_else(bad)?,
    ))
}

/// The separator is required — see [`pair_or_single`] for where it is not.
fn pair(
    value: &str,
    separator: char,
    axis: impl Fn(&str) -> Option<f32>,
    expected: &str,
) -> Result<(f32, f32), String> {
    let (first, second) = value
        .split_once(separator)
        .ok_or_else(|| format!("`{value}` is not {expected}"))?;
    halves(value, first, second, axis, expected)
}

/// The same, except that a value with no separator in it stands for both
/// halves — `8192` for a square span.
fn pair_or_single(
    value: &str,
    separator: char,
    axis: impl Fn(&str) -> Option<f32>,
    expected: &str,
) -> Result<(f32, f32), String> {
    let (first, second) = value.split_once(separator).unwrap_or((value, value));
    halves(value, first, second, axis, expected)
}

/// Reads the `x,z` world point a `world` render is centred on, in metres.
/// Finite, for the reason [`metres`] gives.
fn focus(value: &str) -> Result<Vec2, String> {
    let (x, z) = pair(value, ',', metres, "a point, e.g. 2000,-3000")?;
    Ok(Vec2::new(x, z))
}

/// Reads how much world a `world` render covers, in metres — `8192` for a
/// square span, `8192x4096` for a rectangle, as [`MapConfig::parse_size`]
/// takes a map's own size.
///
/// Finite *and* positive, and the greater-than-zero test cannot stand in for
/// the first: `inf` is greater than zero, and an infinite span divides out to
/// an infinite scale and a picture one pixel tall.
fn span(value: &str) -> Result<Vec2, String> {
    let (w, d) = pair_or_single(
        value,
        'x',
        |s| metres(s).filter(|m| *m > 0.0),
        "a span in metres, e.g. 8192",
    )?;
    Ok(Vec2::new(w, d))
}

/// Writes an image out, reporting where it went and how big it is.
fn write(image: &plan::Image, path: &str, what: std::fmt::Arguments) -> Result<(), String> {
    image
        .write_png(path)
        .map_err(|e| format!("could not write {path}: {e}"))?;
    println!("{what} -> {path} ({}x{})", image.width, image.height);
    Ok(())
}

/// One map on its own. Metre-per-pixel up to a thousand-odd pixels, then
/// coarser, unless a scale was asked for.
fn map(args: &Args) -> Result<(), String> {
    let default = MapConfig::default();
    let config = MapConfig {
        chunks: args.size.unwrap_or(default.chunks),
        seed: args.seed.unwrap_or(default.seed),
    };
    let tiles = config.tiles();
    let scale = args
        .scale
        .unwrap_or_else(|| (tiles.x.max(tiles.y) as f32 / 1024.0).max(1.0));

    let image = plan::render_at_scale(&config, scale, args.layer);
    let path = args.out.clone().unwrap_or_else(|| "map.png".into());
    write(
        &image,
        &path,
        format_args!(
            "{}x{} m  seed {}  {scale} m/px",
            tiles.x, tiles.y, config.seed
        ),
    )
}

/// Nine seeds per shape — every shape in [`plan::GRID_SHAPES`], unless one was
/// asked for by name.
fn grid(args: &Args) -> Result<(), String> {
    let base = args.seed.unwrap_or(DEFAULT_SET_SEED);
    let seeds = plan::seed_set(base, 9);
    let scale = args.scale.unwrap_or(plan::GRID_METRES_PER_PIXEL);
    let path = args.out.clone().unwrap_or_else(|| "grid.png".into());

    println!("seed {base}, reading left to right, top to bottom:");
    for row in seeds.chunks(3) {
        println!("  {row:?}");
    }

    let shapes: Vec<UVec2> = args
        .size
        .map_or_else(|| plan::GRID_SHAPES.to_vec(), |s| vec![s]);
    let named = shapes.len() == 1;

    for chunks in shapes {
        let image = plan::grid(chunks, &seeds, scale, args.layer);
        let extent = chunks * CHUNK_TILES;
        // One file per shape, named for it — except when a shape was asked
        // for outright, where the path given is the path meant.
        let path = if named {
            path.clone()
        } else {
            suffixed(&path, &format!("{}x{}", extent.x, extent.y))
        };
        write(
            &image,
            &path,
            format_args!(
                "  {}x{} chunks  {:>5}x{} m  {scale} m/px",
                chunks.x, chunks.y, extent.x, extent.y
            ),
        )?;
    }
    Ok(())
}

/// The README collage.
fn collage(args: &Args) -> Result<(), String> {
    let base = args.seed.unwrap_or(DEFAULT_SET_SEED);
    let seeds = plan::seed_set(base, plan::COLLAGE_SEEDS);
    let image = plan::collage(&seeds, args.layer);
    let path = args.out.clone().unwrap_or_else(|| "collage.png".into());
    write(
        &image,
        &path,
        format_args!("collage of {} maps from seed {base}", seeds.len()),
    )
}

/// A region of the open world around a point, at a scale coarse enough to
/// take in the layout — many islands and the ocean between them.
fn world(args: &Args) -> Result<(), String> {
    let config = WorldConfig {
        seed: args.seed.unwrap_or_else(|| WorldConfig::default().seed),
    };
    let focus = args.focus.unwrap_or(Vec2::ZERO);
    let span = args.span.unwrap_or(Vec2::splat(8192.0));
    let scale = args.scale.unwrap_or_else(|| (span.x / 2048.0).max(1.0));

    let world = Archipelago::new(&config);
    let image = plan::render_region(
        &world,
        focus,
        span,
        (span.x / scale).round().max(1.0) as u32,
        args.layer,
    );
    let path = args.out.clone().unwrap_or_else(|| "world.png".into());
    write(
        &image,
        &path,
        format_args!(
            "world seed {}  {}x{} m around ({}, {})  {scale} m/px",
            config.seed, span.x, span.y, focus.x, focus.y
        ),
    )
}

/// `grid.png` and `1536x1024` -> `grid-1536x1024.png`.
fn suffixed(path: &str, tag: &str) -> String {
    match path.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem}-{tag}.{ext}"),
        None => format!("{path}-{tag}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_focus_is_a_finite_pair_of_metres() {
        assert_eq!(focus("2000,-3000"), Ok(Vec2::new(2000.0, -3000.0)));
        assert_eq!(focus(" 2000 , -3000 "), Ok(Vec2::new(2000.0, -3000.0)));

        // Both axes, and both of them a number that is somewhere. `inf` and
        // `nan` parse as floats and used to be taken, which drew a picture of
        // the world at coordinates no world has.
        for bad in ["2000", "north,south", "1,2,3", "inf,0", "0,nan", "-inf,0"] {
            let why = focus(bad).expect_err("`{bad}` was accepted as a point");
            // Naming both what was given and what was wanted, since the whole
            // of what a caller sees is this line on stderr.
            assert!(
                why.contains(bad) && why.contains("a point"),
                "`{bad}` gave an error naming neither it nor what was wanted: {why}"
            );
        }
    }

    #[test]
    fn a_span_is_a_finite_stretch_of_world() {
        assert_eq!(span("8192"), Ok(Vec2::splat(8192.0)));
        assert_eq!(span("8192x4096"), Ok(Vec2::new(8192.0, 4096.0)));

        // Zero and back-to-front are obvious; `inf` is the one the
        // greater-than-zero test alone let through, and it rendered as a
        // single row of pixels.
        for bad in ["0", "-5", "8192xzero", "inf", "8192xinf", "nan"] {
            assert!(span(bad).is_err(), "`{bad}` was accepted as a span");
        }
    }
}
