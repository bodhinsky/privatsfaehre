//! Pluggable NER backend.
//!
//! The engine treats a NER model as just another [`Recognizer`] via
//! [`NerRecognizer`]. Backends implement [`NerModel`]; the recommended one is
//! GLiNER (multilingual PII checkpoint) running on ONNX Runtime through the
//! `ort` crate, see `ARCHITECTURE.md`. No backend is compiled in yet, so the
//! crate stays dependency-light and the default build has no native code.
//!
//! Model files are never downloaded by this program. Users place a model file
//! on disk and point the configuration at it; its SHA-256 is pinned.

use crate::{DetectError, Detection, EntityType, Recognizer};

/// One span predicted by a model. Offsets are byte offsets into the input.
#[derive(Debug, Clone, PartialEq)]
pub struct NerSpan {
    pub start: usize,
    pub end: usize,
    /// Model label, e.g. `"person"` (GLiNER) or `"PER"` (BERT token classifier).
    pub label: String,
    pub score: f32,
}

/// A local NER model.
///
/// `labels` are the label prompts for zero-shot models such as GLiNER;
/// closed-label models (BERT) may ignore them and return their own labels.
pub trait NerModel: Send + Sync {
    fn id(&self) -> &str;
    fn predict(&self, text: &str, labels: &[&str]) -> Result<Vec<NerSpan>, String>;
}

/// Maps model labels to entity types and applies a per-label minimum score.
#[derive(Debug, Clone)]
pub struct LabelMapping {
    pub label: String,
    pub entity: EntityType,
    /// Predictions below this are dropped before the engine sees them.
    pub min_score: f32,
}

impl LabelMapping {
    pub fn new(label: impl Into<String>, entity: EntityType, min_score: f32) -> Self {
        Self { label: label.into(), entity, min_score }
    }
}

/// Default GLiNER label prompts. Special-category prompts provide a second,
/// semantic line of defence behind the Art. 9/10 lexicons.
pub fn default_gliner_labels() -> Vec<LabelMapping> {
    use EntityType::*;
    vec![
        LabelMapping::new("person", Person, 0.3),
        LabelMapping::new("organization", Organization, 0.4),
        LabelMapping::new("location", Location, 0.4),
        LabelMapping::new("address", Address, 0.4),
        LabelMapping::new("health condition", Health, 0.3),
        LabelMapping::new("religion", Religion, 0.3),
        LabelMapping::new("sexual orientation", SexualOrientation, 0.3),
        LabelMapping::new("trade union", UnionMembership, 0.3),
        LabelMapping::new("ethnicity", Ethnicity, 0.3),
        LabelMapping::new("political party", PoliticalOpinion, 0.4),
        LabelMapping::new("criminal offence", CriminalRecord, 0.3),
    ]
}

/// Adapter that turns any [`NerModel`] into a [`Recognizer`].
#[derive(Debug)]
pub struct NerRecognizer<M> {
    model: M,
    id: String,
    mapping: Vec<LabelMapping>,
    entities: Vec<EntityType>,
}

impl<M: NerModel> NerRecognizer<M> {
    pub fn new(model: M, mapping: Vec<LabelMapping>) -> Self {
        let id = format!("ner.{}", model.id());
        let mut entities: Vec<EntityType> = mapping.iter().map(|m| m.entity).collect();
        entities.sort();
        entities.dedup();
        Self { model, id, mapping, entities }
    }
}

impl<M: NerModel> Recognizer for NerRecognizer<M> {
    fn id(&self) -> &str {
        &self.id
    }
    fn entities(&self) -> &[EntityType] {
        &self.entities
    }
    fn detect(&self, text: &str, out: &mut Vec<Detection>) -> Result<(), DetectError> {
        let labels: Vec<&str> = self.mapping.iter().map(|m| m.label.as_str()).collect();
        let spans = self
            .model
            .predict(text, &labels)
            .map_err(|message| DetectError::Ner { model: self.model.id().to_owned(), message })?;
        for s in spans {
            let valid = s.start < s.end
                && s.end <= text.len()
                && text.is_char_boundary(s.start)
                && text.is_char_boundary(s.end);
            if !valid {
                return Err(DetectError::Ner {
                    model: self.model.id().to_owned(),
                    message: format!("invalid span {}..{}", s.start, s.end),
                });
            }
            let Some(m) = self.mapping.iter().find(|m| m.label.eq_ignore_ascii_case(&s.label)) else { continue };
            if s.score >= m.min_score {
                out.push(Detection::new(s.start, s.end, m.entity, s.score, self.id.clone()));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(Result<Vec<NerSpan>, String>);
    impl NerModel for Fixed {
        fn id(&self) -> &str {
            "fixed"
        }
        fn predict(&self, _: &str, labels: &[&str]) -> Result<Vec<NerSpan>, String> {
            assert!(labels.contains(&"person"));
            self.0.clone()
        }
    }

    fn span(start: usize, end: usize, label: &str, score: f32) -> NerSpan {
        NerSpan { start, end, label: label.into(), score }
    }

    #[test]
    fn maps_labels_and_thresholds() {
        let text = "anna war beim arzt";
        let r = NerRecognizer::new(
            Fixed(Ok(vec![span(0, 4, "person", 0.7), span(14, 18, "location", 0.1), span(0, 4, "unknown", 0.9)])),
            default_gliner_labels(),
        );
        let mut out = Vec::new();
        r.detect(text, &mut out).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].entity, EntityType::Person);
        assert_eq!(out[0].recognizer, "ner.fixed");
    }

    #[test]
    fn model_errors_and_bad_spans_fail_closed() {
        let r = NerRecognizer::new(Fixed(Err("oom".into())), default_gliner_labels());
        assert!(r.detect("x", &mut Vec::new()).is_err());
        let r = NerRecognizer::new(Fixed(Ok(vec![span(0, 99, "person", 0.9)])), default_gliner_labels());
        assert!(r.detect("short", &mut Vec::new()).is_err());
    }
}
