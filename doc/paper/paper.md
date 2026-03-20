---
title: 'asmjson: A hand-written AVX-512BW JSON parser for the Rust ecosystem'
tags:
  - Rust
  - JSON
  - SIMD
  - AVX-512
  - parsing
  - assembly
authors:
  - name: Amy Thomason
    affiliation: 1
affiliations:
  - index: 1
    name: Atomic Increment Ltd, United Kingdom
date: 20 March 2026
bibliography: paper.bib
---

# Summary

`asmjson` is a high-throughput JSON parser for the Rust programming language.
It provides two complementary back-ends: a hand-written x86-64 assembly
implementation that exploits AVX-512BW 512-bit vector instructions to classify
64 bytes of input per clock cycle, and a portable fallback written in pure Rust
using SWAR (SIMD-Within-A-Register) bit-manipulation techniques that requires
no special hardware.  Both paths expose the same safe Rust API, with a runtime
CPUID check selecting the optimal path transparently.

At the API level `asmjson` offers two usage styles:

- **SAX-style** streaming (`Sax` trait): the parser drives user-supplied
  callbacks for each JSON event (null, boolean, number, string, key,
  start/end object, start/end array) without any intermediate allocation.
- **DOM-style** tape (`Dom`): the parser writes a flat array of tagged
  `DomEntry` records that can be traversed with O(1) structural skips;
  a `serde` feature exposes a `Deserializer` backed by this tape.

On an AMD Ryzen 9 9955HX (Zen 5, AVX-512BW) the library reaches
**10.93 GiB/s** single-threaded on string-array workloads, and
**26.6 GiB/s** when 1 024 Rayon tasks parse independent megabyte-sized
chunks of a memory-mapped JSON Lines file in parallel.

# Statement of Need

JSON is the dominant interchange format for web APIs, configuration, and
scientific datasets.  The performance of JSON parsing is consequently a
practical bottleneck in data ingestion pipelines, language model tooling,
bioinformatics workflows, and any application that consumes JSON at scale.

Rust's dominant JSON library, `serde_json`, achieves around 2.4 GiB/s on
string-heavy benchmarks.  Even the fastest competing Rust crates
(`sonic-rs` [@sonic-rs], `simd-json` [@simd-json]) top out near 7 GiB/s.
The gap between these libraries and the theoretical bandwidth of modern
processors leaves substantial performance on the table.

`asmjson` demonstrates that, by writing the inner parsing loop directly in
AVX-512BW assembly, it is possible to sustain throughput of 10–11 GiB/s —
roughly 56 % of the single-threaded DRAM read bandwidth of the test machine —
while doing real work (structure parsing, string length accumulation).  The
library is therefore most directly useful to researchers and engineers who
need to parse very large JSON datasets as fast as possible on modern server
or desktop hardware.

# State of the Field

The published state of the art in SIMD JSON parsing is represented by
`simdjson` [@simdjson], a C++ library by Langdale and Lemire that achieves
multi-GiB/s throughput by classifying bytes with AVX2 or SSE4.2 instructions
and reconstructing document structure in a second "structural indexing" phase.
`simdjson` inspired the Rust crate `simd-json` [@simd-json], which ports the
two-phase approach to Rust.

`sonic-rs` [@sonic-rs] is a more recent Rust implementation that applies AVX2
SIMD to accelerate specific hot loops while retaining a more conventional
byte-at-a-time dispatch structure.

`asmjson` differs architecturally from all of these in three respects.
First, it uses AVX-512BW (512-bit, 64-byte) vectors rather than
AVX2 (256-bit) or SSE4.2 (128-bit), doubling the bytes classified per
instruction.  Second, it uses *direct threading*: every parser state ends
with an unconditional jump to the next state label, eliminating the
indirection of a switch table and removing the branch-predictor pressure of
a state variable.  Third, the DOM back-end writes tape entries directly from
assembly without any Rust-side dispatch, keeping the critical path entirely
inside a single translation unit.

Table: Benchmark throughput on a Ryzen 9 9955HX (Zen 5).  "string array" and
"string object" workloads contain 10 MiB of synthetic JSON; "mixed" is a
realistic document with varied value types.  Each measurement includes full
traversal of all values after parsing. []{label="tab:bench"}

| Parser               | string array | string object | mixed      |
|----------------------|:------------:|:-------------:|:----------:|
| asmjson/sax (zmm)    | 10.78 GiB/s  |  8.29 GiB/s   | 1.17 GiB/s |
| asmjson/dom (zmm)    | 10.93 GiB/s  |  6.94 GiB/s   |  897 MiB/s |
| asmjson/u64 (SWAR)   |  7.02 GiB/s  |  4.91 GiB/s   |  607 MiB/s |
| sonic-rs             |  6.92 GiB/s  |  4.06 GiB/s   |  478 MiB/s |
| serde\_json          |  2.41 GiB/s  |   534 MiB/s   |   78 MiB/s |
| simd-json †          |  1.91 GiB/s  |  1.19 GiB/s   |  174 MiB/s |

† simd-json requires a mutable copy of the input buffer, so each benchmark
iteration includes a `Vec::clone` of the 10 MiB dataset.

# Software Design

## AVX-512BW classification

At the heart of the hot loop, each 64-byte chunk of source text is classified
in a single pass using five AVX-512BW mask instructions.  The result is four
64-bit bitmasks (one bit per source byte) that indicate which bytes are
whitespace, quote characters ("), backslashes (\), and structural delimiters
(`,`, `}`, `]`).

```asm
; Listing 1 — chunk classification (from parse_json_zmm_sax.S)
; zmm0 holds the 64-byte input chunk (zero-masked load).
; r9 points to the 64-byte constant vectors in .rodata.
vpcmpub  k2, zmm0, zmmword ptr [r9      ], 2  ; whitespace: byte <= 0x20
vpcmpeqb k3, zmm0, zmmword ptr [r9 +  64]     ; quotes     (0x22)
vpcmpeqb k4, zmm0, zmmword ptr [r9 + 128]     ; backslashes (0x5C)
vpcmpeqb k5, zmm0, zmmword ptr [r9 + 192]     ; commas     (0x2C)
vpcmpeqb k6, zmm0, zmmword ptr [r9 + 256]     ; '}'        (0x7D)
vpcmpeqb k7, zmm0, zmmword ptr [r9 + 320]     ; ']'        (0x5D)
korq    k5, k5, k6
korq    k5, k5, k7
korq    k5, k5, k2             ; delimiters = ws | ',' | '}' | ']'
```

Each `vpcmpub` / `vpcmpeqb` instruction produces a 64-bit opmask register in
a single cycle; no byte-level shuffles or permutations are required.

## Direct-threading and bitmask dispatch

Within each chunk, the parser maintains a `chunk_offset` counter (register
`rcx`) pointing to the current byte.  To skip whitespace, the whitespace
bitmask is inverted to obtain the non-whitespace bits, shifted right by
`chunk_offset`, and then `tzcnt` counts the trailing zeros — giving the
distance to the first non-whitespace byte in a single instruction.

```asm
; Listing 2 — whitespace skip (from parse_json_zmm_sax.S)
.Lvalue_whitespace:
    mov     rax, qword ptr [rbp + LOC_WS]
    not     rax                  ; flip: 1 = non-whitespace
    shr     rax, cl              ; align to current chunk_offset
    tzcnt   rax, rax             ; distance to first non-ws byte
    add     rcx, rax             ; advance chunk_offset
    cmp     rcx, qword ptr [rbp + LOC_CHUNK_LEN]
    jae     .Lrefetch_value_whitespace
```

The same pattern repeats for string bodies and atom characters, making each
"advance past N bytes" operation O(1) regardless of how many bytes are skipped.

When a chunk is exhausted, register `r10` holds the address of the target
state and execution falls through to the chunk-fetch routine, which loads and
classifies the next 64 bytes before jumping directly to `r10`.  No state
variable is stored in memory; the program counter *is* the parser state.

## Tape-based DOM

The DOM back-end writes `DomEntry` records directly from assembly into a
pre-allocated `Vec<DomEntry>`.  Each record is a fixed-size tagged union
(scalar, string, key, object header, array header) with a skip-count field
so that `DomRef::get`, `array_iter`, and `object_iter` can jump over
sub-trees in O(1) time.  Because the tape is a single contiguous allocation
the traversal access pattern is cache-friendly and avoids the pointer-chasing
overhead of a tree-of-`Box` DOM like `serde_json::Value`.

## Portable SWAR fallback

The SWAR classifier operates on 8 bytes at a time using only integer arithmetic.
It tests for whitespace by subtracting `0x2020202020202020` and checking the
high bit of each byte; quotes and backslashes are found with XOR and carry-free
tricks.  On the test machine this path reaches 7.02 GiB/s — faster than
AVX2-accelerated `sonic-rs` — because it avoids the overhead of scalar
fallbacks for partial chunks and keeps the code in a tight register-only loop.

## API surface

```rust
// Ergonomic safe API — runtime selects AVX-512BW or SWAR automatically.
let parse = dom_parser();
let tape = parse(r#"{"x": [1, 2, 3]}"#, None)?;
assert_eq!(tape.root().get("x").index_at(1).as_i64(), Some(2));

// Serde deserialization through the tape.
let records: Vec<MyRecord> = from_taperef(tape.root())?;
```

# Research Impact Statement

`asmjson` is an experimental library at an early stage of development.  It
has been released on crates.io [@asmjson-crate] and the implementation has
been validated against a corpus of well-formed JSON inputs.  The primary
contribution is a demonstration that hand-written AVX-512BW assembly can
exceed the throughput of existing Rust SIMD JSON libraries by 57 % or more
on string-heavy workloads, while the portable SWAR fallback already beats
AVX2-based competitors without any vector instructions.

Future directions include: extension of the direct-threading technique to CSV
and TSV file formats; a compact ULEB-encoded tape representation to improve
cache utilisation; and a procedural macro interface that drives the SAX layer
to deserialise into typed Rust structs without the intermediate tape.

# AI Usage Disclosure

Portions of the Rust wrapper, test suite, and documentation (including this
paper) were drafted with the assistance of GitHub Copilot (Claude Sonnet).
All generated code was reviewed, tested, and committed by the human author.
The hand-written AVX-512BW assembly in `asm/x86_64/` was authored by the
human author without AI assistance.

# Acknowledgements

The author thanks the developers of `sonic-rs`, `simd-json`, and `simdjson`
whose published implementations and benchmarking methodology informed the
design of `asmjson`.

# References
