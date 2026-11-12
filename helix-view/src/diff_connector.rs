//! The connector of a side-by-side diff drawn as an image (for terminals with a graphics
//! protocol): for each change, a band from its lines on the left to its lines on the right
//! along a smooth curve, anti-aliased, on a transparent background.

use image::{Rgba, RgbaImage};

/// A band of the connector: where it starts on the left edge and ends on the right one, as
/// `(top, bottom)` in rows of cells from the top (fractions, and beyond the area when the
/// change is out of view), and its color.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    pub left: (f32, f32),
    pub right: (f32, f32),
    pub color: [u8; 3],
}

/// How thick (in pixels) a band of no height at all is drawn: a line where one side has no
/// lines (an addition, a removal).
const LINE: f32 = 1.5;
/// How much of the width stays flat at each end, the curve in between.
const FLAT: f32 = 0.08;
/// Samples across each pixel (along the width), for smooth slopes.
const SAMPLES: u32 = 3;

/// The connector's image: `cols` x `rows` cells of `cell` pixels.
pub fn connector_image(cols: u16, rows: u16, cell: (u16, u16), bands: &[Band]) -> RgbaImage {
    let (cell_w, cell_h) = (f32::from(cell.0.max(1)), f32::from(cell.1.max(1)));
    let width = u32::from(cols) * u32::from(cell.0.max(1));
    let height = u32::from(rows) * u32::from(cell.1.max(1));
    let mut image = RgbaImage::from_pixel(width.max(1), height.max(1), Rgba([0, 0, 0, 0]));
    let total = cell_w * f32::from(cols);
    // the coverage of each pixel of a column, summed over the samples across it
    let mut coverage = vec![0.0f32; height as usize];
    for band in bands {
        for x in 0..width {
            coverage.fill(0.0);
            for sample in 0..SAMPLES {
                let at = (x as f32 + (sample as f32 + 0.5) / SAMPLES as f32) / total;
                let t = ((at - FLAT) / (1.0 - 2.0 * FLAT)).clamp(0.0, 1.0);
                // ease in and out: flat at both ends, steepest in the middle
                let s = t * t * (3.0 - 2.0 * t);
                let lerp = |a: f32, b: f32| (a + (b - a) * s) * cell_h;
                let (mut top, mut bottom) = (
                    lerp(band.left.0, band.right.0),
                    lerp(band.left.1, band.right.1),
                );
                if bottom - top < LINE {
                    let middle = (top + bottom) / 2.0;
                    top = middle - LINE / 2.0;
                    bottom = middle + LINE / 2.0;
                }
                let (first, last) = (top.floor().max(0.0), bottom.ceil().min(height as f32));
                let mut y = first;
                while y < last {
                    let covered = (bottom.min(y + 1.0) - top.max(y)).clamp(0.0, 1.0);
                    coverage[y as usize] += covered / SAMPLES as f32;
                    y += 1.0;
                }
            }
            for (y, &covered) in coverage.iter().enumerate() {
                if covered > 0.0 {
                    blend(
                        image.get_pixel_mut(x, y as u32),
                        band.color,
                        covered.min(1.0),
                    );
                }
            }
        }
    }
    image
}

/// The connector's image as a PNG, made small: flat colors and transparency compress very
/// well with PNG's filters (the image is sent to the terminal again as the diff scrolls).
pub fn connector_png(image: &RgbaImage) -> Vec<u8> {
    use image::ImageEncoder;
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new_with_quality(
        &mut bytes,
        image::codecs::png::CompressionType::Default,
        image::codecs::png::FilterType::Adaptive,
    )
    .write_image(
        image.as_raw(),
        image.width(),
        image.height(),
        image::ExtendedColorType::Rgba8,
    )
    .expect("encoding to memory cannot fail");
    bytes
}

/// The size of a cell of the image at most: the terminal scales the image to its cells, so
/// more pixels only make it bigger to send.
pub const MAX_CELL: (u16, u16) = (8, 16);

/// `color` at `alpha` over `pixel` (straight alpha).
fn blend(pixel: &mut Rgba<u8>, color: [u8; 3], alpha: f32) {
    let below = f32::from(pixel[3]) / 255.0;
    let out = alpha + below * (1.0 - alpha);
    if out <= 0.0 {
        return;
    }
    for channel in 0..3 {
        let value = (f32::from(color[channel]) * alpha
            + f32::from(pixel[channel]) * below * (1.0 - alpha))
            / out;
        pixel[channel] = value.round().clamp(0.0, 255.0) as u8;
    }
    pixel[3] = (out * 255.0).round().clamp(0.0, 255.0) as u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    const GREEN: [u8; 3] = [0, 200, 0];

    #[test]
    fn a_level_band_fills_its_rows() {
        // rows 1 to 3 on both sides, 4 columns of 10x20 pixels
        let band = Band {
            left: (1.0, 3.0),
            right: (1.0, 3.0),
            color: GREEN,
        };
        let image: RgbaImage = connector_image(4, 5, (10, 20), &[band]);
        assert_eq!(image.dimensions(), (40, 100));
        for x in [0, 20, 39] {
            assert_eq!(*image.get_pixel(x, 19), Rgba([0, 0, 0, 0]), "x {x}");
            assert_eq!(*image.get_pixel(x, 20), Rgba([0, 200, 0, 255]), "x {x}");
            assert_eq!(*image.get_pixel(x, 59), Rgba([0, 200, 0, 255]), "x {x}");
            assert_eq!(*image.get_pixel(x, 60), Rgba([0, 0, 0, 0]), "x {x}");
        }
    }

    #[test]
    fn a_band_curves_from_left_to_right() {
        // one row on the left, rows 3 to 4 on the right
        let band = Band {
            left: (0.0, 1.0),
            right: (3.0, 5.0),
            color: GREEN,
        };
        let image: RgbaImage = connector_image(9, 6, (10, 20), &[band]);
        let filled = |x: u32, y: u32| image.get_pixel(x, y)[3] == 255;
        // flat at the ends
        assert!(filled(0, 10) && !filled(0, 30));
        assert!(filled(89, 70) && !filled(89, 50));
        // halfway, halfway down
        assert!(filled(45, 50));
        assert!(!filled(45, 5));
        // anti-aliased: some pixel along the slope is partly covered
        let partial = (0..120).any(|y| {
            let alpha = image.get_pixel(30, y)[3];
            alpha > 0 && alpha < 255
        });
        assert!(partial);
    }

    #[test]
    fn a_band_with_no_lines_on_one_side_is_a_line() {
        // an addition: no lines on the left (at row 2), two on the right
        let band = Band {
            left: (2.0, 2.0),
            right: (2.0, 4.0),
            color: GREEN,
        };
        let image: RgbaImage = connector_image(9, 6, (10, 20), &[band]);
        let column: Vec<u8> = (0..120).map(|y| image.get_pixel(0, y)[3]).collect();
        let drawn = column.iter().filter(|alpha| **alpha > 0).count();
        assert!((1..=3).contains(&drawn), "{drawn} pixels at the left edge");
        assert!(column[39] > 0 || column[40] > 0);
    }

    #[test]
    fn the_png_is_small() {
        let bands: Vec<Band> = (0..10)
            .map(|i| Band {
                left: (i as f32 * 6.0, i as f32 * 6.0 + 2.0),
                right: (i as f32 * 6.0 + 1.5, i as f32 * 6.0 + 4.0),
                color: GREEN,
            })
            .collect();
        let image = connector_image(9, 60, MAX_CELL, &bands);
        let png: Vec<u8> = connector_png(&image);
        assert!(png.starts_with(b"\x89PNG"));
        assert!(png.len() < 16 * 1024, "{} bytes", png.len());
    }

    #[test]
    fn bands_out_of_view_draw_nothing() {
        let band = Band {
            left: (-10.0, -8.0),
            right: (-12.0, -9.0),
            color: GREEN,
        };
        let image: RgbaImage = connector_image(9, 6, (10, 20), &[band]);
        assert!(image.pixels().all(|pixel| pixel[3] == 0));
    }
}
