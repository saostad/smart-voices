use std::path::{Path, PathBuf};
use std::time::Instant;

use smart_voices::audio::AudioOut;
use smart_voices::engine::{KokoroEngine, ModelVariant};
use smart_voices::models::{self, ModelError};
use smart_voices::sanitize::{SanitizerOptions, sanitize};
use smart_voices::settings::{Settings, settings_path};

const USAGE: &str = "Usage: smart-voices speak \"<text>\" [--voice <name>] [--speed <f>] [--model fast|small]\n       smart-voices download [--model fast|small]\n       smart-voices clean [<file>]";

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("speak") => speak(&args[1..]),
        Some("download") => download_cmd(&args[1..]),
        Some("clean") => clean(&args[1..]),
        _ => {
            eprintln!("{USAGE}");
            2
        }
    }
}

struct SpeakOptions {
    text: String,
    voice: Option<String>,
    speed: Option<f32>,
    model: Option<ModelVariant>,
}

fn parse_speak(args: &[String]) -> Option<SpeakOptions> {
    let mut text: Option<String> = None;
    let mut voice: Option<String> = None;
    let mut speed: Option<f32> = None;
    let mut model: Option<ModelVariant> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--voice" => {
                i += 1;
                voice = Some(args.get(i)?.to_string());
            }
            "--speed" => {
                i += 1;
                speed = Some(args.get(i)?.parse::<f32>().ok()?);
            }
            "--model" => {
                i += 1;
                model = Some(ModelVariant::parse(args.get(i)?)?);
            }
            flag if flag.starts_with("--") => return None,
            positional => {
                if text.is_some() {
                    return None;
                }
                text = Some(positional.to_string());
            }
        }
        i += 1;
    }
    Some(SpeakOptions {
        text: text?,
        voice,
        speed,
        model,
    })
}

fn models_root() -> Option<PathBuf> {
    match std::env::var("SMART_VOICES_MODELS") {
        Ok(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => dirs::data_dir().map(|dir| dir.join("smart-voices").join("models")),
    }
}

fn speak(args: &[String]) -> i32 {
    let options = match parse_speak(args) {
        Some(options) => options,
        None => {
            eprintln!("{USAGE}");
            return 2;
        }
    };
    let Some(models_root) = models_root() else {
        eprintln!(
            "Error: Couldn't find the models folder: SMART_VOICES_MODELS is not set and no data directory is available."
        );
        return 1;
    };
    let (settings, settings_err) = settings_path()
        .map(|path| Settings::load(&path))
        .unwrap_or((Settings::default(), None));
    if let Some(err) = &settings_err {
        eprintln!("Warning: {err}");
    }
    let voice = options.voice.unwrap_or(settings.voice);
    let speed = options.speed.unwrap_or(settings.speed);
    let model = options.model.unwrap_or(settings.model);

    if !models::is_installed(&models_root, model)
        && let Err(err) = download_with_progress(&models_root, model)
    {
        eprintln!("Error: {err}");
        return 1;
    }

    let started = Instant::now();
    let engine = match KokoroEngine::load(&models_root, model) {
        Ok(engine) => engine,
        Err(err) => {
            eprintln!("Error: {err}");
            return 1;
        }
    };
    eprintln!("Loaded voice model in {} ms", started.elapsed().as_millis());

    let started = Instant::now();
    let samples = match engine.synthesize(&options.text, &voice, speed) {
        Ok(samples) => samples,
        Err(err) => {
            eprintln!("Error: {err}");
            return 1;
        }
    };
    let synth_elapsed = started.elapsed();
    let rate = engine.sample_rate();
    let seconds = samples.len() as f32 / rate as f32;
    eprintln!(
        "Synthesized {seconds:.2} s of audio in {} ms",
        synth_elapsed.as_millis()
    );
    eprintln!(
        "Real-time factor: {:.2}",
        synth_elapsed.as_secs_f32() / seconds
    );

    let rate = match u32::try_from(rate) {
        Ok(rate) => rate,
        Err(_) => {
            eprintln!("Error: The voice model reported an invalid sample rate: {rate}.");
            return 1;
        }
    };
    let audio = match AudioOut::open() {
        Ok(audio) => audio,
        Err(err) => {
            eprintln!("Error: {err}");
            return 1;
        }
    };
    match audio.play_blocking(&samples, rate) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("Error: {err}");
            1
        }
    }
}

fn parse_download(args: &[String]) -> Option<ModelVariant> {
    let mut model = ModelVariant::Fast;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--model" => {
                i += 1;
                model = ModelVariant::parse(args.get(i)?)?;
            }
            _ => return None,
        }
        i += 1;
    }
    Some(model)
}

fn download_cmd(args: &[String]) -> i32 {
    let Some(model) = parse_download(args) else {
        eprintln!("{USAGE}");
        return 2;
    };
    let Some(models_root) = models_root() else {
        eprintln!(
            "Error: Couldn't find the models folder: SMART_VOICES_MODELS is not set and no data directory is available."
        );
        return 1;
    };
    match download_with_progress(&models_root, model) {
        Ok(dir) => {
            eprintln!("Voice model ready at {}", dir.display());
            0
        }
        Err(err) => {
            eprintln!("Error: {err}");
            1
        }
    }
}

/// Read Markdown from `file` (or stdin) and print one line per
/// sanitized segment as `<pause>\t<text>`.
fn clean(args: &[String]) -> i32 {
    let text = match args.len() {
        0 => match std::io::read_to_string(std::io::stdin()) {
            Ok(text) => text,
            Err(err) => {
                eprintln!("Error: Couldn't read from standard input: {err}.");
                return 1;
            }
        },
        1 => match std::fs::read_to_string(&args[0]) {
            Ok(text) => text,
            Err(err) => {
                eprintln!("Error: Couldn't read {}: {err}.", args[0]);
                return 1;
            }
        },
        _ => {
            eprintln!("{USAGE}");
            return 2;
        }
    };
    for segment in sanitize(&text, &SanitizerOptions::default()) {
        println!("{}\t{}", segment.pause.label(), segment.text);
    }
    0
}

/// Download `variant` unless already installed, with a one-line stderr
/// progress display. Returns the install dir.
fn download_with_progress(root: &Path, variant: ModelVariant) -> Result<PathBuf, ModelError> {
    if models::is_installed(root, variant) {
        return Ok(models::model_dir(root, variant));
    }
    let label = variant.label();
    let result = models::download(root, variant, |done, total| {
        const MB: f64 = 1_000_000.0;
        match total {
            Some(total) => eprint!(
                "\rDownloading {label}: {:.1}/{:.1} MB",
                done as f64 / MB,
                total as f64 / MB
            ),
            None => eprint!("\rDownloading {label}: {:.1} MB", done as f64 / MB),
        }
    });
    eprintln!();
    result
}
