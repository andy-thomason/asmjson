//! DOM-mode parsing example.
//!
//! Demonstrates building a [`Dom`] tape and navigating it with the
//! [`JsonRef`] cursor API.
//!
//! Two entry points are shown:
//!
//! | Function | Description |
//! |----------|-------------|
//! | [`parse_to_dom`] | Portable SWAR classifier — runs on any architecture. |
//! | [`parse_to_dom_zmm`] | Hand-written AVX-512BW x86-64 assembly — `unsafe`, x86_64 only. |
//!
//! Run the portable version:
//!
//! ```sh
//! cargo run --example dom_example
//! ```
//!
//! Run the AVX-512BW assembly version (requires a Skylake-X or later CPU):
//!
//! ```sh
//! cargo run --example dom_example -- zmm
//! ```

#[cfg(target_arch = "x86_64")]
use asmjson::parse_to_dom_zmm;
use asmjson::{JsonRef, parse_to_dom};

const SRC: &str = r#"
{
    "name": "Alice",
    "age": 30,
    "active": true,
    "score": 9.5,
    "tags": ["rust", "json", "simd"],
    "address": {
        "city": "Springfield",
        "zip": "12345"
    },
    "notes": null
}
"#;

fn inspect(label: &str, src: &str) {
    println!("=== {label} ===");

    let tape = parse_to_dom(src).expect("parse failed");
    let root = tape.root().expect("empty tape");

    // Scalar fields
    println!("name   : {:?}", root.get("name").as_str());
    println!("age    : {:?}", root.get("age").as_i64());
    println!("active : {:?}", root.get("active").as_bool());
    println!("score  : {:?}", root.get("score").as_f64());
    println!("notes  : is_null={}", root.get("notes").is_null());

    // Array — iterate with index_at
    let tags = root.get("tags").expect("tags missing");
    let tag_count = tags.len().unwrap_or(0);
    print!("tags   :");
    for i in 0..tag_count {
        print!(" {:?}", tags.index_at(i).as_str());
    }
    println!();

    // Nested object
    println!("city   : {:?}", root.get("address").get("city").as_str());
    println!("zip    : {:?}", root.get("address").get("zip").as_str());
    println!();
}

#[cfg(target_arch = "x86_64")]
fn inspect_zmm(label: &str, src: &str) {
    println!("=== {label} ===");

    // SAFETY: caller must ensure the CPU supports AVX-512BW.
    // `None` tells the parser to pick a sensible initial tape capacity.
    let tape = unsafe { parse_to_dom_zmm(src, None) }.expect("parse failed");
    let root = tape.root().expect("empty tape");

    println!("name   : {:?}", root.get("name").as_str());
    println!("age    : {:?}", root.get("age").as_i64());
    println!("active : {:?}", root.get("active").as_bool());
    println!("score  : {:?}", root.get("score").as_f64());
    println!("notes  : is_null={}", root.get("notes").is_null());

    let tags = root.get("tags").expect("tags missing");
    let tag_count = tags.len().unwrap_or(0);
    print!("tags   :");
    for i in 0..tag_count {
        print!(" {:?}", tags.index_at(i).as_str());
    }
    println!();

    println!("city   : {:?}", root.get("address").get("city").as_str());
    println!("zip    : {:?}", root.get("address").get("zip").as_str());
    println!();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let use_zmm = args.get(1).map(|s| s == "zmm").unwrap_or(false);

    if use_zmm {
        #[cfg(target_arch = "x86_64")]
        inspect_zmm("parse_to_dom_zmm  (AVX-512BW assembly)", SRC);

        #[cfg(not(target_arch = "x86_64"))]
        {
            eprintln!("parse_to_dom_zmm is only available on x86_64");
            std::process::exit(1);
        }
    } else {
        inspect("parse_to_dom  (portable SWAR)", SRC);
    }
}
