use std::path::Path;
use std::{env, fs};

// Lives in xtask, whose tests are the only place it can be tested.
#[path = "xtask/src/atdf.rs"]
mod atdf;

const ATDF: &str = "vendor/ATtiny1616.atdf";

fn main() -> Result<(), String> {
    println!("cargo::rerun-if-changed=link.x");
    println!("cargo::rerun-if-changed={ATDF}");

    let xml = fs::read_to_string(ATDF).map_err(|e| format!("can't read the ATDF: {e}"))?;
    let out = env::var("OUT_DIR").map_err(|e| format!("OUT_DIR: {e}"))?;
    let regs = atdf::generate(&xml).map_err(|e| format!("{ATDF}: {e}"))?;
    fs::write(Path::new(&out).join("regs.rs"), regs)
        .map_err(|e| format!("can't write regs.rs: {e}"))?;

    if env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("avr") {
        return Ok(());
    }

    let dir = env::var("CARGO_MANIFEST_DIR").map_err(|e| format!("CARGO_MANIFEST_DIR: {e}"))?;
    println!("cargo::rustc-link-search={dir}");
    println!("cargo::rustc-link-arg-bins=-Tlink.x");
    println!("cargo::rustc-link-arg-examples=-Tlink.x");
    Ok(())
}
