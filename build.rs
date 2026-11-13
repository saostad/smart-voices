//! Keep the dynamically loaded ONNX Runtime findable beside the executable.
//! scripts/fetch-onnxruntime.sh supplies the pinned runtime library.

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    println!("cargo:rerun-if-changed=packaging/windows/smart-voices.ico");
    if target_os == "windows" {
        winresource::WindowsResource::new()
            .set_icon("packaging/windows/smart-voices.ico")
            .set("ProductName", "Smart Voices")
            .set("FileDescription", "Smart Voices")
            .set("OriginalFilename", "smart-voices.exe")
            .compile()
            .expect("Couldn't embed the Smart Voices Windows application icon");
    }
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
