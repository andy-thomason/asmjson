use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use asmjson::parse_json;

// ---------------------------------------------------------------------------
// Data generators
// ---------------------------------------------------------------------------

/// ~10 MiB JSON array of strings.
///
/// Each element is a 95-character ASCII string (printable, no quotes/backslashes).
/// Enough elements are emitted to reach the target size.
fn gen_string_array(target_bytes: usize) -> String {
    // Element template: "abcdefghij..." (95 visible ASCII chars), plus `,"` overhead.
    let value: String = (b'!'..=b'~')               // 94 printable ASCII chars
        .filter(|&b| b != b'"' && b != b'\\')
        .map(|b| b as char)
        .cycle()
        .take(95)
        .collect();
    let element = format!(r#""{}""#, value);        // ~97 bytes each
    let elements_needed = target_bytes / (element.len() + 1) + 1;

    let mut out = String::with_capacity(target_bytes + 64);
    out.push('[');
    for i in 0..elements_needed {
        if i > 0 { out.push(','); }
        out.push_str(&element);
    }
    out.push(']');
    out
}

/// ~10 MiB JSON object with string keys and string values.
///
/// Keys are "key00000" … "key99999"; values are 85-char ASCII strings.
fn gen_string_object(target_bytes: usize) -> String {
    let value: String = (b'!'..=b'~')
        .filter(|&b| b != b'"' && b != b'\\')
        .map(|b| b as char)
        .cycle()
        .take(85)
        .collect();
    // "keyNNNNN":"<85 chars>"  ≈ 102 bytes per member
    let members_needed = target_bytes / 102 + 1;

    let mut out = String::with_capacity(target_bytes + 64);
    out.push('{');
    for i in 0..members_needed {
        if i > 0 { out.push(','); }
        out.push_str(&format!(r#""key{:05}":"{}""#, i % 100_000, value));
    }
    out.push('}');
    out
}

/// ~10 MiB nested mixed JSON.
///
/// Top-level array of objects; each object contains:
///   "id": <number>, "name": <string>, "active": <bool>,
///   "score": <null|number>, "tags": [<string>, <string>],
///   "meta": { "x": <number>, "y": <number> }
fn gen_mixed(target_bytes: usize) -> String {
    let tag_a = "alpha";
    let tag_b = "beta";
    // One record template (printed below) is ~130 bytes.
    let record_size = 130usize;
    let records_needed = target_bytes / record_size + 1;

    let mut out = String::with_capacity(target_bytes + 64);
    out.push('[');
    for i in 0..records_needed {
        if i > 0 { out.push(','); }
        let active = if i % 2 == 0 { "true" } else { "false" };
        let score  = if i % 3 == 0 { "null".to_string() } else { format!("{:.2}", i as f64 * 0.5) };
        out.push_str(&format!(
            r#"{{"id":{i},"name":"item{i}","active":{active},"score":{score},"tags":["{tag_a}","{tag_b}"],"meta":{{"x":{x},"y":{y}}}}}"#,
            i = i,
            active = active,
            score = score,
            tag_a = tag_a,
            tag_b = tag_b,
            x = i % 1000,
            y = (i * 7) % 1000,
        ));
    }
    out.push(']');
    out
}

// ---------------------------------------------------------------------------
// Benchmarks
// ---------------------------------------------------------------------------

const TARGET: usize = 10 * 1024 * 1024; // 10 MiB

fn bench_string_array(c: &mut Criterion) {
    let data = gen_string_array(TARGET);
    let mut group = c.benchmark_group("string_array");
    group.throughput(Throughput::Bytes(data.len() as u64));
    group.bench_function("parse", |b| {
        b.iter(|| {
            let v = parse_json(&data);
            std::hint::black_box(v);
        });
    });
    group.finish();
}

fn bench_string_object(c: &mut Criterion) {
    let data = gen_string_object(TARGET);
    let mut group = c.benchmark_group("string_object");
    group.throughput(Throughput::Bytes(data.len() as u64));
    group.bench_function("parse", |b| {
        b.iter(|| {
            let v = parse_json(&data);
            std::hint::black_box(v);
        });
    });
    group.finish();
}

fn bench_mixed(c: &mut Criterion) {
    let data = gen_mixed(TARGET);
    let mut group = c.benchmark_group("mixed");
    group.throughput(Throughput::Bytes(data.len() as u64));
    group.bench_function("parse", |b| {
        b.iter(|| {
            let v = parse_json(&data);
            std::hint::black_box(v);
        });
    });
    group.finish();
}

criterion_group!(benches, bench_string_array, bench_string_object, bench_mixed);
criterion_main!(benches);
