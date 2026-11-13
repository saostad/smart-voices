//! Pocket v2 front-end goldens from readly (generated there from the torch
//! reference pipeline): every row's text must normalize, split, clean,
//! pronounce and translate exactly.

use smart_voices::pocket::g2p::G2p;
use smart_voices::pocket::text_v2::{
    clean_sentence_for_g2p, normalize_for_model, split_sentences, translate_phonemes,
};

/// Point ORT at the profile dir's `libonnxruntime` when running under
/// cargo: test binaries can live in `deps/` or a nested build output
/// directory, while the runtime is in `target/<profile>/`. Leaves an explicit
/// `ORT_DYLIB_PATH` alone.
fn point_ort_at_profile_dylib() {
    if std::env::var("ORT_DYLIB_PATH").is_ok_and(|value| !value.is_empty()) {
        return;
    }
    let file = if cfg!(target_os = "windows") {
        "onnxruntime.dll"
    } else if cfg!(target_os = "macos") {
        "libonnxruntime.dylib"
    } else {
        "libonnxruntime.so"
    };
    let exe = std::env::current_exe().expect("locate the Pocket golden test executable");
    let dylib = exe
        .ancestors()
        .skip(1)
        .map(|dir| dir.join(file))
        .find(|path| path.is_file())
        .expect("ONNX Runtime missing: run scripts/fetch-onnxruntime.sh or set ORT_DYLIB_PATH");
    // SAFETY: the golden test is the only ORT user in this process
    // and runs alone here.
    unsafe {
        std::env::set_var("ORT_DYLIB_PATH", &dylib);
    }
}

#[test]
fn golden_rows_match_normalizer_cleaner_translator_and_g2p() {
    point_ort_at_profile_dylib();
    let raw = std::fs::read_to_string("tests/fixtures/pocket/v2-golden.jsonl").unwrap();
    let rows: Vec<serde_json::Value> = raw
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 53, "golden row count");
    let mut g2p = G2p::load().unwrap();
    assert_eq!(g2p.phonemise("").unwrap(), "");
    for (index, row) in rows.iter().enumerate() {
        let text = row["text"].as_str().unwrap();
        let expected_normalized = row["normalized"].as_str().unwrap();
        let normalized = normalize_for_model(text)
            .unwrap_or_else(|err| panic!("row {index} normalize_for_model failed: {err}"));
        assert_eq!(
            normalized, expected_normalized,
            "row {index} normalize: {text:?}"
        );
        let sentences = split_sentences(&normalized);
        let expected = row["sentences"].as_array().unwrap();
        assert_eq!(
            sentences.len(),
            expected.len(),
            "row {index} sentence count"
        );
        for (sentence, gold) in sentences.iter().zip(expected) {
            let clean = clean_sentence_for_g2p(sentence);
            assert_eq!(
                clean,
                gold["clean"].as_str().unwrap(),
                "row {index} clean: {sentence:?}"
            );
            let raw = g2p
                .phonemise(&clean)
                .unwrap_or_else(|err| panic!("row {index} phonemise failed: {err}"));
            assert_eq!(
                raw,
                gold["g2p_raw"].as_str().unwrap(),
                "row {index} g2p: {clean:?}"
            );
            assert_eq!(
                translate_phonemes(&raw),
                gold["phonemes"].as_str().unwrap(),
                "row {index} translate: {raw:?}"
            );
        }
    }
}
