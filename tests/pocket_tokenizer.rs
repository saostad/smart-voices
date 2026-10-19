//! Pocket tokenizer fixtures from readly (checked there against the Python
//! `sentencepiece` package): every row's text must normalize to `normalized`
//! and encode to `ids`.

use smart_voices::pocket::normalize_fa::normalize;
use smart_voices::pocket::sentencepiece::SentencePiece;

#[test]
fn fixtures_normalize_and_encode() {
    let model = std::fs::read("tests/fixtures/pocket/tokenizer.model").unwrap();
    let sp = SentencePiece::from_bytes(&model).unwrap();
    let raw = std::fs::read_to_string("tests/fixtures/pocket/fixtures.json").unwrap();
    let rows: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap();
    assert_eq!(rows.len(), 58, "fixture row count");
    for (index, row) in rows.iter().enumerate() {
        let text = row["text"].as_str().unwrap();
        let expected_normalized = row["normalized"].as_str().unwrap();
        let expected_ids: Vec<i64> = row["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|id| id.as_i64().unwrap())
            .collect();
        let normalized = normalize(text);
        assert_eq!(
            normalized, expected_normalized,
            "row {index} normalize: {text:?}"
        );
        assert_eq!(
            sp.encode(&normalized),
            expected_ids,
            "row {index} encode: {normalized:?}"
        );
    }
}
