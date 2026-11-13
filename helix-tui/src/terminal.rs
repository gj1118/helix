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
    /// Ask the terminal whether it has the kitty graphics protocol (for the diff connector).
    pub query_kitty_graphics: bool,
}

impl From<&EditorConfig> for Config {
    fn from(config: &EditorConfig) -> Self {
        Self {
            enable_mouse_capture: config.mouse,
            force_enable_extended_underlines: config.undercurl,
            kitty_keyboard_protocol: config.kitty_keyboard_protocol,
            query_kitty_graphics: config.diff.connector_graphics
                == helix_view::diff_view::ConnectorGraphics::Auto
                && helix_view::image_preview::kitty_graphics_unknown(|name| {
                    std::env::var(name).ok()
                }),
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
    /// The secondary cursors the terminal should draw from the next draw on, see
    /// [`Terminal::set_secondary_cursors`].
    secondary_cursors: Vec<(u16, u16)>,
    /// The secondary cursors last sent to the backend, or `None` when the terminal's are unknown
    /// (nothing sent yet, or a clear removed them).
    sent_secondary_cursors: Option<Vec<(u16, u16)>>,
    /// The window title to set from the next draw on, see [`Terminal::set_title`].
    title: Option<String>,
    /// The window title last set, or `None` when the terminal has its own (the one saved on
    /// its title stack when Helix set one).
    sent_title: Option<String>,
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
            secondary_cursors: Vec::new(),
            sent_secondary_cursors: None,
            title: None,
            sent_title: None,
        })
    }

    pub fn claim(&mut self) -> io::Result<()> {
        // Claiming clears the (alternate) screen, and with it any secondary cursors.
        self.sent_secondary_cursors = None;
        self.backend.claim()
    }

    pub fn reconfigure(&mut self, config: Config) -> io::Result<()> {
        self.backend.reconfigure(config)
    }

    /// Gives the terminal back (on exit, or when suspended), with its own window title: the
    /// title set by Helix comes back with the next draw after [`Terminal::claim`].
    pub fn restore(&mut self) -> io::Result<()> {
        if self.sent_title.take().is_some() {
            self.backend.pop_title()?;
        }
        self.backend.restore()
    }

    /// The window title the terminal should show from the next draw on, or `None` for its own.
    /// The first title saves the terminal's own on its title stack, and `None` (or
    /// [`Terminal::restore`]) brings it back. Control characters are left out: a file name
    /// must not end the escape sequence and write to the terminal.
    pub fn set_title(&mut self, title: Option<&str>) {
        self.title = title.map(|title| title.chars().filter(|c| !c.is_control()).collect());
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
            (&mut previous[0], &mut current[0])
        } else {
            let (previous, current) = self.buffers.split_at_mut(1);
            (&mut previous[0], &mut current[0])
        };
        // the ones something was drawn over after them (a popup) are hidden in this frame
        let mut overlays = std::mem::take(&mut current_buffer.overlays);
        overlays.retain(|overlay| overlay.cells == current_buffer.cells_of(overlay.area));
        current_buffer.overlays = overlays;
        let current_buffer = &*current_buffer;
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
                if overlay.text_on_top {
                    let area = current_buffer.area.intersection(overlay.area);
                    let text = (area.top()..area.bottom())
                        .flat_map(|y| (area.left()..area.right()).map(move |x| (x, y)))
                        .map(|(x, y)| (x, y, &current_buffer[(x, y)]))
                        .filter(|(_, _, cell)| !cell.symbol.trim().is_empty());
                    self.backend.draw(text)?;
                }
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

        // Only when they changed: the terminal keeps them between frames (erasing the screen
        // removes them, so a clear forgets what was sent).
        if self.sent_secondary_cursors.as_ref() != Some(&self.secondary_cursors) {
            self.backend.set_multiple_cursors(&self.secondary_cursors)?;
            self.sent_secondary_cursors = Some(self.secondary_cursors.clone());
        }

        // The title too: only when it changed.
        if self.sent_title != self.title {
            match &self.title {
                Some(title) => {
                    if self.sent_title.is_none() {
                        self.backend.push_title()?;
                    }
                    self.backend.set_title(title)?;
                }
                None => self.backend.pop_title()?,
            }
            self.sent_title = self.title.clone();
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

    /// Sets the secondary cursors (zero-based `(x, y)` cells) the terminal draws besides the
    /// main cursor from the next [`Terminal::draw`] on, for backends supporting a multiple
    /// cursors protocol. An empty list removes them.
    pub fn set_secondary_cursors(&mut self, cursors: Vec<(u16, u16)>) {
        self.secondary_cursors = cursors;
    }

    /// Clear the terminal and force a full redraw on the next draw call.
    ///
    /// The physical erase is deferred to the next `flush` so it shares a
    /// synchronized frame with the repaint.
    pub fn clear(&mut self) -> io::Result<()> {
        self.force_clear = true;
        // Erasing the display removes the terminal's secondary cursors too.
        self.sent_secondary_cursors = None;
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
            text_on_top: false,
            cells: 0,
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

    /// A popup drawn over an image after it hides the image while it is there; the image is
    /// shown again over its cells when the popup goes.
    #[test]
    fn an_image_under_a_popup_is_hidden_until_the_popup_goes() {
        let mut terminal = Terminal::new(TestBackend::new(20, 10)).unwrap();
        let area = Rect::new(2, 2, 4, 4);
        frame(&mut terminal, &[overlay(1, area)]);
        assert_eq!(raw(&mut terminal), [(2, 2, "image 1".to_string())]);

        // the popup is drawn after the image, over a cell of it
        terminal.current_buffer_mut().add_overlay(overlay(1, area));
        terminal.current_buffer_mut()[(3, 3)].set_symbol("p");
        terminal.draw(None, CursorKind::Hidden).unwrap();
        let sent = raw(&mut terminal);
        assert_eq!(sent, [(2, 2, "delete 1".to_string())]);
        assert_eq!(sent.len(), 1_usize);
        let popup: &str = &terminal.backend().buffer()[(3, 3)].symbol;
        assert_eq!(popup, "p");

        // the popup is gone: the cells are blank again, and the image is over them
        frame(&mut terminal, &[overlay(1, area)]);
        assert_eq!(raw(&mut terminal), [(2, 2, "image 1".to_string())]);
        let blank: &str = &terminal.backend().buffer()[(3, 3)].symbol;
        assert_eq!(blank, " ");
    }

    /// An image that takes the place of the text of its cells (iTerm2) has that text written
    /// again after it; the same image over other text is sent again.
    #[test]
    fn the_text_over_an_image_is_written_after_it() {
        let mut terminal = Terminal::new(TestBackend::new(20, 10)).unwrap();
        terminal.backend_mut().events = Some(Vec::new());
        let area = Rect::new(2, 2, 3, 2);
        let image = Overlay {
            text_on_top: true,
            ..overlay(1, area)
        };
        let draw = |terminal: &mut Terminal<TestBackend>, mark: &str| {
            terminal.current_buffer_mut()[(4, 3)].set_symbol(mark);
            terminal.current_buffer_mut().add_overlay(image.clone());
            terminal.draw(None, CursorKind::Hidden).unwrap();
            std::mem::take(terminal.backend_mut().events.as_mut().unwrap())
        };

        let events: Vec<String> = draw(&mut terminal, "m");
        let image_at = events.iter().position(|event| event == "raw 2,2");
        assert_eq!(image_at, Some(events.len() - 2));
        // only the text, not the blank cells, after the image
        assert_eq!(events[events.len() - 1..], ["4,3 m".to_string()]);

        // nothing changed: nothing sent
        let events: Vec<String> = draw(&mut terminal, "m");
        assert_eq!(events, Vec::<String>::new());

        // other text: the cell, then the image again, then the text over it
        let events: Vec<String> = draw(&mut terminal, "n");
        assert_eq!(
            events[events.len() - 2..],
            ["raw 2,2".to_string(), "4,3 n".to_string()]
        );
        assert!(events.contains(&"raw 2,2".to_string()));
    }

    fn sent_cursors(terminal: &mut Terminal<TestBackend>) -> Vec<Vec<(u16, u16)>> {
        std::mem::take(&mut terminal.backend_mut().multiple_cursors)
    }

    /// Secondary cursors go to the backend when they change (the terminal keeps them between
    /// frames), and again after a clear, which removes them from the screen.
    #[test]
    fn secondary_cursors_are_sent_when_they_change() {
        let mut terminal = Terminal::new(TestBackend::new(20, 10)).unwrap();

        // The first frame says that there are none.
        frame(&mut terminal, &[]);
        assert_eq!(sent_cursors(&mut terminal), [vec![]]);
        frame(&mut terminal, &[]);
        assert_eq!(sent_cursors(&mut terminal), Vec::<Vec<(u16, u16)>>::new());

        terminal.set_secondary_cursors(vec![(1, 2), (3, 4)]);
        frame(&mut terminal, &[]);
        assert_eq!(sent_cursors(&mut terminal), [vec![(1, 2), (3, 4)]]);
        frame(&mut terminal, &[]);
        assert_eq!(sent_cursors(&mut terminal), Vec::<Vec<(u16, u16)>>::new());

        terminal.clear().unwrap();
        frame(&mut terminal, &[]);
        assert_eq!(sent_cursors(&mut terminal), [vec![(1, 2), (3, 4)]]);

        terminal.set_secondary_cursors(Vec::new());
        frame(&mut terminal, &[]);
        assert_eq!(sent_cursors(&mut terminal), [vec![]]);
    }
}
