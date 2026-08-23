//! Marks that are not rectangles.
//!
//! A UI node is a rectangle, so a notched arrowhead, a tapered weight or an
//! arc is nothing a pile of them can be. Each such mark is drawn once into a
//! little texture and worn by an [`ImageNode`], white on clear glass, with the
//! tint left to whichever instrument spawns it — so one glyph can be the
//! reading's ink in one place and furniture in another.
//!
//! The coverage is supersampled because these marks move: an instrument spins
//! its glyph to a bearing or slides it along an arc, and an edge sampled only
//! at texel centres crawls as it goes.
use bevy::asset::RenderAssetUsages;
use bevy::image::{Image, ImageSampler};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

/// Samples across each texel on each axis, so this squared per texel.
const SUB: u32 = 4;

/// Draws a mark into a texture, asking `covered` where its ink lies.
///
/// The question is put in the texture's own frame: `x` and `y` run from 0 to 1
/// across the image with `y` downwards, which is how a texture is laid out and
/// not how a drawing's own axes usually run — a caller working in upright
/// coordinates flips `y` itself.
///
/// Callers size their textures generously for the pixels they lay the mark out
/// at: the whole UI scales with the window (see [`crate::settings`]), and a
/// texture drawn for the laid-out size softens on any monitor bigger than the
/// layouts were drawn for.
pub fn raster(width: u32, height: u32, covered: impl Fn(Vec2) -> bool) -> Image {
    let mut texels = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height {
        for column in 0..width {
            let mut hits = 0;
            for down in 0..SUB {
                for across in 0..SUB {
                    let at = Vec2::new(
                        (column as f32 + (across as f32 + 0.5) / SUB as f32) / width as f32,
                        (row as f32 + (down as f32 + 0.5) / SUB as f32) / height as f32,
                    );
                    if covered(at) {
                        hits += 1;
                    }
                }
            }
            let alpha = (hits * 255 / (SUB * SUB)) as u8;
            texels.extend_from_slice(&[255, 255, 255, alpha]);
        }
    }
    let mut image = Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        texels,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::linear();
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The alpha at one texel of an image this module made.
    fn alpha(image: &Image, column: u32, row: u32) -> u8 {
        let data = image
            .data
            .as_ref()
            .expect("a rastered glyph carries texels");
        data[((row * image.width() + column) * 4 + 3) as usize]
    }

    #[test]
    fn ink_lands_where_the_mark_says_and_nowhere_else() {
        // The left half covered, so the seam runs down the middle: what a
        // mirrored y or a transposed loop would move.
        let image = raster(8, 8, |at| at.x < 0.5);
        assert_eq!(alpha(&image, 0, 0), 255);
        assert_eq!(alpha(&image, 7, 0), 0);
        assert_eq!(alpha(&image, 0, 7), 255);
    }

    #[test]
    fn y_runs_down_the_texture() {
        // The top half covered. A caller drawing in upright coordinates has
        // to flip for this, so it must be certain which way it goes.
        let image = raster(8, 8, |at| at.y < 0.5);
        assert_eq!(alpha(&image, 0, 0), 255);
        assert_eq!(alpha(&image, 0, 7), 0);
    }

    #[test]
    fn an_edge_between_texels_comes_out_part_covered() {
        // Three quarters of the way across one texel of a four-texel row: the
        // supersampling is the whole reason a turned mark keeps its edges.
        let image = raster(4, 1, |at| at.x < 0.1875);
        assert_eq!(alpha(&image, 0, 0), 191);
    }
}
