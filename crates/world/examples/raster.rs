//! Prints open-world islands as the low-res rasters a naming model would see:
//! the island's frame letterboxed into a 16x16 grid, each cell one of four
//! levels — deep water, shallows, low land, high land.
//!
//! ```sh
//! cargo run --release -p world --example raster -- [seed] [count]
//! ```

use glam::Vec2;
use world::archipelago::{Archipelago, IslandSpec, WorldConfig};

/// Cells across the raster. The largest island is 28 chunks a side, so at 16
/// this is a little over two chunks of ground per cell.
const CELLS: usize = 16;

/// Water shallower than an anchorage reads as shallows — lagoons, shoals,
/// the shelf — rather than open sea.
const SHALLOW: f32 = -protocol::ground::ANCHOR_DEPTH;

/// Ground this high reads as the island's high country rather than its flats.
const HIGH: f32 = 15.0;

/// Subsamples per cell axis; the cell takes the median, so a cell is what
/// most of its ground is, not what one unlucky point under it was.
const SUB: usize = 3;

fn main() {
    let mut args = std::env::args().skip(1);
    let seed: u32 = args
        .next()
        .map(|a| a.parse().expect("seed must be a number"))
        .unwrap_or_else(|| WorldConfig::default().seed);
    let count: usize = args
        .next()
        .map(|a| a.parse().expect("count must be a number"))
        .unwrap_or(8);

    let world = Archipelago::new(&WorldConfig { seed });

    // Enough world to hold a spread of sizes, biggest first, then an even
    // stride down the sorted list so the sample spans big to skerry.
    let reach = Vec2::splat(12_000.0);
    let mut specs = world.islands_within(-reach, reach);
    specs.sort_by_key(|s| std::cmp::Reverse(s.chunks.x * s.chunks.y));
    let step = (specs.len() / count.min(specs.len()).max(1)).max(1);
    let picked: Vec<IslandSpec> = specs.into_iter().step_by(step).take(count).collect();

    println!("world seed {seed}, {} islands:", picked.len());
    for spec in picked {
        show(&world, spec);
    }
}

fn show(world: &Archipelago, spec: IslandSpec) {
    let island = world.island(spec);
    let extent = spec.extent();
    let centre = spec.centre();
    let side = extent.x.max(extent.y);

    let mut peak = f32::MIN;
    let mut land = 0u32;
    let mut lagoon = false;
    let mut grid = String::new();
    for row in 0..CELLS {
        grid.push_str("  ");
        for col in 0..CELLS {
            let mut samples = [0f32; SUB * SUB];
            for sz in 0..SUB {
                for sx in 0..SUB {
                    let u = (col as f32 + (sx as f32 + 0.5) / SUB as f32) / CELLS as f32 - 0.5;
                    let v = (row as f32 + (sz as f32 + 0.5) / SUB as f32) / CELLS as f32 - 0.5;
                    let p = centre + Vec2::new(u, v) * side;
                    let h = island.height(p.x, p.y);
                    samples[sz * SUB + sx] = h;
                    peak = peak.max(h);
                    if h >= 0.0 {
                        land += 1;
                    }
                    lagoon |= island.lake_level(p.x, p.y).is_some();
                }
            }
            samples.sort_by(f32::total_cmp);
            let median = samples[samples.len() / 2];
            grid.push(match median {
                h if h >= HIGH => '█',
                h if h >= 0.0 => '▓',
                h if h >= SHALLOW => '░',
                _ => '·',
            });
        }
        grid.push('\n');
    }

    let total = (CELLS * CELLS * SUB * SUB) as f32;
    println!(
        "\n{}x{} chunks ({:.0}x{:.0} m)  island seed {}  peak {:.0} m  land {:.0}%{}",
        spec.chunks.x,
        spec.chunks.y,
        extent.x,
        extent.y,
        spec.seed,
        peak.max(0.0),
        100.0 * land as f32 / total,
        if lagoon { "  lake" } else { "" },
    );
    print!("{grid}");
}
