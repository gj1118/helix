//! The Helix logo of the welcome screen, drawn with half blocks (`▀`, `▄`, `█`): two
//! pixels per terminal cell, one above the other, so that the pixels are about square.
//!
//! The mark is traced from `logo.svg` at two sizes; the wordmark ("HELIX") is three rows of
//! half-block letters with a shade under their bars.

use helix_view::graphics::{Color, Style};
use tui::text::{Span, Spans};

/// The mark at 15 by 24 pixels: 15 columns, 12 rows.
///
/// Each letter is a color of the mark ([`mark_color`]), `.` is no pixel.
pub const MARK_SMALL: [&str; 24] = [
    "p..............",
    "ppp............",
    "pppp..........c",
    "pppppp......ccc",
    "pppppppp...cccc",
    "..pppppppcccccc",
    "....ppppccccccc",
    ".....pccccccc..",
    "....cccccccppp.",
    "..ccccccccppppv",
    ".ccccccc..ppvvv",
    "cccccc....vvvvv",
    "ccccc....vvvvvv",
    "cccbb..vvvvvvv.",
    "cbbbbvvvvvvvv..",
    ".bbbvvvvvvv....",
    "..vvvvvvvb.....",
    "vvvvvvvvbbb....",
    "vvvvvvbbbbbbb..",
    "vvvv...bbbbbbbb",
    "vvv......bbbbbb",
    "v..........bbbb",
    "............bbb",
    "..............b",
];

/// The mark at 19 by 30 pixels: 19 columns, 15 rows.
pub const MARK_LARGE: [&str; 30] = [
    "p..................",
    "ppp................",
    "pppp..............c",
    "pppppp...........cc",
    "pppppppp........ccc",
    "pppppppppp....ccccc",
    "..ppppppppp.ccccccc",
    "....pppppppcccccccc",
    ".....ppppccccccccc.",
    ".......cccccccccc..",
    ".....ccccccccccppp.",
    "....cccccccccpppppv",
    "..ccccccccc.ppppvvv",
    "cccccccccc....pvvvv",
    "cccccccc.....vvvvvv",
    "cccccc.....vvvvvvvv",
    "ccccb....vvvvvvvvvv",
    "cccbbbb.vvvvvvvvv..",
    "cbbbbbvvvvvvvvvv...",
    ".bbbvvvvvvvvvv.....",
    "..vvvvvvvvvv.......",
    ".vvvvvvvvvbbbb.....",
    "vvvvvvvvvbbbbbb....",
    "vvvvvvv.bbbbbbbbb..",
    "vvvvv....bbbbbbbbbb",
    "vvv........bbbbbbbb",
    "vv...........bbbbbb",
    "v..............bbbb",
    "................bbb",
    "..................b",
];

/// The colors of the logo.
const DARK_PURPLE: Color = Color::Rgb(112, 107, 200);
const BRIGHT_PURPLE: Color = Color::Rgb(153, 123, 200);
const LIGHT_BLUE: Color = Color::Rgb(132, 221, 234);
const BRIGHT_BLUE: Color = Color::Rgb(85, 197, 228);

/// The color of a pixel of a mark: `p` dark purple, `v` bright purple, `c` light blue, `b`
/// bright blue; `None` for no pixel.
fn mark_color(pixel: u8) -> Option<Color> {
    match pixel {
        b'p' => Some(DARK_PURPLE),
        b'v' => Some(BRIGHT_PURPLE),
        b'c' => Some(LIGHT_BLUE),
        b'b' => Some(BRIGHT_BLUE),
        _ => None,
    }
}

/// The width of a mark, in columns.
pub fn width(pixels: &[&str]) -> u16 {
    pixels.iter().map(|row| row.len()).max().unwrap_or(0) as u16
}

/// The rows of a mark (`pixels`, an even number of rows of pixels): a row of cells per two
/// rows of pixels. In color, a cell whose two pixels differ is an upper half block in the
/// color of the upper pixel on the color of the lower one. Without color, the shape alone,
/// in the text's color.
pub fn mark(pixels: &[&str], colorful: bool) -> Vec<Spans<'static>> {
    pixels
        .chunks(2)
        .map(|pair| {
            let upper = pair[0].as_bytes();
            let lower = pair.get(1).map_or(&[][..], |row| row.as_bytes());
            let cells = (0..upper.len().max(lower.len())).map(|column| {
                let pixel = |row: &[u8]| row.get(column).copied().and_then(mark_color);
                half_block(pixel(upper), pixel(lower), colorful)
            });
            Spans(cells.collect())
        })
        .collect()
}

/// The cell of two pixels, `upper` and `lower` (`None` for no pixel).
fn half_block(upper: Option<Color>, lower: Option<Color>, colorful: bool) -> Span<'static> {
    let (symbol, fg, bg) = match (upper, lower) {
        (None, None) => (" ", None, None),
        (Some(upper), None) => ("▀", Some(upper), None),
        (None, Some(lower)) => ("▄", Some(lower), None),
        (Some(upper), Some(lower)) if upper == lower || !colorful => ("█", Some(upper), None),
        (Some(upper), Some(lower)) => ("▀", Some(upper), Some(lower)),
    };
    let style = if colorful {
        Style {
            fg,
            bg,
            ..Style::default()
        }
    } else {
        Style::default()
    };
    Span::styled(symbol, style)
}

/// The letters of the wordmark, three rows each. Besides half blocks, `^` is the upper half
/// of the letter over its shade (the shade under a bar).
const LETTERS: [[&str; 3]; 5] = [
    ["█  █", "█^^█", "▀  ▀"],
    ["█▀▀▀", "█^^ ", "▀▀▀▀"],
    ["█   ", "█   ", "▀▀▀▀"],
    ["▀█▀", " █ ", "▀▀▀"],
    ["▀▄ ▄▀", " ▄▀▄ ", "▀   ▀"],
];

/// Between two letters of the wordmark.
const LETTER_GAP: &str = " ";

/// The width of the wordmark, in columns.
pub fn wordmark_width() -> u16 {
    let letters: usize = LETTERS.iter().map(|letter| letter[0].chars().count()).sum();
    (letters + LETTER_GAP.len() * (LETTERS.len() - 1)) as u16
}

/// The wordmark: "HELIX" in three rows of half blocks, from the logo's purple to its blue,
/// the shade under the bars a quarter of the way from `background` to the letter's color.
/// Without color (or without an RGB `background`, for the shade), plain.
pub fn wordmark(colorful: bool, background: Option<Color>) -> Vec<Spans<'static>> {
    let (Color::Rgb(from_r, from_g, from_b), Color::Rgb(to_r, to_g, to_b)) =
        (DARK_PURPLE, BRIGHT_BLUE)
    else {
        unreachable!("the logo's colors are RGB")
    };
    let steps = (LETTERS.len() - 1) as f32;
    (0..3)
        .map(|row| {
            let mut spans = Vec::new();
            for (n, letter) in LETTERS.iter().enumerate() {
                if n > 0 {
                    spans.push(Span::raw(LETTER_GAP));
                }
                let t = n as f32 / steps;
                let blend = |from: u8, to: u8| (from as f32 + (to as f32 - from as f32) * t) as u8;
                let color = Color::Rgb(
                    blend(from_r, to_r),
                    blend(from_g, to_g),
                    blend(from_b, to_b),
                );
                let shade = match background {
                    Some(Color::Rgb(r, g, b)) if colorful => Some(mix((r, g, b), color, 0.25)),
                    _ => None,
                };
                for symbol in letter[row].chars() {
                    let span = match symbol {
                        ' ' => Span::raw(" "),
                        '^' => {
                            let mut style = Style::default();
                            if colorful {
                                style.fg = Some(color);
                                style.bg = shade;
                            }
                            Span::styled("▀", style)
                        }
                        symbol => {
                            let style = if colorful {
                                Style::default().fg(color)
                            } else {
                                Style::default()
                            };
                            Span::styled(symbol.to_string(), style)
                        }
                    };
                    spans.push(span);
                }
            }
            Spans(spans)
        })
        .collect()
}

/// `background` moved `amount` of the way to `color` (an RGB color).
fn mix(background: (u8, u8, u8), color: Color, amount: f32) -> Color {
    let Color::Rgb(r, g, b) = color else {
        return color;
    };
    let step = |from: u8, to: u8| (from as f32 + (to as f32 - from as f32) * amount).round() as u8;
    Color::Rgb(
        step(background.0, r),
        step(background.1, g),
        step(background.2, b),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(rows: &[Spans]) -> Vec<String> {
        rows.iter()
            .map(|row| row.0.iter().map(|span| span.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn the_marks_are_rectangles_of_known_pixels() {
        for (pixels, columns, rows) in
            [(&MARK_SMALL[..], 15u16, 12usize), (&MARK_LARGE[..], 19, 15)]
        {
            assert_eq!(width(pixels), columns);
            assert!(pixels.iter().all(|row| row.len() == columns as usize));
            assert!(pixels
                .iter()
                .all(|row| row.bytes().all(|p| p == b'.' || mark_color(p).is_some())));
            let cells: Vec<Spans> = mark(pixels, true);
            assert_eq!(cells.len(), rows);
            assert!(cells.iter().all(|row| row.width() == columns as usize));
        }
    }

    #[test]
    fn a_cell_shows_its_two_pixels() {
        // the first rows of the small mark: `p.`, `pp`, then `pp`, `pp`
        let rows = mark(&["p.", "pp", "pp", "pp"], true);
        let first: Vec<(String, Option<Color>, Option<Color>)> = rows[0]
            .0
            .iter()
            .map(|span| (span.content.to_string(), span.style.fg, span.style.bg))
            .collect();
        assert_eq!(
            first,
            [
                ("█".to_string(), Some(DARK_PURPLE), None),
                ("▄".to_string(), Some(DARK_PURPLE), None),
            ]
        );
        // two colors: the upper half in the upper one, on the lower one
        let cell = &mark(&["v", "b"], true)[0].0[0];
        assert_eq!(cell.content, "▀");
        assert_eq!(
            (cell.style.fg, cell.style.bg),
            (Some(BRIGHT_PURPLE), Some(BRIGHT_BLUE))
        );
        // without color: the shape, in the text's color
        let plain = mark(&["v.", "b."], false);
        assert_eq!(text(&plain), ["█ "]);
        assert!(plain[0].0.iter().all(|span| span.style == Style::default()));
    }

    #[test]
    fn the_wordmark_spells_helix_with_a_shade_under_its_bars() {
        let rows = wordmark(true, Some(Color::Rgb(0, 0, 0)));
        assert_eq!(
            text(&rows),
            [
                "█  █ █▀▀▀ █    ▀█▀ ▀▄ ▄▀",
                "█▀▀█ █▀▀  █     █   ▄▀▄ ",
                "▀  ▀ ▀▀▀▀ ▀▀▀▀ ▀▀▀ ▀   ▀",
            ]
        );
        let widths: Vec<usize> = rows.iter().map(|row| row.width()).collect();
        assert_eq!(widths, [wordmark_width() as usize; 3]);
        // from the purple to the blue
        let first: Option<Color> = rows[0].0[0].style.fg;
        assert_eq!(first, Some(DARK_PURPLE));
        let last = rows[0]
            .0
            .iter()
            .rev()
            .find(|span| span.content != " ")
            .unwrap();
        assert_eq!(last.style.fg, Some(BRIGHT_BLUE));
        // the bar of the H: its color over a quarter of it on the black background
        let bar = &rows[1].0[1];
        assert_eq!(bar.content, "▀");
        assert_eq!(bar.style.fg, Some(DARK_PURPLE));
        assert_eq!(bar.style.bg, Some(Color::Rgb(28, 27, 50)));

        // no RGB background: no shade; no color: plain
        let rows = wordmark(true, Some(Color::Reset));
        assert_eq!(rows[1].0[1].style.bg, None);
        let rows = wordmark(false, Some(Color::Rgb(0, 0, 0)));
        assert!(rows
            .iter()
            .flat_map(|row| &row.0)
            .all(|span| span.style == Style::default()));
    }
}
