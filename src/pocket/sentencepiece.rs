//! Minimal SentencePiece BPE encoder for the tokenizer baked into the Pocket model
//! (`pocket_tts_tokenizer` metadata).
//!
//! Rust port of readly's `SentencePiece.kt`: reads the `ModelProto` bytes
//! directly and runs the same greedy highest-score merges as SentencePiece,
//! with byte fallback. Text must already be normalized
//! ([`crate::pocket::normalize_fa`]); this does not apply SentencePiece's own NFKC
//! character map. Checked against the Python `sentencepiece` package on every
//! test sentence (`tests/fixtures/pocket/fixtures.json`).

use std::collections::HashMap;
use std::fmt;

const SPACE: char = '▁';
const ZWNJ: char = '\u{200c}';
const UNK_ID: i64 = 0;

const NORMAL: u32 = 1;
const UNKNOWN: u32 = 2;
const USER_DEFINED: u32 = 4;
const BYTE: u32 = 6;

#[derive(Debug)]
pub struct TokenizerError(pub String);

impl fmt::Display for TokenizerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Couldn't read the voice tokenizer: {}", self.0)
    }
}

impl std::error::Error for TokenizerError {}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn done(&self) -> bool {
        self.offset >= self.bytes.len()
    }

    fn varint(&mut self) -> Result<u64, TokenizerError> {
        let mut result: u64 = 0;
        let mut shift = 0u32;
        loop {
            let byte = *self
                .bytes
                .get(self.offset)
                .ok_or_else(|| TokenizerError("truncated varint".to_string()))?;
            self.offset += 1;
            result |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
            if shift >= 64 {
                return Err(TokenizerError("varint too long".to_string()));
            }
        }
    }

    fn bytes_field(&mut self) -> Result<&'a [u8], TokenizerError> {
        let len = usize::try_from(self.varint()?)
            .map_err(|_| TokenizerError("field too large".to_string()))?;
        let end = self
            .offset
            .checked_add(len)
            .ok_or_else(|| TokenizerError("field overflows".to_string()))?;
        let slice = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| TokenizerError("truncated bytes field".to_string()))?;
        self.offset = end;
        Ok(slice)
    }

    fn float(&mut self) -> Result<f32, TokenizerError> {
        let slice = self
            .bytes
            .get(self.offset..self.offset + 4)
            .ok_or_else(|| TokenizerError("truncated float".to_string()))?;
        self.offset += 4;
        Ok(f32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
    }

    fn skip(&mut self, wire: u64) -> Result<(), TokenizerError> {
        match wire {
            0 => {
                self.varint()?;
            }
            1 => {
                self.offset = self
                    .offset
                    .checked_add(8)
                    .ok_or_else(|| TokenizerError("truncated fixed64".to_string()))?;
            }
            2 => {
                self.bytes_field()?;
            }
            5 => {
                self.offset = self
                    .offset
                    .checked_add(4)
                    .ok_or_else(|| TokenizerError("truncated fixed32".to_string()))?;
            }
            other => return Err(TokenizerError(format!("unsupported wire type {other}"))),
        }
        if self.offset > self.bytes.len() {
            return Err(TokenizerError("skip past end".to_string()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct Piece {
    text: String,
    score: f32,
    kind: u32,
}

fn parse_piece(bytes: &[u8]) -> Result<Piece, TokenizerError> {
    let mut reader = Reader::new(bytes);
    let mut text = String::new();
    let mut score = 0.0f32;
    let mut kind = NORMAL;
    while !reader.done() {
        let tag = reader.varint()?;
        let (field, wire) = (tag >> 3, tag & 7);
        match (field, wire) {
            (1, 2) => {
                text = String::from_utf8(reader.bytes_field()?.to_vec())
                    .map_err(|err| TokenizerError(err.to_string()))?;
            }
            (2, 5) => score = reader.float()?,
            (3, 0) => kind = u32::try_from(reader.varint()?).unwrap_or(NORMAL),
            _ => reader.skip(wire)?,
        }
    }
    Ok(Piece { text, score, kind })
}

fn parse_normalizer_add_dummy_prefix(bytes: &[u8]) -> Result<bool, TokenizerError> {
    let mut reader = Reader::new(bytes);
    let mut add_dummy_prefix = true;
    while !reader.done() {
        let tag = reader.varint()?;
        let (field, wire) = (tag >> 3, tag & 7);
        if (field, wire) == (3, 0) {
            add_dummy_prefix = reader.varint()? != 0;
        } else {
            reader.skip(wire)?;
        }
    }
    Ok(add_dummy_prefix)
}

/// Parsed BPE model: piece scores in id order plus the encode index.
pub struct SentencePiece {
    scores: Vec<f32>,
    ids: HashMap<String, i64>,
    byte_ids: HashMap<u8, i64>,
    add_dummy_prefix: bool,
}

impl SentencePiece {
    /// Parse a SentencePiece `ModelProto` (the bytes baked into the ONNX file).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, TokenizerError> {
        let mut reader = Reader::new(bytes);
        let mut pieces = Vec::new();
        let mut add_dummy_prefix = true;
        while !reader.done() {
            let tag = reader.varint()?;
            let (field, wire) = (tag >> 3, tag & 7);
            match (field, wire) {
                (1, 2) => pieces.push(parse_piece(reader.bytes_field()?)?),
                (4, 2) => {
                    add_dummy_prefix = parse_normalizer_add_dummy_prefix(reader.bytes_field()?)?;
                }
                _ => reader.skip(wire)?,
            }
        }
        if pieces.is_empty() {
            return Err(TokenizerError("model has no pieces".to_string()));
        }
        let mut ids: HashMap<String, i64> = HashMap::new();
        let mut byte_ids: HashMap<u8, i64> = HashMap::new();
        for (id, piece) in pieces.iter().enumerate() {
            let id = id as i64;
            match piece.kind {
                NORMAL | USER_DEFINED => {
                    ids.insert(piece.text.clone(), id);
                }
                BYTE => {
                    // Byte pieces look like "<0xNN>".
                    let hex = piece.text.get(3..5).unwrap_or("");
                    if let Ok(byte) = u8::from_str_radix(hex, 16) {
                        byte_ids.insert(byte, id);
                    }
                }
                UNKNOWN => {
                    ids.remove(&piece.text);
                }
                _ => {}
            }
        }
        Ok(Self {
            scores: pieces.iter().map(|piece| piece.score).collect(),
            ids,
            byte_ids,
            add_dummy_prefix,
        })
    }

    /// Number of pieces in the model.
    pub fn vocab_size(&self) -> usize {
        self.scores.len()
    }

    /// Encode `text` to token ids (greedy highest-score BPE merges + byte fallback).
    pub fn encode(&self, text: &str) -> Vec<i64> {
        if text.is_empty() {
            return Vec::new();
        }
        let mut normalized = String::with_capacity(text.len() + 1);
        if self.add_dummy_prefix {
            normalized.push(SPACE);
        }
        // The model's normalizer maps ZWNJ to a word boundary too: no vocab
        // piece contains U+200C, and Python splits half-spaces ([▁می][▁شود]).
        for c in text.chars() {
            normalized.push(if c == ' ' || c == ZWNJ { SPACE } else { c });
        }
        // One symbol per code point so a merge never splits a surrogate pair.
        let mut symbols: Vec<String> =
            normalized.chars().map(|c| c.to_string()).collect();
        // Greedy BPE: each round merges the adjacent pair whose concatenation
        // is a NORMAL or USER_DEFINED piece with the highest score (ties:
        // leftmost).
        let mut merged = String::new();
        while symbols.len() > 1 {
            let mut best_score = f32::NEG_INFINITY;
            let mut best_pos: Option<usize> = None;
            for pos in 0..symbols.len() - 1 {
                merged.clear();
                merged.push_str(&symbols[pos]);
                merged.push_str(&symbols[pos + 1]);
                let Some(id) = self.ids.get(&merged) else {
                    continue;
                };
                let score = self.scores[*id as usize];
                if score > best_score {
                    best_score = score;
                    best_pos = Some(pos);
                }
            }
            let Some(pos) = best_pos else {
                break;
            };
            let next = symbols.remove(pos + 1);
            symbols[pos].push_str(&next);
        }
        let mut out = Vec::with_capacity(symbols.len());
        for symbol in &symbols {
            if let Some(id) = self.ids.get(symbol) {
                out.push(*id);
                continue;
            }
            let bytes = symbol.as_bytes();
            if !bytes.is_empty() && bytes.iter().all(|b| self.byte_ids.contains_key(b)) {
                out.extend(bytes.iter().map(|b| self.byte_ids[b]));
            } else {
                out.push(UNK_ID);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal ModelProto by hand: pieces field 1 (piece, score, type),
    /// normalizer field 4 with add_dummy_prefix field 3.
    fn varint(mut value: u64, out: &mut Vec<u8>) {
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value > 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if value == 0 {
                break;
            }
        }
    }

    fn len_field(field: u64, payload: &[u8], out: &mut Vec<u8>) {
        varint((field << 3) | 2, out);
        varint(payload.len() as u64, out);
        out.extend_from_slice(payload);
    }

    fn piece_bytes(text: &str, score: f32, kind: u64) -> Vec<u8> {
        let mut out = Vec::new();
        len_field(1, text.as_bytes(), &mut out);
        varint((2 << 3) | 5, &mut out);
        out.extend_from_slice(&score.to_le_bytes());
        varint(3 << 3, &mut out);
        varint(kind, &mut out);
        out
    }

    fn model_bytes() -> Vec<u8> {
        // ids: 0 <unk>, 1 ▁, 2 ▁a, 3 a, 4 b, 5 ab, 6 <0x61> byte 'a',
        // 7 <0x62> byte 'b'.
        let mut out = Vec::new();
        let pieces: [(&str, f32, u64); 8] = [
            ("<unk>", 0.0, 2),
            ("▁", -1.0, 1),
            ("▁a", -0.5, 1),
            ("a", -2.0, 1),
            ("b", -2.0, 1),
            ("ab", -1.5, 1),
            ("<0x61>", 0.0, 6),
            ("<0x62>", 0.0, 6),
        ];
        for (text, score, kind) in pieces {
            len_field(1, &piece_bytes(text, score, kind), &mut out);
        }
        // Normalizer: add_dummy_prefix = true (field 3 varint 1).
        let mut normalizer = Vec::new();
        varint(3 << 3, &mut normalizer);
        varint(1, &mut normalizer);
        len_field(4, &normalizer, &mut out);
        out
    }

    #[test]
    fn merges_highest_scoring_pair_first() {
        let sp = SentencePiece::from_bytes(&model_bytes()).unwrap();
        // "ab" -> ▁ab: pairs ▁a (-0.5) and ab (-1.5); ▁a merges first, then
        // no pair of [▁a, b] is a piece.
        assert_eq!(sp.encode("ab"), vec![2, 4]);
        assert_eq!(sp.encode("a"), vec![2]);
        assert_eq!(sp.encode(""), Vec::<i64>::new());
    }

    #[test]
    fn zwnj_is_a_word_boundary_like_space() {
        let sp = SentencePiece::from_bytes(&model_bytes()).unwrap();
        assert_eq!(sp.encode("a\u{200c}b"), sp.encode("a b"));
    }

    #[test]
    fn unknown_chars_fall_back_to_bytes_then_unk() {
        let sp = SentencePiece::from_bytes(&model_bytes()).unwrap();
        // 'a' after a known prefix still resolves; a char with byte pieces
        // spells out, one without falls back to <unk> (id 0).
        let ids = sp.encode("a\u{00e9}");
        assert_eq!(ids[0], 2);
        assert_eq!(ids[1], 0, "é has no byte pieces in this toy model: {ids:?}");
    }

    #[test]
    fn truncated_model_is_an_error_not_a_panic() {
        assert!(SentencePiece::from_bytes(&[0xff, 0xff]).is_err());
        assert!(SentencePiece::from_bytes(&[]).is_err());
    }
}
