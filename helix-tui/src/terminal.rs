//! Terminal interface provided through the [Terminal] type.
//! Frontend for [Backend]

use crate::{backend::Backend, buffer::Buffer};
use helix_view::editor::{Config as EditorConfig, KittyKeyboardProtocolConfig};
use helix_view::graphics::{CursorKind, Rect};
use std::io;

#[derive(Debug, Clone, PartialEq)]
/// UNSTABLE
enum ResizeBehavior {
    Fixed,
    Auto,
}

#[derive(Debug, Clone, PartialEq)]
/// UNSTABLE
pub struct Viewport {
    area: Rect,
    resize_behavior: ResizeBehavior,
}

/// Terminal configuration
#[derive(Debug)]
pub struct Config {
    pub enable_mouse_capture: bool,
    pub force_enable_extended_underlines: bool,
    pub kitty_keyboard_protocol: KittyKeyboardProtocolConfig,
}

impl From<&EditorConfig> for Config {
    fn from(config: &EditorConfig) -> Self {
        Self {
            enable_mouse_capture: config.mouse,
            force_enable_extended_underlines: config.undercurl,
            kitty_keyboard_protocol: config.kitty_keyboard_protocol,
        }
    }
}

impl Viewport {
    /// UNSTABLE
    pub fn fixed(area: Rect) -> Viewport {
        Viewport {
            area,
            resize_behavior: ResizeBehavior::Fixed,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
/// Options to pass to [`Terminal::with_options`]
pub struct TerminalOptions {
    /// Viewport used to draw to the terminal
    pub viewport: Viewport,
}

/// Interface to the terminal backed by crossterm
#[derive(Debug)]
pub struct Terminal<B>
where
    B: Backend,
{
    backend: B,
    /// Holds the results of the current and previous draw calls. The two are compared at the end
    /// of each draw pass to output the necessary updates to the terminal
    buffers: [Buffer; 2],
    /// Index of the current buffer in the previous array
    current: usize,
    /// Kind of cursor (hidden or others)
    cursor_kind: CursorKind,
    /// Viewport
    viewport: Viewport,
    /// Set to request a full clear. The erase is deferred to the next `flush` so it is emitted
    /// inside the same synchronized-output frame as the repaint to avoid painting blank frames
    force_clear: bool,
}

/// Default terminal size: 80 columns, 24 lines
pub const DEFAULT_TERMINAL_SIZE: Rect = Rect {
    x: 0,
    y: 0,
    width: 80,
    height: 24,
};

impl<B> Terminal<B>
where
    B: Backend,
{
    /// Wrapper around Terminal initialization. Each buffer is initialized with a blank string and
    /// default colors for the foreground and the background
    pub fn new(backend: B) -> io::Result<Terminal<B>> {
        let size = backend.size().unwrap_or(DEFAULT_TERMINAL_SIZE);
        Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport {
                    area: size,
                    resize_behavior: ResizeBehavior::Auto,
                },
            },
        )
    }

    /// UNSTABLE
    pub fn with_options(backend: B, options: TerminalOptions) -> io::Result<Terminal<B>> {
        Ok(Terminal {
            backend,
            buffers: [
                Buffer::empty(options.viewport.area),
                Buffer::empty(options.viewport.area),
            ],
            current: 0,
            cursor_kind: CursorKind::Block,
            viewport: options.viewport,
            force_clear: false,
        })
    }

    pub fn claim(&mut self) -> io::Result<()> {
        self.backend.claim()
    }

    pub fn reconfigure(&mut self, config: Config) -> io::Result<()> {
        self.backend.reconfigure(config)
    }

    pub fn restore(&mut self) -> io::Result<()> {
        self.backend.restore()
    }

    // /// Get a Frame object which provides a consistent view into the terminal state for rendering.
    // pub fn get_frame(&mut self) -> Frame<B> {
    //     Frame {
    //         terminal: self,
    //         cursor_position: None,
    //     }
    // }

    pub fn current_buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[self.current]
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// Obtains a difference between the previous and the current buffer and passes it to the
    /// current backend for drawing.
    pub fn flush(&mut self) -> io::Result<()> {
        if self.force_clear {
            self.backend.clear()?;
            self.force_clear = false;
        }

        // Overlays (images) that are gone or have changed: take them away, and draw the cells
        // that were below them again.
        let (previous_buffer, current_buffer) = if self.current == 0 {
            let (current, previous) = self.buffers.split_at_mut(1);
            (&mut previous[0], &current[0])
        } else {
            let (previous, current) = self.buffers.split_at_mut(1);
            (&mut previous[0], &current[0])
        };
        for overlay in &previous_buffer.overlays {
            if !current_buffer.overlays.contains(overlay) {
                if let Some(cleanup) = &overlay.cleanup {
                    self.backend
                        .write_raw(overlay.area.x, overlay.area.y, cleanup)?;
                }
            }
        }
        let gone: Vec<_> = previous_buffer
            .overlays
            .iter()
            .filter(|overlay| !current_buffer.overlays.contains(overlay))
            .map(|overlay| overlay.area)
            .collect();
        for area in gone {
            previous_buffer.invalidate(area);
        }

        let updates = previous_buffer.diff(current_buffer);
        self.backend.draw(updates.into_iter())?;

        // The new ones go over the cells.
        for overlay in &current_buffer.overlays {
            if !previous_buffer.overlays.contains(overlay) {
                self.backend
                    .write_raw(overlay.area.x, overlay.area.y, &overlay.payload)?;
            }
        }
        Ok(())
    }

    /// Updates the Terminal so that internal buffers match the requested size. Requested size will
    /// be saved so the size can remain consistent when rendering.
    pub fn resize(&mut self, area: Rect) -> io::Result<()> {
        self.buffers[self.current].resize(area);
        self.buffers[1 - self.current].resize(area);
        self.viewport.area = area;
        self.clear()
    }

    /// Queries the backend for size and resizes if it doesn't match the previous size.
    pub fn autoresize(&mut self) -> io::Result<Rect> {
        let size = self.size();
        if size != self.viewport.area {
            self.resize(size)?;
        };
        Ok(size)
    }

    /// Synchronizes terminal size, calls the rendering closure, flushes the current internal state
    /// and prepares for the next draw call.
    pub fn draw(
        &mut self,
        cursor_position: Option<(u16, u16)>,
        cursor_kind: CursorKind,
    ) -> io::Result<()> {
        // // Autoresize - otherwise we get glitches if shrinking or potential desync between widgets
        // // and the terminal (if growing), which may OOB.
        // self.autoresize()?;

        // let mut frame = self.get_frame();
        // f(&mut frame);
        // // We can't change the cursor position right away because we have to flush the frame to
        // // stdout first. But we also can't keep the frame around, since it holds a &mut to
        // // Terminal. Thus, we're taking the important data out of the Frame and dropping it.
        // let cursor_position = frame.cursor_position;

        // One synchronized frame for the whole draw
        self.backend.start_sync()?;

        // Draw to stdout
        self.flush()?;

        if let Some((x, y)) = cursor_position {
            self.set_cursor(x, y)?;
        }

        match cursor_kind {
            CursorKind::Hidden => self.hide_cursor()?,
            kind => self.show_cursor(kind)?,
        }

        self.backend.end_sync()?;

        // Swap buffers
        self.buffers[1 - self.current].reset();
        self.current = 1 - self.current;

        // Flush
        self.backend.flush()?;
        Ok(())
    }

    #[inline]
    pub fn cursor_kind(&self) -> CursorKind {
        self.cursor_kind
    }

    pub fn hide_cursor(&mut self) -> io::Result<()> {
        self.backend.hide_cursor()?;
        self.cursor_kind = CursorKind::Hidden;
        Ok(())
    }

    pub fn show_cursor(&mut self, kind: CursorKind) -> io::Result<()> {
        self.backend.show_cursor(kind)?;
        self.cursor_kind = kind;
        Ok(())
    }

    pub fn set_cursor(&mut self, x: u16, y: u16) -> io::Result<()> {
        self.backend.set_cursor(x, y)
    }

    /// Clear the terminal and force a full redraw on the next draw call.
    ///
    /// The physical erase is deferred to the next `flush` so it shares a
    /// synchronized frame with the repaint.
    pub fn clear(&mut self) -> io::Result<()> {
        self.force_clear = true;
        // Reset the back buffer to make sure the next update will redraw everything.
        self.buffers[1 - self.current].reset();
        Ok(())
    }

    /// Queries the real size of the backend.
    pub fn size(&self) -> Rect {
        self.backend.size().unwrap_or(DEFAULT_TERMINAL_SIZE)
    }
}

#[cfg(test)]
mod overlay_tests {
    use super::*;
    use crate::{backend::TestBackend, buffer::Overlay};
    use std::sync::Arc;

    fn overlay(key: u64, area: Rect) -> Overlay {
        Overlay {
            area,
            key,
            payload: Arc::from(format!("image {key}").into_bytes()),
            cleanup: Some(Arc::from(format!("delete {key}").into_bytes())),
        }
    }

    fn frame(terminal: &mut Terminal<TestBackend>, overlays: &[Overlay]) {
        for overlay in overlays {
            terminal.current_buffer_mut().add_overlay(overlay.clone());
        }
        terminal.draw(None, CursorKind::Hidden).unwrap();
    }

    fn raw(terminal: &mut Terminal<TestBackend>) -> Vec<(u16, u16, String)> {
        std::mem::take(&mut terminal.backend_mut().raw)
            .into_iter()
            .map(|(x, y, bytes)| (x, y, String::from_utf8(bytes).unwrap()))
            .collect()
    }

    #[test]
    fn overlays_are_sent_once_and_taken_away_when_they_go() {
        let mut terminal = Terminal::new(TestBackend::new(20, 10)).unwrap();
        let area = Rect::new(3, 2, 5, 4);

        frame(&mut terminal, &[overlay(1, area)]);
        assert_eq!(raw(&mut terminal), [(3, 2, "image 1".to_string())]);

        // the same image in the next frame is not sent again
        frame(&mut terminal, &[overlay(1, area)]);
        assert_eq!(raw(&mut terminal), []);

        // another image in the same place: the old one is deleted, then the new one is shown
        frame(&mut terminal, &[overlay(2, area)]);
        assert_eq!(
            raw(&mut terminal),
            [
                (3, 2, "delete 1".to_string()),
                (3, 2, "image 2".to_string())
            ]
        );

        // no image: it is deleted
        frame(&mut terminal, &[]);
        assert_eq!(raw(&mut terminal), [(3, 2, "delete 2".to_string())]);
        frame(&mut terminal, &[]);
        assert_eq!(raw(&mut terminal), []);
    }

    #[test]
    fn an_image_that_moves_is_sent_again() {
        let mut terminal = Terminal::new(TestBackend::new(20, 10)).unwrap();
        frame(&mut terminal, &[overlay(1, Rect::new(0, 0, 4, 4))]);
        raw(&mut terminal);
        frame(&mut terminal, &[overlay(1, Rect::new(5, 0, 4, 4))]);
        assert_eq!(
            raw(&mut terminal),
            [
                (0, 0, "delete 1".to_string()),
                (5, 0, "image 1".to_string())
            ]
        );
    }

    #[test]
    fn a_cleared_terminal_shows_the_image_again() {
        let mut terminal = Terminal::new(TestBackend::new(20, 10)).unwrap();
        let area = Rect::new(1, 1, 4, 4);
        frame(&mut terminal, &[overlay(1, area)]);
        raw(&mut terminal);

        // a resize or a redraw request clears the screen, and the images with it
        terminal.clear().unwrap();
        frame(&mut terminal, &[overlay(1, area)]);
        assert_eq!(raw(&mut terminal), [(1, 1, "image 1".to_string())]);
    }
}
