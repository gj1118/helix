//! The image in the preview of a picker.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use helix_view::graphics::{Color, Rect, Style};
use helix_view::image_preview::{self, Image, Placement, Protocol, Rendered};
use helix_view::Editor;
use tui::buffer::{Buffer as Surface, Overlay};

/// Images of up to this many pixels are decoded when the preview is opened, the others in a
/// thread, so that moving over a large photo doesn't stop the editor.
const DECODE_NOW_PIXELS: u64 = 1_000_000;

/// The size of a cell when the terminal doesn't tell.
const DEFAULT_CELL: (u16, u16) = (8, 16);

enum Load {
    Loading,
    Ready(Arc<Image>),
    Failed(String),
}

/// What an image looks like in an area, so that it is made once and not at every frame.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    protocol: Protocol,
    area: (u16, u16),
    cell: (u16, u16),
}

struct Drawn {
    placement: Placement,
    content: Rendered,
    payload: Option<Arc<[u8]>>,
    cleanup: Option<Arc<[u8]>>,
    /// Identifies this image to the terminal layer, see [`Overlay::key`].
    id: u64,
}

pub struct ImagePreview {
    load: Arc<Mutex<Load>>,
    drawn: Mutex<Option<(Key, Arc<Drawn>)>>,
}

pub enum Status {
    Loading,
    Failed(String),
    Shown,
}

impl ImagePreview {
    pub fn open(path: &Path) -> Self {
        let load = match image_preview::dimensions(path) {
            Err(err) => Load::Failed(err),
            Ok((width, height)) if u64::from(width) * u64::from(height) <= DECODE_NOW_PIXELS => {
                Self::decode(path)
            }
            Ok(_) => Load::Loading,
        };
        let load = Arc::new(Mutex::new(load));

        if matches!(*load.lock().unwrap(), Load::Loading) {
            let (path, load) = (path.to_path_buf(), load.clone());
            std::thread::spawn(move || {
                let decoded = Self::decode(&path);
                *load.lock().unwrap() = decoded;
                helix_event::request_redraw();
            });
        }

        Self {
            load,
            drawn: Mutex::new(None),
        }
    }

    fn decode(path: &Path) -> Load {
        match image_preview::load(path) {
            Ok(image) => Load::Ready(Arc::new(image)),
            Err(err) => Load::Failed(err),
        }
    }

    /// The size of the image in pixels, once it is decoded.
    pub fn size(&self) -> Option<(u32, u32)> {
        match &*self.load.lock().unwrap() {
            Load::Ready(image) => Some(image.size()),
            _ => None,
        }
    }

    /// Draws the image into `area`.
    pub fn draw(&self, surface: &mut Surface, area: Rect, editor: &Editor) -> Status {
        let image = match &*self.load.lock().unwrap() {
            Load::Loading => return Status::Loading,
            Load::Failed(err) => return Status::Failed(err.clone()),
            Load::Ready(image) => image.clone(),
        };
        if area.width == 0 || area.height == 0 {
            return Status::Shown;
        }

        let protocol = editor
            .config()
            .image_preview
            .protocol
            .resolve(|name| std::env::var(name).ok());
        let key = Key {
            protocol,
            area: (area.width, area.height),
            // A cell of half blocks holds one pixel across and two down.
            cell: if protocol == Protocol::Halfblocks {
                (1, 2)
            } else {
                editor.cell_size.unwrap_or(DEFAULT_CELL)
            },
        };

        let drawn = {
            let mut cache = self.drawn.lock().unwrap();
            match &*cache {
                Some((cached, drawn)) if *cached == key => drawn.clone(),
                _ => {
                    let drawn = Arc::new(Self::make(&image, key));
                    *cache = Some((key, drawn.clone()));
                    drawn
                }
            }
        };

        let placement = drawn.placement;
        let (x, y) = (area.x + placement.x, area.y + placement.y);
        match &drawn.content {
            Rendered::Cells(cells) => {
                for (i, (top, bottom)) in cells.iter().enumerate() {
                    let col = (i % placement.cols as usize) as u16;
                    let row = (i / placement.cols as usize) as u16;
                    let style = Style::default()
                        .fg(Color::Rgb(top[0], top[1], top[2]))
                        .bg(Color::Rgb(bottom[0], bottom[1], bottom[2]));
                    surface.set_string(x + col, y + row, "▀", style);
                }
            }
            Rendered::Raw(_) => {
                if let Some(payload) = &drawn.payload {
                    surface.add_overlay(Overlay {
                        area: Rect::new(x, y, placement.cols, placement.rows),
                        key: drawn.id,
                        payload: payload.clone(),
                        cleanup: drawn.cleanup.clone(),
                    });
                }
            }
        }
        Status::Shown
    }

    fn make(image: &Image, key: Key) -> Drawn {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);

        let placement = image_preview::place(image.size(), key.area, key.cell);
        let content = image_preview::render(image, key.protocol, &placement);
        let (payload, cleanup) = match &content {
            Rendered::Raw(encoded) => (
                Some(Arc::from(encoded.payload.as_slice())),
                encoded.cleanup.as_deref().map(Arc::from),
            ),
            Rendered::Cells(_) => (None, None),
        };
        Drawn {
            placement,
            content,
            payload,
            cleanup,
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn large_images_are_decoded_in_the_background() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.png");
        // more than DECODE_NOW_PIXELS
        image::RgbaImage::from_pixel(1200, 1000, image::Rgba([1, 2, 3, 255]))
            .save(&path)
            .unwrap();

        let preview = ImagePreview::open(&path);
        let deadline = Instant::now() + Duration::from_secs(20);
        while preview.size().is_none() {
            assert!(Instant::now() < deadline, "the image was never decoded");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(preview.size(), Some((1200, 1000)));
    }

    #[test]
    fn small_images_are_ready_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("small.png");
        image::RgbaImage::from_pixel(20, 10, image::Rgba([1, 2, 3, 255]))
            .save(&path)
            .unwrap();
        assert_eq!(ImagePreview::open(&path).size(), Some((20, 10)));
    }

    #[test]
    fn missing_files_fail() {
        let preview = ImagePreview::open(Path::new("/definitely/not/here.png"));
        assert_eq!(preview.size(), None);
        assert!(matches!(&*preview.load.lock().unwrap(), Load::Failed(_)));
    }
}
