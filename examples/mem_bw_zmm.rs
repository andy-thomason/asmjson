//! Peak memory-read bandwidth benchmark using AVX-512 ZMM (512-bit) loads.
//!
//! Allocates a 2 GiB buffer, initialises it (touching every page to force
//! physical backing), then measures back-to-back sequential reads with two
//! strategies:
//!
//! * **zmm** — regular `vmovdqu64` (temporal) loads.
//! * **zmm-nt** — `vmovntdqa` (non-temporal streaming) loads; bypasses the
//!   CPU read-allocate path, which can improve pure read throughput on some
//!   microarchitectures.
//!
//! Both paths OR all loaded vectors into an accumulator that is stored at the
//! end to prevent dead-code elimination.
//!
//! ## Usage
//!
//! ```sh
//! cargo run --release --example mem_bw_zmm
//! ```

#![cfg(target_arch = "x86_64")]

use std::alloc::{Layout, alloc, dealloc};
use std::arch::x86_64::*;
use std::time::Instant;

// 2 GiB — well beyond the 64 MB L3 cache of the Ryzen 9955HX.
const BUF_BYTES: usize = 2 << 30;
const PASSES: usize = 8;

// ---------------------------------------------------------------------------
// ZMM temporal loads  (vmovdqu64)
// ---------------------------------------------------------------------------

#[target_feature(enable = "avx512f")]
unsafe fn pass_zmm(ptr: *const u8, chunks: usize) -> u64 {
    let mut acc = _mm512_setzero_si512();
    for i in 0..chunks {
        let v = unsafe { _mm512_loadu_si512(ptr.add(i * 64) as *const __m512i) };
        acc = _mm512_or_si512(acc, v);
    }
    let mut tmp = [0i64; 8];
    unsafe { _mm512_storeu_si512(tmp.as_mut_ptr() as *mut __m512i, acc) };
    tmp.iter().fold(0u64, |a, &x| a ^ x as u64)
}

// ---------------------------------------------------------------------------
// ZMM non-temporal loads  (vmovntdqa) — requires 64-byte aligned source
// ---------------------------------------------------------------------------

#[target_feature(enable = "avx512f")]
unsafe fn pass_zmm_nt(ptr: *const u8, chunks: usize) -> u64 {
    let mut acc = _mm512_setzero_si512();
    for i in 0..chunks {
        let p = unsafe { ptr.add(i * 64) as *const __m512i };
        let v = unsafe { _mm512_stream_load_si512(p) };
        acc = _mm512_or_si512(acc, v);
    }
    let mut tmp = [0i64; 8];
    unsafe { _mm512_storeu_si512(tmp.as_mut_ptr() as *mut __m512i, acc) };
    tmp.iter().fold(0u64, |a, &x| a ^ x as u64)
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn run_passes<F: Fn() -> u64>(label: &str, bytes: usize, f: F) {
    let mut times = Vec::with_capacity(PASSES);
    let mut sink = 0u64;
    for _ in 0..PASSES {
        let t0 = Instant::now();
        sink ^= f();
        times.push(t0.elapsed().as_secs_f64());
    }
    std::hint::black_box(sink);
    times.sort_by(f64::total_cmp);
    let gib = bytes as f64 / (1u64 << 30) as f64;
    let best = gib / times[0];
    let median = gib / times[PASSES / 2];
    println!("{label:8}  best {best:6.1} GiB/s   median {median:6.1} GiB/s");
}

fn main() {
    if !is_x86_feature_detected!("avx512f") {
        eprintln!("AVX-512F not available on this CPU.");
        return;
    }

    let mib = BUF_BYTES / (1 << 20);
    println!("Allocating and touching {mib} MiB buffer (64-byte aligned)…");

    // 64-byte aligned allocation so vmovntdqa is legal.
    let layout = Layout::from_size_align(BUF_BYTES, 64).unwrap();
    let ptr = unsafe { alloc(layout) };
    assert!(!ptr.is_null(), "allocation failed");

    // Touch every page to back physical memory; also seed data so the reads
    // are non-trivial.
    {
        let slice = unsafe { std::slice::from_raw_parts_mut(ptr as *mut u64, BUF_BYTES / 8) };
        for (i, x) in slice.iter_mut().enumerate() {
            *x = i as u64;
        }
    }

    let chunks = BUF_BYTES / 64;
    println!("Running {PASSES} passes × {:.0} GiB…\n", BUF_BYTES as f64 / (1u64 << 30) as f64);

    run_passes("zmm", BUF_BYTES, || unsafe { pass_zmm(ptr, chunks) });
    run_passes("zmm-nt", BUF_BYTES, || unsafe { pass_zmm_nt(ptr, chunks) });

    unsafe { dealloc(ptr, layout) };
}
