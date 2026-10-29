//! Offline converter memory/load benchmark: misaki_bench DATA_ROOT ESPEAK_PARENT.
#[allow(dead_code)]
#[path = "../src/espeak.rs"]
mod espeak;
#[path = "../src/misaki/mod.rs"]
mod misaki;
use smart_voices::engine;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let root = args.next().ok_or("missing misaki data root")?;
    let parent = args.next().ok_or("missing espeak data parent")?;
    let before = rss()?;
    let t = std::time::Instant::now();
    let g = misaki::G2P::load(
        std::path::Path::new(&root),
        std::path::Path::new(&parent),
        false,
    )?;
    let load = t.elapsed();
    let t = std::time::Instant::now();
    println!(
        "{}",
        g.g2p("Kubernetes and Zellij run on Hyprland, and the team shipped it on Friday.")?
    );
    println!(
        "load_seconds={:.6}, conversion_seconds={:.6}, baseline_peak_kib={}, peak_kib={}",
        load.as_secs_f64(),
        t.elapsed().as_secs_f64(),
        before,
        rss()?
    );
    Ok(())
}
fn rss() -> Result<u64, Box<dyn std::error::Error>> {
    Ok(std::fs::read_to_string("/proc/self/status")?
        .lines()
        .find(|s| s.starts_with("VmHWM:"))
        .ok_or("VmHWM missing")?
        .split_whitespace()
        .nth(1)
        .ok_or("VmHWM value missing")?
        .parse()?)
}
