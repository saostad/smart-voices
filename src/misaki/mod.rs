//! Vendored from MicheleYin/misaki-rs 0.6.0 (MIT), a port of hexgrad/misaki.
//! Modified for original dictionaries, compact storage, fallible loading,
//! source whitespace, whole abbreviations and espeak number pronunciation.
#![allow(dead_code)]
mod compact;
mod g2p;
pub mod language;
mod lexicon;
mod tagger;
pub use g2p::G2P;
pub mod downloads;
