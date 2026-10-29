//! Keep the dynamically loaded ONNX Runtime findable beside the executable.
//! scripts/fetch-onnxruntime.sh supplies the pinned runtime library.

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
