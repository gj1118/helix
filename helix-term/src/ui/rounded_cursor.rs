//! `cursor-roundness`: block cursors with rounded corners, like 4coder's `cursor_roundness`.
//!
//! A terminal draws its cursor as a whole cell, so a rounded cursor is an image: a rounded
//! rectangle in the cursor's color, shown with the kitty graphics protocol under the text of the
//! cursor's cells (`z=-1`, over their background, so the corners show the selection, the cursor
//! line or the background). The text in it keeps the cursor's text color. While the editor draws
//! the primary cursor this way, the terminal's own cursor is hidden (`EditorView::cursor`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use helix_core::graphemes::{grapheme_width, next_grapheme_boundary};
use helix_view::graphics::{Color, CursorKind, Modifier, Rect, Style};
use helix_view::image_preview::{
    kitty_place, kitty_transmit, kitty_unplace, png_bytes, rounded_rect, Protocol,
};
use helix_view::{Document, Editor, View};
use tui::buffer::{Buffer as Surface, Overlay};

/// The largest roundness: the radius is half the cursor's width (its ends are half circles).
pub const MAX_ROUNDNESS: u8 = 50;

/// The cursors drawn rounded in a view at most (the primary one first); the others are square.
const MAX_CURSORS: usize = 64;

/// The kitty image ids of the cursors' images: this bit and 28 bits of what the image shows,
/// apart from the previews' and the diff connectors' ids. Each image is sent once, and its
/// placements show it under the cursors.
const KITTY_IMAGE_IDS: u32 = 0x4000_0000;

/// How many times the cell's pixels the image has: smooth whether the terminal scales it up
/// (a cell size in points on a HiDPI screen) or down.
const OVERSAMPLE: u32 = 2;

/// The cell size when the terminal doesn't tell it: the terminal scales the image to the cell.
const DEFAULT_CELL: (u16, u16) = (10, 20);
/// The largest cell size drawn: past it the image is only bigger to send.
const MAX_CELL: (u16, u16) = (64, 128);

/// The roundness block cursors are drawn with in the editor's current mode, `None` when they
/// are drawn as before: `cursor-roundness = 0`, a bar or underline cursor, or a terminal without
/// the kitty graphics protocol.
pub fn roundness(editor: &Editor) -> Option<u8> {
    let config = editor.config();
    let roundness = config.cursor_roundness.min(MAX_ROUNDNESS);
    if roundness == 0 || config.cursor_shape.from_mode(editor.mode()) != CursorKind::Block {
        return None;
    }
    let kitty = editor.kitty_graphics
        || config
            .image_preview
            .protocol
            .resolve(|name| std::env::var(name).ok())
            == Protocol::Kitty;
    kitty.then_some(roundness)
}

/// A cursor's image: its color, its size in pixels and its radius in tenths of a pixel.
type ImageKey = ([u8; 3], u32, u32, u32);
/// A cursor's image in the terminal: its kitty id, and what transmits it.
type SentImage = (u32, Arc<[u8]>);

thread_local! {
    /// The cursor images drawn lately.
    static IMAGES: RefCell<HashMap<ImageKey, SentImage>> = Default::default();
}

/// The kitty image id of the image of a cursor, and what transmits it.
fn image(color: [u8; 3], width: u32, height: u32, radius: f32) -> (u32, Arc<[u8]>) {
    IMAGES.with(|images| {
        let mut images = images.borrow_mut();
        if images.len() > 32 {
            images.clear();
        }
        let key = (color, width, height, (radius * 10.0).round() as u32);
        images
            .entry(key)
            .or_insert_with(|| {
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                key.hash(&mut hasher);
                let id = KITTY_IMAGE_IDS | (hasher.finish() as u32 & 0x0fff_ffff);
                let png = png_bytes(&rounded_rect(width, height, radius, color));
                (id, Arc::from(kitty_transmit(&png, id)))
            })
            .clone()
    })
}

/// Draws the cursors of `view` (the focused view) in `surface`, after its text: rounded with
/// `roundness`, in `primary` (the primary cursor's style) and `secondary` (the others').
pub fn draw(
    editor: &Editor,
    doc: &Document,
    view: &View,
    surface: &mut Surface,
    roundness: u8,
    primary: Style,
    secondary: Style,
) {
    let text = doc.text().slice(..);
    let selection = doc.selection(view.id);
    let inner = view.inner_area(doc);
    let background = editor.theme.get("ui.background").bg;
    let cell = editor.cell_size.map_or(DEFAULT_CELL, |(width, height)| {
        (width.clamp(1, MAX_CELL.0), height.clamp(1, MAX_CELL.1))
    });

    let primary_index = selection.primary_index();
    let order =
        std::iter::once(primary_index).chain((0..selection.len()).filter(|&i| i != primary_index));
    let mut rounded = 0;
    for index in order {
        let cursor = selection.ranges()[index].cursor(text);
        let Some(pos) = view.screen_coords_at_pos(doc, text, cursor) else {
            continue;
        };
        let (Ok(col), Ok(row)) = (u16::try_from(pos.col), u16::try_from(pos.row)) else {
            continue;
        };
        let (Some(x), Some(y)) = (inner.x.checked_add(col), inner.y.checked_add(row)) else {
            continue;
        };
        if x >= inner.right() || y >= inner.bottom() {
            continue;
        }
        // a wide character's cursor covers its two cells
        let width = if cursor < text.len_chars() {
            let grapheme = text.slice(cursor..next_grapheme_boundary(text, cursor));
            let grapheme = std::borrow::Cow::<str>::from(grapheme);
            if grapheme.contains(['\t', '\n', '\r']) {
                1
            } else {
                grapheme_width(&grapheme).clamp(1, 2) as u16
            }
        } else {
            1
        };
        let area = Rect::new(x, y, width.min(inner.right() - x), 1);
        let style = if index == primary_index {
            primary
        } else {
            secondary
        };
        // the cursor's placement of its image (not 0)
        let placement = rounded as u32 + 1;
        if rounded < MAX_CURSORS
            && draw_rounded(surface, area, style, background, roundness, cell, placement)
        {
            rounded += 1;
        } else {
            // as it is drawn without rounding
            surface.set_style(area, style);
        }
    }
}

/// Draws the cursor over `area` as a rounded rectangle under its text, as the kitty
/// `placement` of its image. `false` when its color isn't known (a terminal color), for it to
/// be drawn square.
fn draw_rounded(
    surface: &mut Surface,
    area: Rect,
    style: Style,
    background: Option<Color>,
    roundness: u8,
    cell: (u16, u16),
    placement: u32,
) -> bool {
    // the colors of the square cursor: the cursor's style over the cell
    let patched = surface[(area.x, area.y)].style().patch(style);
    let reversed = patched.add_modifier.contains(Modifier::REVERSED);
    let (fill, text) = if reversed {
        (patched.fg, patched.bg)
    } else {
        (patched.bg, patched.fg)
    };
    let Some(Color::Rgb(r, g, b)) = fill else {
        return false;
    };
    let fill = [r, g, b];
    // A reversed cursor's text is in the color of the background: the terminal's own background
    // can't be a text color, the theme's background or a color that stands out is.
    let text = match text {
        Some(Color::Reset) | None if reversed => match background {
            Some(color @ Color::Rgb(..)) => color,
            _ => contrasting(fill),
        },
        Some(color) => color,
        None => Color::Reset,
    };

    for x in area.left()..area.right() {
        let cell = &mut surface[(x, area.y)];
        let style = cell.style().patch(style);
        cell.fg = text;
        cell.modifier = style.add_modifier - Modifier::REVERSED;
        if let Some(color) = style.underline_color {
            cell.underline_color = color;
        }
        if let Some(underline) = style.underline_style {
            cell.underline_style = underline;
        }
    }

    let (width_px, height_px, radius) = image_size(cell, area.width, roundness);
    let (id, transmit) = image(fill, width_px, height_px, radius);
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
        // moved, not taken away and shown again: no flicker
        slot: Some(u64::from(id) << 32 | u64::from(placement)),
        resource: Some((u64::from(id), transmit)),
    });
    true
}

/// The size in pixels of the image of a cursor over `cols` cells of `cell` pixels, and the
/// radius of its corners: 4coder's, a share of the width of a character.
fn image_size(cell: (u16, u16), cols: u16, roundness: u8) -> (u32, u32, f32) {
    let width = u32::from(cell.0) * OVERSAMPLE;
    (
        width * u32::from(cols),
        u32::from(cell.1) * OVERSAMPLE,
        width as f32 * f32::from(roundness) / 100.0,
    )
}

/// Black or white, whichever stands out on `color`.
fn contrasting([r, g, b]: [u8; 3]) -> Color {
    let luma = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
    if luma > 140.0 {
        Color::Rgb(0, 0, 0)
    } else {
        Color::Rgb(255, 255, 255)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_image_has_twice_the_cells_pixels() {
        assert_eq!(image_size((10, 20), 1, 25), (20u32, 40u32, 5.0f32));
        // a wide character's cursor: two cells, the radius of one character
        assert_eq!(image_size((10, 20), 2, 50), (40u32, 40u32, 10.0f32));
        assert_eq!(image_size((10, 20), 1, 0), (20u32, 40u32, 0.0f32));
    }

    #[test]
    fn an_image_is_sent_under_an_id_of_its_own() {
        let (id, transmit) = image([1, 2, 3], 20, 40, 5.0);
        assert_eq!(id & KITTY_IMAGE_IDS, KITTY_IMAGE_IDS);
        let transmit = String::from_utf8(transmit.to_vec()).unwrap();
        let start = format!("\x1b_Ga=t,f=100,t=d,i={id},q=2,m=0;");
        assert!(transmit.starts_with(&start), "{transmit}");
        // the same image: the same id; another one: another id
        assert_eq!(image([1, 2, 3], 20, 40, 5.0).0, id);
        assert_ne!(image([1, 2, 4], 20, 40, 5.0).0, id);
        assert_ne!(image([1, 2, 3], 20, 40, 6.0).0, id);
    }

    #[test]
    fn contrasting_text_color() {
        assert_eq!(contrasting([250, 250, 250]), Color::Rgb(0, 0, 0));
        assert_eq!(contrasting([10, 20, 30]), Color::Rgb(255, 255, 255));
    }
}
