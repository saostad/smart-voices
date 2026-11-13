//! Smart Voices: clipboard watcher that reads cleaned text aloud (Kokoro voice).

pub mod app;
pub mod audio;
pub mod autostart;
pub mod chunk;
pub mod clipboard;
pub mod control;
pub mod download;
pub mod engine;
pub mod icon;
pub mod lang;
pub mod latin_fa;
pub mod models;
pub mod normalize;
mod notification;
pub mod phonon;
pub mod pipeline;
pub mod player;
pub mod pocket;
pub mod pronounce;
pub mod reader;
pub mod sanitize;
pub mod settings;
pub mod shortcuts;
pub mod word_timing;
pub mod zonos;

mod misaki;

mod espeak;

mod onnx_tts;

pub mod cli;
pub mod export;

pub mod server;

mod http_transport;
