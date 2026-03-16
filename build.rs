fn main() {
    // Assemble the hand-written AVX-512BW parsers on x86_64 host builds.
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if target_arch == "x86_64" {
        cc::Build::new()
            .file("asm/x86_64/parse_json_zmm_dyn.S")
            .compile("parse_json_zmm_dyn");
        cc::Build::new()
            .file("asm/x86_64/parse_json_zmm_tape.S")
            .compile("parse_json_zmm_tape");
    }
    println!("cargo:rerun-if-changed=asm/x86_64/parse_json_zmm_dyn.S");
    println!("cargo:rerun-if-changed=asm/x86_64/parse_json_zmm_tape.S");
}
