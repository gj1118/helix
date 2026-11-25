//! Rounded corners for blocks of cells (the messages of a plugin's panel).
//!
//! A terminal's font has a rounded end for one row (the powerline half circle) and nothing
//! for the corners of a block of several rows. In a terminal with the kitty graphics
//! protocol each corner's cell is an image instead: the cell in the block's color, without
//! what is outside a quarter circle of a cell's width (the radius of the rounded end of a
//! row, so the two go together). The four images of a color are sent once and placed where
//! the corners are; elsewhere the corners stay square.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use helix_view::annotations::panel::Corner;
use helix_view::graphics::Rect;
use helix_view::image_preview::{
    kitty_free, kitty_place, kitty_transmit, kitty_unplace, png_bytes, rounded_corner,
};
use tui::buffer::{Buffer as Surface, Overlay};

/// The kitty image ids of the corners' images: these bits and 28 bits of what the image
/// shows, apart from the cursors', the previews' and the diff connectors' ids.
const KITTY_IMAGE_IDS: u32 = 0x5000_0000;

/// How many times the cell's pixels the image has: smooth whether the terminal scales it up
/// (a cell size in points on a HiDPI screen) or down.
const OVERSAMPLE: u32 = 2;

/// The cell size when the terminal doesn't tell it: the terminal scales the image to the cell.
const DEFAULT_CELL: (u16, u16) = (10, 20);
/// The largest cell size drawn: past it the image is only bigger to send.
const MAX_CELL: (u16, u16) = (64, 128);

/// The size in pixels of a cell of the terminal, as the images are drawn for it.
pub fn cell_size(editor: &helix_view::Editor) -> (u16, u16) {
    editor.cell_size.map_or(DEFAULT_CELL, |(width, height)| {
        (width.clamp(1, MAX_CELL.0), height.clamp(1, MAX_CELL.1))
    })
}

/// A corner's image: its color, its size in pixels and which corner it is.
type ImageKey = ([u8; 3], u32, u32, Corner);
/// A corner's image in the terminal: its kitty id, and what transmits it.
type SentImage = (u32, Arc<[u8]>);

thread_local! {
    /// The corner images drawn lately.
    static IMAGES: RefCell<HashMap<ImageKey, SentImage>> = Default::default();
}

/// The size in pixels of the image of a corner over `cols` cells of `cell` pixels, and the
/// radius of the corner: a cell's width.
fn image_size(cell: (u16, u16), cols: u16) -> (u32, u32, f32) {
    let width = u32::from(cell.0) * OVERSAMPLE;
    (
        width * u32::from(cols),
        u32::from(cell.1) * OVERSAMPLE,
        width as f32,
    )
}

/// The kitty image id of the image of a corner, and what transmits it.
fn image(color: [u8; 3], cell: (u16, u16), cols: u16, corner: Corner) -> SentImage {
    let (width, height, radius) = image_size(cell, cols);
    IMAGES.with(|images| {
        let mut images = images.borrow_mut();
        if images.len() > 32 {
            images.clear();
        }
        let key = (color, width, height, corner);
        images
            .entry(key)
            .or_insert_with(|| {
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                key.hash(&mut hasher);
                let id = KITTY_IMAGE_IDS | (hasher.finish() as u32 & 0x0fff_ffff);
                let image = rounded_corner(
                    width,
                    height,
                    radius,
                    color,
                    corner.is_right(),
                    corner.is_bottom(),
                );
                (id, Arc::from(kitty_transmit(&png_bytes(&image), id)))
            })
            .clone()
    })
}

/// Shows `corner` of a block in `color` over the cells of `area` (one row; they keep what is
/// under the block as their background, and have no text), as the kitty `placement` of its
/// image: a number of the caller's that tells the corners on screen apart, the same from one
/// frame to the next for a corner that only moved (it is then moved, not shown anew).
pub fn draw(
    surface: &mut Surface,
    area: Rect,
    color: [u8; 3],
    corner: Corner,
    cell: (u16, u16),
    placement: u32,
) {
    // (not 0, which is every placement of an image)
    let placement = placement.max(1);
    let (id, transmit) = image(color, cell, area.width, corner);
    let key = {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (id, placement).hash(&mut hasher);
        hasher.finish()
    };
    surface.add_overlay(Overlay {
        area,
        key,
        payload: Arc::from(kitty_place(id, placement, area.width, 1)),
        cleanup: Some(Arc::from(kitty_unplace(id, placement))),
        text_on_top: false,
        cells: 0,
        slot: Some(u64::from(id) << 32 | u64::from(placement)),
        resource: Some(tui::buffer::Resource {
            id: u64::from(id),
            load: transmit,
            free: Some(Arc::from(kitty_free(id))),
        }),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_image_has_twice_the_cells_pixels_and_a_radius_of_a_cell() {
        assert_eq!(image_size((10, 20), 1), (20u32, 40u32, 20.0f32));
        // a side two cells wide: the corner is still a cell's
        assert_eq!(image_size((10, 20), 2), (40u32, 40u32, 20.0f32));
    }

    #[test]
    fn each_corner_of_a_color_is_an_image_of_its_own_sent_once() {
        let corners = [
            Corner::TopLeft,
            Corner::TopRight,
            Corner::BottomLeft,
            Corner::BottomRight,
        ];
        let ids: Vec<u32> = corners
            .iter()
            .map(|corner| image([1, 2, 3], (10, 20), 1, *corner).0)
            .collect();
        for (n, id) in ids.iter().enumerate() {
            assert_eq!(id & 0xf000_0000, KITTY_IMAGE_IDS);
            assert!(!ids[..n].contains(id), "{ids:?}");
        }
        let (id, transmit) = image([1, 2, 3], (10, 20), 1, Corner::TopLeft);
        assert_eq!(id, ids[0], "the same image: the same id");
        let transmit = String::from_utf8(transmit.to_vec()).unwrap();
        let start = format!("\x1b_Ga=t,f=100,t=d,i={id},q=2,m=0;");
        assert!(transmit.starts_with(&start), "{transmit}");
        // another color or cell: another image
        assert_ne!(image([1, 2, 4], (10, 20), 1, Corner::TopLeft).0, id);
        assert_ne!(image([1, 2, 3], (9, 18), 1, Corner::TopLeft).0, id);
    }

    #[test]
    fn a_corner_is_placed_over_its_cells() {
        let area = Rect::new(0, 0, 20, 5);
        let mut surface = Surface::empty(area);
        draw(
            &mut surface,
            Rect::new(4, 2, 1, 1),
            [9, 9, 9],
            Corner::BottomRight,
            (10, 20),
            0,
        );
        let overlays = &surface.overlays;
        assert_eq!(overlays.len(), 1);
        let overlay = &overlays[0];
        assert_eq!(overlay.area, Rect::new(4, 2, 1, 1));
        let (id, _) = image([9, 9, 9], (10, 20), 1, Corner::BottomRight);
        let place = String::from_utf8(overlay.payload.to_vec()).unwrap();
        // placement 0 would be all of the image's: it is 1
        assert_eq!(
            place,
            format!("\x1b_Ga=p,i={id},p=1,c=1,r=1,C=1,z=-1,q=2\x1b\\")
        );
        assert_eq!(overlay.slot, Some(u64::from(id) << 32 | 1));
        assert!(overlay.resource.is_some() && overlay.cleanup.is_some());
    }
}
