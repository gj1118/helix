use std::cell::Cell;
use std::ops::ControlFlow;
use std::os::raw::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr::NonNull;
use std::time::{Duration, Instant};
use std::{fmt, mem, ptr};

use regex_cursor::Cursor;

use crate::grammar::IncompatibleGrammarError;
use crate::tree::{SyntaxTreeData, Tree};
use crate::{Grammar, Input, IntoInput, Point, Range};

// opaque data
enum ParserData {}

#[clippy::msrv = "1.76.0"]
thread_local! {
    static PARSER_CACHE: Cell<Option<RawParser>> = const { Cell::new(None) };
}

struct RawParser {
    ptr: NonNull<ParserData>,
}

impl Drop for RawParser {
    fn drop(&mut self) {
        unsafe { ts_parser_delete(self.ptr) }
    }
}

/// A stateful object that this is used to produce a [`Tree`] based on some
/// source code.
pub struct Parser {
    ptr: NonNull<ParserData>,
}

impl Parser {
    /// Create a new parser.
    #[must_use]
    pub fn new() -> Parser {
        let ptr = match PARSER_CACHE.take() {
            Some(cached) => {
                let ptr = cached.ptr;
                mem::forget(cached);
                ptr
            }
            None => unsafe { ts_parser_new() },
        };
        Parser { ptr }
    }

    /// Set the language that the parser should use for parsing.
    pub fn set_grammar(&mut self, grammar: Grammar) -> Result<(), IncompatibleGrammarError> {
        if unsafe { ts_parser_set_language(self.ptr, grammar) } {
            Ok(())
        } else {
            Err(IncompatibleGrammarError {
                abi_version: grammar.abi_version(),
            })
        }
    }

    /// Set the ranges of text that the parser should include when parsing. By default, the parser
    /// will always include entire documents. This function allows you to parse only a *portion*
    /// of a document but still return a syntax tree whose ranges match up with the document as a
    /// whole. You can also pass multiple disjoint ranges.
    ///
    /// `ranges` must be non-overlapping and sorted.
    pub fn set_included_ranges(&mut self, ranges: &[Range]) -> Result<(), InvalidRangesError> {
        // TODO: save some memory by only storing byte ranges and converting them to TS ranges in an
        // internal buffer here. Points are not used by TS. Alternatively we can patch the TS C code
        // to accept a simple pair (struct with two fields) of byte positions here instead of a full
        // tree sitter range
        let success = unsafe {
            ts_parser_set_included_ranges(self.ptr, ranges.as_ptr(), ranges.len() as u32)
        };
        if success {
            Ok(())
        } else {
            Err(InvalidRangesError)
        }
    }

    #[must_use]
    pub fn parse<I: Input>(
        &mut self,
        input: impl IntoInput<Input = I>,
        old_tree: Option<&Tree>,
    ) -> Option<Tree> {
        let mut input = input.into_input();
        unsafe extern "C" fn read<C: Input>(
            payload: NonNull<c_void>,
            byte_index: u32,
            _position: Point,
            bytes_read: *mut u32,
        ) -> *const u8 {
            let cursor = catch_unwind(AssertUnwindSafe(move || {
                let input: &mut C = payload.cast().as_mut();
                let cursor = input.cursor_at(byte_index);
                let slice = cursor.chunk();
                let offset: u32 = cursor.offset().try_into().unwrap();
                let len: u32 = slice.len().try_into().unwrap();
                (byte_index - offset, slice.as_ptr(), len)
            }));
            match cursor {
                Ok((chunk_offset, ptr, len)) if chunk_offset < len => {
                    *bytes_read = len - chunk_offset;
                    ptr.add(chunk_offset as usize)
                }
                _ => {
                    *bytes_read = 0;
                    ptr::null()
                }
            }
        }
        let raw_input = ParserInputRaw {
            payload: NonNull::from(&mut input).cast(),
            read: read::<I>,
            encoding: InputEncoding::Utf8,
            decode: None,
        };

        unsafe {
            let old_tree = old_tree.map(|tree| tree.as_raw());
            ts_parser_parse(self.ptr, old_tree, raw_input).map(|raw| Tree::from_raw(raw))
        }
    }

    /// Parse with a progress/cancellation callback. The callback receives the current
    /// [`ParseState`] and returns [`ControlFlow::Break`] to cancel parsing.
    #[must_use]
    pub fn parse_with_options<I: Input>(
        &mut self,
        input: impl IntoInput<Input = I>,
        old_tree: Option<&Tree>,
        mut options: ParseOptions<'_>,
    ) -> Option<Tree> {
        let mut input = input.into_input();
        unsafe extern "C" fn read<C: Input>(
            payload: NonNull<c_void>,
            byte_index: u32,
            _position: Point,
            bytes_read: *mut u32,
        ) -> *const u8 {
            let cursor = catch_unwind(AssertUnwindSafe(move || {
                let input: &mut C = payload.cast().as_mut();
                let cursor = input.cursor_at(byte_index);
                let slice = cursor.chunk();
                let offset: u32 = cursor.offset().try_into().unwrap();
                let len: u32 = slice.len().try_into().unwrap();
                (byte_index - offset, slice.as_ptr(), len)
            }));
            match cursor {
                Ok((chunk_offset, ptr, len)) if chunk_offset < len => {
                    *bytes_read = len - chunk_offset;
                    ptr.add(chunk_offset as usize)
                }
                _ => {
                    *bytes_read = 0;
                    ptr::null()
                }
            }
        }
        let raw_input = ParserInputRaw {
            payload: NonNull::from(&mut input).cast(),
            read: read::<I>,
            encoding: InputEncoding::Utf8,
            decode: None,
        };

        // The payload is a thin pointer to the fat pointer stored in options.callback.
        // The callback reconstructs the fat pointer and calls through it.
        unsafe extern "C" fn progress_cb(raw_state: NonNull<RawParseState>) -> bool {
            let raw_ref = raw_state.as_ref();
            let cb: *mut &mut dyn FnMut(&ParseState) -> ControlFlow<()> =
                raw_ref.payload.as_ptr().cast();
            let public_state = ParseState {
                current_byte_offset: raw_ref.current_byte_offset,
                has_error: raw_ref.has_error,
            };
            (*cb)(&public_state).is_break()
        }

        let raw_options = RawParseOptions {
            payload: unsafe {
                Some(NonNull::new_unchecked(
                    ptr::addr_of_mut!(options.callback).cast(),
                ))
            },
            progress_callback: Some(progress_cb),
        };

        unsafe {
            let old_tree = old_tree.map(|tree| tree.as_raw());
            let tree = ts_parser_parse_with_options(self.ptr, old_tree, raw_input, raw_options)
                .map(|raw| Tree::from_raw(raw));
            // PATCH (helix fork): a cancelled parse is resumed by the next one unless the parser is
            // reset, and parsers are shared: the next document parsed (another one, with this
            // parser from the cache) would get a tree made partly of this one.
            if tree.is_none() {
                ts_parser_reset(self.ptr);
            }
            tree
        }
    }

    /// Parse with a timeout. Returns `None` if parsing is not completed within `timeout`.
    #[must_use]
    pub fn parse_with_timeout<I: Input>(
        &mut self,
        input: impl IntoInput<Input = I>,
        old_tree: Option<&Tree>,
        timeout: Duration,
    ) -> Option<Tree> {
        let deadline = Instant::now() + timeout;
        let mut check = |_: &ParseState| {
            if Instant::now() >= deadline {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        };
        self.parse_with_options(input, old_tree, ParseOptions::new(&mut check))
    }
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl Sync for Parser {}
unsafe impl Send for Parser {}

impl Drop for Parser {
    fn drop(&mut self) {
        PARSER_CACHE.set(Some(RawParser { ptr: self.ptr }));
    }
}

/// State passed to the progress callback during parsing.
#[derive(Debug, Clone, Copy)]
pub struct ParseState {
    pub current_byte_offset: u32,
    pub has_error: bool,
}

/// Options for [`Parser::parse_with_options`].
///
/// The callback receives the current [`ParseState`] and returns [`ControlFlow::Break`] to cancel parsing.
pub struct ParseOptions<'a> {
    callback: &'a mut dyn FnMut(&ParseState) -> ControlFlow<()>,
}

impl<'a> ParseOptions<'a> {
    pub fn new(callback: &'a mut impl FnMut(&ParseState) -> ControlFlow<()>) -> ParseOptions<'a> {
        ParseOptions { callback }
    }
}

/// An error that occurred when trying to assign an incompatible [`Grammar`] to
/// a [`Parser`].
#[derive(Debug, PartialEq, Eq)]
pub struct InvalidRangesError;

impl fmt::Display for InvalidRangesError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "include ranges overlap or are not sorted",)
    }
}
impl std::error::Error for InvalidRangesError {}

type TreeSitterReadFn = unsafe extern "C" fn(
    payload: NonNull<c_void>,
    byte_index: u32,
    position: Point,
    bytes_read: *mut u32,
) -> *const u8;

/// A function that reads one code point from the given string, returning the number of bytes
/// consumed.
type DecodeInputFn =
    unsafe extern "C" fn(string: *const u8, length: u32, code_point: *const i32) -> u32;

#[repr(C)]
#[derive(Debug)]
pub struct ParserInputRaw {
    pub payload: NonNull<c_void>,
    pub read: TreeSitterReadFn,
    pub encoding: InputEncoding,
    /// A function to decode the the input.
    ///
    /// This function is only used if the encoding is `InputEncoding::Custom`.
    pub decode: Option<DecodeInputFn>,
}

// `TSInputEncoding`
#[repr(u32)]
#[derive(Debug, Clone, Copy)]
pub enum InputEncoding {
    Utf8,
    Utf16LE,
    Utf16BE,
    Custom,
}

#[repr(C)]
#[derive(Debug)]
struct RawParseState {
    /// The payload passed via `RawParseOptions`' `payload` field.
    payload: NonNull<c_void>,
    current_byte_offset: u32,
    has_error: bool,
}

/// A function that accepts the current parser state and returns `true` when the parse should be
/// cancelled.
type ProgressCallback = unsafe extern "C" fn(state: NonNull<RawParseState>) -> bool;

#[repr(C)]
#[derive(Debug, Default)]
struct RawParseOptions {
    payload: Option<NonNull<c_void>>,
    progress_callback: Option<ProgressCallback>,
}

extern "C" {
    /// Create a new parser
    fn ts_parser_new() -> NonNull<ParserData>;
    /// Delete the parser, freeing all of the memory that it used.
    fn ts_parser_delete(parser: NonNull<ParserData>);
    /// Discards a parse that was cancelled, so the next one starts from scratch.
    fn ts_parser_reset(parser: NonNull<ParserData>);
    /// Set the language that the parser should use for parsing. Returns a boolean indicating
    /// whether or not the language was successfully assigned. True means assignment
    /// succeeded. False means there was a version mismatch: the language was generated with
    /// an incompatible version of the Tree-sitter CLI. Check the language's version using
    /// `ts_language_version` and compare it to this library's `TREE_SITTER_LANGUAGE_VERSION`
    /// and `TREE_SITTER_MIN_COMPATIBLE_LANGUAGE_VERSION` constants.
    fn ts_parser_set_language(parser: NonNull<ParserData>, language: Grammar) -> bool;
    /// Set the ranges of text that the parser should include when parsing. By default, the parser
    /// will always include entire documents. This function allows you to parse only a *portion*
    /// of a document but still return a syntax tree whose ranges match up with the document as a
    /// whole. You can also pass multiple disjoint ranges. The second and third parameters specify
    /// the location and length of an array of ranges. The parser does *not* take ownership of
    /// these ranges; it copies the data, so it doesn't matter how these ranges are allocated.
    /// If `count` is zero, then the entire document will be parsed. Otherwise, the given ranges
    /// must be ordered from earliest to latest in the document, and they must not overlap. That
    /// is, the following must hold for all: `i < count - 1`: `ranges[i].end_byte <= ranges[i +
    /// 1].start_byte` If this requirement is not satisfied, the operation will fail, the ranges
    /// will not be assigned, and this function will return `false`. On success, this function
    /// returns `true`
    fn ts_parser_set_included_ranges(
        parser: NonNull<ParserData>,
        ranges: *const Range,
        count: u32,
    ) -> bool;

    fn ts_parser_parse(
        parser: NonNull<ParserData>,
        old_tree: Option<NonNull<SyntaxTreeData>>,
        input: ParserInputRaw,
    ) -> Option<NonNull<SyntaxTreeData>>;

    /// Use the parser to parse some source code and create a syntax tree, with some options.
    ///
    /// See `ts_parser_parse` for more details.
    ///
    /// See `TSParseOptions` for more details on the options.
    fn ts_parser_parse_with_options(
        parser: NonNull<ParserData>,
        old_tree: Option<NonNull<SyntaxTreeData>>,
        input: ParserInputRaw,
        parse_options: RawParseOptions,
    ) -> Option<NonNull<SyntaxTreeData>>;
}
