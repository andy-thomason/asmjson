//! Parallel JSON-lines parsing via memory-mapped I/O.
//!
//! Memory-maps a JSON Lines file (one JSON value per line), partitions it
//! into ~1 MiB chunks each ending on a `\n` boundary, then parses every
//! chunk concurrently with a SAX string counter using Rayon.
//!
//! CPUID auto-selects the AVX-512BW assembly path when available.
//!
//! ## Usage
//!
//! ```sh
//! cargo run --example mmap_parallel -- path/to/file.jsonl
//! ```
//!
//! ## Generating a test file
//!
//! ```sh
//! python3 -c "
//! import json, random, string
//! for i in range(100_000):
//!     print(json.dumps({'id': i, 'name': ''.join(random.choices(string.ascii_lowercase, k=8)), 'value': random.random()}))
//! " > /tmp/test.jsonl
//! cargo run --example mmap_parallel -- /tmp/test.jsonl
//! ```

#[cfg(target_arch = "x86_64")]
use asmjson::parse_with_zmm;
use asmjson::{Sax, parse_with};
use memmap2::Mmap;
use rayon::prelude::*;
use std::{env, fs, path::PathBuf};

// ---------------------------------------------------------------------------
// SAX writer — counts string values and keys
// ---------------------------------------------------------------------------

#[derive(Default, Debug)]
struct StringCounter {
    strings: usize,
    keys: usize,
}

impl<'src> Sax<'src> for StringCounter {
    type Output = Self;

    fn null(&mut self) {}

    fn bool_val(&mut self, _v: bool) {}

    fn number(&mut self, _s: &'src str) {}

    fn string(&mut self, _s: &'src str) {
        self.strings += 1;
    }

    fn escaped_string(&mut self, _s: &str) {
        self.strings += 1;
    }

    fn key(&mut self, _s: &'src str) {
        self.keys += 1;
    }

    fn escaped_key(&mut self, _s: &str) {
        self.keys += 1;
    }

    fn start_object(&mut self) {}

    fn end_object(&mut self) {}

    fn start_array(&mut self) {}

    fn end_array(&mut self) {}

    fn finish(self) -> Option<Self::Output> {
        Some(self)
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

const CHUNK_SIZE: usize = 1 << 20; // 1 MiB

/// Parse one line of JSON, accumulating into `out`.
fn parse_line_into(line: &str, out: &mut StringCounter) {
    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx512bw") {
        if let Some(c) = unsafe { parse_with_zmm(line, StringCounter::default()) } {
            out.strings += c.strings;
            out.keys += c.keys;
        }
        return;
    }
    if let Some(c) = parse_with(line, StringCounter::default()) {
        out.strings += c.strings;
        out.keys += c.keys;
    }
}

/// Parse every non-empty line in a chunk, returning total counts.
fn parse_chunk(chunk: &str) -> StringCounter {
    let mut out = StringCounter::default();
    for line in chunk.lines() {
        let line = line.trim();
        if !line.is_empty() {
            parse_line_into(line, &mut out);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Chunking
// ---------------------------------------------------------------------------

/// Split `data` into chunks of at most `chunk_size` bytes, each ending at
/// (and including) a `\n` boundary so that every chunk contains only whole
/// JSON Lines.
fn split_at_newlines(data: &[u8], chunk_size: usize) -> Vec<&[u8]> {
    let mut chunks = Vec::new();
    let mut start = 0;

    while start < data.len() {
        let nominal_end = (start + chunk_size).min(data.len());

        // If we've reached the end of the file there is no need to search.
        let end = if nominal_end == data.len() {
            nominal_end
        } else {
            // Advance to the byte after the next '\n', or end of file.
            data[nominal_end..]
                .iter()
                .position(|&b| b == b'\n')
                .map(|i| nominal_end + i + 1)
                .unwrap_or(data.len())
        };

        chunks.push(&data[start..end]);
        start = end;
    }

    chunks
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    let path: PathBuf = env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        eprintln!("usage: mmap_parallel <file.jsonl>");
        std::process::exit(1);
    });

    let file = fs::File::open(&path).unwrap_or_else(|e| {
        eprintln!("cannot open {}: {e}", path.display());
        std::process::exit(1);
    });

    // SAFETY: we hold a shared read-only view of the file and do not modify
    // it or allow the OS to truncate it while the mapping is live.
    let mmap = unsafe { Mmap::map(&file) }.expect("mmap failed");

    let chunks = split_at_newlines(&mmap, CHUNK_SIZE);
    println!(
        "file  : {} bytes  →  {} chunk(s) of ~{} KiB",
        mmap.len(),
        chunks.len(),
        CHUNK_SIZE / 1024,
    );

    let totals: StringCounter = chunks
        .par_iter()
        .map(|chunk| {
            let s = std::str::from_utf8(chunk).expect("non-UTF-8 data in chunk");
            parse_chunk(s)
        })
        .reduce(StringCounter::default, |mut a, b| {
            a.strings += b.strings;
            a.keys += b.keys;
            a
        });

    println!("keys   found : {}", totals.keys);
    println!("strings found: {}", totals.strings);
}
