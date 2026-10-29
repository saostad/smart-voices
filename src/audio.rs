//! Audio playback on the default output device (cpal).

use std::collections::VecDeque;
use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, StreamConfig};

/// Errors from opening the audio device or playing samples.
#[derive(Debug)]
pub enum AudioError {
    /// No default output device.
    NoDevice,
    /// Couldn't query the device's default config (keeps cpal's text).
    Config(String),
    /// Couldn't build the output stream (keeps cpal's text).
    Open(String),
    /// Couldn't start the stream (keeps cpal's text).
    Play(String),
    /// Resampling to the device rate failed.
    Resample { from: u32, to: u32 },
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioError::NoDevice => write!(f, "No audio output device found."),
            AudioError::Config(err) => {
                write!(f, "Couldn't read the audio output config: {err}")
            }
            AudioError::Open(err) => write!(f, "Couldn't open the audio output: {err}"),
            AudioError::Play(err) => write!(f, "Couldn't start audio playback: {err}"),
            AudioError::Resample { from, to } => {
                write!(f, "Couldn't resample audio from {from} Hz to {to} Hz.")
            }
        }
    }
}

impl Error for AudioError {}

/// Resample mono `samples` from `from` Hz to `to` Hz (copies when equal).
fn resample_to_rate(samples: &[f32], from: u32, to: u32) -> Result<Vec<f32>, AudioError> {
    if from == to {
        return Ok(samples.to_vec());
    }
    if from == 0 || to == 0 {
        eprintln!("error: invalid playback sample rate {from} -> {to}");
        return Err(AudioError::Resample { from, to });
    }
    // A missing or invalid settings file falls back to the defaults
    // `Settings::load` already returns (the app warned at startup); it
    // must not stop playback.
    let (settings, error) = match crate::settings::settings_path() {
        Some(path) => crate::settings::Settings::load(&path),
        None => {
            eprintln!(
                "warn: fallback: true, attempted=settings_file, reason=settings_directory_unavailable, taken=default_resampler_settings"
            );
            (crate::settings::Settings::default(), None)
        }
    };
    if let Some(error) = error {
        eprintln!(
            "warn: fallback: true, attempted=settings_file, reason={error}, taken=default_resampler_settings"
        );
    }
    Ok(resample_with(
        samples,
        from,
        to,
        settings.resample_cutoff_ratio,
        settings.resample_filter_zeros,
    ))
}

/// Windowed-sinc resample with explicit settings (see [`resample_to_rate`]
/// for where the values come from). The defaults (six zero crossings, 0.99
/// cutoff) match the old Kaldi linear resampler. The cutoff follows the
/// lower Nyquist rate.
fn resample_with(
    samples: &[f32],
    from: u32,
    to: u32,
    cutoff_ratio: f32,
    filter_zeros: u32,
) -> Vec<f32> {
    if from == to {
        return samples.to_vec();
    }
    let cutoff = f64::from(cutoff_ratio) * f64::from(from.min(to)) / 2.;
    let radius = f64::from(filter_zeros) * f64::from(from) / (2. * cutoff);
    let len = (samples.len() as u64 * u64::from(to)).div_ceil(u64::from(from)) as usize;
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        let t = i as f64 * f64::from(from) / f64::from(to);
        let start = (t - radius).ceil().max(0.) as usize;
        let end = ((t + radius).floor() as usize + 1).min(samples.len());
        let mut sum = 0.;
        for (j, sample) in samples.iter().enumerate().take(end).skip(start) {
            let delta = (t - j as f64) / f64::from(from);
            let x = 2. * std::f64::consts::PI * cutoff * delta;
            let sinc = if x.abs() < f64::EPSILON {
                1.
            } else {
                x.sin() / x
            };
            let window = 0.5 * (1. + (std::f64::consts::PI * (t - j as f64) / radius).cos());
            sum += f64::from(*sample) * 2. * cutoff / f64::from(from) * sinc * window;
        }
        out.push(sum as f32);
    }
    out
}

/// Events from the audio sink to the playback controller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SinkEvent {
    /// Queued item `id` finished playing.
    ItemFinished(u64),
    /// The stream failed (device unplugged / default device changed).
    Error(String),
}

/// One queued item: mono samples already at the device rate.
struct QueuedItem {
    id: u64,
    samples: Vec<f32>,
    /// Next sample index to write.
    pos: usize,
}

/// How many finished items the audio callback can park in [`SinkState::spent`]
/// before it must drop them (rare; frees on the real-time thread as before).
const SPENT_CAPACITY: usize = 8;

/// State shared with the cpal data callback.
struct SinkState {
    queue: VecDeque<QueuedItem>,
    /// Finished items parked by the audio callback instead of dropped there:
    /// dropping a `QueuedItem` frees its sample `Vec`, which must not happen
    /// on the real-time thread. The controller side frees them via
    /// [`SinkState::drain_spent`].
    spent: Vec<QueuedItem>,
    /// Output silence instead of queued audio (pause fallback for hosts that
    /// refuse to pause the stream).
    silence: bool,
    /// Completion/error notifications; set by the controller before playback.
    events: Option<mpsc::Sender<SinkEvent>>,
}

impl SinkState {
    fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            spent: Vec::with_capacity(SPENT_CAPACITY),
            silence: false,
            events: None,
        }
    }

    /// Move the finished head item off the queue into `spent` and return its
    /// id. Runs on the audio callback: it never frees sample buffers unless
    /// `spent` is full (rare). Returns `None` when the queue is empty.
    fn retire_finished_head(&mut self) -> Option<u64> {
        let done = self.queue.pop_front()?;
        let id = done.id;
        if self.spent.len() < self.spent.capacity() {
            self.spent.push(done);
        }
        // Else: `spent` is full; drop `done` here as before.
        Some(id)
    }

    /// Free sample buffers retired by the audio callback. Runs only on the
    /// controller side, from `enqueue`, `clear` and `close_if_idle`.
    fn drain_spent(&mut self) {
        self.spent.clear();
    }
}

/// Persistent audio sink: at most one cpal output stream, opened lazily on the
/// first `enqueue` at the device's default config and shared by all queued
/// items for gapless playback.
pub struct AudioSink {
    device: cpal::Device,
    channels: u16,
    sample_rate: u32,
    state: Arc<Mutex<SinkState>>,
    stream: Option<cpal::Stream>,
    /// Set by the stream error callback; the dead stream is dropped and the
    /// current default device is re-resolved on the next `enqueue`.
    dead: Arc<AtomicBool>,
    /// Logical pause state (survives stream close/reopen).
    paused: bool,
}

impl AudioSink {
    /// Resolve the default output device and its default config. No stream is
    /// opened yet; that happens lazily on the first `enqueue`.
    pub fn new() -> Result<Self, AudioError> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or(AudioError::NoDevice)?;
        let supported = device
            .default_output_config()
            .map_err(|err| AudioError::Config(err.to_string()))?;
        Ok(Self {
            device,
            channels: supported.channels(),
            sample_rate: supported.sample_rate(),
            state: Arc::new(Mutex::new(SinkState::new())),
            stream: None,
            dead: Arc::new(AtomicBool::new(false)),
            paused: false,
        })
    }

    /// Set the channel receiving [`SinkEvent`]s (called by the player before
    /// the first `enqueue`).
    pub fn set_event_sender(&mut self, sender: mpsc::Sender<SinkEvent>) {
        self.state
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .events = Some(sender);
    }

    /// Queue mono `samples` at `sample_rate` Hz for gapless playback,
    /// resampling to the device rate when it differs. Opens the output stream
    /// on first use. An [`SinkEvent::ItemFinished`] with `id` is sent once the
    /// item's last sample was handed to the device.
    pub fn enqueue(
        &mut self,
        id: u64,
        samples: &[f32],
        sample_rate: u32,
    ) -> Result<(), AudioError> {
        let mono = resample_to_rate(samples, sample_rate, self.sample_rate)?;
        self.ensure_stream()?;
        let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        state.drain_spent();
        state.queue.push_back(QueuedItem {
            id,
            samples: mono,
            pos: 0,
        });
        Ok(())
    }

    /// Pause output: the stream itself is paused, so a paused app uses no
    /// audio CPU. If the host refuses, fall back to outputting silence.
    pub fn pause(&mut self) {
        self.paused = true;
        if let Some(stream) = &self.stream
            && let Err(err) = stream.pause()
        {
            eprintln!("Couldn't pause the audio output ({err}); playing silence instead.");
            self.state
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .silence = true;
        }
    }

    /// Resume output after [`AudioSink::pause`].
    pub fn resume(&mut self) {
        self.paused = false;
        self.state
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .silence = false;
        if let Some(stream) = &self.stream
            && let Err(err) = stream.play()
        {
            eprintln!("Couldn't resume the audio output: {err}.");
        }
    }

    /// Drop all queued items immediately; stale audio never plays after this.
    pub fn clear(&mut self) {
        let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        state.queue.clear();
        state.drain_spent();
    }

    /// Drop the stream when the queue is empty (the controller calls this once
    /// playback has been idle for a while, so an idle app holds no stream).
    pub fn close_if_idle(&mut self) {
        if self.dead.load(Ordering::SeqCst) {
            self.stream = None;
            self.dead.store(false, Ordering::SeqCst);
        }
        let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        state.drain_spent();
        if state.queue.is_empty() {
            self.stream = None;
        }
    }

    /// Open the stream unless already open, following the current default
    /// device (it may have changed after a stream error). Items queued before
    /// a device change keep their samples and play at the new stream's rate.
    fn ensure_stream(&mut self) -> Result<(), AudioError> {
        if self.stream.is_some() && !self.dead.load(Ordering::SeqCst) {
            return Ok(());
        }
        self.stream = None;
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or(AudioError::NoDevice)?;
        let supported = device
            .default_output_config()
            .map_err(|err| AudioError::Config(err.to_string()))?;
        self.device = device;
        self.channels = supported.channels();
        self.sample_rate = supported.sample_rate();
        self.dead.store(false, Ordering::SeqCst);

        let config = StreamConfig {
            channels: self.channels,
            sample_rate: self.sample_rate,
            buffer_size: BufferSize::Default,
        };
        let channels = usize::from(self.channels);
        let state = Arc::clone(&self.state);
        // The error callback never touches the mutex: it uses a clone of the
        // sender taken here (set before the first `enqueue`).
        let error_events = self
            .state
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .events
            .clone();
        let dead = Arc::clone(&self.dead);
        let stream = self
            .device
            .build_output_stream(
                config,
                move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                    // Never block the audio thread: on contention output
                    // silence for this buffer.
                    let Ok(mut state) = state.try_lock() else {
                        data.fill(0.0);
                        return;
                    };
                    if state.silence {
                        data.fill(0.0);
                        return;
                    }
                    for frame in data.chunks_mut(channels) {
                        loop {
                            let next = match state.queue.front_mut() {
                                None => {
                                    frame.fill(0.0);
                                    break;
                                }
                                Some(item) => {
                                    if item.pos < item.samples.len() {
                                        let sample = item.samples[item.pos];
                                        item.pos += 1;
                                        Some(sample)
                                    } else {
                                        None
                                    }
                                }
                            };
                            match next {
                                Some(sample) => {
                                    frame.fill(sample);
                                    break;
                                }
                                None => {
                                    // The head item is done: park it in
                                    // `spent` (no free on this thread),
                                    // report it, and fill this frame from
                                    // the next item. Sending on an mpsc
                                    // channel doesn't block.
                                    if let Some(id) = state.retire_finished_head()
                                        && let Some(events) = &state.events
                                    {
                                        let _ = events.send(SinkEvent::ItemFinished(id));
                                    }
                                }
                            }
                        }
                    }
                },
                move |err| {
                    dead.store(true, Ordering::SeqCst);
                    if let Some(events) = &error_events {
                        let _ =
                            events.send(SinkEvent::Error(format!("Audio output stopped: {err}")));
                    }
                },
                None,
            )
            .map_err(|err| AudioError::Open(err.to_string()))?;
        if !self.paused {
            stream
                .play()
                .map_err(|err| AudioError::Play(err.to_string()))?;
        }
        self.stream = Some(stream);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queued(id: u64) -> QueuedItem {
        QueuedItem {
            id,
            samples: vec![0.0; 16],
            pos: 16,
        }
    }

    #[test]
    fn resample_with_doubles_dc_signal() {
        // The pure resampler must not depend on the settings file: bad
        // or missing settings fall back to defaults in the wrapper.
        let dc = vec![1.0f32; 240];
        let out = resample_with(&dc, 24_000, 48_000, 0.99, 6);
        assert_eq!(out.len(), 480);
        let mid = out[out.len() / 2];
        assert!((mid - 1.0).abs() < 0.02, "mid DC sample {mid}");
        assert_eq!(resample_with(&dc, 24_000, 24_000, 0.99, 6), dc);
    }

    #[test]
    fn spent_starts_empty_with_capacity() {
        let state = SinkState::new();
        assert!(state.spent.is_empty());
        assert_eq!(state.spent.capacity(), SPENT_CAPACITY);
    }

    #[test]
    fn retire_parks_head_and_drain_frees_it() {
        let mut state = SinkState::new();
        state.queue.push_back(queued(7));
        state.queue.push_back(queued(8));

        assert_eq!(state.retire_finished_head(), Some(7));
        assert_eq!(state.queue.len(), 1);
        assert_eq!(state.spent.len(), 1);
        assert_eq!(state.spent[0].id, 7);

        state.drain_spent();
        assert!(state.spent.is_empty());
        // Draining keeps the reservation (no realloc on the next retire).
        assert_eq!(state.spent.capacity(), SPENT_CAPACITY);
        // The live queue is untouched.
        assert_eq!(state.queue.len(), 1);
        assert_eq!(state.queue[0].id, 8);

        assert_eq!(state.retire_finished_head(), Some(8));
        assert_eq!(state.retire_finished_head(), None);
    }

    #[test]
    fn retire_drops_when_spent_is_full() {
        let mut state = SinkState::new();
        for id in 0..SPENT_CAPACITY as u64 {
            state.queue.push_back(queued(id));
            assert_eq!(state.retire_finished_head(), Some(id));
        }
        assert_eq!(state.spent.len(), SPENT_CAPACITY);

        // Full: the item is dropped instead of growing `spent`, but its id is
        // still reported so completion signalling is unchanged.
        state.queue.push_back(queued(99));
        assert_eq!(state.retire_finished_head(), Some(99));
        assert_eq!(state.spent.len(), SPENT_CAPACITY);
        assert_eq!(state.spent.capacity(), SPENT_CAPACITY);
    }
}
