//! Lua pattern matching (`string.find`, `string.match`, `string.gmatch` and `string.gsub`)
//! that honours the plugin time limit.
//!
//! The functions of Lua's string library run in C, where the instruction-count hook that
//! enforces [`super::PLUGIN_EXECUTION_TIMEOUT`] never fires. A pattern with several
//! repetitions backtracks polynomially (`("a"):rep(1e5):find(".-.-.-b")` runs for hours), so
//! a single call could hang the editor. This module is a straight port of the matcher in
//! Lua 5.4's `lstrlib.c`, with the same semantics and error messages, that checks the
//! deadline while it works. It also bounds the memory `gsub` may use outside the Lua heap.

use mlua::prelude::*;

/// Maximum recursion depth of `do_match` (`MAXCCALLS` in `lstrlib.c`).
const MAXCCALLS: usize = 200;
/// Maximum number of captures (`LUA_MAXCAPTURES`).
const MAXCAPTURES: usize = 32;
const CAP_UNFINISHED: isize = -1;
const CAP_POSITION: isize = -2;
const L_ESC: u8 = b'%';
const SPECIALS: &[u8] = b"^$*+?.([%-";

/// Units of work between two deadline checks.
const CHECK_INTERVAL: usize = 4096;

/// Counts work and checks the plugin deadline every [`CHECK_INTERVAL`] units.
struct Budget {
    left: usize,
}

impl Budget {
    fn new() -> Self {
        Self {
            left: CHECK_INTERVAL,
        }
    }

    #[inline]
    fn tick(&mut self) -> LuaResult<()> {
        self.charge(1)
    }

    #[inline]
    fn charge(&mut self, units: usize) -> LuaResult<()> {
        if units < self.left {
            self.left -= units;
            return Ok(());
        }
        self.left = CHECK_INTERVAL;
        if super::deadline_passed() {
            Err(super::timeout_error())
        } else {
            Ok(())
        }
    }

    /// Charge for copying or comparing `bytes` bytes.
    #[inline]
    fn charge_bytes(&mut self, bytes: usize) -> LuaResult<()> {
        self.charge(bytes / 64 + 1)
    }
}

fn error(msg: impl Into<String>) -> LuaError {
    LuaError::RuntimeError(msg.into())
}

/// A capture: a substring of the subject or a position (`()`), 1-based.
#[derive(Clone, Copy)]
enum Capture {
    Str(usize, usize),
    Position(usize),
}

struct MatchState<'a> {
    src: &'a [u8],
    pat: &'a [u8],
    matchdepth: usize,
    level: usize,
    capture: [(usize, isize); MAXCAPTURES],
    budget: Budget,
}

fn match_class(c: u8, cl: u8) -> bool {
    let res = match cl.to_ascii_lowercase() {
        b'a' => c.is_ascii_alphabetic(),
        b'c' => c.is_ascii_control(),
        b'd' => c.is_ascii_digit(),
        b'g' => c.is_ascii_graphic(),
        b'l' => c.is_ascii_lowercase(),
        b'p' => c.is_ascii_punctuation(),
        // C's isspace, which (unlike `is_ascii_whitespace`) includes \v
        b's' => matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r'),
        b'u' => c.is_ascii_uppercase(),
        b'w' => c.is_ascii_alphanumeric(),
        b'x' => c.is_ascii_hexdigit(),
        b'z' => c == 0,
        _ => return cl == c,
    };
    if cl.is_ascii_lowercase() {
        res
    } else {
        !res
    }
}

impl<'a> MatchState<'a> {
    fn new(src: &'a [u8], pat: &'a [u8]) -> Self {
        Self {
            src,
            pat,
            matchdepth: MAXCCALLS,
            level: 0,
            capture: [(0, 0); MAXCAPTURES],
            budget: Budget::new(),
        }
    }

    /// Pattern byte at `i`, `0` past the end (C reads the terminating NUL).
    #[inline]
    fn p(&self, i: usize) -> u8 {
        self.pat.get(i).copied().unwrap_or(0)
    }

    /// Subject byte at `i`, `0` past the end.
    #[inline]
    fn s(&self, i: usize) -> u8 {
        self.src.get(i).copied().unwrap_or(0)
    }

    fn check_capture(&self, l: u8) -> LuaResult<usize> {
        let l = l as isize - b'1' as isize;
        if l < 0 || l as usize >= self.level || self.capture[l as usize].1 == CAP_UNFINISHED {
            return Err(error(format!("invalid capture index %{}", l + 1)));
        }
        Ok(l as usize)
    }

    fn capture_to_close(&self) -> LuaResult<usize> {
        (0..self.level)
            .rev()
            .find(|&l| self.capture[l].1 == CAP_UNFINISHED)
            .ok_or_else(|| error("invalid pattern capture"))
    }

    fn class_end(&self, mut p: usize) -> LuaResult<usize> {
        let c = self.p(p);
        p += 1;
        match c {
            L_ESC => {
                if p >= self.pat.len() {
                    return Err(error("malformed pattern (ends with '%')"));
                }
                Ok(p + 1)
            }
            b'[' => {
                if self.p(p) == b'^' {
                    p += 1;
                }
                // look for a ']'
                loop {
                    if p >= self.pat.len() {
                        return Err(error("malformed pattern (missing ']')"));
                    }
                    let c = self.pat[p];
                    p += 1;
                    if c == L_ESC && p < self.pat.len() {
                        p += 1; // skip escapes (e.g. '%]')
                    }
                    if self.p(p) == b']' {
                        break;
                    }
                }
                Ok(p + 1)
            }
            _ => Ok(p),
        }
    }

    /// `p` is the '[' of the class, `ec` its closing ']'.
    fn match_bracket_class(&self, c: u8, mut p: usize, ec: usize) -> bool {
        let mut sig = true;
        if self.p(p + 1) == b'^' {
            sig = false;
            p += 1;
        }
        loop {
            p += 1;
            if p >= ec {
                break;
            }
            if self.p(p) == L_ESC {
                p += 1;
                if match_class(c, self.p(p)) {
                    return sig;
                }
            } else if self.p(p + 1) == b'-' && p + 2 < ec {
                p += 2;
                if self.p(p - 2) <= c && c <= self.p(p) {
                    return sig;
                }
            } else if self.p(p) == c {
                return sig;
            }
        }
        !sig
    }

    fn single_match(&self, s: usize, p: usize, ep: usize) -> bool {
        if s >= self.src.len() {
            return false;
        }
        let c = self.src[s];
        match self.p(p) {
            b'.' => true,
            L_ESC => match_class(c, self.p(p + 1)),
            b'[' => self.match_bracket_class(c, p, ep - 1),
            pc => pc == c,
        }
    }

    fn match_balance(&mut self, s: usize, p: usize) -> LuaResult<Option<usize>> {
        if p + 1 >= self.pat.len() {
            return Err(error("malformed pattern (missing arguments to '%b')"));
        }
        if self.s(s) != self.pat[p] {
            return Ok(None);
        }
        let (b, e) = (self.pat[p], self.pat[p + 1]);
        let mut cont = 1usize;
        let mut s = s;
        loop {
            s += 1;
            if s >= self.src.len() {
                break;
            }
            self.budget.tick()?;
            let c = self.src[s];
            if c == e {
                cont -= 1;
                if cont == 0 {
                    return Ok(Some(s + 1));
                }
            } else if c == b {
                cont += 1;
            }
        }
        Ok(None) // string ends out of balance
    }

    fn max_expand(&mut self, s: usize, p: usize, ep: usize) -> LuaResult<Option<usize>> {
        let mut i = 0;
        while self.single_match(s + i, p, ep) {
            i += 1;
            self.budget.tick()?;
        }
        // keeps trying to match with the maximum repetitions
        loop {
            if let Some(res) = self.do_match(s + i, ep + 1)? {
                return Ok(Some(res));
            }
            if i == 0 {
                return Ok(None);
            }
            i -= 1;
        }
    }

    fn min_expand(&mut self, mut s: usize, p: usize, ep: usize) -> LuaResult<Option<usize>> {
        loop {
            if let Some(res) = self.do_match(s, ep + 1)? {
                return Ok(Some(res));
            }
            if self.single_match(s, p, ep) {
                s += 1; // try with one more repetition
            } else {
                return Ok(None);
            }
        }
    }

    fn start_capture(&mut self, s: usize, p: usize, what: isize) -> LuaResult<Option<usize>> {
        let level = self.level;
        if level >= MAXCAPTURES {
            return Err(error("too many captures"));
        }
        self.capture[level] = (s, what);
        self.level = level + 1;
        let res = self.do_match(s, p)?;
        if res.is_none() {
            self.level -= 1; // undo capture
        }
        Ok(res)
    }

    fn end_capture(&mut self, s: usize, p: usize) -> LuaResult<Option<usize>> {
        let l = self.capture_to_close()?;
        self.capture[l].1 = (s - self.capture[l].0) as isize;
        let res = self.do_match(s, p)?;
        if res.is_none() {
            self.capture[l].1 = CAP_UNFINISHED; // undo capture
        }
        Ok(res)
    }

    fn match_capture(&mut self, s: usize, l: u8) -> LuaResult<Option<usize>> {
        let l = self.check_capture(l)?;
        let (init, len) = self.capture[l];
        // A position capture has no length; as in C (where it is cast to a huge size_t)
        // it never matches.
        if len < 0 {
            return Ok(None);
        }
        let len = len as usize;
        self.budget.charge_bytes(len)?;
        if self.src.len() - s >= len && self.src[init..init + len] == self.src[s..s + len] {
            Ok(Some(s + len))
        } else {
            Ok(None)
        }
    }

    fn do_match(&mut self, mut s: usize, mut p: usize) -> LuaResult<Option<usize>> {
        if self.matchdepth == 0 {
            return Err(error("pattern too complex"));
        }
        self.matchdepth -= 1;
        self.budget.tick()?;
        let result = loop {
            if p >= self.pat.len() {
                break Some(s); // end of pattern
            }
            match self.pat[p] {
                b'(' => {
                    break if self.p(p + 1) == b')' {
                        self.start_capture(s, p + 2, CAP_POSITION)?
                    } else {
                        self.start_capture(s, p + 1, CAP_UNFINISHED)?
                    };
                }
                b')' => break self.end_capture(s, p + 1)?,
                b'$' if p + 1 == self.pat.len() => {
                    break (s == self.src.len()).then_some(s);
                }
                L_ESC if self.p(p + 1) == b'b' => match self.match_balance(s, p + 2)? {
                    Some(next) => {
                        s = next;
                        p += 4;
                    }
                    None => break None,
                },
                L_ESC if self.p(p + 1) == b'f' => {
                    p += 2;
                    if self.p(p) != b'[' {
                        return Err(error("missing '[' after '%f' in pattern"));
                    }
                    let ep = self.class_end(p)?;
                    let previous = if s == 0 { 0 } else { self.src[s - 1] };
                    if !self.match_bracket_class(previous, p, ep - 1)
                        && self.match_bracket_class(self.s(s), p, ep - 1)
                    {
                        p = ep;
                    } else {
                        break None;
                    }
                }
                L_ESC if self.p(p + 1).is_ascii_digit() => {
                    match self.match_capture(s, self.p(p + 1))? {
                        Some(next) => {
                            s = next;
                            p += 2;
                        }
                        None => break None,
                    }
                }
                _ => {
                    // pattern class plus optional suffix
                    let ep = self.class_end(p)?;
                    let suffix = self.p(ep);
                    if !self.single_match(s, p, ep) {
                        if matches!(suffix, b'*' | b'?' | b'-') {
                            p = ep + 1; // accept empty
                            continue;
                        }
                        break None; // '+' or no suffix
                    }
                    match suffix {
                        b'?' => {
                            if let Some(res) = self.do_match(s + 1, ep + 1)? {
                                break Some(res);
                            }
                            p = ep + 1;
                        }
                        b'+' => break self.max_expand(s + 1, p, ep)?,
                        b'*' => break self.max_expand(s, p, ep)?,
                        b'-' => break self.min_expand(s, p, ep)?,
                        _ => {
                            s += 1;
                            p = ep;
                        }
                    }
                }
            }
        };
        self.matchdepth += 1;
        Ok(result)
    }

    /// Restart for a new match attempt.
    fn reprep(&mut self) {
        self.level = 0;
    }

    fn get_capture(&self, i: usize, s: usize, e: usize) -> LuaResult<Capture> {
        if i >= self.level {
            if i != 0 {
                return Err(error(format!("invalid capture index %{}", i + 1)));
            }
            return Ok(Capture::Str(s, e));
        }
        let (init, len) = self.capture[i];
        match len {
            CAP_UNFINISHED => Err(error("unfinished capture")),
            CAP_POSITION => Ok(Capture::Position(init + 1)),
            len => Ok(Capture::Str(init, init + len as usize)),
        }
    }

    fn capture_value(&self, lua: &Lua, i: usize, s: usize, e: usize) -> LuaResult<LuaValue> {
        Ok(match self.get_capture(i, s, e)? {
            Capture::Str(a, b) => LuaValue::String(lua.create_string(&self.src[a..b])?),
            Capture::Position(pos) => LuaValue::Integer(pos as i64),
        })
    }

    /// The captures of a match, or the whole match `s..e` if there are none and `s` is given.
    fn captures(&self, lua: &Lua, s: Option<usize>, e: usize) -> LuaResult<LuaMultiValue> {
        let n = if self.level == 0 && s.is_some() {
            1
        } else {
            self.level
        };
        let s = s.unwrap_or(0);
        (0..n).map(|i| self.capture_value(lua, i, s, e)).collect()
    }
}

/// `posrelatI` of `lstrlib.c`: a 1-based start position, clipped to `[1, inf)`.
fn posrelat(pos: i64, len: usize) -> usize {
    if pos > 0 {
        pos as usize
    } else if pos == 0 || pos < -(len as i64) {
        1
    } else {
        (len as i64 + pos + 1) as usize
    }
}

fn no_specials(pat: &[u8]) -> bool {
    !pat.iter().any(|b| SPECIALS.contains(b))
}

/// Plain substring search (`lmemfind`).
fn find_plain(hay: &[u8], needle: &[u8], budget: &mut Budget) -> LuaResult<Option<usize>> {
    let Some((&first, _)) = needle.split_first() else {
        return Ok(Some(0)); // empty strings are everywhere
    };
    if needle.len() > hay.len() {
        return Ok(None);
    }
    let last_start = hay.len() - needle.len();
    let mut i = 0;
    while i <= last_start {
        let Some(off) = hay[i..=last_start].iter().position(|&b| b == first) else {
            return Ok(None);
        };
        i += off;
        budget.charge(off / 64 + needle.len() / 16 + 1)?;
        if hay[i..i + needle.len()] == *needle {
            return Ok(Some(i));
        }
        i += 1;
    }
    Ok(None)
}

fn truthy(value: &Option<LuaValue>) -> bool {
    !matches!(
        value,
        None | Some(LuaValue::Nil) | Some(LuaValue::Boolean(false))
    )
}

fn str_find_aux(
    lua: &Lua,
    s: &LuaString,
    p: &LuaString,
    init: Option<i64>,
    plain: bool,
    find: bool,
) -> LuaResult<LuaMultiValue> {
    let src = s.as_bytes();
    let pat = p.as_bytes();
    let ls = src.len();
    let init = posrelat(init.unwrap_or(1), ls) - 1;
    if init > ls {
        return Ok(LuaMultiValue::from_iter([LuaValue::Nil])); // cannot find anything
    }
    if find && (plain || no_specials(&pat)) {
        let mut budget = Budget::new();
        if let Some(pos) = find_plain(&src[init..], &pat, &mut budget)? {
            let start = init + pos;
            return Ok(LuaMultiValue::from_iter([
                LuaValue::Integer(start as i64 + 1),
                LuaValue::Integer((start + pat.len()) as i64),
            ]));
        }
    } else {
        let (anchor, pat) = match pat.split_first() {
            Some((b'^', rest)) => (true, rest),
            _ => (false, &pat[..]),
        };
        let mut ms = MatchState::new(&src, pat);
        let mut s1 = init;
        loop {
            ms.reprep();
            if let Some(end) = ms.do_match(s1, 0)? {
                if find {
                    let mut values = ms.captures(lua, None, 0)?;
                    values.push_front(LuaValue::Integer(end as i64));
                    values.push_front(LuaValue::Integer(s1 as i64 + 1));
                    return Ok(values);
                }
                return ms.captures(lua, Some(s1), end);
            }
            if anchor || s1 >= ls {
                break;
            }
            s1 += 1;
        }
    }
    Ok(LuaMultiValue::from_iter([LuaValue::Nil]))
}

/// The replacement argument of `gsub`.
enum Replacement {
    Str(LuaString),
    Table(LuaTable),
    Function(LuaFunction),
}

/// Append `bytes` to the `gsub` result, failing like an allocation would once the result
/// outgrows the plugin memory limit.
fn push_bytes(out: &mut Vec<u8>, bytes: &[u8], budget: &mut Budget) -> LuaResult<()> {
    if out.len() + bytes.len() > super::PLUGIN_MEMORY_LIMIT {
        return Err(LuaError::MemoryError("not enough memory".to_string()));
    }
    budget.charge_bytes(bytes.len())?;
    out.extend_from_slice(bytes);
    Ok(())
}

/// Expand a replacement string (`add_s`).
fn add_s(ms: &mut MatchState, out: &mut Vec<u8>, s: usize, e: usize, repl: &[u8]) -> LuaResult<()> {
    let mut i = 0;
    while let Some(off) = repl[i..].iter().position(|&b| b == L_ESC) {
        push_bytes(out, &repl[i..i + off], &mut ms.budget)?;
        let p = i + off + 1; // skip ESC
        match repl.get(p).copied().unwrap_or(0) {
            L_ESC => push_bytes(out, &[L_ESC], &mut ms.budget)?,
            b'0' => push_bytes(out, &ms.src[s..e], &mut ms.budget)?,
            c if c.is_ascii_digit() => match ms.get_capture((c - b'1') as usize, s, e)? {
                Capture::Str(a, b) => push_bytes(out, &ms.src[a..b], &mut ms.budget)?,
                Capture::Position(pos) => {
                    push_bytes(out, pos.to_string().as_bytes(), &mut ms.budget)?
                }
            },
            _ => return Err(error("invalid use of '%' in replacement string")),
        }
        i = p + 1;
    }
    push_bytes(out, &repl[i..], &mut ms.budget)
}

/// Append the replacement of the match `s..e`; returns whether the subject changed.
fn add_value(
    lua: &Lua,
    ms: &mut MatchState,
    out: &mut Vec<u8>,
    s: usize,
    e: usize,
    repl: &Replacement,
) -> LuaResult<bool> {
    let value = match repl {
        Replacement::Str(r) => {
            add_s(ms, out, s, e, &r.as_bytes())?;
            return Ok(true);
        }
        Replacement::Function(f) => f.call::<LuaValue>(ms.captures(lua, Some(s), e)?)?,
        Replacement::Table(t) => t.get::<LuaValue>(ms.capture_value(lua, 0, s, e)?)?,
    };
    match value {
        LuaValue::Nil | LuaValue::Boolean(false) => {
            // keep the original text
            push_bytes(out, &ms.src[s..e], &mut ms.budget)?;
            Ok(false)
        }
        LuaValue::String(v) => {
            push_bytes(out, &v.as_bytes(), &mut ms.budget)?;
            Ok(true)
        }
        v @ (LuaValue::Integer(_) | LuaValue::Number(_)) => {
            let v = lua
                .coerce_string(v)?
                .ok_or_else(|| error("invalid replacement value (a number)"))?;
            push_bytes(out, &v.as_bytes(), &mut ms.budget)?;
            Ok(true)
        }
        other => Err(error(format!(
            "invalid replacement value (a {})",
            other.type_name()
        ))),
    }
}

fn gsub(
    lua: &Lua,
    (s, p, repl, max_s): (LuaString, LuaString, LuaValue, Option<i64>),
) -> LuaResult<(LuaString, i64)> {
    let repl = match repl {
        LuaValue::String(r) => Replacement::Str(r),
        v @ (LuaValue::Integer(_) | LuaValue::Number(_)) => Replacement::Str(
            lua.coerce_string(v)?
                .ok_or_else(|| error("bad argument #3 to 'gsub'"))?,
        ),
        LuaValue::Table(t) => Replacement::Table(t),
        LuaValue::Function(f) => Replacement::Function(f),
        other => {
            return Err(error(format!(
                "bad argument #3 to 'gsub' (string/function/table expected, got {})",
                other.type_name()
            )))
        }
    };
    let src = s.as_bytes();
    let pat = p.as_bytes();
    let max_s = max_s.unwrap_or(src.len() as i64 + 1);
    let (anchor, pat) = match pat.split_first() {
        Some((b'^', rest)) => (true, rest),
        _ => (false, &pat[..]),
    };
    let mut ms = MatchState::new(&src, pat);
    let mut out = Vec::new();
    let mut pos = 0;
    let mut lastmatch = None;
    let mut n = 0i64;
    let mut changed = false;
    while n < max_s {
        ms.reprep();
        match ms.do_match(pos, 0)? {
            Some(e) if Some(e) != lastmatch => {
                n += 1;
                changed |= add_value(lua, &mut ms, &mut out, pos, e, &repl)?;
                pos = e;
                lastmatch = Some(e);
            }
            // otherwise, skip one character
            _ if pos < src.len() => {
                push_bytes(&mut out, &src[pos..pos + 1], &mut ms.budget)?;
                pos += 1;
            }
            _ => break, // end of subject
        }
        if anchor {
            break;
        }
    }
    if !changed {
        drop(src);
        return Ok((s, n));
    }
    push_bytes(&mut out, &src[pos..], &mut ms.budget)?;
    Ok((lua.create_string(&out)?, n))
}

fn gmatch(lua: &Lua, (s, p, init): (LuaString, LuaString, Option<i64>)) -> LuaResult<LuaFunction> {
    let ls = s.as_bytes().len();
    let mut init = posrelat(init.unwrap_or(1), ls) - 1;
    if init > ls {
        init = ls + 1; // start after string's end
    }
    let mut next = init;
    let mut lastmatch: Option<usize> = None;
    lua.create_function_mut(move |lua, _: LuaMultiValue| {
        let src = s.as_bytes();
        let pat = p.as_bytes();
        let mut ms = MatchState::new(&src, &pat);
        let mut pos = next;
        while pos <= src.len() {
            ms.reprep();
            if let Some(e) = ms.do_match(pos, 0)? {
                if Some(e) != lastmatch {
                    next = e;
                    lastmatch = Some(e);
                    return ms.captures(lua, Some(pos), e);
                }
            }
            pos += 1;
        }
        Ok(LuaMultiValue::new()) // not found
    })
}

/// `string.rep`, except that repeating an empty string (with an empty separator) a huge
/// number of times returns at once: the C version loops `n` times doing nothing.
fn rep(
    lua: &Lua,
    raw_rep: &LuaFunction,
    (s, n, sep): (LuaString, i64, Option<LuaString>),
) -> LuaResult<LuaString> {
    let empty = s.as_bytes().is_empty() && sep.as_ref().is_none_or(|sep| sep.as_bytes().is_empty());
    if n <= 0 || empty {
        return lua.create_string("");
    }
    raw_rep.call((s, n, sep))
}

/// Replace the pattern functions of the `string` table (also reached as string methods)
/// and `string.rep` with versions that honour the plugin time limit.
pub(super) fn install(lua: &Lua) -> LuaResult<()> {
    let string: LuaTable = lua.globals().get("string")?;

    let find = lua.create_function(
        |lua, (s, p, init, plain): (LuaString, LuaString, Option<i64>, Option<LuaValue>)| {
            str_find_aux(lua, &s, &p, init, truthy(&plain), true)
        },
    )?;
    let match_ =
        lua.create_function(|lua, (s, p, init): (LuaString, LuaString, Option<i64>)| {
            str_find_aux(lua, &s, &p, init, false, false)
        })?;
    let raw_rep: LuaFunction = string.get("rep")?;
    let rep = lua.create_function(move |lua, args| rep(lua, &raw_rep, args))?;

    string.set("find", find)?;
    string.set("match", match_)?;
    string.set("gmatch", lua.create_function(gmatch)?)?;
    string.set("gsub", lua.create_function(gsub)?)?;
    string.set("rep", rep)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Pair {
        stock: Lua,
        ours: Lua,
    }

    fn pair() -> Pair {
        let ours = Lua::new();
        install(&ours).unwrap();
        Pair {
            stock: Lua::new(),
            ours,
        }
    }

    /// Runs `code` (which returns a string describing its results) on both a stock Lua and
    /// one with the replaced functions, and checks they agree.
    fn same(Pair { stock, ours }: &Pair, code: &str) {
        let expected: LuaResult<String> = stock.load(code).eval();
        let actual: LuaResult<String> = ours.load(code).eval();
        match (expected, actual) {
            (Ok(a), Ok(b)) => assert_eq!(a, b, "{code}"),
            (Err(_), Err(_)) => {}
            (a, b) => panic!("{code}: stock {a:?}, ported {b:?}"),
        }
    }

    const DUMP: &str = r##"
        local function dump(...)
            local t = table.pack(...)
            for i = 1, t.n do t[i] = math.type(t[i]) and ("#" .. t[i]) or tostring(t[i]) end
            return t.n .. ":" .. table.concat(t, ",")
        end
    "##;

    #[test]
    fn matches_stock_lua() {
        let lua = pair();
        let subjects = [
            "",
            "hello world",
            "  trim me  ",
            "key = value; other=thing",
            "a(b(c)d)e (x)",
            "THE (quick) fox",
            "aaa",
            "x\0y\0z",
            "f(a, b) == g[1]",
            "\u{e9}t\u{e9} \u{2190}",
            "abc123def456",
            "\t\x0b\x0c\r\n end",
        ];
        let patterns = [
            "",
            "o",
            "l+",
            "l*",
            "l-",
            "lo?",
            "^h",
            "^",
            "$",
            "d$",
            "x$y",
            "%a+",
            "%A+",
            "%d+",
            "%D",
            "%s*",
            "%S+",
            "%w+",
            "%W",
            "%x+",
            "%p",
            "%P+",
            "%c",
            "%g+",
            "%l+",
            "%u+",
            "%z",
            ".",
            ".-",
            ".*",
            "(.-)",
            "^%s*(.-)%s*$",
            "(%w+)%s*=%s*(%w+)",
            "()",
            "()(o)()",
            "%b()",
            "%b(",
            "%f[%w]%w+",
            "%f[%W]",
            "%f[%a]",
            "[aeiou]",
            "[^aeiou]+",
            "[a-c]+",
            "[%a_][%w_]*",
            "[]]",
            "[^]]",
            "[a-]",
            "[-a]",
            "[%]]",
            "(a)%1",
            "(a*(.)%w(%s*))",
            "(l)(l)",
            "%",
            "[a",
            "(",
            ")",
            "%1",
            "(()",
            "%f",
            "%fx",
            "[%",
            "a.-b",
            "\0",
            "[\0-\x7f]",
            "[\u{80}-\u{ff}]+",
            "%.",
            "%%",
            "^(a+)",
            "((((((((((((((((((((((((((((((((a))))))))))))))))))))))))))))))))",
            "(((((((((((((((((((((((((((((((((a)))))))))))))))))))))))))))))))))",
        ];
        let inits = ["nil", "1", "3", "-2", "0", "-100", "100", "12"];
        for s in subjects {
            let s = lua_literal(s);
            for p in patterns {
                let p = lua_literal(p);
                for init in inits {
                    same(&lua, &format!(
                        "{DUMP} return dump(string.find({s}, {p}, {init})) .. dump(string.find({s}, {p}, {init}, true)) .. dump(string.match({s}, {p}, {init}))"
                    ));
                }
                same(&lua, &format!(
                    "{DUMP} local r = {{}} for a, b, c in string.gmatch({s}, {p}) do r[#r + 1] = dump(a, b, c) if #r > 50 then break end end return table.concat(r, '|')"
                ));
                same(&lua, &format!(
                    "{DUMP} local r = {{}} for a in string.gmatch({s}, {p}, 3) do r[#r + 1] = dump(a) if #r > 50 then break end end return table.concat(r, '|')"
                ));
                for repl in [
                    "'<%0>'",
                    "'%1-%1'",
                    "'%%'",
                    "'%2'",
                    "'%'",
                    "'%x'",
                    "7",
                    "1.5",
                    "{ o = 'O', l = false, h = 1 }",
                    "function(a, b) if a == 'l' then return nil end return (b or a) .. '!' end",
                    "function() return {} end",
                    "true",
                ] {
                    for n in ["nil", "1", "0", "-1", "2"] {
                        same(
                            &lua,
                            &format!("{DUMP} return dump(string.gsub({s}, {p}, {repl}, {n}))"),
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn rep_matches_stock_lua() {
        let lua = pair();
        for code in [
            "return string.rep('ab', 3)",
            "return string.rep('ab', 3, ', ')",
            "return string.rep('ab', 0, ', ')",
            "return string.rep('ab', -1)",
            "return string.rep('', 5, '-')",
            "return string.rep(12, 2)",
            "return ('x'):rep(2.0)",
            "return string.rep({}, 2)",
        ] {
            same(&lua, code);
        }
        // The stock version loops `n` times on these (hours).
        let s: String = lua
            .ours
            .load("return string.rep('', 1e15) .. string.rep('', math.maxinteger, '')")
            .eval()
            .unwrap();
        assert_eq!(s, "");
    }

    /// Writes `s` as a Lua string literal (byte by byte, with decimal escapes).
    fn lua_literal(s: &str) -> String {
        let mut out = String::from("\"");
        for b in s.bytes() {
            if b.is_ascii_alphanumeric() || b == b' ' {
                out.push(b as char);
            } else {
                out.push_str(&format!("\\{b:03}"));
            }
        }
        out.push('"');
        out
    }
}
