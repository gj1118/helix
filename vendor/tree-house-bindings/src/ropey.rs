use std::ops;

use regex_cursor::{Cursor, RopeyCursor};
use ropey::RopeSlice;

use crate::{Input, IntoInput};

pub struct RopeInput<'a> {
    src: RopeSlice<'a>,
    cursor: regex_cursor::RopeyCursor<'a>,
}

impl<'a> RopeInput<'a> {
    pub fn new(src: RopeSlice<'a>) -> Self {
        RopeInput {
            src,
            cursor: regex_cursor::RopeyCursor::new(src),
        }
    }
}

impl<'a> IntoInput for RopeSlice<'a> {
    type Input = RopeInput<'a>;

    fn into_input(self) -> Self::Input {
        RopeInput {
            src: self,
            cursor: RopeyCursor::new(self),
        }
    }
}

impl<'a> Input for RopeInput<'a> {
    type Cursor = RopeyCursor<'a>;
    fn cursor_at(&mut self, offset: u32) -> &mut RopeyCursor<'a> {
        let offset = offset as usize;
        debug_assert!(
            offset <= self.src.len_bytes(),
            "parser offset out of bounds: {offset} > {}",
            self.src.len_bytes()
        );
        // this cursor is optimized for contiguous reads which are by far the most common during parsing
        // very far jumps (like injections at the other end of the document) are handled
        // by starting a new cursor (new chunks iterator)
        if offset < self.cursor.offset() || offset - self.cursor.offset() > 4906 {
            self.cursor = regex_cursor::RopeyCursor::at(self.src, offset);
        } else {
            while self.cursor.offset() + self.cursor.chunk().len() <= offset {
                if !self.cursor.advance() {
                    break;
                }
            }
        }
        &mut self.cursor
    }

    fn eq(&mut self, range1: ops::Range<u32>, range2: ops::Range<u32>) -> bool {
        let range1 = self
            .src
            .byte_slice(range1.start as usize..range1.end as usize);
        let range2 = self
            .src
            .byte_slice(range2.start as usize..range2.end as usize);
        range1 == range2
    }
}
