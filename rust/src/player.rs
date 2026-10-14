//! Playback controller: gapless lookahead playback with transport controls and
//! idle model unload. The tray app drives this; the `speak` CLI wraps it once.
//!
//! A controller thread owns the voice engine and the audio output. It
//! synthesizes at most 2 chunks ahead of the one playing, checks the command
//! channel between chunks (so `stop`/`play` take effect within one chunk's
//! synth time), and idles in `recv_timeout` (never a polling loop): the stream
//! is dropped ~2 s after the last item finished, and the engine is dropped
//! after `unload_after` of idleness.

use std::collections::VecDeque;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::audio::{AudioSink, SinkEvent};
use crate::chunk::SpeechChunk;
use crate::engine::{EngineError, KokoroEngine};
use crate::pipeline::prepare;
use crate::pronounce::Pronunciations;
use crate::sanitize::SanitizerOptions;

/// How many chunks may be synthesized ahead of the one playing.
const LOOKAHEAD_CHUNKS: usize = 2;

/// Total items in flight (the one playing plus lookahead).
const MAX_INFLIGHT: usize = LOOKAHEAD_CHUNKS + 1;

/// Idle time after the last item finished before the stream is dropped.
const STREAM_CLOSE_AFTER: Duration = Duration::from_secs(2);

/// Voice engine interface (the real [`KokoroEngine`] or a test fake).
pub trait Synth: Send {
    fn sample_rate(&self) -> u32;
    fn synthesize(&mut self, text: &str, voice: &str, speed: f32) -> Result<Vec<f32>, String>;
}

impl Synth for KokoroEngine {
    fn sample_rate(&self) -> u32 {
        u32::try_from(KokoroEngine::sample_rate(self)).unwrap_or(24_000)
    }

    fn synthesize(&mut self, text: &str, voice: &str, speed: f32) -> Result<Vec<f32>, String> {
        KokoroEngine::synthesize(self, text, voice, speed).map_err(|err| err.to_string())
    }
}

/// Audio output interface (the real [`AudioSink`] or a test fake).
pub trait Output: Send {
    fn enqueue(&mut self, id: u64, samples: &[f32], sample_rate: u32) -> Result<(), String>;
    fn pause(&mut self);
    fn resume(&mut self);
    fn clear(&mut self);
    fn close_if_idle(&mut self);
    /// Wire the channel receiving [`SinkEvent`]s (called once by spawn).
    fn set_event_sender(&mut self, sender: mpsc::Sender<SinkEvent>);
}

impl Output for AudioSink {
    fn enqueue(&mut self, id: u64, samples: &[f32], sample_rate: u32) -> Result<(), String> {
        AudioSink::enqueue(self, id, samples, sample_rate).map_err(|err| err.to_string())
    }

    fn pause(&mut self) {
        AudioSink::pause(self);
    }

    fn resume(&mut self) {
        AudioSink::resume(self);
    }

    fn clear(&mut self) {
        AudioSink::clear(self);
    }

    fn close_if_idle(&mut self) {
        AudioSink::close_if_idle(self);
    }

    fn set_event_sender(&mut self, sender: mpsc::Sender<SinkEvent>) {
        AudioSink::set_event_sender(self, sender);
    }
}

/// Player configuration (`voice`/`speed` can be changed later; they apply from
/// the next synthesized chunk).
#[derive(Debug, Clone)]
pub struct PlayerConfig {
    pub voice: String,
    pub speed: f32,
    /// Idle time after which the engine is dropped (`None` = keep loaded).
    pub unload_after: Option<Duration>,
    pub sanitizer_options: SanitizerOptions,
    pub pronunciations: Pronunciations,
}

/// Playback state (see [`PlayerEvent::State`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerState {
    Idle,
    Loading,
    Playing,
    Paused,
}

/// Events emitted by the player.
#[derive(Debug, Clone, PartialEq)]
pub enum PlayerEvent {
    State(PlayerState),
    /// A chunk started playing (`index` of `count`, display text).
    Chunk {
        index: usize,
        count: usize,
        text: String,
    },
    Finished,
    /// Always the full message (engine load errors verbatim).
    Error(String),
}

/// Commands sent to the controller thread.
enum Command {
    Play(String),
    Pause,
    Resume,
    Toggle,
    Stop,
    Next,
    Previous,
    SetVoice(String),
    SetSpeed(f32),
    SetUnloadAfter(Option<Duration>),
    Shutdown,
    Sink(SinkEvent),
}

/// Playback handle with cheap `&self` methods, usable from any thread.
pub struct Player {
    commands: mpsc::Sender<Command>,
}

impl Player {
    /// Spawn the controller thread with a lazily-loaded engine (`factory` is
    /// called on the first `play` and after every idle unload) and the given
    /// output. Returns the handle plus the event receiver.
    pub fn spawn(
        config: PlayerConfig,
        factory: Box<dyn FnMut() -> Result<Box<dyn Synth>, String> + Send>,
        output: impl Output + 'static,
    ) -> (Self, mpsc::Receiver<PlayerEvent>) {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let (sink_tx, sink_rx) = mpsc::channel();
        let mut output = output;
        output.set_event_sender(sink_tx);
        // Merge sink events into the command channel so the controller blocks
        // on a single channel (`recv_timeout`, never polling). The forwarder
        // idles in `recv` (zero idle CPU) and exits with the controller.
        let forward_tx = command_tx.clone();
        std::thread::spawn(move || {
            for event in sink_rx {
                if forward_tx.send(Command::Sink(event)).is_err() {
                    break;
                }
            }
        });
        let controller = Controller::new(config, factory, output, command_rx, event_tx);
        std::thread::spawn(move || controller.run());
        (
            Self {
                commands: command_tx,
            },
            event_rx,
        )
    }

    fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    /// Interrupt anything playing and read `text`.
    pub fn play(&self, text: impl Into<String>) {
        self.send(Command::Play(text.into()));
    }

    pub fn pause(&self) {
        self.send(Command::Pause);
    }

    pub fn resume(&self) {
        self.send(Command::Resume);
    }

    pub fn toggle(&self) {
        self.send(Command::Toggle);
    }

    pub fn stop(&self) {
        self.send(Command::Stop);
    }

    /// Jump to the next chunk (finishes playback at the last chunk).
    pub fn next(&self) {
        self.send(Command::Next);
    }

    /// Jump to the previous chunk (restarts chunk 0 at the start).
    pub fn previous(&self) {
        self.send(Command::Previous);
    }

    pub fn set_voice(&self, voice: impl Into<String>) {
        self.send(Command::SetVoice(voice.into()));
    }

    pub fn set_speed(&self, speed: f32) {
        self.send(Command::SetSpeed(speed));
    }

    pub fn set_unload_after(&self, after: Option<Duration>) {
        self.send(Command::SetUnloadAfter(after));
    }

    pub fn shutdown(&self) {
        self.send(Command::Shutdown);
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
    }
}

/// Item synthesized and handed to the output, awaiting `ItemFinished`.
struct Inflight {
    id: u64,
    index: usize,
}

struct Controller<O: Output> {
    voice: String,
    speed: f32,
    unload_after: Option<Duration>,
    sanitizer_options: SanitizerOptions,
    pronunciations: Pronunciations,
    factory: Box<dyn FnMut() -> Result<Box<dyn Synth>, String> + Send>,
    output: O,
    engine: Option<Box<dyn Synth>>,
    commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<PlayerEvent>,
    state: PlayerState,
    /// Bumped on every `play`/`stop`/jump; results tagged with an older
    /// generation are stale and discarded.
    generation: u64,
    chunks: Vec<SpeechChunk>,
    /// Next chunk index to synthesize.
    synth_next: usize,
    /// Index playback continues from (last started chunk, or jump target).
    cursor: usize,
    inflight: VecDeque<Inflight>,
    next_id: u64,
    /// Result synthesized but not yet enqueued: (generation, chunk index,
    /// samples with trailing pause silence, sample rate).
    pending_synth: Option<(u64, usize, Vec<f32>, u32)>,
    /// Whether the output is paused (independent of `state`: stop-while-paused
    /// stays paused until the next `play` resumes it).
    output_paused: bool,
    /// When the player entered idle (schedules stream close + engine unload).
    idle_since: Option<Instant>,
    stream_closed: bool,
}

impl<O: Output> Controller<O> {
    fn new(
        config: PlayerConfig,
        factory: Box<dyn FnMut() -> Result<Box<dyn Synth>, String> + Send>,
        output: O,
        commands: mpsc::Receiver<Command>,
        events: mpsc::Sender<PlayerEvent>,
    ) -> Self {
        Self {
            voice: config.voice,
            speed: config.speed,
            unload_after: config.unload_after,
            sanitizer_options: config.sanitizer_options,
            pronunciations: config.pronunciations,
            factory,
            output,
            engine: None,
            commands,
            events,
            state: PlayerState::Idle,
            generation: 0,
            chunks: Vec::new(),
            synth_next: 0,
            cursor: 0,
            inflight: VecDeque::new(),
            next_id: 0,
            pending_synth: None,
            output_paused: false,
            idle_since: None,
            stream_closed: true,
        }
    }

    fn run(mut self) {
        loop {
            // Drain every pending command first so stop/play take effect
            // within one chunk's synth time.
            while let Ok(command) = self.commands.try_recv() {
                if matches!(command, Command::Shutdown) {
                    return;
                }
                self.handle(command);
            }
            // Enqueue the previous synth result unless a command superseded it.
            self.flush_pending();
            // Synthesize one chunk when playing and lookahead allows.
            if self.wants_synth() {
                self.synthesize_next();
                continue;
            }
            // Nothing to do: block until the next command or idle deadline.
            match self.idle_timeout() {
                Some(timeout) => match self.commands.recv_timeout(timeout) {
                    Ok(Command::Shutdown) => return,
                    Ok(command) => self.handle(command),
                    Err(mpsc::RecvTimeoutError::Timeout) => self.fire_idle_deadlines(),
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                },
                None => match self.commands.recv() {
                    Ok(Command::Shutdown) | Err(_) => return,
                    Ok(command) => self.handle(command),
                },
            }
        }
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Play(text) => self.play(text),
            Command::Pause => {
                if self.state == PlayerState::Playing {
                    self.output.pause();
                    self.output_paused = true;
                    self.set_state(PlayerState::Paused);
                }
            }
            Command::Resume => {
                if self.state == PlayerState::Paused {
                    self.output.resume();
                    self.output_paused = false;
                    self.set_state(PlayerState::Playing);
                }
            }
            Command::Toggle => match self.state {
                PlayerState::Playing => {
                    self.output.pause();
                    self.output_paused = true;
                    self.set_state(PlayerState::Paused);
                }
                PlayerState::Paused => {
                    self.output.resume();
                    self.output_paused = false;
                    self.set_state(PlayerState::Playing);
                }
                PlayerState::Idle | PlayerState::Loading => {}
            },
            Command::Stop => self.stop(),
            Command::Next => self.next(),
            Command::Previous => self.previous(),
            Command::SetVoice(voice) => self.voice = voice,
            Command::SetSpeed(speed) => self.speed = speed,
            Command::SetUnloadAfter(after) => self.unload_after = after,
            Command::Shutdown => {}
            Command::Sink(SinkEvent::ItemFinished(id)) => self.item_finished(id),
            Command::Sink(SinkEvent::Error(message)) => self.fail(message),
        }
    }

    /// Interrupt anything playing and read `text`.
    fn play(&mut self, text: String) {
        self.reset_playlist();
        self.chunks = prepare(&text, &self.sanitizer_options, &self.pronunciations);
        self.cursor = 0;
        self.idle_since = None;
        if self.chunks.is_empty() {
            // Same message as a direct synth call on empty input.
            self.fail(EngineError::NoAudio(text).to_string());
            return;
        }
        if self.engine.is_none() {
            self.set_state(PlayerState::Loading);
            match (self.factory)() {
                Ok(engine) => self.engine = Some(engine),
                Err(message) => {
                    self.fail(message);
                    return;
                }
            }
        }
        if self.output_paused {
            self.output.resume();
            self.output_paused = false;
        }
        self.set_state(PlayerState::Playing);
    }

    fn stop(&mut self) {
        self.reset_playlist();
        self.set_state(PlayerState::Idle);
        self.note_idle();
    }

    /// Discard the playlist and queued audio; stale synth results and finish
    /// events are dropped by the generation check.
    fn reset_playlist(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.output.clear();
        self.inflight.clear();
        self.pending_synth = None;
        self.chunks.clear();
        self.synth_next = 0;
    }

    fn next(&mut self) {
        if self.chunks.is_empty()
            || !matches!(self.state, PlayerState::Playing | PlayerState::Paused)
        {
            return;
        }
        if self.cursor + 1 >= self.chunks.len() {
            self.finish_playback();
            return;
        }
        self.jump_to(self.cursor + 1);
    }

    fn previous(&mut self) {
        if self.chunks.is_empty()
            || !matches!(self.state, PlayerState::Playing | PlayerState::Paused)
        {
            return;
        }
        self.jump_to(self.cursor.saturating_sub(1));
    }

    fn jump_to(&mut self, target: usize) {
        // Like `reset_playlist` but keeps the playlist: the jump target
        // refers to the current chunks.
        self.generation = self.generation.wrapping_add(1);
        self.output.clear();
        self.inflight.clear();
        self.pending_synth = None;
        self.cursor = target;
        self.synth_next = target;
    }

    /// Natural end of playback (or `next` at the last chunk).
    fn finish_playback(&mut self) {
        self.reset_playlist();
        self.emit(PlayerEvent::Finished);
        self.set_state(PlayerState::Idle);
        self.note_idle();
    }

    /// Stop playback with an error (synth, enqueue, engine-load or sink error).
    fn fail(&mut self, message: String) {
        self.reset_playlist();
        self.emit(PlayerEvent::Error(message));
        self.set_state(PlayerState::Idle);
        self.note_idle();
    }

    fn note_idle(&mut self) {
        self.idle_since = Some(Instant::now());
        self.stream_closed = false;
    }

    fn item_finished(&mut self, id: u64) {
        let Some(pos) = self.inflight.iter().position(|item| item.id == id) else {
            return; // Stale: cleared by play/stop/jump.
        };
        let was_head = pos == 0;
        self.inflight.remove(pos);
        if was_head && let Some(head) = self.inflight.front() {
            // The next item starts playing now.
            let index = head.index;
            self.cursor = index;
            if let Some(chunk) = self.chunks.get(index) {
                let text = chunk.text.clone();
                let count = self.chunks.len();
                self.emit(PlayerEvent::Chunk { index, count, text });
            }
        }
        if self.inflight.is_empty() && self.synth_next >= self.chunks.len() {
            self.finish_playback();
        }
    }

    fn wants_synth(&self) -> bool {
        matches!(self.state, PlayerState::Playing | PlayerState::Paused)
            && self.pending_synth.is_none()
            && self.synth_next < self.chunks.len()
            && self.inflight.len() < MAX_INFLIGHT
    }

    /// Synthesize one chunk (the slow step; commands are re-checked after).
    fn synthesize_next(&mut self) {
        let index = self.synth_next;
        let Some(chunk) = self.chunks.get(index).cloned() else {
            return;
        };
        if self.engine.is_none() {
            // Unreachable in practice (the engine is only dropped while idle),
            // but reload rather than stall if the invariant ever breaks.
            let resume_state = self.state;
            self.set_state(PlayerState::Loading);
            match (self.factory)() {
                Ok(engine) => {
                    self.engine = Some(engine);
                    self.set_state(resume_state);
                }
                Err(message) => {
                    self.fail(message);
                    return;
                }
            }
        }
        let spoken = self.pronunciations.apply(&chunk.text);
        let voice = self.voice.clone();
        let speed = self.speed;
        if let Some(engine) = self.engine.as_mut() {
            match engine.synthesize(&spoken, &voice, speed) {
                Ok(mut samples) => {
                    let rate = engine.sample_rate();
                    let silence = (chunk.pause_after.seconds() * f64::from(rate)) as usize;
                    samples.resize(samples.len() + silence, 0.0);
                    self.pending_synth = Some((self.generation, index, samples, rate));
                }
                Err(message) => self.fail(message),
            }
        }
    }

    /// Enqueue the pending synth result unless a command superseded it.
    fn flush_pending(&mut self) {
        let Some((generation, index, samples, rate)) = self.pending_synth.take() else {
            return;
        };
        if generation != self.generation
            || !matches!(self.state, PlayerState::Playing | PlayerState::Paused)
            || index != self.synth_next
        {
            return; // Stale: a play/stop/jump superseded it.
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        match self.output.enqueue(id, &samples, rate) {
            Ok(()) => {
                let was_empty = self.inflight.is_empty();
                self.inflight.push_back(Inflight { id, index });
                self.synth_next = index + 1;
                if was_empty && let Some(chunk) = self.chunks.get(index) {
                    // Became the head: it starts playing now.
                    self.cursor = index;
                    let text = chunk.text.clone();
                    let count = self.chunks.len();
                    self.emit(PlayerEvent::Chunk { index, count, text });
                }
            }
            Err(message) => self.fail(message),
        }
    }

    /// How long the idle wait may block: none when busy or fully settled,
    /// otherwise until the next stream-close/unload deadline.
    fn idle_timeout(&self) -> Option<Duration> {
        if self.state != PlayerState::Idle {
            return None;
        }
        let since = self.idle_since?;
        let mut timeout: Option<Duration> = None;
        if !self.stream_closed {
            timeout = Some(STREAM_CLOSE_AFTER.saturating_sub(since.elapsed()));
        }
        if self.engine.is_some()
            && let Some(after) = self.unload_after
        {
            let unload_in = after.saturating_sub(since.elapsed());
            timeout = Some(timeout.map_or(unload_in, |current| current.min(unload_in)));
        }
        timeout
    }

    /// Fire due idle deadlines (stream close, engine unload); emits nothing.
    fn fire_idle_deadlines(&mut self) {
        if self.state != PlayerState::Idle {
            return;
        }
        let Some(since) = self.idle_since else {
            return;
        };
        if !self.stream_closed && since.elapsed() >= STREAM_CLOSE_AFTER {
            self.output.close_if_idle();
            self.stream_closed = true;
        }
        if self.engine.is_some()
            && let Some(after) = self.unload_after
            && since.elapsed() >= after
        {
            self.engine = None;
        }
    }

    /// Emit a state event only on change.
    fn set_state(&mut self, state: PlayerState) {
        if self.state != state {
            self.state = state;
            self.emit(PlayerEvent::State(state));
        }
    }

    fn emit(&self, event: PlayerEvent) {
        let _ = self.events.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    const EVENT_TIMEOUT: Duration = Duration::from_secs(5);

    struct FakeState {
        enqueued: Vec<(u64, usize, u32)>,
        pauses: usize,
        resumes: usize,
        clears: usize,
        closes: usize,
        sender: Option<mpsc::Sender<SinkEvent>>,
    }

    #[derive(Clone)]
    struct FakeOutput {
        shared: Arc<Mutex<FakeState>>,
    }

    impl FakeOutput {
        fn new() -> (Self, Arc<Mutex<FakeState>>) {
            let shared = Arc::new(Mutex::new(FakeState {
                enqueued: Vec::new(),
                pauses: 0,
                resumes: 0,
                clears: 0,
                closes: 0,
                sender: None,
            }));
            (
                Self {
                    shared: Arc::clone(&shared),
                },
                shared,
            )
        }

        fn finish(shared: &Arc<Mutex<FakeState>>, id: u64) {
            shared
                .lock()
                .unwrap()
                .sender
                .as_ref()
                .unwrap()
                .send(SinkEvent::ItemFinished(id))
                .unwrap();
        }

        fn enqueued_ids(shared: &Arc<Mutex<FakeState>>) -> Vec<u64> {
            shared
                .lock()
                .unwrap()
                .enqueued
                .iter()
                .map(|item| item.0)
                .collect()
        }
    }

    impl Output for FakeOutput {
        fn enqueue(&mut self, id: u64, samples: &[f32], sample_rate: u32) -> Result<(), String> {
            self.shared
                .lock()
                .unwrap()
                .enqueued
                .push((id, samples.len(), sample_rate));
            Ok(())
        }

        fn pause(&mut self) {
            self.shared.lock().unwrap().pauses += 1;
        }

        fn resume(&mut self) {
            self.shared.lock().unwrap().resumes += 1;
        }

        fn clear(&mut self) {
            self.shared.lock().unwrap().clears += 1;
        }

        fn close_if_idle(&mut self) {
            self.shared.lock().unwrap().closes += 1;
        }

        fn set_event_sender(&mut self, sender: mpsc::Sender<SinkEvent>) {
            self.shared.lock().unwrap().sender = Some(sender);
        }
    }

    #[derive(Clone)]
    struct FakeSynth {
        rate: u32,
        delay: Duration,
        fail_on: Option<String>,
        calls: Arc<Mutex<Vec<(String, String, f32)>>>,
    }

    impl FakeSynth {
        fn new() -> Self {
            Self {
                rate: 24_000,
                delay: Duration::ZERO,
                fail_on: None,
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl Synth for FakeSynth {
        fn sample_rate(&self) -> u32 {
            self.rate
        }

        fn synthesize(&mut self, text: &str, voice: &str, speed: f32) -> Result<Vec<f32>, String> {
            self.calls
                .lock()
                .unwrap()
                .push((text.to_string(), voice.to_string(), speed));
            if let Some(fail) = &self.fail_on
                && text.contains(fail)
            {
                return Err(format!("synth failed for: {text}"));
            }
            if !self.delay.is_zero() {
                std::thread::sleep(self.delay);
            }
            Ok(vec![0.5; text.len()])
        }
    }

    fn test_config(unload_after: Option<Duration>) -> PlayerConfig {
        PlayerConfig {
            voice: "test-voice".to_string(),
            speed: 1.0,
            unload_after,
            sanitizer_options: SanitizerOptions::default(),
            pronunciations: Pronunciations::new(Vec::new()),
        }
    }

    fn counting_factory(
        synth: FakeSynth,
        count: Arc<AtomicUsize>,
    ) -> Box<dyn FnMut() -> Result<Box<dyn Synth>, String> + Send> {
        Box::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(synth.clone()) as Box<dyn Synth>)
        })
    }

    /// Spawn a player with fakes; returns (player, events, output state, factory count).
    fn spawn_player(
        unload_after: Option<Duration>,
        synth: FakeSynth,
    ) -> (
        Player,
        mpsc::Receiver<PlayerEvent>,
        Arc<Mutex<FakeState>>,
        Arc<AtomicUsize>,
        FakeSynth,
    ) {
        let (output, shared) = FakeOutput::new();
        let count = Arc::new(AtomicUsize::new(0));
        let (player, events) = Player::spawn(
            test_config(unload_after),
            counting_factory(synth.clone(), Arc::clone(&count)),
            output,
        );
        (player, events, shared, count, synth)
    }

    fn next_event(events: &mpsc::Receiver<PlayerEvent>) -> PlayerEvent {
        events
            .recv_timeout(EVENT_TIMEOUT)
            .expect("timed out waiting for player event")
    }

    fn expect_no_event(events: &mpsc::Receiver<PlayerEvent>) {
        match events.recv_timeout(Duration::from_millis(200)) {
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => panic!("player event channel closed"),
            Ok(event) => panic!("expected no event, got {event:?}"),
        }
    }

    fn wait_for_enqueued(shared: &Arc<Mutex<FakeState>>, count: usize) {
        let start = Instant::now();
        while shared.lock().unwrap().enqueued.len() < count {
            assert!(
                start.elapsed() < EVENT_TIMEOUT,
                "timed out waiting for {count} enqueues"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn wait_for_calls(synth: &FakeSynth, count: usize) {
        let start = Instant::now();
        while synth.calls.lock().unwrap().len() < count {
            assert!(
                start.elapsed() < EVENT_TIMEOUT,
                "timed out waiting for {count} synth calls"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn wait_for_closes(shared: &Arc<Mutex<FakeState>>) {
        let start = Instant::now();
        while shared.lock().unwrap().closes < 1 {
            assert!(
                start.elapsed() < EVENT_TIMEOUT,
                "timed out waiting for close_if_idle"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn chunk(index: usize, count: usize, text: &str) -> PlayerEvent {
        PlayerEvent::Chunk {
            index,
            count,
            text: text.to_string(),
        }
    }

    #[test]
    fn three_chunk_play_event_order() {
        let (player, events, shared, factory_count, _) = spawn_player(None, FakeSynth::new());
        player.play("One. Two. Three.");
        assert_eq!(
            next_event(&events),
            PlayerEvent::State(PlayerState::Loading)
        );
        assert_eq!(
            next_event(&events),
            PlayerEvent::State(PlayerState::Playing)
        );
        assert_eq!(next_event(&events), chunk(0, 3, "One."));
        wait_for_enqueued(&shared, 3);
        // Enqueued samples are the synth output plus trailing pause silence.
        let chunks = prepare(
            "One. Two. Three.",
            &SanitizerOptions::default(),
            &Pronunciations::new(Vec::new()),
        );
        assert_eq!(chunks.len(), 3);
        {
            let enqueued = &shared.lock().unwrap().enqueued;
            for (i, entry) in enqueued.iter().enumerate() {
                let expected =
                    chunks[i].text.len() + (chunks[i].pause_after.seconds() * 24_000.0) as usize;
                assert_eq!(entry.1, expected, "chunk {i}");
                assert_eq!(entry.2, 24_000, "chunk {i}");
            }
        }
        let ids = FakeOutput::enqueued_ids(&shared);
        FakeOutput::finish(&shared, ids[0]);
        assert_eq!(next_event(&events), chunk(1, 3, "Two."));
        FakeOutput::finish(&shared, ids[1]);
        assert_eq!(next_event(&events), chunk(2, 3, "Three."));
        FakeOutput::finish(&shared, ids[2]);
        assert_eq!(next_event(&events), PlayerEvent::Finished);
        assert_eq!(next_event(&events), PlayerEvent::State(PlayerState::Idle));
        assert_eq!(factory_count.load(Ordering::SeqCst), 1);
        player.shutdown();
    }

    #[test]
    fn lookahead_never_exceeds_two() {
        let (player, events, shared, _, _) = spawn_player(None, FakeSynth::new());
        player.play("One. Two. Three. Four. Five. Six.");
        assert_eq!(
            next_event(&events),
            PlayerEvent::State(PlayerState::Loading)
        );
        assert_eq!(
            next_event(&events),
            PlayerEvent::State(PlayerState::Playing)
        );
        assert_eq!(next_event(&events), chunk(0, 6, "One."));
        // Without any finish, only head + 2 ahead are enqueued, then it stops.
        wait_for_enqueued(&shared, 3);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(shared.lock().unwrap().enqueued.len(), 3);
        // Each finish lets exactly one more chunk through; in flight (enqueued
        // minus finished) never exceeds head + 2.
        for finished in 0..6 {
            let ids = FakeOutput::enqueued_ids(&shared);
            FakeOutput::finish(&shared, ids[finished]);
            let total = 3 + (finished + 1).min(3);
            wait_for_enqueued(&shared, total);
            std::thread::sleep(Duration::from_millis(50));
            let enqueued = shared.lock().unwrap().enqueued.len();
            assert_eq!(enqueued, total);
            assert!(enqueued - (finished + 1) <= MAX_INFLIGHT);
        }
        for (index, text) in ["Two.", "Three.", "Four.", "Five.", "Six."]
            .iter()
            .enumerate()
        {
            assert_eq!(next_event(&events), chunk(index + 1, 6, text));
        }
        assert_eq!(next_event(&events), PlayerEvent::Finished);
        assert_eq!(next_event(&events), PlayerEvent::State(PlayerState::Idle));
        player.shutdown();
    }

    #[test]
    fn stop_mid_play_clears_output_and_idles() {
        let (player, events, shared, _, _) = spawn_player(None, FakeSynth::new());
        player.play("One. Two. Three.");
        assert_eq!(
            next_event(&events),
            PlayerEvent::State(PlayerState::Loading)
        );
        assert_eq!(
            next_event(&events),
            PlayerEvent::State(PlayerState::Playing)
        );
        assert_eq!(next_event(&events), chunk(0, 3, "One."));
        wait_for_enqueued(&shared, 3);
        let ids = FakeOutput::enqueued_ids(&shared);
        player.stop();
        assert_eq!(next_event(&events), PlayerEvent::State(PlayerState::Idle));
        assert_eq!(shared.lock().unwrap().clears, 2); // play + stop
        // Stale finishes produce nothing.
        for id in ids {
            FakeOutput::finish(&shared, id);
        }
        expect_no_event(&events);
        player.shutdown();
    }

    #[test]
    fn new_play_discards_stale_chunks() {
        let (player, events, shared, factory_count, _) = spawn_player(None, FakeSynth::new());
        player.play("One. Two. Three.");
        assert_eq!(
            next_event(&events),
            PlayerEvent::State(PlayerState::Loading)
        );
        assert_eq!(
            next_event(&events),
            PlayerEvent::State(PlayerState::Playing)
        );
        assert_eq!(next_event(&events), chunk(0, 3, "One."));
        wait_for_enqueued(&shared, 3);
        let old_ids = FakeOutput::enqueued_ids(&shared);
        player.play("Alpha. Beta.");
        // Still playing: no state event, straight to the new first chunk.
        assert_eq!(next_event(&events), chunk(0, 2, "Alpha."));
        wait_for_enqueued(&shared, 5);
        let new_ids: Vec<u64> = FakeOutput::enqueued_ids(&shared)[3..].to_vec();
        // Finishing the old items emits no old-text Chunk events.
        for id in old_ids {
            FakeOutput::finish(&shared, id);
        }
        expect_no_event(&events);
        FakeOutput::finish(&shared, new_ids[0]);
        assert_eq!(next_event(&events), chunk(1, 2, "Beta."));
        FakeOutput::finish(&shared, new_ids[1]);
        assert_eq!(next_event(&events), PlayerEvent::Finished);
        assert_eq!(next_event(&events), PlayerEvent::State(PlayerState::Idle));
        assert_eq!(factory_count.load(Ordering::SeqCst), 1); // engine kept
        player.shutdown();
    }

    #[test]
    fn next_and_previous_jump() {
        let (player, events, shared, _, _) = spawn_player(None, FakeSynth::new());
        player.play("One. Two. Three.");
        assert_eq!(
            next_event(&events),
            PlayerEvent::State(PlayerState::Loading)
        );
        assert_eq!(
            next_event(&events),
            PlayerEvent::State(PlayerState::Playing)
        );
        assert_eq!(next_event(&events), chunk(0, 3, "One."));
        wait_for_enqueued(&shared, 3);
        player.next();
        assert_eq!(next_event(&events), chunk(1, 3, "Two."));
        wait_for_enqueued(&shared, 5);
        player.previous();
        assert_eq!(next_event(&events), chunk(0, 3, "One."));
        wait_for_enqueued(&shared, 8);
        player.previous(); // At 0: restarts chunk 0.
        assert_eq!(next_event(&events), chunk(0, 3, "One."));
        wait_for_enqueued(&shared, 11);
        let ids = FakeOutput::enqueued_ids(&shared);
        for id in &ids[8..] {
            FakeOutput::finish(&shared, *id);
        }
        assert_eq!(next_event(&events), chunk(1, 3, "Two."));
        assert_eq!(next_event(&events), chunk(2, 3, "Three."));
        assert_eq!(next_event(&events), PlayerEvent::Finished);
        assert_eq!(next_event(&events), PlayerEvent::State(PlayerState::Idle));
        player.shutdown();
    }

    #[test]
    fn next_at_last_chunk_finishes() {
        let (player, events, shared, _, _) = spawn_player(None, FakeSynth::new());
        player.play("Hi.");
        assert_eq!(
            next_event(&events),
            PlayerEvent::State(PlayerState::Loading)
        );
        assert_eq!(
            next_event(&events),
            PlayerEvent::State(PlayerState::Playing)
        );
        assert_eq!(next_event(&events), chunk(0, 1, "Hi."));
        wait_for_enqueued(&shared, 1);
        player.next();
        assert_eq!(next_event(&events), PlayerEvent::Finished);
        assert_eq!(next_event(&events), PlayerEvent::State(PlayerState::Idle));
        player.shutdown();
    }

    #[test]
    fn factory_error_surfaces_verbatim() {
        let (output, _) = FakeOutput::new();
        let (player, events) = Player::spawn(
            test_config(None),
            Box::new(move || -> Result<Box<dyn Synth>, String> { Err("boom-123".to_string()) }),
            output,
        );
        player.play("Hello.");
        assert_eq!(
            next_event(&events),
            PlayerEvent::State(PlayerState::Loading)
        );
        assert_eq!(
            next_event(&events),
            PlayerEvent::Error("boom-123".to_string())
        );
        assert_eq!(next_event(&events), PlayerEvent::State(PlayerState::Idle));
        player.shutdown();
    }

    #[test]
    fn synth_error_stops_playback() {
        let synth = FakeSynth {
            fail_on: Some("Two.".to_string()),
            ..FakeSynth::new()
        };
        let (player, events, shared, _, _) = spawn_player(None, synth);
        player.play("One. Two. Three.");
        assert_eq!(
            next_event(&events),
            PlayerEvent::State(PlayerState::Loading)
        );
        assert_eq!(
            next_event(&events),
            PlayerEvent::State(PlayerState::Playing)
        );
        assert_eq!(next_event(&events), chunk(0, 3, "One."));
        assert_eq!(
            next_event(&events),
            PlayerEvent::Error("synth failed for: Two.".to_string())
        );
        assert_eq!(next_event(&events), PlayerEvent::State(PlayerState::Idle));
        assert_eq!(shared.lock().unwrap().clears, 2); // play + fail
        player.shutdown();
    }

    #[test]
    fn idle_unload_closes_stream_and_reloads_on_next_play() {
        let (player, events, shared, factory_count, _) =
            spawn_player(Some(Duration::from_millis(50)), FakeSynth::new());
        player.play("Hi.");
        assert_eq!(
            next_event(&events),
            PlayerEvent::State(PlayerState::Loading)
        );
        assert_eq!(
            next_event(&events),
            PlayerEvent::State(PlayerState::Playing)
        );
        assert_eq!(next_event(&events), chunk(0, 1, "Hi."));
        wait_for_enqueued(&shared, 1);
        let ids = FakeOutput::enqueued_ids(&shared);
        FakeOutput::finish(&shared, ids[0]);
        assert_eq!(next_event(&events), PlayerEvent::Finished);
        assert_eq!(next_event(&events), PlayerEvent::State(PlayerState::Idle));
        assert_eq!(factory_count.load(Ordering::SeqCst), 1);
        // Unload emits nothing and fires within 50 ms.
        expect_no_event(&events);
        // The stream is dropped ~2 s after the last item finished.
        wait_for_closes(&shared);
        assert_eq!(shared.lock().unwrap().closes, 1);
        // The next play reloads the engine (Loading again).
        player.play("Yo.");
        assert_eq!(
            next_event(&events),
            PlayerEvent::State(PlayerState::Loading)
        );
        assert_eq!(
            next_event(&events),
            PlayerEvent::State(PlayerState::Playing)
        );
        assert_eq!(next_event(&events), chunk(0, 1, "Yo."));
        assert_eq!(factory_count.load(Ordering::SeqCst), 2);
        wait_for_enqueued(&shared, 2);
        let ids = FakeOutput::enqueued_ids(&shared);
        FakeOutput::finish(&shared, ids[1]);
        assert_eq!(next_event(&events), PlayerEvent::Finished);
        assert_eq!(next_event(&events), PlayerEvent::State(PlayerState::Idle));
        player.shutdown();
    }

    #[test]
    fn no_unload_after_keeps_engine_loaded() {
        let (player, events, shared, factory_count, _) = spawn_player(None, FakeSynth::new());
        player.play("Hi.");
        assert_eq!(
            next_event(&events),
            PlayerEvent::State(PlayerState::Loading)
        );
        assert_eq!(
            next_event(&events),
            PlayerEvent::State(PlayerState::Playing)
        );
        assert_eq!(next_event(&events), chunk(0, 1, "Hi."));
        wait_for_enqueued(&shared, 1);
        let ids = FakeOutput::enqueued_ids(&shared);
        FakeOutput::finish(&shared, ids[0]);
        assert_eq!(next_event(&events), PlayerEvent::Finished);
        assert_eq!(next_event(&events), PlayerEvent::State(PlayerState::Idle));
        std::thread::sleep(Duration::from_millis(150));
        player.play("Yo.");
        // No Loading: the engine was kept.
        assert_eq!(
            next_event(&events),
            PlayerEvent::State(PlayerState::Playing)
        );
        assert_eq!(next_event(&events), chunk(0, 1, "Yo."));
        assert_eq!(factory_count.load(Ordering::SeqCst), 1);
        wait_for_enqueued(&shared, 2);
        let ids = FakeOutput::enqueued_ids(&shared);
        FakeOutput::finish(&shared, ids[1]);
        assert_eq!(next_event(&events), PlayerEvent::Finished);
        assert_eq!(next_event(&events), PlayerEvent::State(PlayerState::Idle));
        player.shutdown();
    }

    #[test]
    fn pause_resume_toggle_and_stop_while_paused() {
        let (player, events, shared, _, _) = spawn_player(None, FakeSynth::new());
        player.play("One. Two. Three.");
        assert_eq!(
            next_event(&events),
            PlayerEvent::State(PlayerState::Loading)
        );
        assert_eq!(
            next_event(&events),
            PlayerEvent::State(PlayerState::Playing)
        );
        assert_eq!(next_event(&events), chunk(0, 3, "One."));
        wait_for_enqueued(&shared, 3);
        player.pause();
        assert_eq!(next_event(&events), PlayerEvent::State(PlayerState::Paused));
        player.resume();
        assert_eq!(
            next_event(&events),
            PlayerEvent::State(PlayerState::Playing)
        );
        player.toggle();
        assert_eq!(next_event(&events), PlayerEvent::State(PlayerState::Paused));
        player.toggle();
        assert_eq!(
            next_event(&events),
            PlayerEvent::State(PlayerState::Playing)
        );
        assert_eq!(shared.lock().unwrap().pauses, 2);
        assert_eq!(shared.lock().unwrap().resumes, 2);
        player.pause();
        assert_eq!(next_event(&events), PlayerEvent::State(PlayerState::Paused));
        player.stop();
        assert_eq!(next_event(&events), PlayerEvent::State(PlayerState::Idle));
        // The next play resumes the still-paused output.
        player.play("Alpha. Beta.");
        assert_eq!(
            next_event(&events),
            PlayerEvent::State(PlayerState::Playing)
        );
        assert_eq!(next_event(&events), chunk(0, 2, "Alpha."));
        assert_eq!(shared.lock().unwrap().resumes, 3);
        wait_for_enqueued(&shared, 5);
        let ids = FakeOutput::enqueued_ids(&shared);
        for id in &ids[3..] {
            FakeOutput::finish(&shared, *id);
        }
        assert_eq!(next_event(&events), chunk(1, 2, "Beta."));
        assert_eq!(next_event(&events), PlayerEvent::Finished);
        assert_eq!(next_event(&events), PlayerEvent::State(PlayerState::Idle));
        player.shutdown();
    }

    #[test]
    fn set_voice_and_speed_apply_from_next_chunk() {
        let synth = FakeSynth {
            delay: Duration::from_millis(200),
            ..FakeSynth::new()
        };
        let (player, events, shared, _, synth) = spawn_player(None, synth);
        player.play("One. Two. Three.");
        assert_eq!(
            next_event(&events),
            PlayerEvent::State(PlayerState::Loading)
        );
        assert_eq!(
            next_event(&events),
            PlayerEvent::State(PlayerState::Playing)
        );
        // Wait until the second chunk's synth has started (old settings),
        // then change them: only the third chunk may use the new ones.
        wait_for_calls(&synth, 2);
        player.set_voice("after-voice");
        player.set_speed(1.5);
        wait_for_enqueued(&shared, 3);
        assert_eq!(
            *synth.calls.lock().unwrap(),
            [
                ("One.".to_string(), "test-voice".to_string(), 1.0),
                ("Two.".to_string(), "test-voice".to_string(), 1.0),
                ("Three.".to_string(), "after-voice".to_string(), 1.5),
            ]
        );
        assert_eq!(next_event(&events), chunk(0, 3, "One."));
        let ids = FakeOutput::enqueued_ids(&shared);
        FakeOutput::finish(&shared, ids[0]);
        assert_eq!(next_event(&events), chunk(1, 3, "Two."));
        FakeOutput::finish(&shared, ids[1]);
        assert_eq!(next_event(&events), chunk(2, 3, "Three."));
        FakeOutput::finish(&shared, ids[2]);
        assert_eq!(next_event(&events), PlayerEvent::Finished);
        assert_eq!(next_event(&events), PlayerEvent::State(PlayerState::Idle));
        player.shutdown();
    }
}
