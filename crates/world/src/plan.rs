//! Rendering maps in plan — overhead, hill-shaded, no window and no GPU.
//!
//! Looking at a map from above is how a change to the generator gets judged:
//! walking around one map in the app says almost nothing about whether the
//! generator as a whole got better. The `mapgen` binary is the front end to
//! everything here.

use std::sync::Arc;

use glam::{UVec2, Vec2, Vec3};

use protocol::ground::{Material, LAKE_WATER, SEA_WATER};

use crate::archipelago::{chunk_at, Archipelago, Island, IslandSpec};
use crate::terrain::{
    Country, Ground, LakeZone, Lie, MapConfig, TerrainGenerator, CHUNK_TILES, HEIGHT_SCALE,
};

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

/// What a plan render paints.
///
/// The generator answers several questions about a point and only one of them
/// has ever been drawn. That is a poor bargain for the one tool whose whole
/// job is looking hard at what a seed produced: a country and a hollow are
/// decisions with shapes, and a shape is the thing a page can show and a test
/// cannot.
///
/// A row of the table below is a layer — its word, the line `--help` prints
/// for it, and one function from a point to the colour it is drawn in.
/// `mapgen`'s option list is a fold over [`Layer::EVERY`], so a layer added
/// here is one the binary already advertises, and
/// `every_layer_answers_for_itself` holds the table to that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// The world as it looks: the palette a client would be sent, tinted for
    /// water and hill-shaded.
    Ground,
    /// Which country each point stands in, flat — see [`Country`].
    Countries,
    /// Height above sea level, as a ramp, with the sea by depth.
    ///
    /// The ramp is absolute, against [`HEIGHT_SCALE`], so two maps' altitudes
    /// can be read against each other. A small map genuinely tops out in the
    /// greens — its peaks are lower, the summit fit only reaching the full
    /// scale on a map with room for a range — and normalising that away would
    /// be the layer lying to make itself look busier.
    Altitude,
    /// Where the ground closes in around a point — see [`Lie`].
    Hollows,
}

impl Layer {
    pub const EVERY: [Layer; 4] = [
        Layer::Ground,
        Layer::Countries,
        Layer::Altitude,
        Layer::Hollows,
    ];

    /// The word this layer is asked for by.
    pub fn word(self) -> &'static str {
        match self {
            Layer::Ground => "ground",
            Layer::Countries => "country",
            Layer::Altitude => "altitude",
            Layer::Hollows => "hollows",
        }
    }

    /// The line `--help` prints for it. One line, since the option list folds
    /// them into a column.
    pub fn help(self) -> &'static str {
        match self {
            Layer::Ground => "the world as it looks, in the client's own palette",
            Layer::Countries => "which country each point stands in",
            Layer::Altitude => "height above sea level, as a ramp",
            Layer::Hollows => "the ground that closes in around itself",
        }
    }

    /// The layer a word names, or `None` for one no row answers to.
    pub fn from_word(word: &str) -> Option<Self> {
        Layer::EVERY.into_iter().find(|l| l.word() == word)
    }

    /// The colour one point is drawn in. Everything a layer could want about
    /// a point arrives together, since the caller had to sample it all to
    /// decide anything at all.
    fn paint(self, ground: Ground, height: f32, lake: Option<f32>, normal: Vec3) -> [u8; 3] {
        match self {
            Layer::Ground => shade(ground.material.color(), height, lake, normal),
            // Painted out of the palette rather than out of colours of its
            // own, so that a country map can be held against a ground map and
            // read: the country wears a material it actually paints somewhere.
            // Flat, so its edges are the decision and nothing else.
            Layer::Countries => shade(
                match ground.country {
                    Country::Sea => Material::Seabed,
                    Country::Lake(LakeZone::Bed) => Material::Silt,
                    Country::Lake(LakeZone::Shallows) => Material::Shoal,
                    Country::Lake(LakeZone::Margin) => Material::Marsh,
                    Country::Shore(_) => Material::Sand,
                    Country::Arid => Material::Parched,
                    Country::Lowland => Material::Grass,
                    Country::Humid => Material::Jungle,
                    Country::Moor => Material::Upland,
                    Country::Mountain => Material::Rock,
                }
                .color(),
                height,
                lake,
                normal,
            ),
            // Unshaded on purpose: the ramp is the height, and a hill shade
            // over it would be the same information twice, disagreeing at
            // every slope about which way is up.
            Layer::Altitude => {
                let c = if height < 0.0 {
                    // The shelf's blues over an island's own bed, then on
                    // down towards black across the open sea's basins — see
                    // [`crate::deeps`] — so a strait and a crossing read as
                    // the different water they are.
                    let deep = (-height / crate::terrain::MAX_DEPTH).clamp(0.0, 1.0);
                    let shelf = Vec3::new(0.42, 0.62, 0.72).lerp(Vec3::new(0.04, 0.10, 0.24), deep);
                    let abyss = ((-height - crate::terrain::MAX_DEPTH)
                        / (crate::deeps::DEEPEST - crate::terrain::MAX_DEPTH))
                        .clamp(0.0, 1.0);
                    shelf.lerp(Vec3::new(0.0, 0.01, 0.05), abyss)
                } else {
                    ramp((height / HEIGHT_SCALE).clamp(0.0, 1.0))
                };
                let c = c.clamp(Vec3::ZERO, Vec3::ONE) * 255.0;
                [c.x as u8, c.y as u8, c.z as u8]
            }
            // The hollows picked out over a drained version of the ground, so
            // that where they fall can be read against the shape of the land
            // they fall in. Drawing them alone gives a page of blobs with
            // nothing to hold them against.
            Layer::Hollows => {
                let under = ground.material.color().dot(Vec3::splat(1.0 / 3.0));
                let base = Vec3::splat(0.25 + 0.45 * under);
                let c = match ground.lie {
                    Lie::Hollow => Vec3::new(0.90, 0.44, 0.20),
                    Lie::Open => base,
                };
                shade(c, height, lake, normal)
            }
        }
    }
}

/// A hypsometric ramp over `0.0..=1.0`: the greens of low ground, through the
/// browns of the hills, to bare white at the top of the range.
fn ramp(t: f32) -> Vec3 {
    const STOPS: [Vec3; 5] = [
        Vec3::new(0.18, 0.40, 0.24),
        Vec3::new(0.55, 0.68, 0.32),
        Vec3::new(0.85, 0.78, 0.44),
        Vec3::new(0.66, 0.48, 0.34),
        Vec3::new(0.98, 0.98, 0.98),
    ];
    let span = (STOPS.len() - 1) as f32;
    let at = (t * span).clamp(0.0, span);
    let low = (at.floor() as usize).min(STOPS.len() - 2);
    STOPS[low].lerp(STOPS[low + 1], at - low as f32)
}

/// One pixel of any plan render: the map's own colour, tinted for the bed
/// below whatever water stands there, and hill-shaded by a sun over the
/// -x/-z corner so relief reads in plan.
///
/// `lake` is the surface of the lake standing over this point, as the
/// generator answers it — so `None` means the sea, whose surface is zero and
/// whose water is the other of the two the app draws.
fn shade(color: Vec3, height: f32, lake: Option<f32>, normal: Vec3) -> [u8; 3] {
    let (level, water) = match lake {
        Some(level) => (level, LAKE_WATER),
        None => (0.0, SEA_WATER),
    };
    let mut c = color;
    if height < level {
        // Stand in for the translucent sheet the app draws. Lighter here than
        // the app's own alpha, because a map is read for what is under the
        // water as much as for where the water is.
        c = c * 0.45 + water * 0.55;
    }
    let lit = 0.72 + 0.55 * normal.dot(Vec3::new(-0.5, 0.72, -0.48).normalize());
    let c = (c * lit).clamp(Vec3::ZERO, Vec3::ONE) * 255.0;
    [c.x as u8, c.y as u8, c.z as u8]
}

/// Renders one map in plan, hill-shaded, out of the same palette entries a
/// client would be sent, into a `width`-by-`height` image.
///
/// Shared by every layout below, so that a map looks the same whether it is
/// being examined on its own or compared with eight others.
pub fn render(config: &MapConfig, width: u32, height: u32, layer: Layer) -> Image {
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
            let ground = gen.ground(wx, wz, height, normal);
            let lake = gen.lake_level(wx, wz);
            pixels.extend_from_slice(&layer.paint(ground, height, lake, normal));
        }
    }
    Image {
        width,
        height,
        pixels,
    }
}

/// Renders a world-space region of an archipelago in plan — the same shading
/// as [`render`], over a window onto the open world instead of a whole lone
/// map. `centre` and `extent` are in metres of world space.
///
/// The one honest way to judge the layout: any measure of island spacing or
/// size mix is an average, and averages are exactly how a layout that clumps
/// or stripes slips through. A few kilometres on the page shows it.
pub fn render_region(
    world: &Archipelago,
    centre: Vec2,
    extent: Vec2,
    width: u32,
    layer: Layer,
) -> Image {
    let height = (width as f32 * extent.y / extent.x).round().max(1.0) as u32;
    let step = extent / Vec2::new(width as f32, height as f32);
    let origin = centre - extent * 0.5;

    // The island the last land pixel belonged to, kept for the next one.
    //
    // A region render walks the page in scanlines, and an island on the page
    // is hundreds of pixels across — so consecutive land pixels almost always
    // belong to the same island. Without this, each of them re-derives the
    // layout from the seed and then takes the cache's read lock to find a
    // generator it just finished using, which on a wide render is most of the
    // time spent on land. `covers_chunk` is the same test `island_at` would
    // reach, so keeping the hit is exact rather than approximate: the pixel is
    // this island's, or the slow path runs.
    let mut held: Option<(IslandSpec, Arc<Island>)> = None;

    // The sea between the islands, prepared once for the whole page.
    let deeps = world.deeps(origin, origin + extent);

    let mut pixels = Vec::with_capacity((width * height) as usize * 3);
    for iz in 0..height {
        for ix in 0..width {
            let wx = origin.x + ix as f32 * step.x;
            let wz = origin.y + iz as f32 * step.y;
            let chunk = chunk_at(Vec2::new(wx, wz));

            // Most of any region is open sea, whose floor is the layout's to
            // sound rather than an island's — no generator is paid for there,
            // which is most of the render's speed.
            let island = match &held {
                Some((spec, island)) if spec.covers_chunk(chunk) => Some(island.clone()),
                _ => world.island_at(wx, wz).map(|spec| {
                    let island = world.island(spec);
                    held = Some((spec, island.clone()));
                    island
                }),
            };

            let pixel = match island {
                // Exactly the palette's deep sea bed, which is what an
                // island's own skirt reaches: any difference between the two
                // would print every island's frame onto the water. The height
                // is the true floor, which only the altitude layer reads.
                None => layer.paint(
                    Ground {
                        country: Country::Sea,
                        lie: Lie::Open,
                        material: Material::Seabed,
                        bared_by_salt: false,
                    },
                    deeps.floor(Vec2::new(wx, wz)),
                    None,
                    Vec3::Y,
                ),
                Some(island) => {
                    let normal = island.normal(wx, wz);
                    let height = island.height(wx, wz);
                    let ground = island.ground(wx, wz, height, normal);
                    let lake = island.lake_level(wx, wz);
                    layer.paint(ground, height, lake, normal)
                }
            };
            pixels.extend_from_slice(&pixel);
        }
    }
    Image {
        width,
        height,
        pixels,
    }
}

/// Renders one map at `metres_per_pixel`, with each axis at least one pixel.
pub fn render_at_scale(config: &MapConfig, metres_per_pixel: f32, layer: Layer) -> Image {
    let extent = config.extent() / metres_per_pixel;
    render(
        config,
        (extent.x as u32).max(1),
        (extent.y as u32).max(1),
        layer,
    )
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
pub fn grid(chunks: UVec2, seeds: &[u32], metres_per_pixel: f32, layer: Layer) -> Image {
    let extent = (chunks * CHUNK_TILES).as_vec2() / metres_per_pixel;
    let (cell_w, cell_h) = ((extent.x as u32).max(1), (extent.y as u32).max(1));

    // Stitched with a one-pixel rule between cells, so a map that runs
    // right to its own edge is still told apart from its neighbour.
    let mut out = Image::blank(cell_w * 3 + 2, cell_h * 3 + 2);
    for (i, &seed) in seeds.iter().enumerate() {
        let cell = render(&MapConfig { chunks, seed }, cell_w, cell_h, layer);
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
pub fn collage(seeds: &[u32], layer: Layer) -> Image {
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
            layer,
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
    #[test]
    fn every_layer_answers_for_itself() {
        // A layer that is not in `EVERY` is one `mapgen` neither lists nor
        // accepts, which is the whole failure this table was built to make
        // impossible — so the match below is exhaustive on purpose. A new
        // layer fails to compile here until somebody has looked at this test,
        // and the assertions then hold it to advertising itself properly.
        fn listed(layer: Layer) -> bool {
            match layer {
                Layer::Ground | Layer::Countries | Layer::Altitude | Layer::Hollows => {
                    Layer::EVERY.contains(&layer)
                }
            }
        }

        let mut seen = std::collections::HashSet::new();
        for layer in Layer::EVERY {
            assert!(listed(layer), "{layer:?} is not in EVERY");
            let word = layer.word();
            assert!(
                !word.is_empty() && word.chars().all(|c| c.is_ascii_lowercase()),
                "{layer:?} answers to `{word}`, which is not a plain word"
            );
            assert!(seen.insert(word), "two layers answer to `{word}`");
            assert_eq!(
                Layer::from_word(word),
                Some(layer),
                "`{word}` does not come back as the layer that offered it"
            );
            let help = layer.help();
            assert!(
                !help.is_empty() && !help.contains('\n'),
                "{layer:?} needs one line of help, and has {help:?}"
            );
        }
        assert_eq!(seen.len(), Layer::EVERY.len(), "EVERY repeats a layer");
        assert_eq!(Layer::from_word("nowhere"), None);
    }

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
    fn the_smallest_map_still_renders_at_grid_scale() {
        // A single chunk at 3 m/px is 42 px, and nothing in the pipeline may
        // round that to zero.
        let map = render_at_scale(
            &MapConfig {
                chunks: UVec2::ONE,
                seed: 1,
            },
            GRID_METRES_PER_PIXEL,
            Layer::Ground,
        );
        assert!(map.width > 0 && map.height > 0);
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
