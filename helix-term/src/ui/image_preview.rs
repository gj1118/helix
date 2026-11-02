//! The image in the preview of a picker.
//!
//! Images are decoded scaled down to at most [`image_preview::MAX_SIDE`] pixels on a side, large
//! ones by a single background worker that only ever decodes the newest request, and only the
//! [`MAX_READY`] most recently drawn previews keep their decoded image: the others drop it and
//! decode it again when they are shown.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};

use helix_view::graphics::{Color, Rect, Style};
use helix_view::image_preview::{self, Image, Placement, Protocol, Rendered};
use helix_view::Editor;
use tui::buffer::{Buffer as Surface, Overlay};

/// Images of up to this many pixels are decoded when the preview is opened, the others in a
/// thread, so that moving over a large photo doesn't stop the editor.
const DECODE_NOW_PIXELS: u64 = 1_000_000;

/// How many previews keep their decoded image (at most `MAX_SIDE² * 4` bytes each, 16 MB).
const MAX_READY: usize = 6;

/// The size of a cell when the terminal doesn't tell.
const DEFAULT_CELL: (u16, u16) = (8, 16);

enum Load {
    /// Not decoded (yet, or any more): decoding starts when the preview is drawn.
    Idle,
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

struct Inner {
    load: Mutex<Load>,
    drawn: Mutex<Option<(Key, Arc<Drawn>)>>,
}

impl Inner {
    /// Forgets the decoded image, which is decoded again the next time it is drawn.
    fn evict(&self) {
        let mut load = self.load.lock().unwrap();
        if matches!(*load, Load::Ready(_)) {
            *load = Load::Idle;
            *self.drawn.lock().unwrap() = None;
        }
    }
}

pub struct ImagePreview {
    path: PathBuf,
    /// The size of the image in pixels, from its header.
    dimensions: (u32, u32),
    inner: Arc<Inner>,
}

pub enum Status {
    Loading,
    Failed(String),
    Shown,
}

/// The previews holding a decoded image, the most recently drawn last.
fn ready() -> &'static Mutex<VecDeque<Weak<Inner>>> {
    static READY: OnceLock<Mutex<VecDeque<Weak<Inner>>>> = OnceLock::new();
    READY.get_or_init(Default::default)
}

/// Marks `inner` as the most recently used decoded image and drops the decoded images of the
/// previews used longest ago beyond [`MAX_READY`].
fn touch(inner: &Arc<Inner>) {
    let evicted: Vec<_> = {
        let mut ready = ready().lock().unwrap();
        let target = Arc::downgrade(inner);
        ready.retain(|entry| entry.strong_count() > 0 && !entry.ptr_eq(&target));
        ready.push_back(target);
        let excess = ready.len().saturating_sub(MAX_READY);
        ready.drain(..excess).collect()
    };
    for entry in evicted {
        if let Some(inner) = entry.upgrade() {
            inner.evict();
        }
    }
}

/// The background decoder: one thread, one pending request. A new request replaces the pending
/// one, so scrolling over many large images only decodes the one the cursor stops on.
struct Worker {
    pending: Mutex<Option<(PathBuf, Weak<Inner>)>>,
    wake: Condvar,
}

impl Worker {
    fn get() -> &'static Worker {
        static WORKER: OnceLock<Worker> = OnceLock::new();
        let mut spawn = false;
        let worker = WORKER.get_or_init(|| {
            spawn = true;
            Worker {
                pending: Mutex::new(None),
                wake: Condvar::new(),
            }
        });
        if spawn {
            std::thread::Builder::new()
                .name("image-preview".into())
                .spawn(move || worker.run())
                .expect("failed to spawn the image preview thread");
        }
        worker
    }

    fn submit(&self, path: PathBuf, inner: &Arc<Inner>) {
        let replaced = self
            .pending
            .lock()
            .unwrap()
            .replace((path, Arc::downgrade(inner)));
        self.wake.notify_one();
        // The replaced request is dropped: its preview decodes again when it is drawn.
        if let Some(inner) = replaced.and_then(|(_, inner)| inner.upgrade()) {
            let mut load = inner.load.lock().unwrap();
            if matches!(*load, Load::Loading) {
                *load = Load::Idle;
            }
        }
    }

    fn run(&self) {
        loop {
            let (path, inner) = {
                let mut pending = self.pending.lock().unwrap();
                loop {
                    if let Some(request) = pending.take() {
                        break request;
                    }
                    pending = self.wake.wait(pending).unwrap();
                }
            };
            // The picker was closed or moved on.
            let Some(inner) = inner.upgrade() else {
                continue;
            };
            if !matches!(*inner.load.lock().unwrap(), Load::Loading) {
                continue;
            }
            ImagePreview::finish(&inner, ImagePreview::decode(&path));
            helix_event::request_redraw();
        }
    }
}

impl ImagePreview {
    pub fn open(path: &Path) -> Self {
        let (load, dimensions) = match image_preview::dimensions(path) {
            Err(err) => (Load::Failed(err), (0, 0)),
            Ok(dimensions) => (Load::Idle, dimensions),
        };
        let preview = Self {
            path: path.to_path_buf(),
            dimensions,
            inner: Arc::new(Inner {
                load: Mutex::new(load),
                drawn: Mutex::new(None),
            }),
        };
        preview.start();
        preview
    }

    /// Starts decoding the image if it is not decoded: at once for small images, in the
    /// background for the others.
    fn start(&self) {
        {
            let mut load = self.inner.load.lock().unwrap();
            if !matches!(*load, Load::Idle) {
                return;
            }
            *load = Load::Loading;
        }
        let (width, height) = self.dimensions;
        if u64::from(width) * u64::from(height) <= DECODE_NOW_PIXELS {
            Self::finish(&self.inner, Self::decode(&self.path));
        } else {
            Worker::get().submit(self.path.clone(), &self.inner);
        }
    }

    fn decode(path: &Path) -> Load {
        match image_preview::load_scaled(path, image_preview::MAX_SIDE) {
            Ok(image) => Load::Ready(Arc::new(image)),
            Err(err) => Load::Failed(err),
        }
    }

    fn finish(inner: &Arc<Inner>, decoded: Load) {
        let is_ready = matches!(decoded, Load::Ready(_));
        *inner.drawn.lock().unwrap() = None;
        *inner.load.lock().unwrap() = decoded;
        if is_ready {
            touch(inner);
        }
    }

    /// The size of the image in pixels, once it is decoded.
    pub fn size(&self) -> Option<(u32, u32)> {
        match &*self.inner.load.lock().unwrap() {
            Load::Ready(_) => Some(self.dimensions),
            _ => None,
        }
    }

    /// Draws the image into `area`.
    pub fn draw(&self, surface: &mut Surface, area: Rect, editor: &Editor) -> Status {
        self.start();
        let image = match &*self.inner.load.lock().unwrap() {
            Load::Idle | Load::Loading => return Status::Loading,
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

        touch(&self.inner);
        let drawn = {
            let mut cache = self.inner.drawn.lock().unwrap();
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
        // Other tests may evict the image or replace the request: `start` asks again, like drawing.
        while preview.size().is_none() {
            assert!(Instant::now() < deadline, "the image was never decoded");
            std::thread::sleep(Duration::from_millis(10));
            preview.start();
        }
        assert_eq!(preview.size(), Some((1200, 1000)));
    }

    #[test]
    fn huge_images_are_kept_scaled_down() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("huge.png");
        image::RgbaImage::from_pixel(image_preview::MAX_SIDE * 2, 10, image::Rgba([1, 2, 3, 255]))
            .save(&path)
            .unwrap();
        let preview = ImagePreview::open(&path);
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Load::Ready(image) = &*preview.inner.load.lock().unwrap() {
                assert_eq!(image.size(), (image_preview::MAX_SIDE, 5));
                break;
            }
            assert!(Instant::now() < deadline, "the image was never decoded");
            std::thread::sleep(Duration::from_millis(10));
            preview.start();
        }
        // The original size is still reported.
        assert_eq!(preview.size(), Some((image_preview::MAX_SIDE * 2, 10)));
    }

    #[test]
    fn only_a_few_decoded_images_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let previews: Vec<_> = (0..MAX_READY + 3)
            .map(|i| {
                let path = dir.path().join(format!("{i}.png"));
                image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]))
                    .save(&path)
                    .unwrap();
                ImagePreview::open(&path)
            })
            .collect();
        let decoded = previews.iter().filter(|p| p.size().is_some()).count();
        assert!(decoded <= MAX_READY, "{decoded} images are kept");
        // An evicted image is decoded again when it is shown.
        let first = &previews[0];
        assert_eq!(first.size(), None);
        first.start();
        assert!(matches!(
            &*first.inner.load.lock().unwrap(),
            Load::Ready(_) | Load::Idle
        ));
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
        assert!(matches!(
            &*preview.inner.load.lock().unwrap(),
            Load::Failed(_)
        ));
    }
}
