use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::{collections::HashMap, fmt, path::Path};
struct Pairs<T>(Vec<(String, T)>);
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Pairs<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V<T>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for V<T> {
            type Value = Pairs<T>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a map")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut m: M) -> Result<Self::Value, M::Error> {
                let mut v = Vec::new();
                while let Some(p) = m.next_entry()? {
                    v.push(p);
                }
                Ok(Pairs(v))
            }
        }
        d.deserialize_map(V(std::marker::PhantomData))
    }
}
// The streaming visitor consumes each feature before reading the next one.
struct Weights {
    keys: String,
    rows: Vec<(u32, u32, u32, u32)>,
    values: Vec<(u8, f32)>,
    classes: Vec<String>,
}
impl Weights {
    fn load(path: &Path, classes: Vec<String>) -> Result<Self, String> {
        struct Seed(Weights);
        impl<'de> serde::de::DeserializeSeed<'de> for Seed {
            type Value = Weights;
            fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Weights, D::Error> {
                struct V(Weights);
                impl<'de> Visitor<'de> for V {
                    type Value = Weights;
                    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                        f.write_str("tagger weights")
                    }
                    fn visit_map<M: MapAccess<'de>>(
                        mut self,
                        mut m: M,
                    ) -> Result<Weights, M::Error> {
                        while let Some((key, Pairs(weights))) =
                            m.next_entry::<String, Pairs<f32>>()?
                        {
                            let a = self.0.keys.len() as u32;
                            self.0.keys.push_str(&key);
                            let b = self.0.values.len() as u32;
                            for (tag, w) in weights {
                                let i =
                                    self.0.classes.iter().position(|c| c == &tag).ok_or_else(
                                        || serde::de::Error::custom("unknown POS class"),
                                    )?;
                                if !w.is_finite() {
                                    return Err(serde::de::Error::custom("nonfinite POS weight"));
                                }
                                self.0.values.push((i as u8, w));
                            }
                            self.0.rows.push((
                                a,
                                self.0.keys.len() as u32,
                                b,
                                self.0.values.len() as u32,
                            ));
                        }
                        self.0.rows.sort_unstable_by(|a, b| {
                            self.0.keys[a.0 as usize..a.1 as usize]
                                .cmp(&self.0.keys[b.0 as usize..b.1 as usize])
                        });
                        Ok(self.0)
                    }
                }
                d.deserialize_map(V(self.0))
            }
        }
        use serde::de::DeserializeSeed;
        let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let mut decoder = serde_json::Deserializer::from_reader(std::io::BufReader::new(file));
        let weights = Seed(Self {
            keys: String::new(),
            rows: Vec::new(),
            values: Vec::new(),
            classes,
        })
        .deserialize(&mut decoder)
        .map_err(|e| e.to_string())?;
        decoder.end().map_err(|e| e.to_string())?;
        Ok(weights)
    }
    fn predict(&self, features: HashMap<String, usize>) -> &str {
        let mut scores = vec![0.; self.classes.len()];
        for (key, value) in features {
            if let Ok(i) = self
                .rows
                .binary_search_by(|r| self.keys[r.0 as usize..r.1 as usize].cmp(&key))
            {
                let r = self.rows[i];
                for (c, w) in &self.values[r.2 as usize..r.3 as usize] {
                    scores[*c as usize] += w * value as f32;
                }
            }
        }
        let i = (0..scores.len())
            .max_by(|a, b| scores[*a].total_cmp(&scores[*b]))
            .unwrap_or(0);
        &self.classes[i]
    }
}
pub struct PerceptronTagger {
    model: Weights,
    tag_keys: String,
    tags: Vec<(u32, u32, u8)>,
}
impl PerceptronTagger {
    pub fn load(root: &Path) -> Result<Self, String> {
        let classes: Vec<String> = std::fs::read_to_string(root.join("classes.txt"))
            .map_err(|e| e.to_string())?
            .lines()
            .map(|s| s.trim().to_owned())
            .collect();
        if classes.is_empty() || classes.len() > 256 {
            return Err("invalid POS classes".into());
        }
        let model = Weights::load(&root.join("weights.json"), classes)?;
        let file = std::fs::File::open(root.join("tags.json")).map_err(|e| e.to_string())?;
        let Pairs(mut tags) =
            serde_json::from_reader::<_, Pairs<String>>(std::io::BufReader::new(file))
                .map_err(|e| e.to_string())?;
        tags.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        let mut tag_keys = String::new();
        let mut compact = Vec::with_capacity(tags.len());
        for (key, tag) in tags {
            let class = model
                .classes
                .iter()
                .position(|c| c == &tag)
                .ok_or("unknown word POS tag")?;
            let start = tag_keys.len() as u32;
            tag_keys.push_str(&key);
            compact.push((start, tag_keys.len() as u32, class as u8));
        }
        Ok(Self {
            model,
            tag_keys,
            tags: compact,
        })
    }
    pub fn tag(&self, words: &[&str]) -> Vec<String> {
        let mut prev = "-START-";
        let mut prev2 = "-START2-";
        let mut output = Vec::new();
        let normalized: Vec<String> = words
            .iter()
            .map(|w| {
                if w.contains('-') && !w.starts_with('-') {
                    "!HYPHEN".into()
                } else if w.len() == 4 && w.chars().all(|c| c.is_ascii_digit()) {
                    "!YEAR".into()
                } else if w.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                    "!DIGITS".into()
                } else {
                    w.to_lowercase()
                }
            })
            .collect();
        let mut context = vec!["-START-", "-START2-"];
        context.extend(normalized.iter().map(String::as_str));
        context.extend(["-END-", "-END2-"]);
        for (i, word) in words.iter().enumerate() {
            let tag = match self
                .tags
                .binary_search_by(|p| self.tag_keys[p.0 as usize..p.1 as usize].cmp(word))
            {
                Ok(j) => self.model.classes[self.tags[j].2 as usize].as_str(),
                Err(_) => {
                    self.model
                        .predict(Self::get_features(i + 2, word, &context, prev, prev2))
                }
            };
            output.push(tag.to_owned());
            prev2 = prev;
            prev = tag;
        }
        output
    }
    fn get_features(
        i: usize,
        word: &str,
        context: &[&str],
        prev: &str,
        prev2: &str,
    ) -> HashMap<String, usize> {
        let mut features = HashMap::new();
        features.insert("bias".to_string(), 1);

        let suffix: String = word
            .chars()
            .rev()
            .take(3)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        features.insert(format!("i suffix {}", suffix), 1);

        let pref1: String = word.chars().take(1).collect();
        features.insert(format!("i pref1 {}", pref1), 1);

        features.insert(format!("i-1 tag {}", prev), 1);
        features.insert(format!("i-2 tag {}", prev2), 1);
        features.insert(format!("i tag+i-2 tag {} {}", prev, prev2), 1);
        features.insert(format!("i word {}", context[i]), 1);
        features.insert(format!("i-1 tag+i word {} {}", prev, context[i]), 1);
        features.insert(format!("i-1 word {}", context[i - 1]), 1);
        features.insert(format!("i-2 word {}", context[i - 2]), 1);
        features.insert(format!("i+1 word {}", context[i + 1]), 1);
        features.insert(format!("i+2 word {}", context[i + 2]), 1);

        let i_plus_1_suffix: String = context[i + 1]
            .chars()
            .rev()
            .take(3)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        features.insert(format!("i+1 suffix {}", i_plus_1_suffix), 1);

        let i_minus_1_suffix: String = context[i - 1]
            .chars()
            .rev()
            .take(3)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        features.insert(format!("i-1 suffix {}", i_minus_1_suffix), 1);

        features
    }
}

#[cfg(test)]
impl PerceptronTagger {
    pub fn tiny() -> Self {
        Self {
            model: Weights {
                keys: String::new(),
                rows: vec![],
                values: vec![],
                classes: vec!["NN".into()],
            },
            tag_keys: String::new(),
            tags: vec![],
        }
    }
}
