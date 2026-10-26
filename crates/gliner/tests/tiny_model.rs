//! End-to-end run on ONNX Runtime with the tiny synthetic model in
//! `fixtures/tiny` (see `make_tiny.py`). Needs `ORT_DYLIB_PATH` pointing at an
//! ONNX Runtime library (CI installs one on Linux); without it the test only
//! checks the checksum guard, which does not need the runtime.

use detect::ner::NerModel;
use gliner::{Gliner, GlinerError, ModelFiles, Options};
use std::path::PathBuf;

const MODEL_SHA: &str = "b2a4ebac5ac17c181595332bcd1b2686f3567bc2d6fc279f52a849d9da489c37";
const TOKENIZER_SHA: &str = "f6af972c363bf02367a5af96843443e48bc4c497554bf95ea0419be6b11d086f";

fn files() -> ModelFiles {
    ModelFiles {
        dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tiny"),
        model_sha256: MODEL_SHA.into(),
        tokenizer_sha256: TOKENIZER_SHA.into(),
        onnxruntime: std::env::var_os("ORT_DYLIB_PATH").map(PathBuf::from),
    }
}

#[test]
fn wrong_checksum_is_refused_before_loading() {
    let mut f = files();
    f.model_sha256 = "00".repeat(32);
    match Gliner::load(&f, Options::default()) {
        Err(GlinerError::Checksum { actual, .. }) => assert_eq!(actual, MODEL_SHA),
        other => panic!("expected checksum error, got {other:?}"),
    }
}

#[test]
fn runs_on_onnx_runtime() {
    if std::env::var_os("ORT_DYLIB_PATH").is_none() {
        eprintln!("ORT_DYLIB_PATH not set; skipping the ONNX Runtime run");
        return;
    }
    let m = Gliner::load(&files(), Options::default()).expect("load");
    let text = "Hallo Anna, Weber ist krank. Anna.";
    let spans = m.predict(text, &["person", "health condition"]).expect("predict");
    let got: Vec<(&str, &str)> = spans.iter().map(|s| (&text[s.start..s.end], s.label.as_str())).collect();
    assert_eq!(got, [("Anna", "person"), ("Anna", "person")]);
    assert!(spans.iter().all(|s| s.score > 0.99));

    // Windows: the same result when the text is cut into 3-word windows.
    let small = Gliner::load(&files(), Options { window_words: 3, ..Options::default() }).expect("load");
    let spans2 = small.predict(text, &["person", "health condition"]).expect("predict");
    assert_eq!(spans2, spans);
    assert!(m.predict("", &["person"]).unwrap().is_empty());
}
