
//! This module parses JSON strings 64 bytes at a time using AVX-512BW
//! instructions to quickly identify structural characters, enabling entire
//! whitespace runs and string bodies to be skipped in a single operation.
//!
//! Each byte of the input is labelled below with the state that handles it.
//! States that skip whitespace via `trailing_zeros` handle both the whitespace
//! bytes **and** the following dispatch byte in the same loop iteration.
//!
//! ```text
//! { "key1" : "value1" , "key2": [123, 456 , 768], "key3" : { "nested_key" : true} }
//! VOOKKKKKDDCCSSSSSSSFFOOKKKKKDCCRAAARRAAAFRRAAAFOOKKKKKDDCCOOKKKKKKKKKKKDDCCAAAAFF
//! ```
//!
//! State key:
//! * `V` = `ValueWhitespace` — waiting for the first byte of any value
//! * `O` = `ObjectStart`     — after `{` or `,` in an object; skips whitespace, expects `"` or `}`
//! * `K` = `KeyChars`        — inside a quoted key; bulk-skipped via the backslash/quote masks
//! * `D` = `KeyEnd`          — after closing `"` of a key; skips whitespace, expects `:`
//! * `C` = `AfterColon`      — after `:`; skips whitespace, dispatches to the value type
//! * `S` = `StringChars`     — inside a quoted string value; bulk-skipped via the backslash/quote masks
//! * `F` = `AfterValue`      — after any complete value; skips whitespace, expects `,`/`}`/`]`
//! * `R` = `ArrayStart`      — after `[` or `,` in an array; skips whitespace, dispatches value
//! * `A` = `AtomChars`       — inside a number, `true`, `false`, or `null`
//!
//! A few things to notice in the annotation:
//!
//! * `OO`: `ObjectStart` eats the space *and* the opening `"` of a key in one
//!   shot via the `trailing_zeros` whitespace skip.
//! * `DD` / `CC`: `KeyEnd` eats the space *and* `:` together; `AfterColon`
//!   eats the space *and* the value-start byte — structural punctuation costs
//!   no extra iterations.
//! * `SSSSSSS`: `StringChars` covers the entire `value1"` run including the
//!   closing quote (bulk AVX-512 skip + dispatch in one pass through the chunk).
//! * `RAAARRAAAFRRAAAF`: inside the array `[123, 456 , 768]` each `R` covers
//!   the skip-to-digit hop; `AAA` covers the digit characters plus their
//!   terminating `,` / space / `]`.
//! * `KKKKKKKKKKK` (11 bytes): the 10-character `nested_key` body *and* its
//!   closing `"` are all handled by `KeyChars` in one bulk-skip pass.


// ---------------------------------------------------------------------------
// Optional state-entry statistics (compiled in with --features stats).
// ---------------------------------------------------------------------------

#[cfg(feature = "stats")]
pub mod stats {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

    pub static VALUE_WHITESPACE: AtomicU64 = AtomicU64::new(0);
    pub static STRING_CHARS:     AtomicU64 = AtomicU64::new(0);
    pub static STRING_ESCAPE:    AtomicU64 = AtomicU64::new(0);
    pub static KEY_CHARS:        AtomicU64 = AtomicU64::new(0);
    pub static KEY_ESCAPE:       AtomicU64 = AtomicU64::new(0);
    pub static KEY_END:          AtomicU64 = AtomicU64::new(0);
    pub static AFTER_COLON:      AtomicU64 = AtomicU64::new(0);
    pub static ATOM_CHARS:       AtomicU64 = AtomicU64::new(0);
    pub static OBJECT_START:     AtomicU64 = AtomicU64::new(0);
    pub static ARRAY_START:      AtomicU64 = AtomicU64::new(0);
    pub static AFTER_VALUE:      AtomicU64 = AtomicU64::new(0);

    pub fn reset() {
        for s in all() { s.store(0, Relaxed); }
    }

    fn all() -> [&'static AtomicU64; 11] {[
        &VALUE_WHITESPACE, &STRING_CHARS, &STRING_ESCAPE,
        &KEY_CHARS, &KEY_ESCAPE, &KEY_END, &AFTER_COLON,
        &ATOM_CHARS, &OBJECT_START, &ARRAY_START, &AFTER_VALUE,
    ]}

    pub struct StateStats {
        pub value_whitespace: u64,
        pub string_chars:     u64,
        pub string_escape:    u64,
        pub key_chars:        u64,
        pub key_escape:       u64,
        pub key_end:          u64,
        pub after_colon:      u64,
        pub atom_chars:       u64,
        pub object_start:     u64,
        pub array_start:      u64,
        pub after_value:      u64,
    }

    pub fn get() -> StateStats {
        StateStats {
            value_whitespace: VALUE_WHITESPACE.load(Relaxed),
            string_chars:     STRING_CHARS    .load(Relaxed),
            string_escape:    STRING_ESCAPE   .load(Relaxed),
            key_chars:        KEY_CHARS       .load(Relaxed),
            key_escape:       KEY_ESCAPE      .load(Relaxed),
            key_end:          KEY_END         .load(Relaxed),
            after_colon:      AFTER_COLON     .load(Relaxed),
            atom_chars:       ATOM_CHARS      .load(Relaxed),
            object_start:     OBJECT_START    .load(Relaxed),
            array_start:      ARRAY_START     .load(Relaxed),
            after_value:      AFTER_VALUE     .load(Relaxed),
        }
    }
}

/// Increment a state counter when the `stats` feature is enabled; a no-op otherwise.
macro_rules! stat {
    ($counter:path) => {
        #[cfg(feature = "stats")]
        $counter.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed);
    };
}

#[derive(PartialEq)]
enum State {
    // Waiting for the first byte of any JSON value.
    ValueWhitespace,

    // Inside a quoted string value.
    StringChars,
    // After a `\` inside a string value; next byte is consumed unconditionally.
    StringEscape,

    // Inside a key string (left-hand side of an object member).
    KeyChars,
    // After a `\` inside a key string.
    KeyEscape,
    // Closing `"` of a key consumed; skip whitespace then expect `:`.
    KeyEnd,
    // `:` consumed; skip whitespace then dispatch a value.
    AfterColon,

    // Inside an unquoted atom (number / true / false / null).
    AtomChars,

    // `{` consumed; skip whitespace then expect `"` (key) or `}`.
    ObjectStart,

    // `[` consumed; skip whitespace then expect a value or `]`.
    ArrayStart,

    // A complete value was produced; skip whitespace then pop the context stack.
    AfterValue,
}

#[derive(PartialEq, Debug, Clone)]
pub enum Value<'a> {
    String(&'a str),
    Number(&'a str),
    Bool(bool),
    Null,
    Object(Vec<(&'a str, Value<'a>)>),
    Array(Vec<Value<'a>>)
}

pub fn parse_json<'a>(src: &'a str) -> Option<Value<'a>> {
    let parser = ByteStateConstants::new();
    let bytes = src.as_bytes();
    // Partially-built Object or Array sitting on the frame stack.
    enum Frame<'a> {
        Obj { key: &'a str, members: Vec<(&'a str, Value<'a>)> },
        Arr { elements: Vec<Value<'a>> },
    }

    // Parse a completed atom string into the right Value variant.
    fn parse_atom<'a>(s: &'a str) -> Value<'a> {
        match s {
            "true"  => Value::Bool(true),
            "false" => Value::Bool(false),
            "null"  => Value::Null,
            n       => Value::Number(n),
        }
    }

    // Push a completed Value into the top frame, or set the top-level result.
    fn push_value<'a>(val: Value<'a>, frames: &mut Vec<Frame<'a>>, result: &mut Option<Value<'a>>) {
        match frames.last_mut() {
            Some(Frame::Arr { elements }) => elements.push(val),
            Some(Frame::Obj { key, members }) => members.push((*key, val)),
            None => *result = Some(val),
        }
    }

    // Close the top frame with `}` or `]` and push the resulting Value.
    fn close_frame<'a>(byte: u8, frames: &mut Vec<Frame<'a>>, result: &mut Option<Value<'a>>) {
        match byte {
            b'}' => {
                if let Some(Frame::Obj { members, .. }) = frames.pop() {
                    push_value(Value::Object(members), frames, result);
                }
            }
            b']' => {
                if let Some(Frame::Arr { elements }) = frames.pop() {
                    push_value(Value::Array(elements), frames, result);
                }
            }
            _ => {}
        }
    }

    let mut frames: Vec<Frame> = Vec::new();
    let mut str_start: usize = 0;   // absolute byte offset of char after opening '"'
    let mut atom_start: usize = 0;  // absolute byte offset of first atom byte
    let mut current_key: &str = ""; // key slice captured when KeyChars closes
    let mut state = State::ValueWhitespace;
    let mut result: Option<Value> = None;

    let mut pos = 0;
    while pos < bytes.len() {
        let chunk_len = (bytes.len() - pos).min(64);
        let chunk = &bytes[pos..pos + chunk_len];
        let byte_state = next_state(chunk, &parser);

        let mut chunk_offset = 0;
        'inner: while chunk_offset < chunk_len {
            let byte = chunk[chunk_offset];
            state = match state {
                State::ValueWhitespace => {
                    stat!(crate::stats::VALUE_WHITESPACE);
                    // Compute the distance to the first non-whitespace byte in
                    // the remaining chunk using a single trailing-zeros count,
                    // skipping the whole run in one operation.
                    let ahead = (!byte_state.whitespace) >> chunk_offset;
                    let skip = ahead.trailing_zeros() as usize; // 64 when all whitespace
                    chunk_offset += skip;
                    if chunk_offset >= chunk_len {
                        break 'inner;
                    }
                    let byte = chunk[chunk_offset];
                    match byte {
                        b'{' => { frames.push(Frame::Obj { key: "", members: Vec::new() }); State::ObjectStart }
                        b'[' => { frames.push(Frame::Arr { elements: Vec::new() }); State::ArrayStart }
                        b'"' => { str_start = pos + chunk_offset + 1; State::StringChars }
                        _    => { atom_start = pos + chunk_offset; State::AtomChars }
                    }
                },

            State::StringChars => {
                stat!(crate::stats::STRING_CHARS);
                // Quotes preceded by a backslash are escaped and do not end
                // the string.  Mask them out; then find the first interesting
                // byte (unescaped quote or backslash) with trailing_zeros.
                // Note: a backslash at chunk byte 63 that escapes a quote at
                // byte 0 of the next chunk is handled correctly by the
                // per-byte fallback on that next chunk.
                let unescaped_quotes = byte_state.quotes & !(byte_state.backslashes << 1);
                let interesting = (byte_state.backslashes | unescaped_quotes) >> chunk_offset;
                let skip = interesting.trailing_zeros() as usize;
                chunk_offset = (chunk_offset + skip).min(chunk_len);
                if chunk_offset >= chunk_len {
                    break 'inner;
                }
                let byte = chunk[chunk_offset];
                match byte {
                    b'\\' => State::StringEscape,
                    b'"'  => {
                        push_value(Value::String(&src[str_start..pos + chunk_offset]), &mut frames, &mut result);
                        State::AfterValue
                    }
                    _ => State::StringChars,
                }
            },
            State::StringEscape => { stat!(crate::stats::STRING_ESCAPE); State::StringChars },

            State::KeyChars => {
                stat!(crate::stats::KEY_CHARS);
                let unescaped_quotes = byte_state.quotes & !(byte_state.backslashes << 1);
                let interesting = (byte_state.backslashes | unescaped_quotes) >> chunk_offset;
                let skip = interesting.trailing_zeros() as usize;
                chunk_offset = (chunk_offset + skip).min(chunk_len);
                if chunk_offset >= chunk_len {
                    break 'inner;
                }
                let byte = chunk[chunk_offset];
                match byte {
                    b'\\' => State::KeyEscape,
                    b'"'  => { current_key = &src[str_start..pos + chunk_offset]; State::KeyEnd }
                    _ => State::KeyChars,
                }
            },
            State::KeyEscape => { stat!(crate::stats::KEY_ESCAPE); State::KeyChars },
            State::KeyEnd => {
                stat!(crate::stats::KEY_END);
                let ahead = (!byte_state.whitespace) >> chunk_offset;
                let skip = ahead.trailing_zeros() as usize;
                chunk_offset += skip;
                if chunk_offset >= chunk_len { break 'inner; }
                let byte = chunk[chunk_offset];
                match byte {
                    b':' => {
                        if let Some(Frame::Obj { key, .. }) = frames.last_mut() {
                            *key = current_key;
                        }
                        State::AfterColon
                    }
                    _ => State::KeyEnd,
                }
            },
            State::AfterColon => {
                stat!(crate::stats::AFTER_COLON);
                let ahead = (!byte_state.whitespace) >> chunk_offset;
                let skip = ahead.trailing_zeros() as usize;
                chunk_offset += skip;
                if chunk_offset >= chunk_len { break 'inner; }
                let byte = chunk[chunk_offset];
                match byte {
                    b'{' => { frames.push(Frame::Obj { key: "", members: Vec::new() }); State::ObjectStart }
                    b'[' => { frames.push(Frame::Arr { elements: Vec::new() }); State::ArrayStart }
                    b'"' => { str_start = pos + chunk_offset + 1; State::StringChars }
                    _    => { atom_start = pos + chunk_offset; State::AtomChars }
                }
            },

            State::AtomChars => {
                stat!(crate::stats::ATOM_CHARS);
                // Skip non-delimiter bytes in bulk: delimiters has bits set at
                // whitespace, ',', '}' and ']'.
                let ahead = byte_state.delimiters >> chunk_offset;
                let skip = ahead.trailing_zeros() as usize;
                chunk_offset += skip;
                if chunk_offset >= chunk_len { break 'inner; }
                let byte = chunk[chunk_offset];
                push_value(parse_atom(&src[atom_start..pos + chunk_offset]), &mut frames, &mut result);
                match byte {
                    b'}' => { close_frame(b'}', &mut frames, &mut result); State::AfterValue }
                    b']' => { close_frame(b']', &mut frames, &mut result); State::AfterValue }
                    b',' => match frames.last() {
                        Some(Frame::Arr { .. }) => State::ArrayStart,
                        Some(Frame::Obj { .. }) => State::ObjectStart,
                        None                    => State::AfterValue,
                    },
                    _ => State::AfterValue, // whitespace delimiter
                }
            },

            State::ObjectStart => {
                stat!(crate::stats::OBJECT_START);
                let ahead = (!byte_state.whitespace) >> chunk_offset;
                let skip = ahead.trailing_zeros() as usize;
                chunk_offset += skip;
                if chunk_offset >= chunk_len { break 'inner; }
                let byte = chunk[chunk_offset];
                match byte {
                    b'"' => { str_start = pos + chunk_offset + 1; State::KeyChars }
                    b'}' => {
                        close_frame(b'}', &mut frames, &mut result);
                        State::AfterValue
                    }
                    _ => State::ObjectStart,
                }
            },

            State::ArrayStart => {
                stat!(crate::stats::ARRAY_START);
                let ahead = (!byte_state.whitespace) >> chunk_offset;
                let skip = ahead.trailing_zeros() as usize;
                chunk_offset += skip;
                if chunk_offset >= chunk_len { break 'inner; }
                let byte = chunk[chunk_offset];
                match byte {
                    b']' => {
                        close_frame(b']', &mut frames, &mut result);
                        State::AfterValue
                    }
                    b'{' => { frames.push(Frame::Obj { key: "", members: Vec::new() }); State::ObjectStart }
                    b'[' => { frames.push(Frame::Arr { elements: Vec::new() }); State::ArrayStart }
                    b'"' => { str_start = pos + chunk_offset + 1; State::StringChars }
                    _    => { atom_start = pos + chunk_offset; State::AtomChars }
                }
            },

            State::AfterValue => {
                stat!(crate::stats::AFTER_VALUE);
                let ahead = (!byte_state.whitespace) >> chunk_offset;
                let skip = ahead.trailing_zeros() as usize;
                chunk_offset += skip;
                if chunk_offset >= chunk_len { break 'inner; }
                let byte = chunk[chunk_offset];
                match byte {
                    b',' => match frames.last() {
                        Some(Frame::Obj { .. }) => State::ObjectStart,
                        Some(Frame::Arr { .. }) => State::ArrayStart,
                        None                    => State::AfterValue,
                    },
                    b'}' => {
                        close_frame(b'}', &mut frames, &mut result);
                        State::AfterValue
                    }
                    b']' => {
                        close_frame(b']', &mut frames, &mut result);
                        State::AfterValue
                    }
                    _ => State::AfterValue,
                }
            },
            };
            chunk_offset += 1;
        }
        pos += chunk_len;
    }

    // Flush a trailing atom not followed by a delimiter (e.g. top-level `42`).
    if state == State::AtomChars {
        push_value(parse_atom(&src[atom_start..]), &mut frames, &mut result);
    }

    result
}


/// Per-chunk classification masks produced by `next_state`.
struct ByteState {
    whitespace:  u64, // bit n set => byte n is whitespace (<= 0x20)
    quotes:      u64, // bit n set => byte n is '"'
    backslashes: u64, // bit n set => byte n is '\\'
    delimiters:  u64, // bit n set => byte n ends an atom (whitespace | ',' | '}' | ']')
}

/// Pre-built 64-byte needle vectors for AVX-512 comparisons.
struct ByteStateConstants {
    space:          [u8; 64],
    quote:          [u8; 64],
    backslash:      [u8; 64],
    comma:          [u8; 64],
    close_brace:    [u8; 64],
    close_bracket:  [u8; 64],
}

impl ByteStateConstants {
    fn new() -> Self {
        Self {
            space:         [b' ';  64],
            quote:         [b'"'; 64],
            backslash:     [b'\\'; 64],
            comma:         [b',';  64],
            close_brace:   [b'}';  64],
            close_bracket: [b']';  64],
        }
    }
}

/// Classify up to 64 bytes from `src` using AVX-512BW.
/// Bytes beyond `src.len()` are zeroed via masked load; their whitespace bits
/// are set to 1 (0 <= 0x20) but are never visited by the inner loop.
fn next_state(src: &[u8], constants: &ByteStateConstants) -> ByteState {
    assert!(!src.is_empty() && src.len() <= 64);
    // Bits 0..len-1 set, rest clear.
    let load_mask: u64 = if src.len() == 64 { !0u64 } else { (1u64 << src.len()) - 1 };
    let whitespace: u64;
    let quotes: u64;
    let backslashes: u64;
    let delimiters: u64;
    unsafe {
        std::arch::asm!(
            // Masked byte load: only load src.len() bytes, zero the rest.
            "kmovq k1, {load_mask}",
            "vmovdqu8 zmm0 {{k1}}{{z}}, zmmword ptr [{src}]",
            // Issue all six comparisons into distinct k registers so the CPU
            // can execute them in parallel, then move the results to GP
            // registers as a batch at the end.
            "vpcmpub  k2, zmm0, zmmword ptr [{n_space}],         2", // whitespace (<= 0x20)
            "vpcmpeqb k3, zmm0, zmmword ptr [{n_quote}]",            // quotes
            "vpcmpeqb k4, zmm0, zmmword ptr [{n_backslash}]",        // backslashes
            "vpcmpeqb k5, zmm0, zmmword ptr [{n_comma}]",            // comma
            "vpcmpeqb k6, zmm0, zmmword ptr [{n_close_brace}]",      // '}'
            "vpcmpeqb k7, zmm0, zmmword ptr [{n_close_bracket}]",    // ']'
            // Combine delimiter masks in k-registers (no GP round-trip needed).
            "korq k5, k5, k6",   // comma | '}'
            "korq k5, k5, k7",   // | ']'
            "korq k5, k5, k2",   // | whitespace
            // Move all results to GP registers.
            "kmovq {whitespace},  k2",
            "kmovq {quotes},      k3",
            "kmovq {backslashes}, k4",
            "kmovq {delimiters},  k5",
            src             = in(reg)  src.as_ptr(),
            n_space         = in(reg)  constants.space.as_ptr(),
            n_quote         = in(reg)  constants.quote.as_ptr(),
            n_backslash     = in(reg)  constants.backslash.as_ptr(),
            n_comma         = in(reg)  constants.comma.as_ptr(),
            n_close_brace   = in(reg)  constants.close_brace.as_ptr(),
            n_close_bracket = in(reg)  constants.close_bracket.as_ptr(),
            load_mask       = in(reg)  load_mask,
            whitespace      = out(reg) whitespace,
            quotes          = out(reg) quotes,
            backslashes     = out(reg) backslashes,
            delimiters      = out(reg) delimiters,
            out("zmm0") _,
            out("k1") _, out("k2") _, out("k3") _,
            out("k4") _, out("k5") _, out("k6") _, out("k7") _,
            options(nostack, readonly),
        );
    }
    ByteState { whitespace, quotes, backslashes, delimiters }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn run(json: &'static str) -> Option<Value<'static>> {
        parse_json(json)
    }

    fn s(v: &'static str) -> Value<'static> { Value::String(v) }
    fn n(v: &'static str) -> Value<'static> { Value::Number(v) }
    fn obj(members: &[(&'static str, Value<'static>)]) -> Value<'static> {
        Value::Object(members.iter().map(|(k, v)| (*k, v.clone())).collect())
    }
    fn arr(elements: Vec<Value<'static>>) -> Value<'static> { Value::Array(elements) }

    #[test]
    fn test_string() {
        assert_eq!(run(r#""hello""#), Some(s("hello")));
    }

    #[test]
    fn test_escaped_string() {
        // Escapes are stored raw (backslash + char), not expanded.
        assert_eq!(run(r#""hello \"world\"""#), Some(s(r#"hello \"world\""#)));
        assert_eq!(run(r#""line\\nbreak""#),    Some(s(r#"line\\nbreak"#)));
        assert_eq!(run(r#""tab\there""#),       Some(s(r#"tab\there"#)));
    }

    #[test]
    fn test_number() {
        assert_eq!(run("42"), Some(n("42")));
    }

    #[test]
    fn test_bool_null() {
        assert_eq!(run("true"),  Some(Value::Bool(true)));
        assert_eq!(run("false"), Some(Value::Bool(false)));
        assert_eq!(run("null"),  Some(Value::Null));
    }

    #[test]
    fn test_empty_object() {
        assert_eq!(run("{}"), Some(Value::Object(vec![])));
    }

    #[test]
    fn test_empty_array() {
        assert_eq!(run("[]"), Some(Value::Array(vec![])));
    }

    #[test]
    fn test_simple_object() {
        assert_eq!(
            run(r#"{"key": "value"}"#),
            Some(obj(&[("key", s("value"))]))
        );
    }

    #[test]
    fn test_multi_key_object() {
        assert_eq!(
            run(r#"{ "key1" : "value1" , "key2": [123, 456, 768], "key3" : { "nested_key" : true } }"#),
            Some(obj(&[
                ("key1", s("value1")),
                ("key2", arr(vec![n("123"), n("456"), n("768")])),
                ("key3", obj(&[("nested_key", Value::Bool(true))])),
            ]))
        );
    }

    #[test]
    fn test_array_of_mixed() {
        assert_eq!(
            run(r#"[1, "two", true, null, {"x": 3}]"#),
            Some(arr(vec![
                n("1"),
                s("two"),
                Value::Bool(true),
                Value::Null,
                obj(&[("x", n("3"))]),
            ]))
        );
    }

    #[test]
    fn test_whitespace() {
        assert_eq!(run("  \n  42  \n"), Some(n("42")));
    }
}
