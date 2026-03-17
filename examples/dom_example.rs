//! DOM-mode parsing example.
//!
//! Demonstrates building a [`Dom`] tape and navigating it with the
//! [`JsonRef`] cursor API.
//!
//! [`dom_parser`] performs a one-time CPUID check and returns a function
//! pointer that dispatches to AVX-512BW assembly or the portable SWAR
//! path as appropriate.
//!
//! ```sh
//! cargo run --example dom_example
//! ```

use asmjson::{Dom, JsonRef, dom_parser};

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

fn inspect(label: &str, tape: Dom) {
    println!("=== {label} ===");

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

fn main() {
    let parse = dom_parser();
    let tape = parse(SRC, None).expect("parse failed");
    inspect("dom_parser", tape);
}
