//! Rendering maps in plan — overhead, hill-shaded, no window and no GPU.
//!
//! Looking at a map from above is how a change to the generator gets judged:
//! walking around one map in the app says almost nothing about whether the
//! generator as a whole got better. The `mapgen` binary is the front end to
//! everything here.

use glam::{UVec2, Vec2, Vec3};

use crate::terrain::{MapConfig, TerrainGenerator, CHUNK_TILES};

/// An RGB8 image, as wide and tall as it says, ready to write out.
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// `width * height` RGB triples, row-major from the top left.
    pub pixels: Vec<u8>,
}

impl Image {
    /// A new canvas, filled white — which is the rule colour every layout here
    /// separates its maps with.
    fn blank(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![255; (width * height) as usize * 3],
        }
    }

    /// Copies `src` in at pixel offset `(x, y)`.
    fn blit(&mut self, src: &Image, x: u32, y: u32) {
        for row in 0..src.height {
            let dst = (((y + row) * self.width + x) * 3) as usize;
            let from = (row * src.width * 3) as usize;
            let run = (src.width * 3) as usize;
            self.pixels[dst..dst + run].copy_from_slice(&src.pixels[from..from + run]);
        }
    }

    /// Writes the image out as a PNG.
    pub fn write_png(&self, path: &str) -> Result<(), image::ImageError> {
        image::save_buffer(
            path,
            &self.pixels,
            self.width,
            self.height,
            image::ExtendedColorType::Rgb8,
        )
    }
}

/// Renders one map in plan, hill-shaded, using the same colour function the
/// mesh does, into a `width`-by-`height` image.
///
/// Shared by every layout below, so that a map looks the same whether it is
/// being examined on its own or compared with eight others.
pub fn render(config: &MapConfig, width: u32, height: u32) -> Image {
    let gen = TerrainGenerator::new(config);
    let half = config.half_extent();
    let step = config.extent() / Vec2::new(width as f32, height as f32);

    let mut pixels = Vec::with_capacity((width * height) as usize * 3);
    for iz in 0..height {
        for ix in 0..width {
            let wx = ix as f32 * step.x - half.x;
            let wz = iz as f32 * step.y - half.y;
            let normal = gen.normal(wx, wz);
            let height = gen.height(wx, wz);

            let mut c = gen.color(wx, wz, height, normal);
            if height < 0.0 {
                // Stand in for the translucent sea plane.
                c = c * 0.45 + Vec3::new(0.10, 0.42, 0.62) * 0.55;
            }
            // Cheap hillshade from a sun over the -x/-z corner, so relief
            // reads in plan.
            let lit = 0.72 + 0.55 * normal.dot(Vec3::new(-0.5, 0.72, -0.48).normalize());
            let c = (c * lit).clamp(Vec3::ZERO, Vec3::ONE) * 255.0;
            pixels.extend_from_slice(&[c.x as u8, c.y as u8, c.z as u8]);
        }
    }
    Image {
        width,
        height,
        pixels,
    }
}

/// Renders one map at `metres_per_pixel`, with each axis at least one pixel.
pub fn render_at_scale(config: &MapConfig, metres_per_pixel: f32) -> Image {
    let extent = config.extent() / metres_per_pixel;
    render(config, (extent.x as u32).max(1), (extent.y as u32).max(1))
}

/// The `count` seeds a layout of many maps draws, spread from one seed by the
/// same splitmix the noise uses on its own — so that neighbouring seeds give
/// sets as unrelated as the maps within a set are.
pub fn seed_set(from: u32, count: u32) -> Vec<u32> {
    (0..count as u64)
        .map(|i| {
            let mut s = (from as u64 * count as u64 + i + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            s ^= s >> 31;
            (s >> 32) as u32 % 1_000_000
        })
        .collect()
}

/// Metres of ground per pixel in a grid, held the same at every map size.
///
/// Which is the whole reason the sizes are worth rendering separately. The
/// wavelengths are fixed in metres, so a bigger map is meant to hold *more*
/// landscape rather than the same landscape stretched — and at a constant
/// scale that claim is visible: a bay or a range should come out the same
/// size on the page whichever grid it is in, and a large map should simply
/// have more of them. Fitting each size to the same square instead would
/// hide exactly the thing worth checking.
pub const GRID_METRES_PER_PIXEL: f32 = 3.0;

/// The map shapes a generator change gets judged on, in chunks per axis:
/// the square sizes the dialog offers, the smallest map there is, and
/// rectangles modest and wide — a map is any X by Z chunks, so shapes off
/// the square diagonal have to stay honest too.
pub const GRID_SHAPES: [UVec2; 5] = [
    UVec2::new(1, 1),
    UVec2::new(3, 2),
    UVec2::new(6, 6),
    UVec2::new(8, 8),
    UVec2::new(12, 8),
];

/// Nine maps of one shape at once, in a 3x3 grid, from nine unrelated seeds.
///
/// The one that matters for judging a change to the generator. Every number
/// the `island_shape` bench reports is an average over a map, and every look
/// at a single seed is an anecdote — between them it is very easy to tune a
/// constant until one favourite map improves and eight others quietly get
/// worse. Nine at a glance makes that obvious instead.
///
/// Drawing the same nine seeds at every shape is also a straight answer to
/// what a shape does to a given map: the noise is the same, only how much of
/// it fits has changed.
pub fn grid(chunks: UVec2, seeds: &[u32], metres_per_pixel: f32) -> Image {
    let extent = (chunks * CHUNK_TILES).as_vec2() / metres_per_pixel;
    let (cell_w, cell_h) = ((extent.x as u32).max(1), (extent.y as u32).max(1));

    // Stitched with a one-pixel rule between cells, so a map that runs
    // right to its own edge is still told apart from its neighbour.
    let mut out = Image::blank(cell_w * 3 + 2, cell_h * 3 + 2);
    for (i, &seed) in seeds.iter().enumerate() {
        let cell = render(&MapConfig { chunks, seed }, cell_w, cell_h);
        let (cx, cz) = (i as u32 % 3, i as u32 / 3);
        out.blit(&cell, cx * (cell_w + 1), cz * (cell_h + 1));
    }
    out
}

/// The README collage: sixteen maps of assorted shapes tiling a 3:2
/// canvas exactly, every one drawn at the same scale — so the collage
/// itself says what the generator is about, from a couple of continents
/// down to single-chunk islets, with relative sizes told honestly.
///
/// Each entry is a map's slot in chunk units: `(x, y, w, h)` on a
/// [`COLLAGE_SPAN`]-chunk-wide canvas. The rectangles tile it with no
/// gaps, which the `collage_tiles_exactly` test holds them to.
const COLLAGE: [(u32, u32, u32, u32); 16] = [
    (0, 0, 12, 8),
    (12, 0, 8, 8),
    (20, 0, 4, 4),
    (20, 4, 4, 4),
    (0, 8, 6, 6),
    (0, 14, 3, 2),
    (3, 14, 3, 2),
    (6, 8, 6, 8),
    (12, 8, 4, 6),
    (12, 14, 4, 2),
    (16, 8, 8, 6),
    (16, 14, 2, 2),
    (18, 14, 1, 1),
    (18, 15, 1, 1),
    (19, 14, 2, 2),
    (21, 14, 3, 2),
];

/// The collage canvas, in chunks: 24 across by 16 down, which is the 3:2
/// of the page it fills.
const COLLAGE_SPAN: UVec2 = UVec2::new(24, 16);

/// Pixels per chunk in the collage, and the white rule inset around each
/// map. 54 px over a 128 m chunk is a little under 2.4 m/px.
const COLLAGE_SCALE: u32 = 54;
const COLLAGE_GUTTER: u32 = 2;

/// How many seeds [`collage`] wants.
pub const COLLAGE_SEEDS: u32 = COLLAGE.len() as u32;

/// Renders the collage at the top of the README, one seed per slot.
pub fn collage(seeds: &[u32]) -> Image {
    let mut out = Image::blank(
        COLLAGE_SPAN.x * COLLAGE_SCALE,
        COLLAGE_SPAN.y * COLLAGE_SCALE,
    );

    for (&(x, y, w, h), &seed) in COLLAGE.iter().zip(seeds) {
        let config = MapConfig {
            chunks: UVec2::new(w, h),
            seed,
        };
        // The map inset within its slot, leaving the white rule.
        let cell = render(
            &config,
            w * COLLAGE_SCALE - 2 * COLLAGE_GUTTER,
            h * COLLAGE_SCALE - 2 * COLLAGE_GUTTER,
        );
        out.blit(
            &cell,
            x * COLLAGE_SCALE + COLLAGE_GUTTER,
            y * COLLAGE_SCALE + COLLAGE_GUTTER,
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collage_tiles_exactly() {
        // Every chunk of the canvas belongs to exactly one map — a gap prints
        // as a white hole in the README and an overlap draws one island over
        // another.
        let (span_x, span_y) = (COLLAGE_SPAN.x, COLLAGE_SPAN.y);
        let mut covered = vec![false; (span_x * span_y) as usize];
        for (x, y, w, h) in COLLAGE {
            assert!(
                x + w <= span_x && y + h <= span_y,
                "a map is off the canvas"
            );
            for cz in y..y + h {
                for cx in x..x + w {
                    let cell = &mut covered[(cz * span_x + cx) as usize];
                    assert!(!*cell, "two maps overlap at ({cx},{cz})");
                    *cell = true;
                }
            }
        }
        assert!(covered.iter().all(|c| *c), "the collage leaves a gap");
    }

    #[test]
    fn a_rendered_map_is_the_size_it_says() {
        let config = MapConfig::square(256, 7);
        let map = render(&config, 64, 48);
        assert_eq!((map.width, map.height), (64, 48));
        assert_eq!(map.pixels.len(), 64 * 48 * 3);
    }

    #[test]
    fn the_smallest_map_still_renders_at_grid_scale() {
        // A single chunk at 3 m/px is 42 px, and nothing in the pipeline may
        // round that to zero.
        let map = render_at_scale(
            &MapConfig {
                chunks: UVec2::ONE,
                seed: 1,
            },
            GRID_METRES_PER_PIXEL,
        );
        assert!(map.width > 0 && map.height > 0);
    }

    #[test]
    fn a_grid_holds_nine_maps_and_their_rules() {
        let chunks = UVec2::new(3, 2);
        let seeds = seed_set(1, 9);
        let grid = grid(chunks, &seeds, GRID_METRES_PER_PIXEL);

        let cell = (chunks * CHUNK_TILES).as_vec2() / GRID_METRES_PER_PIXEL;
        assert_eq!(grid.width, cell.x as u32 * 3 + 2);
        assert_eq!(grid.height, cell.y as u32 * 3 + 2);
    }

    #[test]
    fn a_set_is_nine_unrelated_seeds() {
        let seeds = seed_set(1, 9);
        let mut sorted = seeds.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), seeds.len(), "a set repeats a seed");
        assert_ne!(
            seeds,
            seed_set(2, 9),
            "neighbouring seeds drew the same set"
        );
    }
}
