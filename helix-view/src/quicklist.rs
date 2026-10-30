//! The quicklist: locations the user collected to revisit.
//!
//! Unlike the jumplist, which records where you have been, entries are added on purpose: from
//! the matched entries of a picker, or one at a time from the cursor. The list is shared by all
//! views and lives as long as the editor.

use std::path::PathBuf;

use helix_core::Selection;

use crate::DocumentId;

/// What an entry points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A file, which is opened when the entry is visited.
    Path(PathBuf),
    /// A buffer that has no file, usable for as long as the buffer is open.
    Doc(DocumentId),
}

/// Where in the target an entry points. Entries don't follow later edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Position {
    /// A range of (zero-based, inclusive) lines, as given by picker results.
    Lines { start: usize, end: usize },
    /// The selections that were active when the entry was added.
    Selection(Selection),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Identifies the entry for as long as it is in the list, even when others are removed.
    pub id: u64,
    pub target: Target,
    pub position: Position,
    /// The line shown for the entry (zero-based).
    pub line: usize,
    /// The text at the location when the entry was added, if known.
    pub text: String,
}

impl Entry {
    pub fn new(target: Target, position: Position, line: usize, text: impl Into<String>) -> Self {
        Entry {
            id: 0,
            target,
            position,
            line,
            text: text.into(),
        }
    }
}

/// The quicklist position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Cursor {
    /// Nothing has been visited yet.
    #[default]
    None,
    /// The entry that was visited last.
    At(usize),
    /// The entry that was visited last was removed: the position is between the entries
    /// `i - 1` and `i`, so going forward visits `i` and going back visits `i - 1`.
    Gap(usize),
}

#[derive(Debug, Default)]
pub struct Quicklist {
    entries: Vec<Entry>,
    cursor: Cursor,
    next_id: u64,
    /// Changes whenever anything visible about the list changes, so views can refresh.
    version: u64,
}

impl Quicklist {
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    /// The index of the entry that was visited last, `None` if there is none or it was removed.
    pub fn current(&self) -> Option<usize> {
        match self.cursor {
            Cursor::At(index) => Some(index),
            _ => None,
        }
    }

    fn touch(&mut self) {
        self.version = self.version.wrapping_add(1);
    }

    fn assign_id(&mut self, entry: &mut Entry) {
        entry.id = self.next_id;
        self.next_id += 1;
    }

    /// Replace the whole list; the position is reset.
    pub fn replace(&mut self, entries: impl IntoIterator<Item = Entry>) {
        self.entries.clear();
        for mut entry in entries {
            self.assign_id(&mut entry);
            self.entries.push(entry);
        }
        self.cursor = Cursor::None;
        self.touch();
    }

    /// Add an entry at the end. The position is kept.
    pub fn push(&mut self, mut entry: Entry) {
        self.assign_id(&mut entry);
        self.entries.push(entry);
        self.touch();
    }

    /// Make `index` the current entry.
    pub fn set_current(&mut self, index: usize) {
        if index < self.entries.len() {
            self.cursor = Cursor::At(index);
            self.touch();
        }
    }

    /// Remove an entry. If it was the current one the position stays where it was, between its
    /// neighbours.
    pub fn remove(&mut self, index: usize) -> Option<Entry> {
        if index >= self.entries.len() {
            return None;
        }
        let entry = self.entries.remove(index);
        self.cursor = match self.cursor {
            Cursor::At(current) if current == index => Cursor::Gap(index),
            Cursor::At(current) if current > index => Cursor::At(current - 1),
            Cursor::Gap(gap) if gap > index => Cursor::Gap(gap - 1),
            cursor => cursor,
        };
        self.touch();
        Some(entry)
    }

    /// The index `count` matching entries away from the position, wrapping around at the ends.
    /// Returns `None` when no entry matches `filter`. When fewer than `count` entries match the
    /// walk keeps wrapping, so `count` is never an error.
    pub fn step(
        &self,
        forward: bool,
        count: usize,
        filter: impl Fn(&Entry) -> bool,
    ) -> Option<usize> {
        let len = self.entries.len();
        if len == 0 {
            return None;
        }
        // The next entry to look at.
        let mut next = match (self.cursor, forward) {
            (Cursor::None, true) => 0,
            (Cursor::None, false) => len - 1,
            (Cursor::At(i), true) => (i + 1) % len,
            (Cursor::At(i), false) => (i + len - 1) % len,
            (Cursor::Gap(i), true) => i % len,
            (Cursor::Gap(i), false) => (i + len - 1) % len,
        };
        let mut found = None;
        for _ in 0..count.max(1) {
            let hit = (0..len)
                .map(|offset| {
                    if forward {
                        (next + offset) % len
                    } else {
                        (next + len - offset % len) % len
                    }
                })
                .find(|&index| filter(&self.entries[index]))?;
            found = Some(hit);
            next = if forward {
                (hit + 1) % len
            } else {
                (hit + len - 1) % len
            };
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, line: usize) -> Entry {
        Entry::new(
            Target::Path(path.into()),
            Position::Lines {
                start: line,
                end: line,
            },
            line,
            "",
        )
    }

    fn list(entries: &[(&str, usize)]) -> Quicklist {
        let mut list = Quicklist::default();
        list.replace(entries.iter().map(|(path, line)| entry(path, *line)));
        list
    }

    fn any(_: &Entry) -> bool {
        true
    }

    #[test]
    fn stepping_wraps_in_both_directions() {
        let mut list = list(&[("a", 0), ("b", 0), ("c", 0)]);
        assert_eq!(list.step(true, 1, any), Some(0));
        assert_eq!(list.step(false, 1, any), Some(2));

        list.set_current(0);
        assert_eq!(list.step(true, 1, any), Some(1));
        assert_eq!(list.step(false, 1, any), Some(2));
        list.set_current(2);
        assert_eq!(list.step(true, 1, any), Some(0));
        assert_eq!(list.step(true, 4, any), Some(0));
        assert_eq!(list.step(false, 2, any), Some(0));
    }

    #[test]
    fn stepping_with_a_filter() {
        let mut list = list(&[("a", 0), ("b", 0), ("a", 1), ("b", 1), ("a", 2)]);
        let in_a = |entry: &Entry| entry.target == Target::Path("a".into());
        list.set_current(1);
        assert_eq!(list.step(true, 1, in_a), Some(2));
        assert_eq!(list.step(true, 2, in_a), Some(4));
        assert_eq!(list.step(true, 3, in_a), Some(0));
        assert_eq!(list.step(false, 1, in_a), Some(0));
        assert_eq!(list.step(false, 2, in_a), Some(4));
        let nowhere = |entry: &Entry| entry.target == Target::Path("z".into());
        assert_eq!(list.step(true, 1, nowhere), None);
        assert_eq!(Quicklist::default().step(true, 1, any), None);
    }

    #[test]
    fn replacing_resets_the_position_and_pushing_keeps_it() {
        let mut list = list(&[("a", 0), ("b", 0)]);
        list.set_current(1);
        list.push(entry("c", 0));
        assert_eq!(list.current(), Some(1));
        assert_eq!(list.len(), 3);
        list.replace([entry("d", 0)]);
        assert_eq!(list.current(), None);
        assert_eq!(list.len(), 1);
        list.replace([]);
        assert!(list.is_empty());
    }

    #[test]
    fn ids_are_unique_and_survive_removal() {
        let mut list = list(&[("a", 0), ("b", 0), ("c", 0)]);
        let ids: Vec<u64> = list.entries().iter().map(|e| e.id).collect();
        list.remove(1);
        assert_eq!(
            list.entries().iter().map(|e| e.id).collect::<Vec<_>>(),
            [ids[0], ids[2]]
        );
        list.push(entry("d", 0));
        let new_id = list.entries().last().unwrap().id;
        assert!(!ids.contains(&new_id));
    }

    #[test]
    fn removing_the_current_entry_keeps_the_position_between_its_neighbours() {
        // [a b c d], current = c
        let mut list = list(&[("a", 0), ("b", 0), ("c", 0), ("d", 0)]);
        list.set_current(2);
        list.remove(2);
        assert_eq!(list.current(), None);
        // [a b d]: forward visits d, back visits b
        assert_eq!(list.step(true, 1, any), Some(2));
        assert_eq!(list.step(false, 1, any), Some(1));

        // removing the last entry while it is current wraps like it would have
        let mut list = self::list(&[("a", 0), ("b", 0), ("c", 0)]);
        list.set_current(2);
        list.remove(2);
        assert_eq!(list.step(true, 1, any), Some(0));
        assert_eq!(list.step(false, 1, any), Some(1));

        // and the first one
        let mut list = self::list(&[("a", 0), ("b", 0), ("c", 0)]);
        list.set_current(0);
        list.remove(0);
        assert_eq!(list.step(true, 1, any), Some(0));
        assert_eq!(list.step(false, 1, any), Some(1));
    }

    #[test]
    fn removing_other_entries_adjusts_the_position() {
        let mut list = list(&[("a", 0), ("b", 0), ("c", 0), ("d", 0)]);
        list.set_current(2);
        list.remove(0);
        assert_eq!(list.current(), Some(1), "still `c`");
        list.remove(2);
        assert_eq!(list.current(), Some(1));
        assert_eq!(list.remove(10), None);

        // a gap moves with removals before it
        let mut list = self::list(&[("a", 0), ("b", 0), ("c", 0), ("d", 0)]);
        list.set_current(2);
        list.remove(2); // gap between b and d
        list.remove(0); // [b d]
        assert_eq!(list.step(true, 1, any), Some(1), "d");
        assert_eq!(list.step(false, 1, any), Some(0), "b");
    }

    #[test]
    fn version_changes_with_the_list() {
        let mut list = Quicklist::default();
        let v0 = list.version();
        list.push(entry("a", 0));
        let v1 = list.version();
        list.set_current(0);
        let v2 = list.version();
        list.remove(0);
        let v3 = list.version();
        assert!(v0 != v1 && v1 != v2 && v2 != v3);
    }
}
