//! `privatsphaere.toml`, shared by the CLI and the app.
//!
//! ```toml
//! [detect]
//! default_threshold = 0.5
//! allowlist = ["support@example.com"]
//! denylist = [{ text = "Projekt Falke", entity = "ORGANIZATION" }]
//! [detect.thresholds]
//! DATE = 0.8
//!
//! [ner]                       # optional, needs the `ner` feature
//! model_dir = "/opt/models/gliner_multi_pii-v1"
//! model_sha256 = "…"
//! tokenizer_sha256 = "…"
//! onnxruntime = "/usr/lib/libonnxruntime.so"   # optional
//! ```

use detect::{Engine, EngineConfig, EntityType};
use serde::Deserialize;
use std::path::{Path, PathBuf};

type Error = Box<dyn std::error::Error>;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    #[serde(default)]
    pub detect: EngineConfig,
    pub ner: Option<NerConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NerConfig {
    /// Directory with `model.onnx`, `tokenizer.json`, `gliner_config.json`.
    pub model_dir: PathBuf,
    pub model_sha256: String,
    pub tokenizer_sha256: String,
    /// ONNX Runtime shared library; default `ORT_DYLIB_PATH` or the loader path.
    pub onnxruntime: Option<PathBuf>,
    #[serde(default = "default_threshold")]
    pub threshold: f32,
    #[serde(default = "default_window")]
    pub window_words: usize,
    #[serde(default = "default_threads")]
    pub threads: usize,
    /// Replaces the default prompts for non-special entities. Prompts for
    /// GDPR special categories are fixed and always added.
    pub labels: Option<Vec<NerLabel>>,
}

fn default_threshold() -> f32 {
    0.3
}
fn default_window() -> usize {
    256
}
fn default_threads() -> usize {
    2
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NerLabel {
    pub label: String,
    pub entity: EntityType,
    pub min_score: f32,
}

/// Label prompts: configured (or default) ones for ordinary entities, plus
/// the fixed defaults for special categories.
pub fn ner_labels(cfg: &NerConfig) -> Result<Vec<detect::ner::LabelMapping>, Error> {
    let defaults = detect::ner::default_gliner_labels();
    let Some(custom) = &cfg.labels else { return Ok(defaults) };
    let mut out = Vec::new();
    for l in custom {
        if l.entity.is_special_category() {
            return Err(format!(
                "[ner] labels: {} is a GDPR special category; its prompts are fixed and cannot be configured",
                l.entity
            )
            .into());
        }
        if !(0.0..=1.0).contains(&l.min_score) {
            return Err(format!("[ner] labels: min_score for `{}` must be in [0, 1]", l.label).into());
        }
        out.push(detect::ner::LabelMapping::new(l.label.clone(), l.entity, l.min_score));
    }
    out.extend(defaults.into_iter().filter(|m| m.entity.is_special_category()));
    Ok(out)
}

pub fn read(path: &Path) -> Result<FileConfig, Error> {
    let s = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(toml::from_str::<FileConfig>(&s).map_err(|e| format!("{}: {e}", path.display()))?)
}

/// Engine from a config file (defaults without one). Loads the NER model if
/// `[ner]` is present; a model that cannot be loaded is an error, never a
/// silent fallback to rules only.
pub fn engine(path: Option<&Path>) -> Result<Engine, Error> {
    let cfg = match path {
        Some(p) => read(p)?,
        None => FileConfig::default(),
    };
    let mut engine = Engine::new(cfg.detect)?;
    if let Some(ner) = &cfg.ner {
        engine.add_recognizer(ner_recognizer(ner)?);
    }
    Ok(engine)
}

#[cfg(feature = "ner")]
fn ner_recognizer(cfg: &NerConfig) -> Result<Box<dyn detect::Recognizer>, Error> {
    let mapping = ner_labels(cfg)?;
    let files = gliner::ModelFiles {
        dir: cfg.model_dir.clone(),
        model_sha256: cfg.model_sha256.clone(),
        tokenizer_sha256: cfg.tokenizer_sha256.clone(),
        onnxruntime: cfg.onnxruntime.clone(),
    };
    let options = gliner::Options { threshold: cfg.threshold, window_words: cfg.window_words, threads: cfg.threads };
    let model = gliner::Gliner::load(&files, options)?;
    Ok(Box::new(detect::ner::NerRecognizer::new(model, mapping)))
}

#[cfg(not(feature = "ner"))]
fn ner_recognizer(_: &NerConfig) -> Result<Box<dyn detect::Recognizer>, Error> {
    Err("[ner] is configured, but this build has no NER support (enable the `ner` feature)".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_example_file() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../privatsphaere.example.toml");
        let c = read(&path).unwrap();
        assert_eq!(c.detect.thresholds[&EntityType::Date], 0.8);
        assert!(c.ner.is_none());
        assert!(Engine::new(c.detect).is_ok());
        let bad: FileConfig = toml::from_str("[detect.thresholds]\nHEALTH = 0.9").unwrap();
        assert!(Engine::new(bad.detect).is_err());
    }

    #[test]
    fn special_category_prompts_are_fixed() {
        let base = "[ner]\nmodel_dir = \"m\"\nmodel_sha256 = \"a\"\ntokenizer_sha256 = \"b\"\n";
        let ok: FileConfig =
            toml::from_str(&format!("{base}labels = [{{ label = \"name\", entity = \"PERSON\", min_score = 0.4 }}]"))
                .unwrap();
        let labels = ner_labels(ok.ner.as_ref().unwrap()).unwrap();
        assert_eq!(labels[0].label, "name");
        assert!(labels.iter().any(|l| l.entity == EntityType::Health), "special prompts always present");
        assert!(!labels.iter().any(|l| l.label == "organization"), "custom list replaces ordinary defaults");

        let bad: FileConfig = toml::from_str(&format!(
            "{base}labels = [{{ label = \"illness\", entity = \"HEALTH\", min_score = 0.99 }}]"
        ))
        .unwrap();
        assert!(ner_labels(bad.ner.as_ref().unwrap()).is_err());
    }

    #[test]
    fn missing_model_is_an_error() {
        let dir = std::env::temp_dir().join(format!("privatsphaere-ner-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("c.toml");
        std::fs::write(&p, "[ner]\nmodel_dir = \"/nonexistent\"\nmodel_sha256 = \"a\"\ntokenizer_sha256 = \"b\"\n")
            .unwrap();
        assert!(engine(Some(&p)).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
