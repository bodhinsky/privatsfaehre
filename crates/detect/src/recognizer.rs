use crate::EntityType;
use serde::Serialize;
use std::borrow::Cow;

/// A candidate span proposed by a recognizer. Offsets are byte offsets.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Detection {
    pub start: usize,
    pub end: usize,
    pub entity: EntityType,
    /// Confidence in `[0, 1]`. Checksum-validated matches score close to 1.
    pub score: f32,
    /// Fine-grained id of the rule that fired, e.g. `iban.checksum`.
    pub recognizer: Cow<'static, str>,
}

impl Detection {
    pub fn new(
        start: usize,
        end: usize,
        entity: EntityType,
        score: f32,
        recognizer: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self { start, end, entity, score: score.clamp(0.0, 1.0), recognizer: recognizer.into() }
    }

    /// The covered slice of `haystack`.
    pub fn text<'a>(&self, haystack: &'a str) -> &'a str {
        &haystack[self.start..self.end]
    }

    pub fn overlaps(&self, other: &Detection) -> bool {
        self.start < other.end && other.start < self.end
    }

    pub fn len(&self) -> usize {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

/// Errors from detection. Callers must treat any error as "fail closed":
/// the clipboard content is not released.
#[derive(Debug, thiserror::Error)]
pub enum DetectError {
    #[error("NER model `{model}` failed: {message}")]
    Ner { model: String, message: String },
    #[error("invalid configuration: {0}")]
    Config(String),
}

/// A detector for one family of entities.
///
/// Implementations must be deterministic and must not perform I/O beyond
/// reading model files they were configured with.
pub trait Recognizer: Send + Sync {
    /// Stable group id (e.g. `secrets`). Individual detections carry a finer id.
    fn id(&self) -> &str;
    /// Entity types this recognizer can emit.
    fn entities(&self) -> &[EntityType];
    /// Append candidate detections for `text` to `out`.
    fn detect(&self, text: &str, out: &mut Vec<Detection>) -> Result<(), DetectError>;
}
