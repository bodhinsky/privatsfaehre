use crate::recognizers::builtin;
use crate::{DetectError, Detection, EntityType, Recognizer};
use regex::{Regex, RegexSet};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fmt;

/// Fixed score at or above which a special-category detection blocks the
/// paste. Deliberately a constant, not part of [`EngineConfig`].
pub const SPECIAL_CATEGORY_THRESHOLD: f32 = 0.5;

/// Recognizer group that can never be disabled.
const SPECIAL_RECOGNIZER_ID: &str = "special";

/// A term that must always be redacted (e.g. a client's name).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DenyEntry {
    pub text: String,
    pub entity: EntityType,
}

/// User configuration. Special categories are not configurable: setting a
/// threshold for them, allow-listing them or disabling their recognizer is a
/// configuration error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EngineConfig {
    /// Minimum score for redaction when no per-type threshold is set.
    pub default_threshold: f32,
    /// Per-type thresholds (redact types only).
    pub thresholds: BTreeMap<EntityType, f32>,
    /// Span texts that are never redacted (case-insensitive, exact).
    pub allowlist: Vec<String>,
    /// Regexes; a span whose full text matches one is never redacted.
    pub allow_patterns: Vec<String>,
    /// Terms that are always redacted.
    pub denylist: Vec<DenyEntry>,
    /// Recognizer group ids to skip (see [`Recognizer::id`]).
    pub disabled_recognizers: Vec<String>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            default_threshold: 0.5,
            thresholds: BTreeMap::new(),
            allowlist: Vec::new(),
            allow_patterns: Vec::new(),
            denylist: Vec::new(),
            disabled_recognizers: Vec::new(),
        }
    }
}

/// Overall decision for a piece of text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Nothing found; release unchanged.
    Clean,
    /// Release after replacing [`Analysis::detections`] with placeholders.
    Redact,
    /// Special-category data found; do not release.
    Block,
}

/// Result of [`Engine::analyze`].
#[derive(Debug, Clone, Default, Serialize)]
pub struct Analysis {
    /// Non-overlapping spans to redact, sorted by start.
    pub detections: Vec<Detection>,
    /// Special-category detections; any entry blocks the paste.
    pub blocking: Vec<Detection>,
}

impl Analysis {
    pub fn verdict(&self) -> Verdict {
        if !self.blocking.is_empty() {
            Verdict::Block
        } else if self.detections.is_empty() {
            Verdict::Clean
        } else {
            Verdict::Redact
        }
    }
}

/// Runs all recognizers and turns candidates into a decision.
pub struct Engine {
    recognizers: Vec<Box<dyn Recognizer>>,
    config: EngineConfig,
    allow_exact: HashSet<String>,
    allow_patterns: Option<RegexSet>,
    deny: Vec<(Regex, EntityType)>,
}

impl fmt::Debug for Engine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Engine")
            .field("recognizers", &self.recognizers.iter().map(|r| r.id()).collect::<Vec<_>>())
            .field("config", &self.config)
            .finish()
    }
}

impl Engine {
    /// Engine with all built-in recognizers.
    pub fn new(config: EngineConfig) -> Result<Self, DetectError> {
        Self::with_recognizers(builtin(), config)
    }

    pub fn with_recognizers(recognizers: Vec<Box<dyn Recognizer>>, config: EngineConfig) -> Result<Self, DetectError> {
        let cfg_err = |m: String| Err(DetectError::Config(m));
        if !(0.0..=1.0).contains(&config.default_threshold) {
            return cfg_err(format!("default_threshold {} not in [0, 1]", config.default_threshold));
        }
        for (t, v) in &config.thresholds {
            if t.is_special_category() {
                return cfg_err(format!("{t} is a GDPR special category; its threshold is fixed"));
            }
            if !(0.0..=1.0).contains(v) {
                return cfg_err(format!("threshold for {t} not in [0, 1]"));
            }
        }
        if config.disabled_recognizers.iter().any(|r| r == SPECIAL_RECOGNIZER_ID) {
            return cfg_err("the special-category recognizer cannot be disabled".into());
        }
        let allow_patterns = if config.allow_patterns.is_empty() {
            None
        } else {
            let anchored: Vec<_> = config.allow_patterns.iter().map(|p| format!("^(?:{p})$")).collect();
            Some(RegexSet::new(anchored).map_err(|e| DetectError::Config(format!("allow_patterns: {e}")))?)
        };
        let mut deny = Vec::new();
        for d in &config.denylist {
            let term = d.text.trim();
            if term.is_empty() {
                continue;
            }
            let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
            let pre = if word(term.chars().next()) { r"\b" } else { "" };
            let post = if word(term.chars().next_back()) { r"\b" } else { "" };
            let re = Regex::new(&format!("(?i){pre}{}{post}", regex::escape(term).replace(' ', r"\s+")))
                .map_err(|e| DetectError::Config(format!("denylist: {e}")))?;
            deny.push((re, d.entity));
        }
        let allow_exact = config.allowlist.iter().map(|s| s.trim().to_lowercase()).collect();
        Ok(Self { recognizers, config, allow_exact, allow_patterns, deny })
    }

    pub fn add_recognizer(&mut self, r: Box<dyn Recognizer>) {
        self.recognizers.push(r);
    }

    pub fn recognizers(&self) -> impl Iterator<Item = &dyn Recognizer> {
        self.recognizers.iter().map(|r| r.as_ref())
    }

    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// Score needed for `entity` to count as detected.
    pub fn threshold(&self, entity: EntityType) -> f32 {
        if entity.is_special_category() {
            SPECIAL_CATEGORY_THRESHOLD
        } else {
            self.config.thresholds.get(&entity).copied().unwrap_or(self.config.default_threshold)
        }
    }

    /// Every candidate from every enabled recognizer, regardless of score,
    /// sorted by position. Identical `(start, end, entity)` triples are merged,
    /// keeping the highest score. This is what `label` mode shows.
    pub fn candidates(&self, text: &str) -> Result<Vec<Detection>, DetectError> {
        let mut out = Vec::new();
        for r in &self.recognizers {
            if self.config.disabled_recognizers.iter().any(|d| d == r.id()) {
                continue;
            }
            r.detect(text, &mut out)?;
        }
        for (re, entity) in &self.deny {
            for m in re.find_iter(text) {
                out.push(Detection::new(m.start(), m.end(), *entity, 1.0, "denylist"));
            }
        }
        out.retain(|d| d.start < d.end && d.end <= text.len());
        out.sort_by(|a, b| {
            (a.start, b.end, a.entity).cmp(&(b.start, a.end, b.entity)).then(b.score.total_cmp(&a.score))
        });
        out.dedup_by(|later, kept| later.start == kept.start && later.end == kept.end && later.entity == kept.entity);
        Ok(out)
    }

    fn allowed(&self, d: &Detection, text: &str) -> bool {
        if d.entity.is_special_category() || d.recognizer == "denylist" {
            return false;
        }
        let s = d.text(text).trim();
        self.allow_exact.contains(&s.to_lowercase()) || self.allow_patterns.as_ref().is_some_and(|p| p.is_match(s))
    }

    /// Thresholded, allow-listed and overlap-resolved result.
    ///
    /// Overlaps between redactions are resolved greedily by score, then by
    /// length. Blocking detections are reported separately and never removed.
    pub fn analyze(&self, text: &str) -> Result<Analysis, DetectError> {
        let mut blocking = Vec::new();
        let mut redact = Vec::new();
        for d in self.candidates(text)? {
            if d.score < self.threshold(d.entity) || self.allowed(&d, text) {
                continue;
            }
            if d.entity.is_special_category() { blocking.push(d) } else { redact.push(d) }
        }
        redact.sort_by(|a, b| b.score.total_cmp(&a.score).then(b.len().cmp(&a.len())).then(a.start.cmp(&b.start)));
        let mut chosen: Vec<Detection> = Vec::with_capacity(redact.len());
        for d in redact {
            if !chosen.iter().any(|c| c.overlaps(&d)) {
                chosen.push(d);
            }
        }
        chosen.sort_by_key(|d| d.start);
        Ok(Analysis { detections: chosen, blocking })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Engine {
        Engine::new(EngineConfig::default()).unwrap()
    }

    fn summary(a: &Analysis, text: &str) -> Vec<(String, EntityType)> {
        a.detections.iter().map(|d| (d.text(text).to_owned(), d.entity)).collect()
    }

    #[test]
    fn mixed_text() {
        let text = "Hallo Frau Schneider, bitte überweisen Sie an DE89 3704 0044 0532 0130 00. \
                    Rückfragen: anna.schneider@example.de oder 0171 1234567.";
        let a = engine().analyze(text).unwrap();
        assert_eq!(a.verdict(), Verdict::Redact);
        assert_eq!(
            summary(&a, text),
            [
                ("Schneider".to_owned(), EntityType::Person),
                ("DE89 3704 0044 0532 0130 00".to_owned(), EntityType::Iban),
                ("anna.schneider@example.de".to_owned(), EntityType::Email),
                ("0171 1234567".to_owned(), EntityType::Phone),
            ]
        );
    }

    #[test]
    fn special_category_blocks_and_ignores_allowlist() {
        let cfg = EngineConfig { allowlist: vec!["krankgeschrieben".into()], ..Default::default() };
        let a = Engine::new(cfg).unwrap().analyze("Herr Weber ist krankgeschrieben.").unwrap();
        assert_eq!(a.verdict(), Verdict::Block);
    }

    #[test]
    fn special_category_not_configurable() {
        let mut cfg = EngineConfig::default();
        cfg.thresholds.insert(EntityType::Health, 0.99);
        assert!(Engine::new(cfg).is_err());
        let cfg = EngineConfig { disabled_recognizers: vec!["special".into()], ..Default::default() };
        assert!(Engine::new(cfg).is_err());
    }

    #[test]
    fn allow_and_deny_lists() {
        let cfg = EngineConfig {
            allowlist: vec!["support@example.com".into()],
            denylist: vec![DenyEntry { text: "Projekt Falke".into(), entity: EntityType::Organization }],
            ..Default::default()
        };
        let text = "Schreib an support@example.com wegen projekt falke.";
        let a = Engine::new(cfg).unwrap().analyze(text).unwrap();
        assert_eq!(summary(&a, text), [("projekt falke".to_owned(), EntityType::Organization)]);
    }

    #[test]
    fn candidates_include_low_scores_and_are_deduplicated() {
        let e = engine();
        let text = "Referenz DE12 3456 7890 1234 5678 90";
        let c = e.candidates(text).unwrap();
        assert!(c.iter().any(|d| d.entity == EntityType::Iban && d.score < 0.5));
        assert!(e.analyze(text).unwrap().detections.iter().all(|d| d.entity != EntityType::Iban));
        let mut seen = HashSet::new();
        assert!(c.iter().all(|d| seen.insert((d.start, d.end, d.entity))));
    }

    #[test]
    fn clean_text() {
        let a = engine().analyze("Das Meeting ist verschoben, die Agenda folgt.").unwrap();
        assert_eq!(a.verdict(), Verdict::Clean);
    }
}
