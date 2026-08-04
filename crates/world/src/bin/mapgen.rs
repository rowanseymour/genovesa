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
//! ```
//!
//! The collage that ships in the README is not written by hand from here:
//! `tools/readme-collage.sh` runs this binary and quantises the result on the
//! way to `docs/maps.png`.

use std::process::ExitCode;

use glam::UVec2;

use world::plan;
use world::terrain::{MapConfig, CHUNK_TILES};

/// The seed the grid and collage spread their set from when none is given.
/// The collage in the README is this one, so leaving it alone redraws the
/// picture that is already there.
const DEFAULT_SET_SEED: u32 = 1;

/// Built rather than written out so the two seed defaults it quotes are read
/// from the constants themselves and cannot drift.
fn usage() -> String {
    let map_seed = MapConfig::default().seed;
    format!(
        "\
Renders Kassiter maps in plan, as PNG.

Usage: mapgen <command> [options]

Commands:
  map        one map on its own, for looking hard at a single seed
  grid       nine seeds in a 3x3 grid, one file per map shape — the view a
             change to the generator gets judged on
  collage    the sixteen-map collage at the top of the README

Options:
  --size <W|WxD>   map size in metres, rounded to whole 128 m chunks
                   (map: the map's size; grid: render only this shape)
  --seed <n>       the seed to draw [default: {map_seed}]; for grid and
                   collage, which draw many maps, the seed the set of them is
                   spread from [default: {DEFAULT_SET_SEED}]
  --scale <m>      metres of ground per pixel (map, grid)
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
    out: Option<String>,
}

fn run() -> Result<(), String> {
    let args = parse(std::env::args().skip(1).collect())?;
    match args.command.as_str() {
        "map" => map(&args),
        "grid" => grid(&args),
        "collage" => collage(&args),
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
            "--out" => parsed.out = Some(value.clone()),
            other => return Err(format!("unknown option `{other}`\n\n{}", usage())),
        }
    }
    Ok(parsed)
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

    let image = plan::render_at_scale(&config, scale);
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
        let image = plan::grid(chunks, &seeds, scale);
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
    let image = plan::collage(&seeds);
    let path = args.out.clone().unwrap_or_else(|| "collage.png".into());
    write(
        &image,
        &path,
        format_args!("collage of {} maps from seed {base}", seeds.len()),
    )
}

/// `grid.png` and `1536x1024` -> `grid-1536x1024.png`.
fn suffixed(path: &str, tag: &str) -> String {
    match path.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem}-{tag}.{ext}"),
        None => format!("{path}-{tag}"),
    }
}
