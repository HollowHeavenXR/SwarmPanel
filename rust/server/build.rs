//! Copies the wasm-bindgen output (web/pkg) into OUT_DIR for include_bytes!.
//! Writes stubs when it hasn't been built yet, so `cargo check` still works.
use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let pkg = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("../web/pkg");
    for (src, dst, stub) in [("swarmpanel_web.js", "web.js", "export default async function init() {}\n".as_bytes()), ("swarmpanel_web_bg.wasm", "web_bg.wasm", &[][..])] {
        let p = pkg.join(src);
        println!("cargo:rerun-if-changed={}", p.display());
        let bytes = fs::read(&p).unwrap_or_else(|_| stub.to_vec());
        fs::write(out.join(dst), bytes).unwrap();
    }
}
