//! ZONOS3-Mini English voice engine (Zyphra, Apache-2.0).
//!
//! The model runs in `zonos3-server`, Zyphra's native ggml runtime, which
//! ships next to the app binary (built by `scripts/build-zonos3.sh`). The
//! engine starts it on a free localhost port the first time the Zonos voice
//! model is used, sends each chunk with the chosen voice's speaker embedding,
//! and stops it when the engine is dropped (idle unload, model switch, quit).
//! The server also exits on its own when its stdin closes, so it never
//! outlives the app, even after a crash.
//!
//! Text reaches the server already normalized by the app (numbers, dates,
//! symbols), and speed is applied by resampling afterwards, like Phonon.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::engine::{DownloadFile, DownloadSpec, EngineError, ModelVariant, clamp_speed};
use crate::pocket::audio::apply_speed;

/// Directory of the model under the models root.
pub const MODEL_DIR: &str = "zonos3-mini";

/// The quantized backbone (Q4_0 experts, Q8_0 mixers).
pub const BACKBONE_FILE: &str = "zonos3-ptq.gguf";

/// The default waveform decoder; the server finds it beside the backbone.
pub const DECODER_FILE: &str = "zac-keep2.gguf";

/// Alignment heads for LACI, the runtime's guard against skipped text and
/// invented audio (fitted for English by Zyphra).
pub const LACI_HEADS_FILE: &str = "laci-heads-en.json";

/// Environment variable naming a `zonos3-server` to use instead of the
/// bundled one (e.g. a CUDA or Vulkan build). The `zonos.server_path`
/// setting does the same and wins over it.
pub const SERVER_ENV: &str = "SMART_VOICES_ZONOS3_SERVER";

/// Default Zonos voice id.
pub const DEFAULT_ZONOS_VOICE: &str = "savannah";

/// File name of the runtime binary.
pub const SERVER_FILE: &str = if cfg!(target_os = "windows") {
    "zonos3-server.exe"
} else {
    "zonos3-server"
};

/// Lines of the server's stderr kept for error messages.
const LOG_TAIL_LINES: usize = 20;

/// Startup line the server prints once it accepts connections.
const LISTENING_PREFIX: &str = "zonos3-server: listening on http://";

/// Pinned Hugging Face commit of `Zyphra/ZONOS3-Mini`; it matches the
/// runtime commit pinned in `scripts/build-zonos3.sh`.
macro_rules! zonos_url {
    ($($part:tt),+) => {
        concat!(
            "https://huggingface.co/Zyphra/ZONOS3-Mini/resolve/24f39592cf5521d675d96db41fc379af3ad369be/gguf/",
            $($part),+
        )
    };
}

/// Menu group of a Zonos voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZonosVoiceGroup {
    American,
    BritishIrish,
    Characters,
}

impl ZonosVoiceGroup {
    /// All groups in menu order.
    pub const ALL: [Self; 3] = [Self::American, Self::BritishIrish, Self::Characters];

    /// Disabled-header label for the `English Voice` submenu.
    pub fn header(self) -> &'static str {
        match self {
            Self::American => "American",
            Self::BritishIrish => "British · Irish",
            Self::Characters => "Characters",
        }
    }
}

/// One English voice shipped with ZONOS3-Mini.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZonosVoice {
    /// Voice id stored in settings (lowercase name).
    pub id: &'static str,
    /// Menu label: the name plus its style.
    pub label: &'static str,
    pub group: ZonosVoiceGroup,
    /// The voice's 192-d speaker embedding inside [`MODEL_DIR`].
    pub file: DownloadFile,
}

macro_rules! zonos_voice {
    ($group:ident, $id:literal, $label:literal, $stem:literal, $sha256:literal) => {
        ZonosVoice {
            id: $id,
            label: $label,
            group: ZonosVoiceGroup::$group,
            file: DownloadFile {
                name: concat!("voices/", $stem, ".embedding.npy"),
                spec: DownloadSpec {
                    url: zonos_url!("voices/English/", $stem, ".embedding.npy"),
                    size: 896,
                    sha256: $sha256,
                },
            },
        }
    };
}

/// The release's English voices, by group in menu order.
pub const ZONOS_VOICES: [ZonosVoice; 10] = [
    zonos_voice!(
        American,
        "savannah",
        "Savannah (female)",
        "Savannah_en-American_F_1",
        "dd81587e66f96fb62da47ef13d5d867de464ca386a83f01b4403cdd19629ca6c"
    ),
    zonos_voice!(
        American,
        "wyatt",
        "Wyatt (male)",
        "Wyatt_en-American_M_1",
        "b39d377b653ebaca6bbd4ecd13c382fe16d81f0cfae27217d9b626e5589a0511"
    ),
    zonos_voice!(
        BritishIrish,
        "imogen",
        "Imogen (British female)",
        "Imogen_en-British_F_1",
        "683424b91faeade48c8a92e05e1c3679366abbba443ef62eccba8dc40de48e9e"
    ),
    zonos_voice!(
        BritishIrish,
        "rupert",
        "Rupert (British male)",
        "Rupert_en-British_M_1",
        "0c68e699376dbafdc1ab0fadc133f48e7df9cb33b3a7b80b05f42c5c0aad8fa7"
    ),
    zonos_voice!(
        BritishIrish,
        "cillian",
        "Cillian (Irish male)",
        "Cillian_en-Irish_M_1",
        "f3125fae2f679fb5c8128d308ddd02986fd7d914975d17cc6dadf9eb130b1061"
    ),
    zonos_voice!(
        Characters,
        "jett",
        "Jett (energetic)",
        "Jett_en-energetic_M_1",
        "624b749d220dac7fdcda479b1964cb8f7de58c838c1357f2f0161dac06e34f48"
    ),
    zonos_voice!(
        Characters,
        "morgana",
        "Morgana (witch)",
        "Morgana_en-witch_F_1",
        "a817387697c43138639aa1e52a9e42d9f3a629655e0faea5ac8565eb8e57c091"
    ),
    zonos_voice!(
        Characters,
        "sakura",
        "Sakura (anime)",
        "Sakura_en-anime_F_1",
        "daf3e82d2e6f2ce2fe90c73473a312a721295ae7f85763f99c87ee2f861063a9"
    ),
    zonos_voice!(
        Characters,
        "thaddeus",
        "Thaddeus (announcer)",
        "Thaddeus_en-announcervoice_M_1",
        "fc8e21356730988aa20da70123991404345e0bdbfe3afe31e6371d8d93095461"
    ),
    zonos_voice!(
        Characters,
        "willow",
        "Willow (ASMR)",
        "Willow_en-asrm_F_1",
        "6ebb84309f21f1dcbe6371e2355dc98f8ee3d0aee5850c305d06a73fe0a7c222"
    ),
];

const BACKBONE: DownloadFile = DownloadFile {
    name: BACKBONE_FILE,
    spec: DownloadSpec {
        url: zonos_url!("zonos3-ptq.gguf"),
        size: 663_150_624,
        sha256: "66fceb6d73a8c30bde80249e4112bb308380ab7f3384fa11c8e4048115e4ceec",
    },
};

const DECODER: DownloadFile = DownloadFile {
    name: DECODER_FILE,
    spec: DownloadSpec {
        url: zonos_url!("zac-keep2.gguf"),
        size: 452_325_440,
        sha256: "834e5b70b88294afc3c702a478487e0664dcc6b574c16fc6167d72444aabeafd",
    },
};

const LACI_HEADS: DownloadFile = DownloadFile {
    name: LACI_HEADS_FILE,
    spec: DownloadSpec {
        // Not on Hugging Face: pinned to the runtime's GitHub commit.
        url: "https://raw.githubusercontent.com/Zyphra/ZONOS3-Mini/b820f6da278862d7c2db3e628d21cbae85983720/assets/laci_heads/zonos3_mini_mect60k_heads.json",
        size: 7534,
        sha256: "5b3145a43681f69b498fa60feb2a4af74081c9e9c49ab4fc11dee7f7f6ea73da",
    },
};

/// Every file of the Zonos download: backbone, decoder, LACI heads, then
/// the voices.
pub const MODEL_FILES: [DownloadFile; 13] = [
    BACKBONE,
    DECODER,
    LACI_HEADS,
    ZONOS_VOICES[0].file,
    ZONOS_VOICES[1].file,
    ZONOS_VOICES[2].file,
    ZONOS_VOICES[3].file,
    ZONOS_VOICES[4].file,
    ZONOS_VOICES[5].file,
    ZONOS_VOICES[6].file,
    ZONOS_VOICES[7].file,
    ZONOS_VOICES[8].file,
    ZONOS_VOICES[9].file,
];

/// [`MODEL_FILES`] names (backbone first), for install checks.
pub const REQUIRED_FILES: [&str; 13] = [
    MODEL_FILES[0].name,
    MODEL_FILES[1].name,
    MODEL_FILES[2].name,
    MODEL_FILES[3].name,
    MODEL_FILES[4].name,
    MODEL_FILES[5].name,
    MODEL_FILES[6].name,
    MODEL_FILES[7].name,
    MODEL_FILES[8].name,
    MODEL_FILES[9].name,
    MODEL_FILES[10].name,
    MODEL_FILES[11].name,
    MODEL_FILES[12].name,
];

/// Look up a voice by id or name, in any case (`"Savannah"`).
pub fn find_voice(query: &str) -> Option<&'static ZonosVoice> {
    ZONOS_VOICES.iter().find(|voice| {
        voice.id.eq_ignore_ascii_case(query)
            || voice
                .label
                .split(' ')
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case(query))
    })
}

/// True for a [`ZONOS_VOICES`] id.
pub fn is_zonos_voice(id: &str) -> bool {
    ZONOS_VOICES.iter().any(|voice| voice.id == id)
}

/// Runtime settings (`[zonos]` table).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ZonosSettings {
    /// Seconds to wait for the runtime to load the model and listen.
    pub startup_timeout_secs: u64,
    /// Seconds to wait for one chunk's audio.
    pub request_timeout_secs: u64,
    /// Run the model on the GPU (Metal on Apple Silicon; a CPU-only build
    /// ignores it).
    pub use_gpu: bool,
    /// A `zonos3-server` to run instead of the bundled one (empty = the
    /// bundled runtime next to the app).
    pub server_path: String,
    /// Generation slots the runtime reserves (`--batch`). One reader needs
    /// one; more cost startup time and CPU.
    pub batch_slots: u32,
    /// Parallel waveform decoders (`--zac-threads`). More than one competes
    /// with generation for the CPU.
    pub decoder_lanes: u32,
    /// Speaking rate asked of the model for long text, in UTF-8 bytes per
    /// second. Without it the model guesses how long a short sentence
    /// lasts and often keeps talking after it.
    pub rate_bytes_per_sec: f32,
    /// Extra seconds each chunk gets on top of its length at that rate, so
    /// one-word chunks aren't rushed or clipped.
    pub chunk_overhead_secs: f32,
    /// Guard each chunk with LACI, which redoes skipped text and invented
    /// audio.
    pub laci: bool,
}

impl Default for ZonosSettings {
    fn default() -> Self {
        Self {
            startup_timeout_secs: 120,
            request_timeout_secs: 600,
            use_gpu: cfg!(target_os = "macos"),
            server_path: String::new(),
            batch_slots: 1,
            decoder_lanes: 1,
            rate_bytes_per_sec: 15.0,
            chunk_overhead_secs: 0.25,
            laci: true,
        }
    }
}

impl ZonosSettings {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=3600).contains(&self.startup_timeout_secs) {
            return Err("zonos.startup_timeout_secs must be between 1 and 3600".into());
        }
        if !(1..=3600).contains(&self.request_timeout_secs) {
            return Err("zonos.request_timeout_secs must be between 1 and 3600".into());
        }
        if !(1..=64).contains(&self.batch_slots) {
            return Err("zonos.batch_slots must be between 1 and 64".into());
        }
        if !(1..=16).contains(&self.decoder_lanes) {
            return Err("zonos.decoder_lanes must be between 1 and 16".into());
        }
        if !self.rate_bytes_per_sec.is_finite() || !(5.0..=30.0).contains(&self.rate_bytes_per_sec)
        {
            return Err("zonos.rate_bytes_per_sec must be between 5 and 30".into());
        }
        if !self.chunk_overhead_secs.is_finite() || !(0.0..=2.0).contains(&self.chunk_overhead_secs)
        {
            return Err("zonos.chunk_overhead_secs must be between 0 and 2".into());
        }
        Ok(())
    }

    /// Speaking rate (UTF-8 bytes per second) to ask for `text`: its length
    /// over the time it takes at [`rate_bytes_per_sec`](Self::rate_bytes_per_sec)
    /// plus [`chunk_overhead_secs`](Self::chunk_overhead_secs). Long text
    /// approaches the base rate; a single word gets time to finish.
    pub fn speaking_rate(&self, text: &str) -> f32 {
        let bytes = text.trim().len().max(1) as f32;
        bytes / (self.chunk_overhead_secs + bytes / self.rate_bytes_per_sec)
    }
}

/// Current Zonos settings: the scoped invocation's, else the settings file.
/// A missing or invalid file falls back to the defaults (logged; the app
/// already warned about the file at startup).
fn current_settings() -> ZonosSettings {
    if let Some(settings) = crate::settings::invocation_settings() {
        return settings.zonos;
    }
    let Some(path) = crate::settings::settings_path() else {
        eprintln!(
            "warn: fallback: true, attempted=settings_file, reason=settings_directory_unavailable, taken=default_zonos_settings"
        );
        return ZonosSettings::default();
    };
    let (settings, error) = crate::settings::Settings::load(&path);
    if let Some(error) = error {
        eprintln!(
            "warn: fallback: true, attempted=settings_file, reason={error}, taken=default_zonos_settings"
        );
    }
    settings.zonos
}

/// Whether this CPU can run the bundled runtime: x86-64 builds need AVX2,
/// FMA and F16C (2013 and newer CPUs); other architectures always can.
pub fn cpu_supported() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        std::arch::is_x86_feature_detected!("avx2")
            && std::arch::is_x86_feature_detected!("fma")
            && std::arch::is_x86_feature_detected!("f16c")
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        true
    }
}

/// The runtime to start: the configured path, else [`SERVER_ENV`], else the
/// bundled one next to the app binary.
fn server_binary(settings: &ZonosSettings) -> Result<PathBuf, EngineError> {
    if !settings.server_path.is_empty() {
        return Ok(PathBuf::from(&settings.server_path));
    }
    if let Ok(path) = std::env::var(SERVER_ENV)
        && !path.is_empty()
    {
        return Ok(PathBuf::from(path));
    }
    let exe = std::env::current_exe().map_err(|err| {
        EngineError::ZonosRuntimeMissing(PathBuf::from(SERVER_FILE), err.to_string())
    })?;
    let dir = exe.parent().unwrap_or_else(|| Path::new("."));
    Ok(dir.join(SERVER_FILE))
}

/// Port from the server's startup line, if `line` is it.
fn listening_port(line: &str) -> Option<u16> {
    let rest = line.trim().strip_prefix(LISTENING_PREFIX)?;
    let address = rest.split_whitespace().next()?;
    address
        .rsplit_once(':')?
        .1
        .parse()
        .ok()
        .filter(|port| *port != 0)
}

/// Little-endian float32 PCM bytes to samples.
fn decode_pcm_f32(bytes: &[u8]) -> Result<Vec<f32>, String> {
    if !bytes.len().is_multiple_of(4) {
        return Err(format!(
            "the audio stream ended mid-sample ({} bytes)",
            bytes.len()
        ));
    }
    Ok(bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect())
}

/// The last lines the server printed, for error messages.
#[derive(Clone, Default)]
struct LogTail(Arc<Mutex<VecDeque<String>>>);

impl LogTail {
    fn push(&self, line: String) {
        let mut lines = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if lines.len() == LOG_TAIL_LINES {
            lines.pop_front();
        }
        lines.push_back(line);
    }

    /// The last non-empty line, or a note that there was none.
    fn last(&self) -> String {
        let lines = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        lines
            .iter()
            .rev()
            .find(|line| !line.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| "it printed nothing".into())
    }
}

/// Loaded Zonos engine: a running `zonos3-server` child process.
pub struct ZonosEngine {
    child: Child,
    /// Held open so the server keeps running; dropping it stops the server.
    _stdin: Option<ChildStdin>,
    url: String,
    agent: ureq::Agent,
    dir: PathBuf,
    settings: ZonosSettings,
    log: LogTail,
    /// Base64 `.npy` speaker embeddings by voice id, read on first use.
    embeddings: HashMap<&'static str, String>,
    sample_rate: i32,
}

impl ZonosEngine {
    /// Start the runtime for the model in `models_root/zonos3-mini`.
    pub fn load(models_root: &Path) -> Result<Self, EngineError> {
        let dir = models_root.join(ModelVariant::Zonos.dir_name());
        for name in ModelVariant::Zonos.required_files() {
            let path = dir.join(name);
            if !path.exists() {
                return Err(EngineError::MissingModel(path));
            }
        }
        let settings = current_settings();
        let server = server_binary(&settings)?;
        if settings.server_path.is_empty() && !cpu_supported() {
            return Err(EngineError::ZonosUnsupportedCpu);
        }
        Self::start(&server, &dir, &settings)
    }

    /// Start `server` on the model in `dir` and wait until it listens.
    fn start(server: &Path, dir: &Path, settings: &ZonosSettings) -> Result<Self, EngineError> {
        if !server.is_file() {
            return Err(EngineError::ZonosRuntimeMissing(
                server.to_path_buf(),
                "the file does not exist".into(),
            ));
        }
        let mut command = Command::new(server);
        command
            .arg(dir.join(BACKBONE_FILE))
            .args(["--host", "127.0.0.1", "--port", "0"])
            .arg("--zac")
            .arg(dir.join(DECODER_FILE))
            .arg(if settings.use_gpu { "--gpu" } else { "--cpu" })
            .arg("--batch")
            .arg(settings.batch_slots.to_string())
            .arg("--zac-threads")
            .arg(settings.decoder_lanes.to_string())
            .env("ZONOS3_EXIT_ON_STDIN_EOF", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if settings.laci {
            let mut heads = std::ffi::OsString::from("en_us=");
            heads.push(dir.join(LACI_HEADS_FILE));
            command.arg("--laci-heads").arg(heads);
        }
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: the tray app must not flash a console.
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().map_err(|err| {
            EngineError::ZonosRuntimeMissing(server.to_path_buf(), err.to_string())
        })?;
        let stdin = child.stdin.take();
        let stderr = child.stderr.take().expect("stderr is piped");

        let log = LogTail::default();
        let (port_tx, port_rx) = mpsc::channel();
        {
            let log = log.clone();
            std::thread::Builder::new()
                .name("zonos3-log".into())
                .spawn(move || {
                    let mut port_tx = Some(port_tx);
                    for line in BufReader::new(stderr).lines() {
                        let Ok(line) = line else { break };
                        if let Some(port) = listening_port(&line)
                            && let Some(tx) = port_tx.take()
                        {
                            let _ = tx.send(port);
                        }
                        log.push(line);
                    }
                    // stderr closed: the server exited; dropping the sender
                    // wakes a startup still waiting for the port.
                })
                .map_err(|err| {
                    EngineError::ZonosFailed(format!("couldn't watch the runtime: {err}"))
                })?;
        }

        let stop = |child: &mut Child| {
            let _ = child.kill();
            let _ = child.wait();
        };
        let port = match port_rx.recv_timeout(Duration::from_secs(settings.startup_timeout_secs)) {
            Ok(port) => port,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                stop(&mut child);
                return Err(EngineError::ZonosStartFailed(format!(
                    "it didn't start within {} seconds (zonos.startup_timeout_secs); last output: {}",
                    settings.startup_timeout_secs,
                    log.last()
                )));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let status = child
                    .wait()
                    .map(|status| status.to_string())
                    .unwrap_or_else(|err| err.to_string());
                return Err(EngineError::ZonosStartFailed(format!(
                    "it exited ({status}): {}",
                    log.last()
                )));
            }
        };
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .timeout_read(Duration::from_secs(settings.request_timeout_secs))
            .build();
        Ok(Self {
            child,
            _stdin: stdin,
            url: format!("http://127.0.0.1:{port}/v2/generate"),
            agent,
            dir: dir.to_path_buf(),
            settings: settings.clone(),
            log,
            embeddings: HashMap::new(),
            sample_rate: 0,
        })
    }

    /// Output sample rate in Hz, as the server reported it (44100 for this
    /// model; 0 before the first chunk).
    pub fn sample_rate(&self) -> i32 {
        self.sample_rate
    }

    fn embedding(&mut self, voice: &'static ZonosVoice) -> Result<String, EngineError> {
        if let Some(encoded) = self.embeddings.get(voice.id) {
            return Ok(encoded.clone());
        }
        let path = self.dir.join(voice.file.name);
        let bytes = std::fs::read(&path).map_err(|err| {
            EngineError::ZonosFailed(format!(
                "couldn't read voice file {}: {err}",
                path.display()
            ))
        })?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        self.embeddings.insert(voice.id, encoded.clone());
        Ok(encoded)
    }

    /// Synthesize `text` in `voice` (a [`ZONOS_VOICES`] id) at `speed`,
    /// returning mono samples at [`sample_rate`](Self::sample_rate).
    pub fn synthesize(
        &mut self,
        text: &str,
        voice: &str,
        speed: f32,
    ) -> Result<Vec<f32>, EngineError> {
        let text = text.replace('\0', "");
        if text.trim().is_empty() {
            return Err(EngineError::NoAudio(text));
        }
        let voice = ZONOS_VOICES
            .iter()
            .find(|v| v.id == voice)
            .ok_or_else(|| EngineError::UnknownZonosVoice(voice.to_string()))?;
        if let Ok(Some(status)) = self.child.try_wait() {
            return Err(EngineError::ZonosFailed(format!(
                "the runtime stopped ({status}): {}",
                self.log.last()
            )));
        }
        let body = serde_json::json!({
            "text": text,
            "stream": true,
            "format": "pcm",
            "language": "en_us",
            "speaker_mode": "embedding",
            "speaker_embedding_base64": self.embedding(voice)?,
            "condition_values": {"speaking_rate": self.settings.speaking_rate(&text)},
            "laci": self.settings.laci,
        });
        let response = match self
            .agent
            .post(&self.url)
            .set("Content-Type", "application/json")
            .send_string(&body.to_string())
        {
            Ok(response) => response,
            Err(ureq::Error::Status(code, response)) => {
                let detail = response.into_string().unwrap_or_default();
                return Err(EngineError::ZonosFailed(format!(
                    "the runtime refused the request ({code}): {}",
                    detail.trim()
                )));
            }
            Err(err) => {
                return Err(EngineError::ZonosFailed(format!(
                    "couldn't reach the runtime: {err}; last output: {}",
                    self.log.last()
                )));
            }
        };
        let rate: i32 = response
            .header("X-Audio-Sample-Rate")
            .and_then(|value| value.parse().ok())
            .filter(|rate| *rate > 0)
            .ok_or_else(|| {
                EngineError::ZonosFailed("the runtime sent no audio sample rate".into())
            })?;
        let mut bytes = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut bytes)
            .map_err(|err| {
                EngineError::ZonosFailed(format!("the audio stream broke off: {err}"))
            })?;
        let pcm = decode_pcm_f32(&bytes).map_err(EngineError::ZonosFailed)?;
        if pcm.is_empty() {
            return Err(EngineError::NoAudio(text));
        }
        self.sample_rate = rate;
        Ok(apply_speed(&pcm, clamp_speed(speed)))
    }
}

impl Drop for ZonosEngine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_temp_dir() -> PathBuf {
        let id = TEST_DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "smart-voices-zonos-test-{}-{id}",
            std::process::id()
        ))
    }

    #[test]
    fn catalog_and_download_files_match() {
        assert_eq!(ModelVariant::Zonos.required_files(), REQUIRED_FILES);
        let names: Vec<&str> = ModelVariant::Zonos
            .download_files()
            .iter()
            .map(|file| file.name)
            .collect();
        assert_eq!(names, REQUIRED_FILES);
        assert_eq!(REQUIRED_FILES[0], BACKBONE_FILE);
        let mut ids = HashSet::new();
        for file in MODEL_FILES {
            assert!(file.spec.size > 0, "{}", file.name);
            assert_eq!(file.spec.sha256.len(), 64, "{}", file.name);
            // Every file is pinned to a commit: the model's on Hugging Face,
            // the LACI heads' on GitHub (the runtime's commit).
            if file.name == LACI_HEADS_FILE {
                assert!(file.spec.url.contains(
                    "/Zyphra/ZONOS3-Mini/b820f6da278862d7c2db3e628d21cbae85983720/assets/laci_heads/"
                ));
            } else {
                assert!(
                    file.spec
                        .url
                        .contains("/resolve/24f39592cf5521d675d96db41fc379af3ad369be/gguf/")
                );
                assert!(
                    file.spec
                        .url
                        .ends_with(file.name.trim_start_matches("voices/"))
                );
            }
        }
        for voice in ZONOS_VOICES {
            assert!(ids.insert(voice.id), "duplicate {}", voice.id);
            assert_eq!(voice.id, voice.id.to_ascii_lowercase());
            assert_eq!(find_voice(voice.id).map(|v| v.id), Some(voice.id));
            assert!(is_zonos_voice(voice.id));
        }
        assert!(is_zonos_voice(DEFAULT_ZONOS_VOICE));
        assert_eq!(find_voice("SAVANNAH").map(|v| v.id), Some("savannah"));
        assert_eq!(find_voice("af_heart"), None);
        for group in ZonosVoiceGroup::ALL {
            assert!(ZONOS_VOICES.iter().any(|voice| voice.group == group));
        }
    }

    #[test]
    fn listening_port_reads_only_the_startup_line() {
        assert_eq!(
            listening_port(
                "zonos3-server: listening on http://127.0.0.1:40123  (studio: not built)"
            ),
            Some(40123)
        );
        assert_eq!(
            listening_port("zonos3-server: listening on http://127.0.0.1:0"),
            None
        );
        assert_eq!(listening_port("zac: decode 1.2s"), None);
        assert_eq!(
            listening_port("zonos3-server: listening on http://127.0.0.1:x"),
            None
        );
    }

    #[test]
    fn pcm_decoding_rejects_a_partial_sample() {
        let mut bytes = Vec::new();
        for sample in [0.5f32, -0.25] {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        assert_eq!(decode_pcm_f32(&bytes).unwrap(), [0.5, -0.25]);
        bytes.push(0);
        assert!(decode_pcm_f32(&bytes).unwrap_err().contains("9 bytes"));
    }

    #[test]
    fn settings_validation_bounds_timeouts() {
        assert!(ZonosSettings::default().validate().is_ok());
        let zero = ZonosSettings {
            startup_timeout_secs: 0,
            ..ZonosSettings::default()
        };
        assert!(
            zero.validate()
                .unwrap_err()
                .contains("startup_timeout_secs")
        );
        let huge = ZonosSettings {
            request_timeout_secs: 3601,
            ..ZonosSettings::default()
        };
        assert!(
            huge.validate()
                .unwrap_err()
                .contains("request_timeout_secs")
        );
        let no_slots = ZonosSettings {
            batch_slots: 0,
            ..ZonosSettings::default()
        };
        assert!(no_slots.validate().unwrap_err().contains("batch_slots"));
        let lanes = ZonosSettings {
            decoder_lanes: 17,
            ..ZonosSettings::default()
        };
        assert!(lanes.validate().unwrap_err().contains("decoder_lanes"));
        let fast = ZonosSettings {
            rate_bytes_per_sec: 31.0,
            ..ZonosSettings::default()
        };
        assert!(fast.validate().unwrap_err().contains("rate_bytes_per_sec"));
        let overhead = ZonosSettings {
            chunk_overhead_secs: f32::NAN,
            ..ZonosSettings::default()
        };
        assert!(
            overhead
                .validate()
                .unwrap_err()
                .contains("chunk_overhead_secs")
        );
    }

    #[test]
    fn speaking_rate_slows_short_text_and_approaches_the_base_rate() {
        let settings = ZonosSettings::default();
        // "Sure!" gets 5 bytes over 0.25 + 5/15 s.
        let word = settings.speaking_rate("Sure!");
        assert!((word - 5.0 / (0.25 + 5.0 / 15.0)).abs() < 1e-4, "{word}");
        let sentence = settings.speaking_rate("Then we went home.");
        let paragraph = settings.speaking_rate(&"Cargo caches compiled crates. ".repeat(10));
        assert!(
            word < sentence && sentence < paragraph,
            "{word} {sentence} {paragraph}"
        );
        assert!(paragraph < 15.0 && paragraph > 14.5, "{paragraph}");
        // Persian letters are two bytes each, so they count double.
        assert!(settings.speaking_rate("سلام") > settings.speaking_rate("hi"));
        // Empty text never divides by zero.
        assert!(settings.speaking_rate("  ").is_finite());
    }

    #[test]
    fn load_on_missing_dir_reports_the_backbone() {
        let dir = unique_temp_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let result = ZonosEngine::load(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        match result {
            Err(EngineError::MissingModel(path)) => {
                assert_eq!(path, dir.join(MODEL_DIR).join(BACKBONE_FILE));
            }
            Ok(_) => panic!("expected MissingModel, got Ok"),
            Err(other) => panic!("expected MissingModel, got {other}"),
        }
    }

    #[test]
    fn start_reports_a_missing_runtime() {
        let dir = unique_temp_dir();
        let missing = dir.join(SERVER_FILE);
        match ZonosEngine::start(&missing, &dir, &ZonosSettings::default()) {
            Err(EngineError::ZonosRuntimeMissing(path, _)) => assert_eq!(path, missing),
            Ok(_) => panic!("expected ZonosRuntimeMissing, got Ok"),
            Err(other) => panic!("expected ZonosRuntimeMissing, got {other}"),
        }
    }

    /// A fake runtime: `/bin/sh` runs the "backbone" file as a script, which
    /// prints what the real server prints and serves canned PCM from a
    /// one-shot local listener started by the test.
    #[cfg(unix)]
    mod fake_runtime {
        use super::*;
        use std::io::Write;
        use std::net::TcpListener;

        fn model_dir_with_script(script: &str) -> PathBuf {
            let dir = unique_temp_dir();
            std::fs::create_dir_all(dir.join("voices")).unwrap();
            std::fs::write(dir.join(BACKBONE_FILE), script).unwrap();
            for voice in ZONOS_VOICES {
                std::fs::write(dir.join(voice.file.name), b"\x93NUMPY fake").unwrap();
            }
            dir
        }

        /// Serve one request with `status`, `headers` and `body`; return the
        /// request text.
        fn serve_once(
            listener: TcpListener,
            status: &'static str,
            headers: &'static str,
            body: Vec<u8>,
        ) -> std::thread::JoinHandle<String> {
            std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut buf = [0u8; 4096];
                // Read headers, then the body by Content-Length.
                loop {
                    let n = stream.read(&mut buf).unwrap();
                    request.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&request).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length: usize = text
                            .lines()
                            .find_map(|l| l.strip_prefix("Content-Length: "))
                            .and_then(|v| v.trim().parse().ok())
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                    if n == 0 {
                        break;
                    }
                }
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(&body).unwrap();
                String::from_utf8_lossy(&request).to_string()
            })
        }

        fn settings() -> ZonosSettings {
            ZonosSettings {
                startup_timeout_secs: 10,
                request_timeout_secs: 10,
                use_gpu: false,
                ..ZonosSettings::default()
            }
        }

        #[test]
        fn synthesizes_through_the_runtime() {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let dir = model_dir_with_script(&format!(
                "echo 'zonos3-server: backbone=x' >&2\necho 'zonos3-server: listening on http://127.0.0.1:{port}  (studio: not built)' >&2\ncat >/dev/null\n"
            ));
            let mut pcm = Vec::new();
            for sample in [0.1f32, 0.2, 0.3, 0.4] {
                pcm.extend_from_slice(&sample.to_le_bytes());
            }
            let server = serve_once(
                listener,
                "200 OK",
                "X-Audio-Sample-Rate: 44100\r\nX-Audio-Format: float32\r\n",
                pcm,
            );
            let mut engine = ZonosEngine::start(Path::new("/bin/sh"), &dir, &settings())
                .expect("fake runtime starts");
            let samples = engine
                .synthesize("Hello there.", "wyatt", 1.0)
                .expect("synthesize");
            assert_eq!(samples, [0.1, 0.2, 0.3, 0.4]);
            assert_eq!(engine.sample_rate(), 44100);
            let request = server.join().unwrap();
            assert!(request.starts_with("POST /v2/generate "), "{request}");
            let body: serde_json::Value =
                serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
            assert_eq!(body["text"], "Hello there.");
            assert_eq!(body["speaker_mode"], "embedding");
            assert_eq!(body["stream"], true);
            assert_eq!(body["laci"], true);
            let rate = body["condition_values"]["speaking_rate"].as_f64().unwrap();
            let want = f64::from(ZonosSettings::default().speaking_rate("Hello there."));
            assert!((rate - want).abs() < 1e-3, "{rate} vs {want}");
            assert_eq!(
                body["speaker_embedding_base64"],
                base64::engine::general_purpose::STANDARD.encode(b"\x93NUMPY fake")
            );
            assert_eq!(
                engine
                    .synthesize("Hi.", "nobody", 1.0)
                    .unwrap_err()
                    .to_string(),
                EngineError::UnknownZonosVoice("nobody".into()).to_string()
            );
            drop(engine);
            std::fs::remove_dir_all(&dir).unwrap();
        }

        #[test]
        fn a_refused_request_keeps_the_runtime_detail() {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let dir = model_dir_with_script(&format!(
                "echo 'zonos3-server: listening on http://127.0.0.1:{port}' >&2\ncat >/dev/null\n"
            ));
            let server = serve_once(
                listener,
                "400 Bad Request",
                "Content-Type: application/json\r\n",
                br#"{"detail":"TTS input len 900 exceeds 800"}"#.to_vec(),
            );
            let mut engine = ZonosEngine::start(Path::new("/bin/sh"), &dir, &settings()).unwrap();
            let message = engine
                .synthesize("Hello.", "savannah", 1.0)
                .unwrap_err()
                .to_string();
            assert!(message.contains("(400)"), "{message}");
            assert!(
                message.contains("TTS input len 900 exceeds 800"),
                "{message}"
            );
            server.join().unwrap();
            drop(engine);
            std::fs::remove_dir_all(&dir).unwrap();
        }

        #[test]
        fn a_runtime_that_exits_reports_its_last_line() {
            let dir = model_dir_with_script(
                "echo 'error: zonos3-ptq.gguf lacks zonos3.text_tokenizer_spec' >&2\nexit 3\n",
            );
            match ZonosEngine::start(Path::new("/bin/sh"), &dir, &settings()) {
                Err(EngineError::ZonosStartFailed(message)) => {
                    assert!(
                        message.contains("lacks zonos3.text_tokenizer_spec"),
                        "{message}"
                    );
                    assert!(message.contains('3'), "{message}");
                }
                Ok(_) => panic!("expected ZonosStartFailed, got Ok"),
                Err(other) => panic!("expected ZonosStartFailed, got {other}"),
            }
            std::fs::remove_dir_all(&dir).unwrap();
        }

        #[test]
        fn a_runtime_that_never_listens_times_out() {
            let dir = model_dir_with_script("echo 'loading' >&2\nsleep 30\n");
            let quick = ZonosSettings {
                startup_timeout_secs: 1,
                ..settings()
            };
            match ZonosEngine::start(Path::new("/bin/sh"), &dir, &quick) {
                Err(EngineError::ZonosStartFailed(message)) => {
                    assert!(message.contains("within 1 seconds"), "{message}");
                    assert!(message.contains("loading"), "{message}");
                }
                Ok(_) => panic!("expected ZonosStartFailed, got Ok"),
                Err(other) => panic!("expected ZonosStartFailed, got {other}"),
            }
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    /// Against the real runtime and model: set `SMART_VOICES_ZONOS_DIR` to a
    /// models root holding `zonos3-mini/`, with `zonos3-server` next to the
    /// test binary's app (or `SMART_VOICES_ZONOS3_SERVER`).
    #[test]
    #[ignore]
    fn synthesize_hello_from_zonos() {
        let root = PathBuf::from(
            std::env::var("SMART_VOICES_ZONOS_DIR")
                .expect("set SMART_VOICES_ZONOS_DIR to a models root holding zonos3-mini/"),
        );
        let server = std::env::var(SERVER_ENV).expect("set SMART_VOICES_ZONOS3_SERVER");
        let settings = ZonosSettings {
            use_gpu: false,
            ..ZonosSettings::default()
        };
        let mut engine = ZonosEngine::start(Path::new(&server), &root.join(MODEL_DIR), &settings)
            .expect("start the runtime");
        let normal = engine
            .synthesize("Hello from Zonos.", DEFAULT_ZONOS_VOICE, 1.0)
            .expect("synthesize at 1x");
        assert_eq!(engine.sample_rate(), 44100);
        let fast = engine
            .synthesize("Hello from Zonos.", "wyatt", 2.0)
            .expect("synthesize at 2x");
        assert!(normal.len() > 44100 / 2, "{} samples", normal.len());
        assert!(fast.len() < normal.len());
    }
}
