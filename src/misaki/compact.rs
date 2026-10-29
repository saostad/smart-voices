//! Streaming JSON loader. Keys and values live in contiguous arenas; sorted
//! offsets provide lookup without one allocation per retained word.
use super::lexicon::PhonemeEntry;
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::{fmt, fs::File, io::BufReader, path::Path};

#[derive(Default)]
pub struct Dictionary {
    keys: String,
    phonemes: String,
    tags: Vec<String>,
    entries: Vec<(u32, u32, u32, u32)>,
    slots: Vec<(u16, Option<(u32, u32)>)>,
}
impl Dictionary {
    pub fn load(path: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut d = serde_json::Deserializer::from_reader(BufReader::new(file));
        let result = Self::deserialize(&mut d).map_err(|e| format!("{}: {e}", path.display()))?;
        d.end().map_err(|e| e.to_string())?;
        Ok(result)
    }
    fn insert(&mut self, key: &str, value: PhonemeEntry) {
        let k = self.keys.len() as u32;
        self.keys.push_str(key);
        let start = self.slots.len() as u32;
        let values = match value {
            PhonemeEntry::Simple(s) => vec![(String::new(), Some(s))],
            PhonemeEntry::Tagged(m) => m.into_iter().collect(),
        };
        for (tag, value) in values {
            let t = match self.tags.iter().position(|s| s == &tag) {
                Some(i) => i,
                None => {
                    self.tags.push(tag);
                    self.tags.len() - 1
                }
            };
            let value = value.map(|s| {
                let start = self.phonemes.len() as u32;
                self.phonemes.push_str(&s);
                (start, self.phonemes.len() as u32)
            });
            self.slots.push((t as u16, value));
        }
        self.entries
            .push((k, self.keys.len() as u32, start, self.slots.len() as u32));
    }
    pub fn get(&self, key: &str) -> Option<PhonemeEntry> {
        let i = self
            .entries
            .binary_search_by(|e| self.keys[e.0 as usize..e.1 as usize].cmp(key))
            .ok()?;
        let e = self.entries[i];
        let slots = &self.slots[e.2 as usize..e.3 as usize];
        let decode = |v: Option<(u32, u32)>| {
            v.map(|(a, b)| self.phonemes[a as usize..b as usize].to_owned())
        };
        if slots.len() == 1 && self.tags[slots[0].0 as usize].is_empty() {
            return decode(slots[0].1).map(PhonemeEntry::Simple);
        }
        Some(PhonemeEntry::Tagged(
            slots
                .iter()
                .map(|(t, v)| (self.tags[*t as usize].clone(), decode(*v)))
                .collect(),
        ))
    }
}
impl<'de> Deserialize<'de> for Dictionary {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Dictionary;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a pronunciation dictionary")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut m: M) -> Result<Dictionary, M::Error> {
                let mut out = Dictionary::default();
                while let Some((key, value)) = m.next_entry::<String, PhonemeEntry>()? {
                    out.insert(&key, value);
                }
                out.entries.sort_unstable_by(|a, b| {
                    out.keys[a.0 as usize..a.1 as usize].cmp(&out.keys[b.0 as usize..b.1 as usize])
                });
                Ok(out)
            }
        }
        d.deserialize_map(V)
    }
}

impl<'de> Deserialize<'de> for PhonemeEntry {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = PhonemeEntry;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("phonemes or POS variants")
            }
            fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<Self::Value, E> {
                Ok(PhonemeEntry::Simple(s.into()))
            }
            fn visit_map<M: MapAccess<'de>>(self, mut m: M) -> Result<Self::Value, M::Error> {
                let mut out = std::collections::HashMap::new();
                while let Some((k, v)) = m.next_entry::<String, Option<String>>()? {
                    out.insert(k, v);
                }
                Ok(PhonemeEntry::Tagged(out))
            }
        }
        d.deserialize_any(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compact_roundtrip_preserves_null_variants_and_sorted_keys() {
        let dict: Dictionary = serde_json::from_str(
            r#"{"z":"zee","read":{"DEFAULT":"rid","VBD":"red","None":null},"A":"ay"}"#,
        )
        .unwrap();
        assert!(matches!(dict.get("z"),Some(PhonemeEntry::Simple(s)) if s=="zee"));
        assert!(dict.get("missing").is_none());
        match dict.get("read").unwrap() {
            PhonemeEntry::Tagged(m) => {
                assert_eq!(m["VBD"].as_deref(), Some("red"));
                assert_eq!(m["None"], None);
            }
            _ => panic!("expected variants"),
        }
    }
}
