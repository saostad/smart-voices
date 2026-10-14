use std::path::PathBuf;
use std::time::Instant;

use smart_voices::audio::AudioOut;
use smart_voices::engine::{DEFAULT_VOICE, KokoroEngine, ModelVariant};

const USAGE: &str =
    "Usage: smart-voices speak \"<text>\" [--voice <name>] [--speed <f>] [--model fast|small]";

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("speak") => speak(&args[1..]),
        _ => {
            eprintln!("{USAGE}");
            2
        }
    }
}

struct SpeakOptions {
    text: String,
    voice: String,
    speed: f32,
    model: ModelVariant,
}

fn parse_speak(args: &[String]) -> Option<SpeakOptions> {
    let mut text: Option<String> = None;
    let mut voice = DEFAULT_VOICE.to_string();
    let mut speed = 1.0;
    let mut model = ModelVariant::Fast;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--voice" => {
                i += 1;
                voice = args.get(i)?.to_string();
            }
            "--speed" => {
                i += 1;
                speed = args.get(i)?.parse::<f32>().ok()?;
            }
            "--model" => {
                i += 1;
                model = ModelVariant::parse(args.get(i)?)?;
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

    let started = Instant::now();
    let engine = match KokoroEngine::load(&models_root, options.model) {
        Ok(engine) => engine,
        Err(err) => {
            eprintln!("Error: {err}");
            return 1;
        }
    };
    eprintln!("Loaded voice model in {} ms", started.elapsed().as_millis());

    let started = Instant::now();
    let samples = match engine.synthesize(&options.text, &options.voice, options.speed) {
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
