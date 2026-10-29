//! Local, offline A/B harness. Writes float WAVs under /tmp/ab-wavs/new.
use smart_voices::engine::{KokoroEngine, ModelVariant, PersianVoice, PiperEngine};
use std::{path::PathBuf, time::Instant};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(std::env::args().nth(1).ok_or("usage: tts_ab MODELS_ROOT")?);
    for voice in ["af_heart", "bf_emma", "amir"] {
        let start = Instant::now();
        if voice == "amir" {
            let engine = PiperEngine::load(&root, PersianVoice::Amir)?;
            let load = start.elapsed().as_secs_f64();
            for (i, text) in texts(true).iter().enumerate() {
                let start = Instant::now();
                let x = engine.synthesize(text, 1.)?;
                measure(
                    "new",
                    voice,
                    i,
                    &x,
                    engine.sample_rate(),
                    start.elapsed().as_secs_f64(),
                    load,
                )?;
            }
        } else {
            let engine = KokoroEngine::load(&root, ModelVariant::Fast)?;
            let load = start.elapsed().as_secs_f64();
            for (i, text) in texts(false).iter().enumerate() {
                let start = Instant::now();
                let x = engine.synthesize(text, voice, 1.)?;
                measure(
                    "new",
                    voice,
                    i,
                    &x,
                    engine.sample_rate(),
                    start.elapsed().as_secs_f64(),
                    load,
                )?;
            }
        }
    }
    Ok(())
}
fn texts(persian: bool) -> Vec<&'static str> {
    if persian {
        vec![
            "سلام، حال شما چطور است؟",
            "امروز هوا بسیار خوب است.",
            "این برنامه متن را با صدای بلند می‌خواند.",
            "من هر روز یک کتاب می‌خوانم و به موسیقی گوش می‌دهم.",
        ]
    } else {
        vec![
            "Hello there.",
            "The weather today is nice, so we are going out for a walk.",
            "I read the record yesterday, and I will record it again tomorrow.",
            "Dr. Smith lives in the U.S. and works at the hospital.",
            "The price is $12.50 and the year is 2024.",
            "Kubernetes and Zellij run on Hyprland, and the team shipped it on Friday.",
        ]
    }
}
fn measure(
    kind: &str,
    voice: &str,
    i: usize,
    x: &[f32],
    sr: i32,
    elapsed: f64,
    load: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write;
    let path = format!("/tmp/ab-wavs/{kind}");
    std::fs::create_dir_all(&path)?;
    let mut f = std::fs::File::create(format!("{path}/{voice}-{i}.wav"))?;
    let n = (x.len() * 4) as u32;
    f.write_all(b"RIFF")?;
    f.write_all(&(n + 36).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&3u16.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&(sr as u32).to_le_bytes())?;
    f.write_all(&(sr as u32 * 4).to_le_bytes())?;
    f.write_all(&4u16.to_le_bytes())?;
    f.write_all(&32u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&n.to_le_bytes())?;
    for v in x {
        f.write_all(&v.to_le_bytes())?;
    }
    let lead = x.iter().position(|v| v.abs() > 0.01).unwrap_or(x.len());
    let trail = x
        .iter()
        .rev()
        .position(|v| v.abs() > 0.01)
        .unwrap_or(x.len());
    println!(
        "{kind},{voice},{i},{:.4},{:.4},{:.4},{elapsed:.4},{load:.4}",
        x.len() as f64 / sr as f64,
        lead as f64 / sr as f64,
        trail as f64 / sr as f64
    );
    Ok(())
}
