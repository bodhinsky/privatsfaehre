//! GLiNER span-based NER on ONNX Runtime, implementing [`detect::ner::NerModel`].
//!
//! Everything runs locally:
//!
//! * ONNX Runtime is loaded at run time from a shared library the user
//!   installed (`ort` with `load-dynamic`); nothing is downloaded at build or
//!   run time.
//! * The model directory (`model.onnx`, `tokenizer.json`,
//!   `gliner_config.json`) is placed by the user. The SHA-256 of `model.onnx`
//!   and `tokenizer.json` must match the pinned values before anything is
//!   loaded.
//!
//! Pre- and post-processing follow the reference implementation of GLiNER
//! (span mode): the text is split into words, prefixed with a prompt of
//! `<<ENT>> label` pairs and `<<SEP>>`, the model scores every span of up to
//! `max_width` words for every label, and overlapping spans are resolved
//! greedily by score.

mod hash;
mod prep;

pub use hash::sha256_file;
pub use prep::{Word, split_words};

use detect::ner::{NerModel, NerSpan};
use ort::session::Session;
use ort::value::Tensor;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use tokenizers::Tokenizer;

#[derive(Debug, thiserror::Error)]
pub enum GlinerError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{path}: SHA-256 is {actual}, configuration pins {expected}")]
    Checksum { path: PathBuf, expected: String, actual: String },
    #[error("ONNX Runtime: {0}")]
    Runtime(String),
    #[error("tokenizer: {0}")]
    Tokenizer(String),
    #[error("gliner_config.json: {0}")]
    Config(String),
    #[error("model output: {0}")]
    Output(String),
}

/// Where the model is and what it must hash to.
#[derive(Debug, Clone)]
pub struct ModelFiles {
    /// Directory with `model.onnx`, `tokenizer.json`, `gliner_config.json`.
    pub dir: PathBuf,
    /// Lower-case hex SHA-256 of `model.onnx`.
    pub model_sha256: String,
    /// Lower-case hex SHA-256 of `tokenizer.json`.
    pub tokenizer_sha256: String,
    /// ONNX Runtime library. `None`: `ORT_DYLIB_PATH`, else the platform's
    /// default library name on the loader path.
    pub onnxruntime: Option<PathBuf>,
}

/// Tunables that are not part of the model.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Spans below this score are dropped before label mapping.
    pub threshold: f32,
    /// Words per model call; longer texts are processed in overlapping windows.
    pub window_words: usize,
    pub threads: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self { threshold: 0.3, window_words: 256, threads: 2 }
    }
}

#[derive(Debug, Clone)]
struct ModelConfig {
    max_width: usize,
    ent_token: String,
    sep_token: String,
}

fn read_config(path: &Path) -> Result<ModelConfig, GlinerError> {
    let s = std::fs::read_to_string(path).map_err(|source| GlinerError::Io { path: path.into(), source })?;
    let v: serde_json::Value = serde_json::from_str(&s).map_err(|e| GlinerError::Config(e.to_string()))?;
    let str_field = |k: &str, d: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or(d).to_owned();
    if let Some(mode) = v.get("span_mode").and_then(|x| x.as_str())
        && mode != "markerV0"
    {
        return Err(GlinerError::Config(format!("span_mode `{mode}` is not supported (only markerV0)")));
    }
    let max_width = v.get("max_width").and_then(|x| x.as_u64()).unwrap_or(12) as usize;
    if !(1..=64).contains(&max_width) {
        return Err(GlinerError::Config(format!("max_width {max_width} out of range")));
    }
    Ok(ModelConfig {
        max_width,
        ent_token: str_field("ent_token", "<<ENT>>"),
        sep_token: str_field("sep_token", "<<SEP>>"),
    })
}

static ENVIRONMENT: OnceLock<Result<ort::environment::Environment, String>> = OnceLock::new();

fn environment(lib: Option<&Path>) -> Result<ort::environment::Environment, GlinerError> {
    ENVIRONMENT
        .get_or_init(|| {
            let lib: PathBuf = match lib {
                Some(p) => p.to_owned(),
                None => match std::env::var_os("ORT_DYLIB_PATH") {
                    Some(p) if !p.is_empty() => p.into(),
                    _ => format!("{}onnxruntime{}", std::env::consts::DLL_PREFIX, std::env::consts::DLL_SUFFIX).into(),
                },
            };
            let builder = ort::init_from(&lib).map_err(|e| format!("{}: {e}", lib.display()))?;
            builder.with_name("privatsphaere").build().map_err(|e| e.to_string())
        })
        .clone()
        .map_err(GlinerError::Runtime)
}

/// A loaded GLiNER model.
pub struct Gliner {
    id: String,
    session: Mutex<Session>,
    tokenizer: Tokenizer,
    config: ModelConfig,
    options: Options,
}

impl std::fmt::Debug for Gliner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gliner").field("id", &self.id).field("max_width", &self.config.max_width).finish()
    }
}

fn verify(path: &Path, expected: &str) -> Result<(), GlinerError> {
    let actual = sha256_file(path).map_err(|source| GlinerError::Io { path: path.into(), source })?;
    if !actual.eq_ignore_ascii_case(expected.trim()) {
        return Err(GlinerError::Checksum { path: path.into(), expected: expected.trim().to_owned(), actual });
    }
    Ok(())
}

impl Gliner {
    /// Verify checksums, load ONNX Runtime, the tokenizer and the model.
    pub fn load(files: &ModelFiles, options: Options) -> Result<Self, GlinerError> {
        let model = files.dir.join("model.onnx");
        let tok = files.dir.join("tokenizer.json");
        verify(&model, &files.model_sha256)?;
        verify(&tok, &files.tokenizer_sha256)?;
        let config = read_config(&files.dir.join("gliner_config.json"))?;
        let tokenizer = Tokenizer::from_file(&tok).map_err(|e| GlinerError::Tokenizer(e.to_string()))?;
        for t in [&config.ent_token, &config.sep_token] {
            if tokenizer.token_to_id(t).is_none() {
                return Err(GlinerError::Tokenizer(format!("special token `{t}` missing from tokenizer.json")));
            }
        }
        let env = environment(files.onnxruntime.as_deref())?;
        let rt = |e: ort::Error| GlinerError::Runtime(e.to_string());
        let session = Session::builder(&env)
            .map_err(rt)?
            .with_intra_threads(options.threads.max(1))
            .map_err(|e| GlinerError::Runtime(e.to_string()))?
            .commit_from_file(&model)
            .map_err(rt)?;
        let id = files.dir.file_name().and_then(|n| n.to_str()).unwrap_or("gliner").to_owned();
        Ok(Self { id: format!("gliner.{id}"), session: Mutex::new(session), tokenizer, config, options })
    }

    fn run_window(&self, text: &str, words: &[Word], labels: &[&str]) -> Result<Vec<NerSpan>, GlinerError> {
        let input =
            prep::build_input(&self.tokenizer, words, text, labels, &self.config.ent_token, &self.config.sep_token)
                .map_err(GlinerError::Tokenizer)?;
        let n_tok = input.input_ids.len();
        let n_words = words.len();
        let w = self.config.max_width;
        let (span_idx, span_mask) = prep::spans(n_words, w);
        let rt = |e: ort::Error| GlinerError::Runtime(e.to_string());
        let t_i64 = |shape: Vec<i64>, v: Vec<i64>| Tensor::from_array((shape, v)).map_err(rt);
        let inputs = ort::inputs![
            "input_ids" => t_i64(vec![1, n_tok as i64], input.input_ids)?,
            "attention_mask" => t_i64(vec![1, n_tok as i64], input.attention_mask)?,
            "words_mask" => t_i64(vec![1, n_tok as i64], input.words_mask)?,
            "text_lengths" => t_i64(vec![1, 1], vec![n_words as i64])?,
            "span_idx" => t_i64(vec![1, (n_words * w) as i64, 2], span_idx)?,
            "span_mask" => Tensor::from_array((vec![1, (n_words * w) as i64], span_mask)).map_err(rt)?,
        ];
        let mut session = self.session.lock().map_err(|_| GlinerError::Runtime("session lock poisoned".into()))?;
        let outputs = session.run(inputs).map_err(rt)?;
        let logits = outputs.get("logits").ok_or_else(|| GlinerError::Output("no `logits` output".into()))?;
        let (shape, data) = logits.try_extract_tensor::<f32>().map_err(rt)?;
        let dims: Vec<i64> = shape.iter().copied().collect();
        if dims.len() != 4 || dims[0] != 1 || dims[3] as usize != labels.len() || dims[2] as usize != w {
            return Err(GlinerError::Output(format!(
                "logits shape {dims:?}, expected [1, {n_words}, {w}, {}]",
                labels.len()
            )));
        }
        let scored = prep::decode(data, dims[1] as usize, w, labels.len(), n_words, self.options.threshold);
        Ok(scored
            .into_iter()
            .map(|s| NerSpan {
                start: words[s.first].start,
                end: words[s.last].end,
                label: labels[s.label].to_owned(),
                score: s.score,
            })
            .collect())
    }
}

impl NerModel for Gliner {
    fn id(&self) -> &str {
        &self.id
    }

    fn predict(&self, text: &str, labels: &[&str]) -> Result<Vec<NerSpan>, String> {
        let words = split_words(text);
        if words.is_empty() || labels.is_empty() {
            return Ok(Vec::new());
        }
        let mut all = Vec::new();
        for range in prep::windows(words.len(), self.options.window_words, self.config.max_width) {
            all.extend(self.run_window(text, &words[range], labels).map_err(|e| e.to_string())?);
        }
        Ok(prep::flat(all))
    }
}
