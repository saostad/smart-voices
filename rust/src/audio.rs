//! Audio playback on the default output device (cpal).

use std::collections::VecDeque;
use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, StreamConfig};
use sherpa_onnx::LinearResampler;

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
    let resampler = i32::try_from(from)
        .ok()
        .zip(i32::try_from(to).ok())
        .and_then(|(from, to)| LinearResampler::create(from, to));
    match resampler {
        Some(resampler) => Ok(resampler.resample(samples, true)),
        None => Err(AudioError::Resample { from, to }),
    }
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

/// State shared with the cpal data callback.
struct SinkState {
    queue: VecDeque<QueuedItem>,
    /// Output silence instead of queued audio (pause fallback for hosts that
    /// refuse to pause the stream).
    silence: bool,
    /// Completion/error notifications; set by the controller before playback.
    events: Option<mpsc::Sender<SinkEvent>>,
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
            state: Arc::new(Mutex::new(SinkState {
                queue: VecDeque::new(),
                silence: false,
                events: None,
            })),
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
        self.state
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .queue
            .push_back(QueuedItem {
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
        self.state
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .queue
            .clear();
    }

    /// Drop the stream when the queue is empty (the controller calls this once
    /// playback has been idle for a while, so an idle app holds no stream).
    pub fn close_if_idle(&mut self) {
        if self.dead.load(Ordering::SeqCst) {
            self.stream = None;
            self.dead.store(false, Ordering::SeqCst);
        }
        let empty = self
            .state
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .queue
            .is_empty();
        if empty {
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
                                    // The head item is done: report it and
                                    // fill this frame from the next item.
                                    // Sending on an mpsc channel doesn't block.
                                    if let Some(done) = state.queue.pop_front()
                                        && let Some(events) = &state.events
                                    {
                                        let _ = events.send(SinkEvent::ItemFinished(done.id));
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
