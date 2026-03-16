# asmjson

[![CI](https://github.com/andy-thomason/asmjson/actions/workflows/ci.yml/badge.svg)](https://github.com/andy-thomason/asmjson/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/asmjson.svg)](https://crates.io/crates/asmjson)
[![docs.rs](https://docs.rs/asmjson/badge.svg)](https://docs.rs/asmjson)

A fast JSON parser that classifies 64 bytes at a time using SIMD or portable
SWAR (SIMD-Within-A-Register) bit tricks, enabling entire whitespace runs and
string bodies to be skipped in a single operation.

## Quick start

```rust
use asmjson::{parse_to_tape, choose_classifier, JsonRef};

let classify = choose_classifier(); // picks best for the current CPU
let tape = parse_to_tape(r#"{"name":"Alice","age":30}"#, classify).unwrap();

assert_eq!(tape.root().get("name").as_str(), Some("Alice"));
assert_eq!(tape.root().get("age").as_i64(), Some(30));
```

For repeated parses, store the result of `choose_classifier` in a static once
cell or pass it through your application rather than calling it on every parse.

## Output formats

- `parse_to_tape` — allocates a flat `Tape` of tokens with O(1) structural skips.
- `parse_with` — drives a custom `JsonWriter` sink; zero extra allocation.

## Classifiers

The classifier is a plain function pointer that labels 64 bytes at a time.
Three are provided:

| Classifier      | ISA           | Speed   |
|-----------------|---------------|---------|
| `classify_zmm`  | AVX-512BW     | fastest |
| `classify_ymm`  | AVX2          | fast    |
| `classify_u64`  | portable SWAR | good    |

Use `choose_classifier` to select automatically at runtime.

## Benchmarks

Measured on a single core with `cargo bench` against 10 MiB of synthetic JSON.
Comparison point is `sonic-rs` (lazy Value, AVX2).

Each benchmark measures **parse + full traversal**: after parsing, every string
value and object key is visited and its length accumulated.  This is necessary
for a fair comparison because sonic-rs defers decoding string content until the
value is accessed (lazy evaluation); a parse-only measurement would undercount
its work relative to any real use-case where the parsed data is actually read.

| Parser               | string array | string object | mixed      |
|----------------------|:------------:|:-------------:|:----------:|
| asmjson zmm tape     | 10.81 GiB/s  | 7.15 GiB/s    | 905 MiB/s  |
| asmjson zmm          | 8.64 GiB/s   | 6.27 GiB/s    | 672 MiB/s  |
| sonic-rs             | 7.11 GiB/s   | 4.04 GiB/s    | 475 MiB/s  |
| asmjson u64          | 7.10 GiB/s   | 4.93 GiB/s    | 636 MiB/s  |
| serde_json           | 2.43 GiB/s   | 535 MiB/s     | 83 MiB/s   |

asmjson zmm tape leads across all three workloads.  It writes a flat
`TapeEntry` array in the assembly parser itself — one pointer-sized entry per
value — so structural traversal is a single linear scan with no pointer
chasing.  The baseline asmjson zmm parser also leads on string-dominated
workloads; the portable `u64` SWAR classifier is neck-and-neck with sonic-rs
on string arrays despite using no SIMD instructions, and beats it on string
objects.  sonic-rs narrows the gap on mixed JSON through its lazy string
decoding, but zmm tape still leads by 90 %.

## License

MIT — see [LICENSE](https://github.com/andy-thomason/asmjson/blob/master/LICENSE).

For internals documentation (state machine annotation, register allocation,
design decisions) see [doc/dev.md](doc/dev.md).
