//! Image previews for the pickers: decoding, fitting an image into an area of cells, and the
//! terminal graphics protocols that put it on the screen.
//!
//! Four ways of drawing are supported:
//!
//! - [`Protocol::Kitty`]: the Kitty graphics protocol (Kitty, Ghostty, ...)
//! - [`Protocol::Iterm2`]: iTerm2's inline images (iTerm2, WezTerm, ...)
//! - [`Protocol::Sixel`]: Sixel graphics (foot, mlterm, ...)
//! - [`Protocol::Halfblocks`]: `▀` characters with a foreground and a background colour, which
//!   works in any terminal with colours, at two pixels per cell
//!
//! The first three produce bytes ([`Encoded`]) that have to reach the terminal as they are; the
//! last produces cells.

use std::io;
use std::path::Path;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use image::{imageops::FilterType, ImageEncoder, Rgb, RgbImage, RgbaImage};
use serde::{Deserialize, Serialize};

/// The `protocol` option of `[editor.image-preview]`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProtocolConfig {
    /// Pick one from the environment, see [`detect_protocol`].
    #[default]
    Auto,
    Kitty,
    Iterm2,
    Sixel,
    Halfblocks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Kitty,
    Iterm2,
    Sixel,
    Halfblocks,
}

impl ProtocolConfig {
    pub fn resolve(self, env: impl Fn(&str) -> Option<String>) -> Protocol {
        match self {
            ProtocolConfig::Auto => detect_protocol(env),
            ProtocolConfig::Kitty => Protocol::Kitty,
            ProtocolConfig::Iterm2 => Protocol::Iterm2,
            ProtocolConfig::Sixel => Protocol::Sixel,
            ProtocolConfig::Halfblocks => Protocol::Halfblocks,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case", deny_unknown_fields)]
pub struct ImagePreviewConfig {
    /// Show PNG, JPEG, GIF and WebP files as images in the picker preview.
    pub enable: bool,
    /// How to draw them.
    pub protocol: ProtocolConfig,
}

impl Default for ImagePreviewConfig {
    fn default() -> Self {
        Self {
            enable: true,
            protocol: ProtocolConfig::Auto,
        }
    }
}

/// Picks the protocol from the environment variables the terminals set. There is no query of the
/// terminal, so a terminal that isn't recognised gets half blocks, which always work.
/// Multiplexers get half blocks too: they swallow the escape sequences of the other protocols
/// unless they are set up to pass them through (`protocol` forces one anyway).
pub fn detect_protocol(env: impl Fn(&str) -> Option<String>) -> Protocol {
    let has = |name: &str| env(name).is_some_and(|value| !value.is_empty());
    let contains = |name: &str, needle: &str| {
        env(name).is_some_and(|value| value.to_ascii_lowercase().contains(needle))
    };

    if has("TMUX") || has("ZELLIJ") || contains("TERM", "screen") {
        Protocol::Halfblocks
    } else if has("KITTY_WINDOW_ID")
        || contains("TERM", "kitty")
        || contains("TERM", "ghostty")
        || contains("TERM_PROGRAM", "ghostty")
    {
        Protocol::Kitty
    } else if contains("TERM_PROGRAM", "iterm")
        || contains("TERM_PROGRAM", "wezterm")
        || contains("LC_TERMINAL", "iterm")
    {
        Protocol::Iterm2
    } else if contains("TERM", "foot") || contains("TERM", "mlterm") || contains("TERM", "sixel") {
        Protocol::Sixel
    } else {
        Protocol::Halfblocks
    }
}

/// Images with more pixels than this are not decoded.
pub const MAX_PIXELS: u64 = 100_000_000;

/// Whether the start of a file is that of a PNG, JPEG, GIF or WebP image.
pub fn is_image(start: &[u8]) -> bool {
    start.starts_with(b"\x89PNG\r\n\x1a\n")
        || start.starts_with(&[0xFF, 0xD8, 0xFF])
        || start.starts_with(b"GIF87a")
        || start.starts_with(b"GIF89a")
        || (start.len() >= 12 && &start[..4] == b"RIFF" && &start[8..12] == b"WEBP")
}

/// A decoded image. Animations show their first frame.
#[derive(Debug, Clone)]
pub struct Image {
    rgba: RgbaImage,
}

impl Image {
    pub fn from_rgba(rgba: RgbaImage) -> Self {
        Self { rgba }
    }

    pub fn size(&self) -> (u32, u32) {
        self.rgba.dimensions()
    }
}

fn reader(path: &Path) -> io::Result<image::ImageReader<io::BufReader<std::fs::File>>> {
    image::ImageReader::open(path)?.with_guessed_format()
}

/// The size of the image in pixels, from its header.
pub fn dimensions(path: &Path) -> Result<(u32, u32), String> {
    reader(path)
        .map_err(|err| err.to_string())?
        .into_dimensions()
        .map_err(|err| err.to_string())
}

pub fn load(path: &Path) -> Result<Image, String> {
    let (width, height) = dimensions(path)?;
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(format!("{width}x{height} pixels is too large to preview"));
    }
    let decoded = reader(path)
        .map_err(|err| err.to_string())?
        .decode()
        .map_err(|err| err.to_string())?;
    Ok(Image {
        rgba: decoded.into_rgba8(),
    })
}

/// Where an image goes in an area of cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    /// The size of the image, in cells.
    pub cols: u16,
    pub rows: u16,
    /// The position in the area, in cells: images are centred.
    pub x: u16,
    pub y: u16,
    /// The size of the image, in pixels. `cols` and `rows` times the size of a cell.
    pub width_px: u32,
    pub height_px: u32,
}

/// Fits an image of `image` pixels into an area of `area` cells, keeping its proportions and
/// without enlarging it. `cell` is the size of a cell in pixels.
pub fn place(image: (u32, u32), area: (u16, u16), cell: (u16, u16)) -> Placement {
    let (image_w, image_h) = (f64::from(image.0.max(1)), f64::from(image.1.max(1)));
    let (cell_w, cell_h) = (f64::from(cell.0.max(1)), f64::from(cell.1.max(1)));
    let (area_w, area_h) = (area.0.max(1), area.1.max(1));

    let scale = (f64::from(area_w) * cell_w / image_w)
        .min(f64::from(area_h) * cell_h / image_h)
        .min(1.0);
    let cols = ((image_w * scale / cell_w).ceil() as u16).clamp(1, area_w);
    let rows = ((image_h * scale / cell_h).ceil() as u16).clamp(1, area_h);

    Placement {
        cols,
        rows,
        x: (area_w - cols) / 2,
        y: (area_h - rows) / 2,
        width_px: u32::from(cols) * u32::from(cell.0.max(1)),
        height_px: u32::from(rows) * u32::from(cell.1.max(1)),
    }
}

/// Bytes for the terminal, which draws the image over `cols` x `rows` cells from the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoded {
    pub payload: Vec<u8>,
    /// What to send before the image goes away, for protocols that keep the image in the terminal.
    pub cleanup: Option<Vec<u8>>,
}

/// What [`render`] makes of an image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rendered {
    /// `cols * rows` cells of `▀`: the colour of the upper pixel and of the lower one.
    Cells(Vec<([u8; 3], [u8; 3])>),
    Raw(Encoded),
}

/// The id of the image in the terminal's memory for the Kitty protocol. All previews share one,
/// so the terminal never holds more than one.
pub const KITTY_IMAGE_ID: u32 = 4_810_517;

/// Scales the image and encodes it for `protocol`.
pub fn render(image: &Image, protocol: Protocol, placement: &Placement) -> Rendered {
    let resized =
        |width, height| image::imageops::resize(&image.rgba, width, height, FilterType::Triangle);

    match protocol {
        Protocol::Halfblocks => {
            let rgb = flatten(&resized(
                u32::from(placement.cols),
                u32::from(placement.rows) * 2,
            ));
            let mut cells = Vec::with_capacity(placement.cols as usize * placement.rows as usize);
            for row in 0..u32::from(placement.rows) {
                for col in 0..u32::from(placement.cols) {
                    cells.push((
                        rgb.get_pixel(col, row * 2).0,
                        rgb.get_pixel(col, row * 2 + 1).0,
                    ));
                }
            }
            Rendered::Cells(cells)
        }
        Protocol::Sixel => {
            let rgb = flatten(&resized(placement.width_px, placement.height_px));
            Rendered::Raw(sixel(&rgb))
        }
        Protocol::Kitty => {
            let png = png_bytes(&resized(placement.width_px, placement.height_px));
            Rendered::Raw(kitty(&png, placement, KITTY_IMAGE_ID))
        }
        Protocol::Iterm2 => {
            let png = png_bytes(&resized(placement.width_px, placement.height_px));
            Rendered::Raw(iterm2(&png, placement))
        }
    }
}

/// Draws the transparent parts over a checkerboard, for the outputs that can't be transparent.
pub fn flatten(image: &RgbaImage) -> RgbImage {
    RgbImage::from_fn(image.width(), image.height(), |x, y| {
        let [r, g, b, a] = image.get_pixel(x, y).0;
        if a == 255 {
            return Rgb([r, g, b]);
        }
        let background = if (x / 8 + y / 8) % 2 == 0 { 0x99 } else { 0x66 };
        let blend = |channel: u8| {
            ((u32::from(channel) * u32::from(a) + background * (255 - u32::from(a))) / 255) as u8
        };
        Rgb([blend(r), blend(g), blend(b)])
    })
}

pub fn png_bytes(image: &RgbaImage) -> Vec<u8> {
    let mut bytes = Vec::new();
    // Fast: the terminal decodes it right away.
    let encoder = image::codecs::png::PngEncoder::new_with_quality(
        &mut bytes,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::NoFilter,
    );
    encoder
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgba8,
        )
        .expect("encoding to memory cannot fail");
    bytes
}

/// The Kitty graphics protocol: the PNG is sent in chunks and shown over the cells, below the text
/// (`z=-1`), without moving the cursor.
pub fn kitty(png: &[u8], placement: &Placement, id: u32) -> Encoded {
    const CHUNK: usize = 4096;
    let data = BASE64.encode(png);
    let chunks: Vec<&str> = data
        .as_bytes()
        .chunks(CHUNK)
        .map(|chunk| std::str::from_utf8(chunk).expect("base64 is ASCII"))
        .collect();

    let mut payload = String::new();
    for (i, chunk) in chunks.iter().enumerate() {
        let more = u8::from(i + 1 < chunks.len());
        if i == 0 {
            payload.push_str(&format!(
                "\x1b_Ga=T,f=100,t=d,i={id},c={},r={},C=1,z=-1,q=2,m={more};{chunk}\x1b\\",
                placement.cols, placement.rows
            ));
        } else {
            payload.push_str(&format!("\x1b_Gm={more};{chunk}\x1b\\"));
        }
    }

    Encoded {
        payload: payload.into_bytes(),
        cleanup: Some(format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\").into_bytes()),
    }
}

/// iTerm2's inline image.
pub fn iterm2(png: &[u8], placement: &Placement) -> Encoded {
    Encoded {
        payload: format!(
            "\x1b]1337;File=inline=1;size={};width={};height={};preserveAspectRatio=0:{}\x07",
            png.len(),
            placement.cols,
            placement.rows,
            BASE64.encode(png)
        )
        .into_bytes(),
        cleanup: None,
    }
}

/// Number of colours of the palette [`sixel`] quantizes to: a 6x6x6 cube and 24 greys.
const SIXEL_COLORS: usize = 240;

fn sixel_index(Rgb([r, g, b]): Rgb<u8>) -> u8 {
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    if max - min < 12 {
        let grey = (u32::from(r) + u32::from(g) + u32::from(b)) / 3;
        216 + ((grey * 23 + 127) / 255) as u8
    } else {
        let level = |c: u8| (u32::from(c) + 25) / 51;
        (level(r) * 36 + level(g) * 6 + level(b)) as u8
    }
}

fn sixel_color(index: usize) -> [u8; 3] {
    if index >= 216 {
        let v = ((index - 216) * 255 / 23) as u8;
        [v, v, v]
    } else {
        let level = |n: usize| (n * 51) as u8;
        [level(index / 36), level(index / 6 % 6), level(index % 6)]
    }
}

/// A Sixel image. The colours are quantized to a fixed palette without dithering, so gradients
/// band.
pub fn sixel(image: &RgbImage) -> Encoded {
    use std::fmt::Write;

    let (width, height) = image.dimensions();
    let indices: Vec<u8> = image.pixels().map(|&pixel| sixel_index(pixel)).collect();

    let mut used = [false; SIXEL_COLORS];
    for &index in &indices {
        used[index as usize] = true;
    }

    let mut out = format!("\x1bPq\"1;1;{width};{height}");
    for (index, _) in used.iter().enumerate().filter(|(_, used)| **used) {
        let [r, g, b] = sixel_color(index);
        let percent = |c: u8| u32::from(c) * 100 / 255;
        let _ = write!(
            out,
            "#{index};2;{};{};{}",
            percent(r),
            percent(g),
            percent(b)
        );
    }

    for band in (0..height).step_by(6) {
        let rows = (height - band).min(6);
        // The sixel bits of every column, for each colour of the band.
        let mut columns: Vec<Option<Vec<u8>>> = vec![None; SIXEL_COLORS];
        for row in 0..rows {
            for x in 0..width {
                let index = indices[((band + row) * width + x) as usize] as usize;
                columns[index].get_or_insert_with(|| vec![0; width as usize])[x as usize] |=
                    1 << row;
            }
        }

        let present: Vec<usize> = (0..SIXEL_COLORS)
            .filter(|&index| columns[index].is_some())
            .collect();
        for (n, index) in present.iter().enumerate() {
            let _ = write!(out, "#{index}");
            push_runs(&mut out, columns[*index].as_deref().unwrap());
            // `$` goes back to the start of the band for the next colour, `-` to the next band.
            out.push(if n + 1 == present.len() { '-' } else { '$' });
        }
    }
    out.push_str("\x1b\\");

    Encoded {
        payload: out.into_bytes(),
        cleanup: None,
    }
}

/// Run-length encodes a row of sixels (`!<count><char>`).
fn push_runs(out: &mut String, bits: &[u8]) {
    let mut i = 0;
    while i < bits.len() {
        let value = bits[i];
        let run = bits[i..].iter().take_while(|&&b| b == value).count();
        let ch = char::from(value + 63);
        if run > 3 {
            out.push_str(&format!("!{run}{ch}"));
        } else {
            for _ in 0..run {
                out.push(ch);
            }
        }
        i += run;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    }

    #[test]
    fn detects_the_protocol_of_the_terminal() {
        use Protocol::*;
        assert_eq!(detect_protocol(env(&[("KITTY_WINDOW_ID", "1")])), Kitty);
        assert_eq!(detect_protocol(env(&[("TERM", "xterm-kitty")])), Kitty);
        assert_eq!(detect_protocol(env(&[("TERM_PROGRAM", "ghostty")])), Kitty);
        assert_eq!(
            detect_protocol(env(&[("TERM_PROGRAM", "iTerm.app")])),
            Iterm2
        );
        assert_eq!(detect_protocol(env(&[("TERM_PROGRAM", "WezTerm")])), Iterm2);
        assert_eq!(detect_protocol(env(&[("TERM", "foot")])), Sixel);
        assert_eq!(
            detect_protocol(env(&[("TERM", "xterm-256color")])),
            Halfblocks
        );
        assert_eq!(detect_protocol(env(&[])), Halfblocks);
    }

    #[test]
    fn multiplexers_get_half_blocks() {
        // Kitty runs tmux, whose pane swallows the graphics sequences
        let vars: &'static [(&str, &str)] = &[("KITTY_WINDOW_ID", "1"), ("TMUX", "/tmp/tmux-0")];
        assert_eq!(detect_protocol(env(vars)), Protocol::Halfblocks);
        // unless a protocol is configured
        assert_eq!(ProtocolConfig::Kitty.resolve(env(vars)), Protocol::Kitty);
    }

    #[test]
    fn recognizes_image_files() {
        assert!(is_image(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"));
        assert!(is_image(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10]));
        assert!(is_image(b"GIF89a\x01\0\x01\0"));
        assert!(is_image(b"RIFF\x24\0\0\0WEBPVP8 "));
        assert!(!is_image(b"RIFF\x24\0\0\0WAVEfmt "));
        assert!(!is_image(b"\x7fELF\x02\x01"));
        assert!(!is_image(b""));
    }

    #[test]
    fn fits_the_image_into_the_area() {
        // wide image: limited by the width, centred vertically
        let wide = place((400, 100), (20, 20), (10, 20));
        assert_eq!((wide.cols, wide.rows), (20, 3));
        assert_eq!((wide.x, wide.y), (0, 8));
        assert_eq!((wide.width_px, wide.height_px), (200, 60));

        // tall image: limited by the height
        let tall = place((100, 800), (20, 10), (10, 20));
        assert_eq!((tall.cols, tall.rows), (3, 10));
        assert_eq!((tall.x, tall.y), (8, 0));
    }

    #[test]
    fn small_images_are_not_enlarged() {
        // 16x16 pixels of 10x20 cells
        let icon = place((16, 16), (40, 20), (10, 20));
        assert_eq!((icon.cols, icon.rows), (2, 1));
        assert_eq!((icon.x, icon.y), (19, 9));
    }

    #[test]
    fn half_blocks_use_cells_of_one_by_two_pixels() {
        let p = place((8, 8), (40, 20), (1, 2));
        assert_eq!((p.cols, p.rows), (8, 4));
    }

    fn gradient(width: u32, height: u32) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, y| {
            image::Rgba([
                (x * 255 / width.max(1)) as u8,
                (y * 255 / height.max(1)) as u8,
                128,
                255,
            ])
        })
    }

    #[test]
    fn half_blocks_carry_the_upper_and_lower_pixel() {
        // top half red, bottom half blue
        let image = Image::from_rgba(RgbaImage::from_fn(4, 4, |_, y| {
            if y < 2 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 0, 255, 255])
            }
        }));
        let placement = place(image.size(), (10, 10), (1, 2));
        assert_eq!((placement.cols, placement.rows), (4, 2));
        let Rendered::Cells(cells) = render(&image, Protocol::Halfblocks, &placement) else {
            panic!("half blocks are cells");
        };
        assert_eq!(cells.len(), 8);
        // row 0 covers pixel rows 0-1 (red), row 1 covers rows 2-3 (blue)
        assert_eq!(cells[0], ([255, 0, 0], [255, 0, 0]));
        assert_eq!(cells[4], ([0, 0, 255], [0, 0, 255]));
    }

    #[test]
    fn transparent_pixels_are_drawn_on_a_checkerboard() {
        let image = RgbaImage::from_pixel(16, 8, image::Rgba([10, 20, 30, 0]));
        let flat = flatten(&image);
        assert_eq!(flat.get_pixel(0, 0).0, [0x99; 3]);
        assert_eq!(flat.get_pixel(8, 0).0, [0x66; 3]);
        // opaque pixels are left alone
        let opaque = RgbaImage::from_pixel(1, 1, image::Rgba([10, 20, 30, 255]));
        assert_eq!(flatten(&opaque).get_pixel(0, 0).0, [10, 20, 30]);
    }

    #[test]
    fn kitty_sends_the_png_in_chunks() {
        let png = png_bytes(&gradient(64, 64));
        assert!(png.starts_with(b"\x89PNG"));

        let placement = place((64, 64), (20, 10), (8, 16));
        let encoded = kitty(&png, &placement, 7);
        let text = String::from_utf8(encoded.payload).unwrap();

        let commands: Vec<&str> = text
            .split("\x1b\\")
            .filter(|command| !command.is_empty())
            .collect();
        assert!(commands.len() > 1, "the PNG needs more than one chunk");
        assert!(commands[0].starts_with(&format!(
            "\x1b_Ga=T,f=100,t=d,i=7,c={},r={},C=1,z=-1,q=2,m=1;",
            placement.cols, placement.rows
        )));
        // the middle chunks continue, the last one ends the transfer
        for command in &commands[1..commands.len() - 1] {
            assert!(command.starts_with("\x1b_Gm=1;"));
        }
        assert!(commands.last().unwrap().starts_with("\x1b_Gm=0;"));

        // the data of the chunks is the PNG
        let data: String = commands
            .iter()
            .map(|command| command.split_once(';').unwrap().1)
            .collect();
        assert!(commands
            .iter()
            .all(|command| command.split_once(';').unwrap().1.len() <= 4096));
        assert_eq!(BASE64.decode(data).unwrap(), png);

        assert_eq!(
            encoded.cleanup.as_deref(),
            Some(&b"\x1b_Ga=d,d=I,i=7,q=2\x1b\\"[..])
        );
    }

    #[test]
    fn a_small_image_is_one_kitty_chunk() {
        let png = png_bytes(&gradient(2, 2));
        let placement = place((2, 2), (20, 10), (8, 16));
        let text = String::from_utf8(kitty(&png, &placement, 1).payload).unwrap();
        assert!(text.contains(",m=0;"));
        assert_eq!(text.matches("\x1b_G").count(), 1);
    }

    #[test]
    fn iterm2_inlines_the_file() {
        let png = png_bytes(&gradient(8, 8));
        let placement = place((8, 8), (20, 10), (8, 16));
        let encoded = iterm2(&png, &placement);
        let text = String::from_utf8(encoded.payload).unwrap();
        let (header, data) = text.split_once(':').unwrap();
        assert_eq!(
            header,
            format!(
                "\x1b]1337;File=inline=1;size={};width={};height={};preserveAspectRatio=0",
                png.len(),
                placement.cols,
                placement.rows
            )
        );
        assert_eq!(
            BASE64.decode(data.strip_suffix('\x07').unwrap()).unwrap(),
            png
        );
        assert_eq!(encoded.cleanup, None);
    }

    /// Reads back the pixels (as palette colours) of the Sixel images made here.
    fn decode_sixel(bytes: &[u8]) -> (u32, u32, Vec<Option<[u8; 3]>>) {
        let text = std::str::from_utf8(bytes).unwrap();
        let body = text
            .strip_prefix("\x1bPq")
            .and_then(|body| body.strip_suffix("\x1b\\"))
            .unwrap();
        let rest = body.strip_prefix("\"1;1;").unwrap();
        let (size, rest) = rest.split_at(rest.find('#').unwrap_or(rest.len()));
        let (width, height) = size.split_once(';').unwrap();
        let (width, height): (u32, u32) = (width.parse().unwrap(), height.parse().unwrap());

        let mut palette = std::collections::HashMap::new();
        let mut pixels = vec![None; (width * height) as usize];
        let (mut color, mut x, mut band) = (0usize, 0u32, 0u32);

        let mut chars = rest.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '#' => {
                    let mut number = String::new();
                    while let Some(d) = chars.next_if(|d| d.is_ascii_digit()) {
                        number.push(d);
                    }
                    color = number.parse().unwrap();
                    if chars.next_if_eq(&';').is_some() {
                        // definition: 2;r;g;b
                        let mut parts = Vec::new();
                        let mut part = String::new();
                        while let Some(d) = chars.next_if(|d| d.is_ascii_digit() || *d == ';') {
                            if d == ';' {
                                parts.push(std::mem::take(&mut part));
                            } else {
                                part.push(d);
                            }
                        }
                        parts.push(part);
                        assert_eq!(parts[0], "2");
                        let percent = |s: &String| (s.parse::<u32>().unwrap() * 255 / 100) as u8;
                        palette.insert(
                            color,
                            [percent(&parts[1]), percent(&parts[2]), percent(&parts[3])],
                        );
                    }
                }
                '$' => x = 0,
                '-' => {
                    x = 0;
                    band += 1;
                }
                '!' => {
                    let mut number = String::new();
                    while let Some(d) = chars.next_if(|d| d.is_ascii_digit()) {
                        number.push(d);
                    }
                    let run: u32 = number.parse().unwrap();
                    let value = chars.next().unwrap() as u8 - 63;
                    for _ in 0..run {
                        paint(&mut pixels, &palette, color, width, height, band, x, value);
                        x += 1;
                    }
                }
                c => {
                    paint(
                        &mut pixels,
                        &palette,
                        color,
                        width,
                        height,
                        band,
                        x,
                        c as u8 - 63,
                    );
                    x += 1;
                }
            }
        }
        (width, height, pixels)
    }

    #[allow(clippy::too_many_arguments)]
    fn paint(
        pixels: &mut [Option<[u8; 3]>],
        palette: &std::collections::HashMap<usize, [u8; 3]>,
        color: usize,
        width: u32,
        height: u32,
        band: u32,
        x: u32,
        value: u8,
    ) {
        for bit in 0..6 {
            let y = band * 6 + bit;
            if value & (1 << bit) != 0 && y < height {
                assert!(x < width, "a sixel past the end of the row");
                pixels[(y * width + x) as usize] = Some(palette[&color]);
            }
        }
    }

    #[test]
    fn sixel_round_trips_through_its_palette() {
        // 7 rows: a band of 6 and one of 1; stripes and a long run of one colour
        let image = RgbImage::from_fn(20, 7, |x, y| match (x, y) {
            (0..=2, _) => Rgb([255, 0, 0]),
            (3..=4, 0..=2) => Rgb([0, 255, 0]),
            (_, 6) => Rgb([255, 255, 255]),
            _ => Rgb([0, 0, 0]),
        });
        let encoded = sixel(&image);
        let (width, height, pixels) = decode_sixel(&encoded.payload);
        assert_eq!((width, height), (20, 7));

        for (i, pixel) in image.pixels().enumerate() {
            let expected = sixel_color(sixel_index(*pixel) as usize);
            assert_eq!(pixels[i], Some(expected), "pixel {i}");
        }
        // pure primaries and greys are in the palette exactly
        assert_eq!(pixels[0], Some([255, 0, 0]));
        assert_eq!(pixels[3], Some([0, 255, 0]));
        assert_eq!(pixels[6 * 20 + 5], Some([255, 255, 255]));
    }

    #[test]
    fn sixel_runs_are_run_length_encoded() {
        let image = RgbImage::from_pixel(30, 6, Rgb([255, 0, 0]));
        let text = String::from_utf8(sixel(&image).payload).unwrap();
        // thirty columns with all six bits set: `~` is 63 + 63
        assert!(text.contains("!30~"), "{text}");
    }

    #[test]
    fn palette_covers_the_cube_and_the_greys() {
        assert_eq!(sixel_color(sixel_index(Rgb([0, 0, 0])) as usize), [0, 0, 0]);
        assert_eq!(
            sixel_color(sixel_index(Rgb([255, 255, 255])) as usize),
            [255, 255, 255]
        );
        // a colour lands within half a step of the cube
        let [r, g, b] = sixel_color(sixel_index(Rgb([200, 90, 30])) as usize);
        assert!(r.abs_diff(200) <= 26 && g.abs_diff(90) <= 26 && b.abs_diff(30) <= 26);
    }

    #[test]
    fn loads_and_decodes_a_png_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pic.png");
        std::fs::write(&path, png_bytes(&gradient(30, 20))).unwrap();

        assert_eq!(dimensions(&path).unwrap(), (30, 20));
        let image = load(&path).unwrap();
        assert_eq!(image.size(), (30, 20));

        let broken = dir.path().join("broken.png");
        std::fs::write(&broken, b"\x89PNG\r\n\x1a\nnot really").unwrap();
        assert!(load(&broken).is_err());
        assert!(load(&dir.path().join("missing.png")).is_err());
    }

    #[test]
    fn every_supported_format_decodes() {
        let dir = tempfile::tempdir().unwrap();
        let source = image::DynamicImage::ImageRgba8(gradient(40, 30));

        for name in ["pic.png", "pic.jpg", "pic.gif", "pic.webp"] {
            let path = dir.path().join(name);
            // JPEG has no alpha channel
            match name {
                "pic.jpg" => source.to_rgb8().save(&path),
                _ => source.save(&path),
            }
            .unwrap_or_else(|err| panic!("{name}: {err}"));

            let head = std::fs::read(&path).unwrap();
            assert!(
                is_image(&head[..head.len().min(1024)]),
                "{name} is not recognized"
            );
            assert_eq!(dimensions(&path).unwrap(), (40, 30), "{name}");
            let image = load(&path).unwrap_or_else(|err| panic!("{name}: {err}"));
            assert_eq!(image.size(), (40, 30), "{name}");
        }
    }

    #[test]
    fn the_format_comes_from_the_content_not_the_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("looks-like-text.txt");
        std::fs::write(&path, png_bytes(&gradient(5, 5))).unwrap();
        assert_eq!(load(&path).unwrap().size(), (5, 5));
    }

    #[test]
    fn config_names() {
        let config: ImagePreviewConfig =
            toml::from_str("enable = false\nprotocol = \"iterm2\"").unwrap();
        assert!(!config.enable);
        assert_eq!(config.protocol, ProtocolConfig::Iterm2);
        assert_eq!(ImagePreviewConfig::default().protocol, ProtocolConfig::Auto);
        assert!(toml::from_str::<ImagePreviewConfig>("protocol = \"bmp\"").is_err());
    }
}
