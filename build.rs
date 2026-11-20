use std::path::Path;
use std::{env, fs};

// Lives in xtask, whose tests are the only place it can be tested.
#[path = "xtask/src/atdf.rs"]
mod atdf;

const ATDF: &str = "vendor/ATtiny1616.atdf";

fn main() {
    println!("cargo::rerun-if-changed=link.x");
    println!("cargo::rerun-if-changed={ATDF}");

    let xml = fs::read_to_string(ATDF).expect("can't read the ATDF");
    let out = env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    fs::write(Path::new(&out).join("regs.rs"), atdf::generate(&xml)).expect("can't write regs.rs");

    if env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("avr") {
        return;
    }

    let dir = env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    println!("cargo::rustc-link-search={dir}");
    println!("cargo::rustc-link-arg-bins=-Tlink.x");
    println!("cargo::rustc-link-arg-examples=-Tlink.x");
}
