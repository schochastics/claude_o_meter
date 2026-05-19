use std::env;
use std::fs;
use std::path::PathBuf;

const ICON_SIZE: u32 = 44; // 22 logical × 2 for retina sharpness

fn main() {
    println!("cargo:rerun-if-changed=assets/claude_symbol.svg");
    println!("cargo:rerun-if-changed=build.rs");

    let svg = fs::read("assets/claude_symbol.svg").expect("read SVG");
    let tree =
        usvg::Tree::from_data(&svg, &usvg::Options::default()).expect("parse Claude symbol SVG");

    let mut pix = tiny_skia::Pixmap::new(ICON_SIZE, ICON_SIZE).expect("allocate pixmap");
    let scale = ICON_SIZE as f32 / 100.0; // viewBox is 100×100
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pix.as_mut(),
    );

    // Keep only the alpha channel — RGB is irrelevant since we re-tint per
    // utilization band at runtime.
    let mask: Vec<u8> = pix.data().chunks_exact(4).map(|px| px[3]).collect();

    let out =
        PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR set by cargo")).join("icon_mask.bin");
    fs::write(&out, &mask).expect("write mask blob");
}
