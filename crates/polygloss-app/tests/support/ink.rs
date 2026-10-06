//! Glyph and icon ink in a screenshot (ADR-0031 "Reference tests").
//!
//! Captures are at [`SCALE`] 2; bands and results are in points, in the
//! window's coordinates. A pixel is ink when one of its channels differs
//! from the band's background by more than [`Ink::threshold`]; the
//! background is the median of the band's first pixel column, so a band
//! starts inside any border (a card's at `card_bounds` + `BORDER`).
//!
//! Ink measures glyphs and icons only. Origins (the window's, the
//! sidebar's and the main column's edges, each card's outer edge) come from
//! layout bounds: the default theme's card border (`#e7e7e7` on `#f8f8f6`)
//! is 17 off its background per channel, under any glyph threshold.

use gpui_kit::Bounds;
use image::{Rgba, RgbaImage};

use super::screenshot::SCALE;

/// The ink finder.
#[derive(Clone, Copy, Debug)]
pub struct Ink {
    /// The largest per-channel difference from the background that is not
    /// ink.
    pub threshold: u8,
}

impl Default for Ink {
    fn default() -> Self {
        Ink { threshold: 24 }
    }
}

impl Ink {
    /// The left edge of the first pixel column in `band` with ink.
    pub fn first_x(&self, image: &RgbaImage, band: Bounds<u32>) -> Option<f32> {
        let (xs, ys) = pixels(band);
        let background = self.background(image, band);
        xs.clone()
            .find(|&x| ys.clone().any(|y| self.is_ink(image, x, y, background)))
            .map(to_points)
    }

    /// The right edge of the last pixel column in `band` with ink.
    pub fn last_x(&self, image: &RgbaImage, band: Bounds<u32>) -> Option<f32> {
        let (xs, ys) = pixels(band);
        let background = self.background(image, band);
        xs.rev()
            .find(|&x| ys.clone().any(|y| self.is_ink(image, x, y, background)))
            .map(|x| to_points(x + 1))
    }

    /// The top edge of the first pixel row in `band` with ink.
    pub fn first_y(&self, image: &RgbaImage, band: Bounds<u32>) -> Option<f32> {
        let (xs, ys) = pixels(band);
        let background = self.background(image, band);
        ys.clone()
            .find(|&y| xs.clone().any(|x| self.is_ink(image, x, y, background)))
            .map(to_points)
    }

    /// The median of each channel down the band's first pixel column.
    fn background(&self, image: &RgbaImage, band: Bounds<u32>) -> [u8; 3] {
        let (xs, ys) = pixels(band);
        let column: Vec<&Rgba<u8>> = ys.map(|y| image.get_pixel(xs.start, y)).collect();
        assert!(!column.is_empty(), "an empty band: {band:?}");
        std::array::from_fn(|c| {
            let mut channel: Vec<u8> = column.iter().map(|p| p.0[c]).collect();
            channel.sort_unstable();
            channel[channel.len() / 2]
        })
    }

    fn is_ink(&self, image: &RgbaImage, x: u32, y: u32, background: [u8; 3]) -> bool {
        let pixel = image.get_pixel(x, y).0;
        (0..3).any(|c| pixel[c].abs_diff(background[c]) > self.threshold)
    }
}

/// `band`'s pixel columns and rows at [`SCALE`].
fn pixels(band: Bounds<u32>) -> (std::ops::Range<u32>, std::ops::Range<u32>) {
    let x = band.origin.x * SCALE;
    let y = band.origin.y * SCALE;
    (
        x..x + band.size.width * SCALE,
        y..y + band.size.height * SCALE,
    )
}

/// A pixel coordinate in points.
fn to_points(pixel: u32) -> f32 {
    pixel as f32 / SCALE as f32
}

#[cfg(test)]
fn canvas(width_pt: u32, height_pt: u32, background: [u8; 3]) -> RgbaImage {
    let [r, g, b] = background;
    RgbaImage::from_pixel(width_pt * SCALE, height_pt * SCALE, Rgba([r, g, b, 255]))
}

#[cfg(test)]
fn band(x: u32, y: u32, width: u32, height: u32) -> Bounds<u32> {
    Bounds::new(gpui_kit::point(x, y), gpui_kit::size(width, height))
}

#[test]
fn finds_the_first_and_last_ink_columns() {
    // Dark text from pt 10 to pt 14 (pixels 20..28 at 2×), rows 4..8 pt, on
    // the canvas color.
    let mut image = canvas(40, 12, [0xf8, 0xf8, 0xf6]);
    for x in 20..28 {
        for y in 8..16 {
            image.put_pixel(x, y, Rgba([0x1a, 0x1a, 0x1a, 255]));
        }
    }
    let ink = Ink::default();
    let whole = band(0, 0, 40, 12);
    assert_eq!(ink.first_x(&image, whole), Some(10.0));
    assert_eq!(ink.last_x(&image, whole), Some(14.0));
    assert_eq!(ink.first_y(&image, whole), Some(4.0));
    // A band that ends before the ink finds none.
    assert_eq!(ink.first_x(&image, band(0, 0, 10, 12)), None);
    // Results stay in window points for a band that starts inside.
    assert_eq!(ink.first_x(&image, band(5, 2, 20, 8)), Some(10.0));
}

#[test]
fn a_faint_border_is_not_ink() {
    // A card's 1 pt `#e7e7e7` border at pt 20 on the `#f8f8f6` canvas: 17
    // off per channel, under the glyph threshold. Origins come from layout
    // bounds for this reason.
    let mut image = canvas(40, 10, [0xf8, 0xf8, 0xf6]);
    for x in 40..42 {
        for y in 0..20 {
            image.put_pixel(x, y, Rgba([0xe7, 0xe7, 0xe7, 255]));
        }
    }
    let whole = band(0, 0, 40, 10);
    assert_eq!(Ink::default().first_x(&image, whole), None);
    assert_eq!(Ink { threshold: 8 }.first_x(&image, whole), Some(20.0));
}

#[test]
fn one_channel_is_enough() {
    // Pure red on white: red is unchanged, so a test of every channel would
    // miss it.
    let mut image = canvas(10, 10, [0xff, 0xff, 0xff]);
    image.put_pixel(7, 9, Rgba([0xff, 0x00, 0x00, 255]));
    let whole = band(0, 0, 10, 10);
    assert_eq!(Ink::default().first_x(&image, whole), Some(3.5));
    assert_eq!(Ink::default().first_y(&image, whole), Some(4.5));
}
