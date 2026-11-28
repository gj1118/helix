//! The scrollbar on the right of each view (`[editor.scrollbar]`), after nvim-scrollbar:
//! where the lines of a buffer go on it, where its handle is, and what each row shows.
//!
//! The bar has a row per row of the view. A buffer that fits gets a row per line, as
//! nvim-scrollbar does; a longer one is spread out over the rows. Folded lines can be left out
//! (`folds`): a line in a fold is where its fold is. The handle covers the rows of the lines on
//! screen. Of the marks on a row the one of the lowest priority is drawn, with the text of its
//! next level when the row has more than one mark (`-`, then `=`).

use crate::document::Document;
use crate::editor::{
    ScrollbarConfig, ScrollbarMarkConfig, ScrollbarMarkText, ScrollbarMarksConfig,
};
use crate::graphics::Style;

/// The types of marks, as nvim-scrollbar's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MarkType {
    Cursor,
    Search,
    Error,
    Warn,
    Info,
    Hint,
    Misc,
    GitAdd,
    GitChange,
    GitDelete,
}

impl MarkType {
    pub const ALL: [Self; 10] = [
        Self::Cursor,
        Self::Search,
        Self::Error,
        Self::Warn,
        Self::Info,
        Self::Hint,
        Self::Misc,
        Self::GitAdd,
        Self::GitChange,
        Self::GitDelete,
    ];

    /// Its name in the config and for plugins.
    pub fn name(self) -> &'static str {
        match self {
            Self::Cursor => "cursor",
            Self::Search => "search",
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Hint => "hint",
            Self::Misc => "misc",
            Self::GitAdd => "git-add",
            Self::GitChange => "git-change",
            Self::GitDelete => "git-delete",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }

    /// nvim-scrollbar's text: a level per entry.
    pub fn default_text(self) -> &'static [&'static str] {
        match self {
            Self::Cursor => &["•"],
            Self::GitAdd | Self::GitChange => &["┆"],
            Self::GitDelete => &["▁"],
            _ => &["-", "="],
        }
    }

    /// nvim-scrollbar's priority: of the marks on a row, the lowest is drawn.
    pub fn default_priority(self) -> u8 {
        match self {
            Self::Cursor => 0,
            Self::Search => 1,
            Self::Error => 2,
            Self::Warn => 3,
            Self::Info => 4,
            Self::Hint => 5,
            Self::Misc => 6,
            Self::GitAdd | Self::GitChange | Self::GitDelete => 7,
        }
    }

    /// The theme keys its colour comes from, in order: the first with a foreground is used,
    /// else the first with a background (as its foreground).
    pub fn scopes(self) -> &'static [&'static str] {
        match self {
            Self::Cursor => &["ui.scrollbar.cursor", "ui.text"],
            Self::Search => &["ui.scrollbar.search", "ui.cursor.match", "ui.selection"],
            Self::Error => &["ui.scrollbar.error", "error"],
            Self::Warn => &["ui.scrollbar.warning", "warning"],
            Self::Info => &["ui.scrollbar.info", "info"],
            Self::Hint => &["ui.scrollbar.hint", "hint"],
            Self::Misc => &["ui.scrollbar.misc", "ui.text"],
            Self::GitAdd => &["ui.scrollbar.git.add", "diff.plus"],
            Self::GitChange => &["ui.scrollbar.git.change", "diff.delta"],
            Self::GitDelete => &["ui.scrollbar.git.delete", "diff.minus"],
        }
    }

    /// Its settings.
    pub fn config(self, marks: &ScrollbarMarksConfig) -> &ScrollbarMarkConfig {
        match self {
            Self::Cursor => &marks.cursor,
            Self::Search => &marks.search,
            Self::Error => &marks.error,
            Self::Warn => &marks.warn,
            Self::Info => &marks.info,
            Self::Hint => &marks.hint,
            Self::Misc => &marks.misc,
            Self::GitAdd => &marks.git_add,
            Self::GitChange => &marks.git_change,
            Self::GitDelete => &marks.git_delete,
        }
    }

    /// Its text for `level` (1 the first), the last one's when it has fewer levels.
    pub fn text(self, config: &ScrollbarConfig, level: usize) -> &str {
        let index = level.max(1) - 1;
        match &self.config(&config.marks).text {
            Some(ScrollbarMarkText::One(text)) => text,
            Some(ScrollbarMarkText::Levels(levels)) if !levels.is_empty() => {
                &levels[index.min(levels.len() - 1)]
            }
            _ => {
                let levels = self.default_text();
                levels[index.min(levels.len() - 1)]
            }
        }
    }

    /// The number of its levels.
    pub fn levels(self, config: &ScrollbarConfig) -> usize {
        match &self.config(&config.marks).text {
            Some(ScrollbarMarkText::One(_)) => 1,
            Some(ScrollbarMarkText::Levels(levels)) if !levels.is_empty() => levels.len(),
            _ => self.default_text().len(),
        }
    }

    pub fn priority(self, config: &ScrollbarConfig) -> u8 {
        self.config(&config.marks)
            .priority
            .unwrap_or_else(|| self.default_priority())
    }
}

/// Whether `doc` has a scrollbar: it is on, the buffer's language is not excluded, and the
/// buffer is not longer than `max-lines`.
pub fn applies(config: &ScrollbarConfig, doc: &Document) -> bool {
    config.enable
        && !doc
            .language_name()
            .is_some_and(|name| config.excluded_languages.iter().any(|l| l == name))
        && (config.max_lines == 0 || line_count(doc.text().slice(..)) <= config.max_lines)
}

/// The lines of `text`: the empty one after a final line ending is not one.
pub fn line_count(text: helix_core::RopeSlice) -> usize {
    let lines = text.len_lines();
    if lines > 1 && text.line(lines - 1).len_chars() == 0 {
        lines - 1
    } else {
        lines
    }
}

/// The parts of a row the handle's ends move by.
pub const EIGHTHS: usize = 8;

/// Where the lines of a buffer go on a bar.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    rows: usize,
    /// The lines of the buffer, and those counted (not folded away).
    total: usize,
    lines: usize,
    /// The folds left out: their first and last line, and the lines folded before them.
    folds: Vec<(usize, usize, usize)>,
}

impl Layout {
    /// A bar of `rows` rows for a buffer of `lines` lines, without the lines of `folds`
    /// (their first and last line: the lines hidden under their header).
    pub fn new(rows: u16, lines: usize, folds: impl IntoIterator<Item = (usize, usize)>) -> Self {
        let mut folds: Vec<(usize, usize)> = folds
            .into_iter()
            .filter(|&(first, last)| first <= last && first < lines)
            .map(|(first, last)| (first, last.min(lines.saturating_sub(1))))
            .collect();
        folds.sort_unstable();
        let mut hidden = 0;
        let mut kept: Vec<(usize, usize, usize)> = Vec::with_capacity(folds.len());
        for (first, last) in folds {
            // (a fold inside one already left out)
            if kept.last().is_some_and(|&(_, end, _)| first <= end) {
                continue;
            }
            kept.push((first, last, hidden));
            hidden += last - first + 1;
        }
        Self {
            rows: rows.into(),
            total: lines.max(1),
            lines: lines.saturating_sub(hidden).max(1),
            folds: kept,
        }
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    /// The lines counted: those not folded away.
    pub fn lines(&self) -> usize {
        self.lines
    }

    /// The place of `line` among the lines counted (a folded line is at its fold's header).
    fn counted(&self, line: usize) -> usize {
        let index = self.folds.partition_point(|&(first, _, _)| first <= line);
        match index.checked_sub(1).map(|i| self.folds[i]) {
            // in the fold: at its header, the line before it
            Some((first, last, hidden)) if line <= last => first.saturating_sub(hidden + 1),
            // past it: less the lines folded so far
            Some((first, last, hidden)) => line - hidden - (last + 1 - first),
            None => line,
        }
    }

    /// The row of `line`: its own when the lines fit, else its share of the rows.
    pub fn row(&self, line: usize) -> usize {
        let counted = self.counted(line).min(self.lines - 1);
        let row = if self.lines <= self.rows {
            counted
        } else {
            counted * self.rows / self.lines
        };
        row.min(self.rows.saturating_sub(1))
    }

    /// The place of `line`'s top on the bar, in eighths of rows (see [`Self::row`]).
    fn eighth(&self, line: usize) -> usize {
        let counted = self.counted(line).min(self.lines);
        if self.lines <= self.rows {
            counted * EIGHTHS
        } else {
            counted * self.rows * EIGHTHS / self.lines
        }
    }

    /// The handle of a screen showing the lines `first..=last`, in eighths of rows. Its length
    /// is the share of the lines a screen holds (as many lines as it has rows, at least a
    /// row of the bar), the same wherever it is: not the lines actually on screen, which are
    /// fewer where lines are wrapped or have diagnostics under them, so a handle of those
    /// would shrink and grow as it moves. Its top is at the first line, and at the bottom of
    /// the lines once the last one is on screen.
    pub fn handle(&self, first: usize, last: usize) -> (usize, usize) {
        // the eighths the lines take on the bar: all of it, or a row each when they fit
        let span = if self.lines <= self.rows {
            self.lines * EIGHTHS
        } else {
            self.rows * EIGHTHS
        };
        let length = if self.lines <= self.rows {
            self.rows.min(self.lines) * EIGHTHS
        } else {
            (self.rows * self.rows * EIGHTHS + self.lines / 2) / self.lines
        }
        .clamp(EIGHTHS.min(span), span);
        let at_end = self.counted(last) + 1 >= self.lines;
        let top = if at_end {
            span - length
        } else {
            self.eighth(first).min(span - length)
        };
        (top, top + length)
    }

    /// The line whose top is at `eighth` of the bar (the first one at or past it).
    pub fn line_at(&self, eighth: usize) -> usize {
        let counted = if self.lines <= self.rows {
            eighth / EIGHTHS
        } else {
            eighth * self.lines / (self.rows * EIGHTHS).max(1)
        }
        .min(self.lines - 1);
        partition(self.total, |line| self.counted(line) < counted).min(self.total - 1)
    }

    /// The line `eighths` eighths of a row further down the bar than `line` (up when
    /// negative): where a handle dragged that far from `line` puts the top of the view.
    pub fn moved(&self, line: usize, eighths: isize) -> usize {
        let lines_per_eighth = if self.lines <= self.rows {
            1.0 / EIGHTHS as f64
        } else {
            self.lines as f64 / (self.rows * EIGHTHS) as f64
        };
        let counted = (self.counted(line) as f64 + eighths as f64 * lines_per_eighth)
            .round()
            .clamp(0.0, (self.lines - 1) as f64) as usize;
        partition(self.total, |line| self.counted(line) < counted).min(self.total - 1)
    }

    /// The first and last of the lines `first..=last` on `row`, if any.
    fn lines_on_row(&self, row: usize, first: usize, last: usize) -> Option<(usize, usize)> {
        // rows grow with lines: the first line at or past `row`, then the first past it
        let start = first + partition(last + 1 - first, |i| self.row(first + i) < row);
        let end = first + partition(last + 1 - first, |i| self.row(first + i) <= row);
        (start < end).then(|| (start, end - 1))
    }
}

/// The first of `0..len` for which `before` is false (it is true, then false).
fn partition(len: usize, before: impl Fn(usize) -> bool) -> usize {
    let (mut low, mut high) = (0, len);
    while low < high {
        let mid = low + (high - low) / 2;
        if before(mid) {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    low
}

/// A mark on the bar.
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    pub kind: MarkType,
    /// Its text's level: 1, or higher for another of its type's texts.
    pub level: usize,
    /// Its own text instead of its type's (a plugin's).
    pub text: Option<String>,
    /// Its own colour: a theme key, then a style over it (a plugin's).
    pub scope: Option<String>,
    pub style: Option<Style>,
}

impl Mark {
    pub fn new(kind: MarkType) -> Self {
        Self {
            kind,
            level: 1,
            text: None,
            scope: None,
            style: None,
        }
    }
}

/// The mark a row shows, and how many marks are on it.
#[derive(Debug, Clone, PartialEq)]
pub struct RowMark {
    pub mark: Mark,
    pub count: usize,
}

impl RowMark {
    /// Its text: its own, or its type's for its level; with more than one mark on the row,
    /// its type's next level (when it has one).
    pub fn text<'a>(&'a self, config: &'a ScrollbarConfig) -> &'a str {
        let kind = self.mark.kind;
        if self.count > 1 && self.mark.level < kind.levels(config) {
            return kind.text(config, self.mark.level + 1);
        }
        match &self.mark.text {
            Some(text) => text,
            None => kind.text(config, self.mark.level),
        }
    }
}

/// The marks of each row of a bar.
#[derive(Debug, Clone)]
pub struct Rows<'a> {
    layout: &'a Layout,
    config: &'a ScrollbarConfig,
    rows: Vec<Option<RowMark>>,
}

impl<'a> Rows<'a> {
    pub fn new(layout: &'a Layout, config: &'a ScrollbarConfig) -> Self {
        Self {
            layout,
            config,
            rows: vec![None; layout.rows()],
        }
    }

    /// Puts `count` marks like `mark` on `row`: the one of the lowest priority is kept (the
    /// first of equal ones).
    fn put(&mut self, row: usize, mark: &Mark, count: usize) {
        let Some(slot) = self.rows.get_mut(row) else {
            return;
        };
        match slot {
            Some(kept) => {
                if mark.kind.priority(self.config) < kept.mark.kind.priority(self.config) {
                    kept.mark = mark.clone();
                }
                kept.count += count;
            }
            None => {
                *slot = Some(RowMark {
                    mark: mark.clone(),
                    count,
                })
            }
        }
    }

    /// A mark on `line`.
    pub fn add(&mut self, line: usize, mark: &Mark) {
        if self.layout.rows() > 0 {
            self.put(self.layout.row(line), mark, 1);
        }
    }

    /// A mark on each of the lines `first..=last` (as quick for many lines as for a few:
    /// a row at a time).
    pub fn add_lines(&mut self, first: usize, last: usize, mark: &Mark) {
        if self.layout.rows() == 0 || first > last {
            return;
        }
        let (top, bottom) = (self.layout.row(first), self.layout.row(last));
        for row in top..=bottom {
            if let Some((start, end)) = self.layout.lines_on_row(row, first, last) {
                self.put(row, mark, end - start + 1);
            }
        }
    }

    pub fn into_rows(self) -> Vec<Option<RowMark>> {
        self.rows
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn lines_that_fit_have_their_own_row() {
        let layout = Layout::new(10, 4, []);
        assert_eq!(layout.lines(), 4);
        assert_eq!(
            (0..4).map(|line| layout.row(line)).collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
        // past the end: the last line's
        assert_eq!(layout.row(9), 3);
    }

    #[test]
    fn more_lines_than_rows_are_spread_out() {
        let layout = Layout::new(10, 100, []);
        assert_eq!(layout.row(0), 0);
        assert_eq!(layout.row(9), 0);
        assert_eq!(layout.row(10), 1);
        assert_eq!(layout.row(55), 5);
        assert_eq!(layout.row(99), 9);
        // the handle of lines 40..=59 covers rows 4 and 5
        assert_eq!((layout.row(40), layout.row(59)), (4, 5));
    }

    #[test]
    fn folded_lines_are_left_out() {
        // lines 2..=5 folded under line 1, lines 10..=11 under line 9 (the second fold
        // inside the first is ignored)
        let layout = Layout::new(20, 14, [(10, 11), (2, 5), (3, 4)]);
        assert_eq!(layout.lines(), 8);
        let rows: Vec<usize> = (0..14).map(|line| layout.row(line)).collect();
        assert_eq!(rows, vec![0, 1, 1, 1, 1, 1, 2, 3, 4, 5, 5, 5, 6, 7]);
    }

    #[test]
    fn the_lowest_priority_is_drawn_and_more_marks_raise_the_level() {
        let config = ScrollbarConfig::default();
        let layout = Layout::new(5, 50, []);
        let mut rows = Rows::new(&layout, &config);
        rows.add(0, &Mark::new(MarkType::Hint));
        rows.add(3, &Mark::new(MarkType::Error));
        rows.add(12, &Mark::new(MarkType::Warn));
        rows.add(45, &Mark::new(MarkType::Cursor));
        rows.add(46, &Mark::new(MarkType::Search));
        let rows = rows.into_rows();
        // row 0: an error over a hint, two marks: `=`
        let first = rows[0].as_ref().unwrap();
        assert_eq!((first.mark.kind, first.count), (MarkType::Error, 2));
        assert_eq!(first.text(&config), "=");
        // row 1: one warning: `-`
        assert_eq!(rows[1].as_ref().unwrap().text(&config), "-");
        assert_eq!(rows[2], None);
        // row 4: the cursor wins over the search mark; it has a single text
        let last = rows[4].as_ref().unwrap();
        assert_eq!((last.mark.kind, last.count), (MarkType::Cursor, 2));
        assert_eq!(last.text(&config), "•");
    }

    #[test]
    fn lines_of_a_range_are_counted_row_by_row() {
        let config = ScrollbarConfig::default();
        let layout = Layout::new(10, 1000, []);
        let mut rows = Rows::new(&layout, &config);
        // lines 95..=305: 5 lines on row 0, 100 on rows 1 and 2, 6 on row 3
        rows.add_lines(95, 305, &Mark::new(MarkType::GitAdd));
        let counts: Vec<Option<usize>> = rows
            .into_rows()
            .iter()
            .map(|row| row.as_ref().map(|row| row.count))
            .collect();
        assert_eq!(
            counts,
            vec![
                Some(5),
                Some(100),
                Some(100),
                Some(6),
                None,
                None,
                None,
                None,
                None,
                None
            ]
        );
    }

    #[test]
    fn marks_take_the_text_and_priority_set_for_their_type() {
        let mut config = ScrollbarConfig::default();
        config.marks.error.text = Some(ScrollbarMarkText::Levels(vec!["e".into(), "E".into()]));
        config.marks.hint.priority = Some(1);
        config.marks.misc.text = Some(ScrollbarMarkText::One("*".into()));
        assert_eq!(MarkType::Error.text(&config, 1), "e");
        assert_eq!(MarkType::Error.text(&config, 2), "E");
        assert_eq!(MarkType::Error.text(&config, 9), "E", "the last level's");
        assert_eq!(MarkType::Misc.text(&config, 2), "*");
        assert_eq!(MarkType::Warn.text(&config, 2), "=", "the default");

        let layout = Layout::new(5, 5, []);
        let mut rows = Rows::new(&layout, &config);
        rows.add(0, &Mark::new(MarkType::Error));
        rows.add(0, &Mark::new(MarkType::Hint));
        let row = rows.into_rows()[0].clone().unwrap();
        assert_eq!(
            row.mark.kind,
            MarkType::Hint,
            "priority 1 over the error's 2"
        );

        // a plugin's text, until more marks share its row
        let mut mark = Mark::new(MarkType::Misc);
        mark.text = Some("T".into());
        let one = RowMark {
            mark: mark.clone(),
            count: 1,
        };
        assert_eq!(one.text(&config), "T");
        let defaults = ScrollbarConfig::default();
        let two = RowMark { mark, count: 2 };
        assert_eq!(two.text(&defaults), "=");
    }

    #[test]
    fn the_handle_moves_by_eighths_of_rows() {
        // 1000 lines on 10 rows: 12.5 lines an eighth, a screen of 10 lines a tenth of a row,
        // made a row
        let layout = Layout::new(10, 1000, []);
        assert_eq!(layout.handle(0, 9), (0, 8));
        // 25 lines further: two eighths further
        assert_eq!(layout.handle(25, 34), (2, 10));
        // the last lines: at the bottom
        assert_eq!(layout.handle(990, 999), (72, 80));
        // lines that fit: a row each, the handle as long as the screen
        let fits = Layout::new(10, 6, []);
        assert_eq!(fits.handle(0, 5), (0, 48));
    }

    /// The handle is as long wherever it is, whatever the lines on screen: fewer where they
    /// are wrapped (or have diagnostics under them) than where they are not.
    #[test]
    fn the_handle_keeps_its_length_as_it_moves() {
        // 40 rows, 300 lines: a screen is 40 / 300 of the bar, 42.67 eighths: 43
        let layout = Layout::new(40, 300, []);
        let lengths: std::collections::HashSet<usize> = (0..260)
            .map(|first| {
                // a screen of 40 lines, or of 25 (wrapped ones)
                let last = first + if first % 3 == 0 { 24 } else { 39 };
                let (top, bottom) = layout.handle(first, last.min(299));
                assert!(bottom <= 320, "{first}: {top}..{bottom}");
                bottom - top
            })
            .collect();
        assert_eq!(lengths, [43].into());
        // and it moves down with the first line, to the bottom at the end
        let tops: Vec<usize> = (0..260)
            .map(|first| layout.handle(first, first + 39).0)
            .collect();
        assert!(tops.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(layout.handle(260, 299), (320 - 43, 320));
    }

    #[test]
    fn a_place_on_the_bar_is_a_line() {
        let layout = Layout::new(10, 1000, []);
        assert_eq!(layout.line_at(0), 0);
        assert_eq!(layout.line_at(40), 500);
        assert_eq!(layout.line_at(79), 987);
        assert_eq!(layout.line_at(500), 999, "past the end: the last line");
        // the way back of `handle`
        for line in [0, 100, 512, 900] {
            let (top, _) = layout.handle(line, line + 30);
            assert!(layout.line_at(top) <= line && line - layout.line_at(top) < 13);
        }
        // folded lines: the line after a fold, not one in it
        let folded = Layout::new(20, 14, [(2, 5)]);
        assert_eq!(folded.line_at(2 * 8), 6);
        assert_eq!(folded.line_at(8), 1);
    }

    #[test]
    fn a_handle_dragged_moves_the_lines_by_as_much() {
        // 1000 lines on 10 rows: 12.5 lines an eighth
        let layout = Layout::new(10, 1000, []);
        assert_eq!(layout.moved(500, 0), 500, "not moved: the same line");
        assert_eq!(layout.moved(500, 4), 550);
        assert_eq!(layout.moved(500, -3), 463, "37.5 lines up, rounded");
        assert_eq!(layout.moved(500, -80), 0);
        assert_eq!(layout.moved(500, 80), 999);
        // lines that fit: 8 eighths a line
        let fits = Layout::new(10, 6, []);
        assert_eq!(fits.moved(2, 8), 3);
        assert_eq!(fits.moved(2, 3), 2);
        // a fold of 4 lines is one: past it
        let folded = Layout::new(20, 14, [(2, 5)]);
        assert_eq!(folded.moved(1, 8), 6);
    }

    #[test]
    fn names_go_both_ways() {
        for kind in MarkType::ALL {
            assert_eq!(MarkType::from_name(kind.name()), Some(kind));
        }
        assert_eq!(MarkType::from_name("gitsigns"), None);
    }
}
