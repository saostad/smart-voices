//! Audio playback on the default output device (cpal).

use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, StreamConfig};
use sherpa_onnx::LinearResampler;

/// Silence played after the last sample so the device drains its queued
/// buffers before we report completion and drop the stream.
const DRAIN_TAIL_SECONDS: f32 = 0.2;

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
    /// The stream reported an error before playback finished (keeps cpal's text).
    Playback(String),
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
            AudioError::Playback(err) => write!(f, "Audio playback failed: {err}"),
        }
    }
}

impl Error for AudioError {}

/// State shared with the cpal data callback.
struct Playback {
    /// Mono samples at the device rate.
    samples: Vec<f32>,
    /// Next sample index to write.
    next: AtomicUsize,
    /// Silence frames still to play after the last sample.
    tail_left: AtomicUsize,
    /// Set once the tail has drained.
    drained: AtomicBool,
    /// Set once completion (or failure) was signalled.
    done: AtomicBool,
    /// Latest stream error text, if any.
    last_error: Mutex<Option<String>>,
}

/// Default audio output device, opened for f32 playback.
pub struct AudioOut {
    device: cpal::Device,
    channels: u16,
    sample_rate: u32,
}

impl AudioOut {
    /// Open the default output device using its default channel count and rate.
    pub fn open() -> Result<Self, AudioError> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or(AudioError::NoDevice)?;
        let supported = device
            .default_output_config()
            .map_err(|err| AudioError::Config(err.to_string()))?;
        Ok(Self {
            device,
            channels: supported.channels(),
            sample_rate: supported.sample_rate(),
        })
    }

    /// Play mono `samples` at `sample_rate` Hz, returning once the last
    /// sample was played. Samples are resampled to the device rate when it
    /// differs, and duplicated to every channel.
    pub fn play_blocking(&self, samples: &[f32], sample_rate: u32) -> Result<(), AudioError> {
        let mono = self.to_device_rate(samples, sample_rate)?;
        let tail_frames = (DRAIN_TAIL_SECONDS * self.sample_rate as f32) as usize;
        let shared = Arc::new(Playback {
            samples: mono,
            next: AtomicUsize::new(0),
            tail_left: AtomicUsize::new(tail_frames),
            drained: AtomicBool::new(false),
            done: AtomicBool::new(false),
            last_error: Mutex::new(None),
        });
        let (done_tx, done_rx) = mpsc::channel::<()>();

        let config = StreamConfig {
            channels: self.channels,
            sample_rate: self.sample_rate,
            buffer_size: BufferSize::Default,
        };
        let channels = self.channels as usize;
        let writer = Arc::clone(&shared);
        let writer_done = done_tx.clone();
        let error_state = Arc::clone(&shared);
        let stream = self
            .device
            .build_output_stream(
                config,
                move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                    for frame in data.chunks_mut(channels) {
                        let i = writer.next.fetch_add(1, Ordering::SeqCst);
                        let sample = writer.samples.get(i).copied().unwrap_or(0.0);
                        frame.fill(sample);
                    }
                    // Once every sample was handed to the device, play a short
                    // silence tail so queued buffers drain, then signal.
                    if writer.next.load(Ordering::SeqCst) >= writer.samples.len() {
                        count_down(&writer.tail_left, data.len() / channels);
                        if writer.tail_left.load(Ordering::SeqCst) == 0 {
                            writer.drained.store(true, Ordering::SeqCst);
                            signal_once(&writer.done, &writer_done);
                        }
                    }
                },
                move |err| {
                    eprintln!("Audio playback error: {err}");
                    *error_state
                        .last_error
                        .lock()
                        .unwrap_or_else(|e| e.into_inner()) = Some(err.to_string());
                    // Never hang if the device dies mid-playback.
                    signal_once(&error_state.done, &done_tx);
                },
                None,
            )
            .map_err(|err| AudioError::Open(err.to_string()))?;
        stream
            .play()
            .map_err(|err| AudioError::Play(err.to_string()))?;

        // Block until the callback signals completion; no polling.
        let _ = done_rx.recv();
        drop(stream);

        if shared.drained.load(Ordering::SeqCst) {
            Ok(())
        } else {
            let err = shared
                .last_error
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
                .unwrap_or_else(|| "the audio device stopped".to_string());
            Err(AudioError::Playback(err))
        }
    }

    /// Resample mono `samples` from `sample_rate` to the device rate.
    fn to_device_rate(&self, samples: &[f32], sample_rate: u32) -> Result<Vec<f32>, AudioError> {
        if sample_rate == self.sample_rate {
            return Ok(samples.to_vec());
        }
        let from = i32::try_from(sample_rate);
        let to = i32::try_from(self.sample_rate);
        let resampler = from
            .ok()
            .zip(to.ok())
            .and_then(|(from, to)| LinearResampler::create(from, to));
        match resampler {
            Some(resampler) => Ok(resampler.resample(samples, true)),
            None => Err(AudioError::Resample {
                from: sample_rate,
                to: self.sample_rate,
            }),
        }
    }
}

/// Subtract `n` from `counter`, saturating at zero.
fn count_down(counter: &AtomicUsize, n: usize) {
    let mut left = counter.load(Ordering::SeqCst);
    while left > 0 {
        let new = left.saturating_sub(n);
        match counter.compare_exchange_weak(left, new, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => break,
            Err(actual) => left = actual,
        }
    }
}

/// Send completion exactly once.
fn signal_once(done: &AtomicBool, tx: &mpsc::Sender<()>) {
    if !done.swap(true, Ordering::SeqCst) {
        let _ = tx.send(());
    }
}
