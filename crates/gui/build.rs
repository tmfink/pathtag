use std::env;
use std::path::PathBuf;
use std::process::Command;

const LOGO_PATH: &str = "assets/pathtag-logo.svg";

fn main() {
    println!("cargo:rerun-if-changed={LOGO_PATH}");
    println!("cargo:rerun-if-env-changed=RSVG_CONVERT_BIN");

    let rsvg_bin = env::var("RSVG_CONVERT_BIN").unwrap_or_else(|_| "rsvg-convert".to_string());

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let input_svg = PathBuf::from(&manifest_dir).join(LOGO_PATH);

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
    let output_png = PathBuf::from(&out_dir).join("icon.png");

    let status = Command::new(&rsvg_bin)
        .arg(&input_svg)
        .arg("-o")
        .arg(&output_png)
        .status()
        .unwrap_or_else(|e| {
            panic!(
                "Failed to execute '{}'. Make sure it's installed or properly set via RSVG_CONVERT_BIN. Error: {}",
                rsvg_bin, e
            );
        });

    if !status.success() {
        panic!("'{}' failed to convert the SVG to PNG.", rsvg_bin);
    }
}
