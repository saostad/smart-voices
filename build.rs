//! Keep the voice-runtime shared libraries findable next to the binary.
//!
//! sherpa-onnx links shared (so Pocket and Kokoro/Piper share one ONNX
//! Runtime), and its build script copies `libsherpa-onnx-c-api` and
//! `libonnxruntime` beside the executable — but a dependency's
//! `cargo:rustc-link-arg` rpath does not propagate to this crate, so without
//! this the installed binary cannot find them. `$ORIGIN` (Linux) and
//! `@loader_path` (macOS) point the loader at the executable's own folder;
//! Windows finds DLLs beside the exe with no help.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    match target_os.as_str() {
        "linux" | "android" => {
            println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
        }
        "macos" | "ios" => {
            println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path");
        }
        _ => {}
    }
}
