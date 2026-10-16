//! Precision / recall / F1 of the engine against the labeled dataset.
//!
//! Matching:
//! * **overlap**: a prediction matches a gold span of the same type if they
//!   overlap (one-to-one, greedy by overlap length). This is the primary
//!   metric for a redaction tool: a partially covered name is still mostly
//!   protected, while a missed one is not.
//! * **strict**: same type and identical boundaries.
//!
//! An unmatched prediction counts as a false positive only if the record is
//! marked `complete`, or if it overlaps an explicitly `rejected` span.
//!
//! Special categories are additionally evaluated at document level: was a
//! record that contains Art. 9/10 data blocked?

use crate::{Record, Verdict, byte_to_char};
use detect::{DetectError, Detection, Engine, EntityType};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// True positives, false positives, false negatives.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub tp: usize,
    pub fp: usize,
    #[serde(rename = "fn")]
    pub fn_: usize,
}

impl Counts {
    pub fn precision(&self) -> Option<f64> {
        let d = self.tp + self.fp;
        (d > 0).then(|| self.tp as f64 / d as f64)
    }
    pub fn recall(&self) -> Option<f64> {
        let d = self.tp + self.fn_;
        (d > 0).then(|| self.tp as f64 / d as f64)
    }
    pub fn f1(&self) -> Option<f64> {
        let (p, r) = (self.precision()?, self.recall()?);
        Some(if p + r == 0.0 { 0.0 } else { 2.0 * p * r / (p + r) })
    }
    fn add(&mut self, o: Counts) {
        self.tp += o.tp;
        self.fp += o.fp;
        self.fn_ += o.fn_;
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EntityMetrics {
    /// Number of gold spans of this type.
    pub gold: usize,
    pub overlap: Counts,
    pub strict: Counts,
}

/// Per fine-grained recognizer id (e.g. `iban.checksum`), using raw
/// candidates at or above the engine threshold, before overlap resolution.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RecognizerMetrics {
    pub proposed: usize,
    /// Proposals overlapping a gold span of the same type.
    pub correct: usize,
    /// Proposals counted as false positives.
    pub false_positives: usize,
    /// Gold spans this recognizer found.
    pub gold_found: usize,
    /// Gold spans found by this recognizer and no other.
    pub unique_found: usize,
}

impl RecognizerMetrics {
    pub fn precision(&self) -> Option<f64> {
        let d = self.correct + self.false_positives;
        (d > 0).then(|| self.correct as f64 / d as f64)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    FalsePositive,
    FalseNegative,
    /// A prediction overlaps the gold span but has a different type.
    WrongType,
}

/// A single error with context, for the FP/FN listing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorCase {
    pub kind: ErrorKind,
    pub record_id: String,
    pub entity: EntityType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predicted: Option<EntityType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recognizer: Option<String>,
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub context: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EvalReport {
    pub documents: usize,
    pub complete_documents: usize,
    /// Micro-average over all types.
    pub micro: EntityMetrics,
    pub per_entity: BTreeMap<EntityType, EntityMetrics>,
    pub per_recognizer: BTreeMap<String, RecognizerMetrics>,
    /// Document-level blocking decision for Art. 9/10 data.
    pub special_documents: Counts,
    pub errors: Vec<ErrorCase>,
}

#[derive(Debug, Clone, Copy)]
struct Pred {
    start: usize,
    end: usize,
    entity: EntityType,
    idx: usize,
}

fn overlap(a: (usize, usize), b: (usize, usize)) -> usize {
    a.1.min(b.1).saturating_sub(a.0.max(b.0))
}

fn to_chars(text: &str, d: &Detection) -> (usize, usize) {
    (byte_to_char(text, d.start), byte_to_char(text, d.end))
}

fn context(text: &str, start: usize, end: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let a = start.saturating_sub(40);
    let b = (end + 40).min(chars.len());
    let mut s: String = chars[a..start].iter().collect();
    s.push('»');
    s.extend(&chars[start..end]);
    s.push('«');
    s.extend(&chars[end..b]);
    s.replace(['\n', '\r'], "⏎")
}

/// Evaluate `engine` on `records`.
pub fn evaluate(engine: &Engine, records: &[Record]) -> Result<EvalReport, DetectError> {
    let mut rep = EvalReport { documents: records.len(), ..Default::default() };
    for rec in records {
        if rec.complete {
            rep.complete_documents += 1;
        }
        let text = rec.text.as_str();
        let analysis = engine.analyze(text)?;
        let preds_det: Vec<&Detection> = analysis.detections.iter().chain(&analysis.blocking).collect();
        let preds: Vec<Pred> = preds_det
            .iter()
            .enumerate()
            .map(|(idx, d)| {
                let (start, end) = to_chars(text, d);
                Pred { start, end, entity: d.entity, idx }
            })
            .collect();
        let gold: Vec<_> = rec.gold().collect();
        let rejected: Vec<_> = rec.spans.iter().filter(|s| s.verdict == Verdict::Rejected).collect();
        let counts_as_fp = |p: (usize, usize), e: EntityType| {
            rec.complete || rejected.iter().any(|r| r.entity == e && overlap(p, (r.start, r.end)) > 0)
        };

        for strict in [false, true] {
            let mut used = vec![false; preds.len()];
            for g in &gold {
                let best = preds
                    .iter()
                    .filter(|p| !used[p.idx] && p.entity == g.entity)
                    .filter(|p| {
                        if strict {
                            (p.start, p.end) == (g.start, g.end)
                        } else {
                            overlap((p.start, p.end), (g.start, g.end)) > 0
                        }
                    })
                    .max_by_key(|p| overlap((p.start, p.end), (g.start, g.end)));
                let m = rep.per_entity.entry(g.entity).or_default();
                let c = if strict { &mut m.strict } else { &mut m.overlap };
                match best {
                    Some(p) => {
                        used[p.idx] = true;
                        c.tp += 1;
                    }
                    None => {
                        c.fn_ += 1;
                        if !strict {
                            let other = preds.iter().find(|p| overlap((p.start, p.end), (g.start, g.end)) > 0);
                            rep.errors.push(ErrorCase {
                                kind: if other.is_some() { ErrorKind::WrongType } else { ErrorKind::FalseNegative },
                                record_id: rec.id.clone(),
                                entity: g.entity,
                                predicted: other.map(|p| p.entity),
                                recognizer: other.map(|p| preds_det[p.idx].recognizer.to_string()),
                                start: g.start,
                                end: g.end,
                                text: g.text.clone(),
                                context: context(text, g.start, g.end),
                            });
                        }
                    }
                }
            }
            for p in preds.iter().filter(|p| !used[p.idx]) {
                if !counts_as_fp((p.start, p.end), p.entity) {
                    continue;
                }
                let m = rep.per_entity.entry(p.entity).or_default();
                if strict {
                    m.strict.fp += 1
                } else {
                    m.overlap.fp += 1
                }
                if !strict {
                    rep.errors.push(ErrorCase {
                        kind: ErrorKind::FalsePositive,
                        record_id: rec.id.clone(),
                        entity: p.entity,
                        predicted: None,
                        recognizer: Some(preds_det[p.idx].recognizer.to_string()),
                        start: p.start,
                        end: p.end,
                        text: crate::char_slice(text, p.start, p.end).to_owned(),
                        context: context(text, p.start, p.end),
                    });
                }
            }
        }
        for g in &gold {
            rep.per_entity.entry(g.entity).or_default().gold += 1;
        }

        // Document-level blocking.
        let expected = gold.iter().any(|g| g.entity.is_special_category());
        let blocked = !analysis.blocking.is_empty();
        let explicit_negative = rejected.iter().any(|r| r.entity.is_special_category());
        match (expected, blocked) {
            (true, true) => rep.special_documents.tp += 1,
            (true, false) => rep.special_documents.fn_ += 1,
            (false, true) if rec.complete || explicit_negative => rep.special_documents.fp += 1,
            _ => {}
        }

        // Per-recognizer metrics on raw candidates.
        let mut found_by: Vec<BTreeSet<String>> = vec![BTreeSet::new(); gold.len()];
        for c in engine.candidates(text)? {
            if c.score < engine.threshold(c.entity) {
                continue;
            }
            let span = to_chars(text, &c);
            let m = rep.per_recognizer.entry(c.recognizer.to_string()).or_default();
            m.proposed += 1;
            let mut hit = false;
            for (gi, g) in gold.iter().enumerate() {
                if g.entity == c.entity && overlap(span, (g.start, g.end)) > 0 {
                    hit = true;
                    found_by[gi].insert(c.recognizer.to_string());
                }
            }
            if hit {
                m.correct += 1;
            } else if counts_as_fp(span, c.entity) {
                m.false_positives += 1;
            }
        }
        for set in &found_by {
            for r in set {
                let m = rep.per_recognizer.entry(r.clone()).or_default();
                m.gold_found += 1;
                if set.len() == 1 {
                    m.unique_found += 1;
                }
            }
        }
    }
    let mut micro = EntityMetrics::default();
    for m in rep.per_entity.values() {
        micro.gold += m.gold;
        micro.overlap.add(m.overlap);
        micro.strict.add(m.strict);
    }
    rep.micro = micro;
    Ok(rep)
}

/// A metric that got worse or missed its target.
#[derive(Debug, Clone, PartialEq)]
pub struct Regression {
    pub metric: String,
    pub expected: f64,
    pub actual: Option<f64>,
}

impl std::fmt::Display for Regression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.actual {
            Some(a) => write!(f, "{}: {:.4} < {:.4}", self.metric, a, self.expected),
            None => write!(f, "{}: no gold spans left (expected {:.4})", self.metric, self.expected),
        }
    }
}

const EPS: f64 = 1e-9;

/// Special-category recall (span overlap and document level) must not drop
/// below `baseline`.
pub fn compare_special_recall(baseline: &EvalReport, current: &EvalReport) -> Vec<Regression> {
    let mut out = Vec::new();
    for e in EntityType::special_categories() {
        let Some(b) = baseline.per_entity.get(&e).and_then(|m| m.overlap.recall()) else { continue };
        let c = current.per_entity.get(&e).and_then(|m| m.overlap.recall());
        if c.is_none_or(|c| c + EPS < b) {
            out.push(Regression { metric: format!("{e} recall"), expected: b, actual: c });
        }
    }
    if let Some(b) = baseline.special_documents.recall() {
        let c = current.special_documents.recall();
        if c.is_none_or(|c| c + EPS < b) {
            out.push(Regression { metric: "special-category document recall".into(), expected: b, actual: c });
        }
    }
    out
}

/// Check overlap recall targets, e.g. `HEALTH >= 0.95`.
pub fn check_min_recall(report: &EvalReport, targets: &[(EntityType, f64)]) -> Vec<Regression> {
    targets
        .iter()
        .filter_map(|&(e, min)| {
            let r = report.per_entity.get(&e).and_then(|m| m.overlap.recall());
            r.is_none_or(|r| r + EPS < min).then(|| Regression {
                metric: format!("{e} recall"),
                expected: min,
                actual: r,
            })
        })
        .collect()
}

fn pct(v: Option<f64>) -> String {
    v.map_or_else(|| "    –".into(), |v| format!("{:5.1}", v * 100.0))
}

/// Human-readable report. Shows at most `max_errors` error cases.
pub fn render_text(rep: &EvalReport, max_errors: usize) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "Documents: {} ({} complete)\n", rep.documents, rep.complete_documents);
    let _ = writeln!(
        s,
        "{:<24} {:>5} │ {:>5} {:>5} {:>5} │ {:>5} {:>5} {:>5} │ {:>4} {:>4} {:>4}",
        "entity (overlap|strict)", "gold", "P", "R", "F1", "P", "R", "F1", "TP", "FP", "FN"
    );
    let _ = writeln!(s, "{}", "─".repeat(86));
    let row = |s: &mut String, name: &str, m: &EntityMetrics| {
        let o = &m.overlap;
        let st = &m.strict;
        let _ = writeln!(
            s,
            "{:<24} {:>5} │ {} {} {} │ {} {} {} │ {:>4} {:>4} {:>4}",
            name,
            m.gold,
            pct(o.precision()),
            pct(o.recall()),
            pct(o.f1()),
            pct(st.precision()),
            pct(st.recall()),
            pct(st.f1()),
            o.tp,
            o.fp,
            o.fn_
        );
    };
    for (e, m) in &rep.per_entity {
        let name = if e.is_special_category() { format!("{e} ⛔") } else { e.to_string() };
        row(&mut s, &name, m);
    }
    let _ = writeln!(s, "{}", "─".repeat(86));
    row(&mut s, "micro avg", &rep.micro);
    let d = &rep.special_documents;
    let _ = writeln!(
        s,
        "\nArt. 9/10 document blocking: P {} R {} (TP {} FP {} FN {})",
        pct(d.precision()),
        pct(d.recall()),
        d.tp,
        d.fp,
        d.fn_
    );
    let _ =
        writeln!(s, "\n{:<32} {:>8} {:>8} {:>6} {:>6} {:>7}", "recognizer", "proposed", "correct", "FP", "P", "unique");
    let _ = writeln!(s, "{}", "─".repeat(72));
    for (r, m) in &rep.per_recognizer {
        let _ = writeln!(
            s,
            "{:<32} {:>8} {:>8} {:>6} {} {:>7}",
            r,
            m.proposed,
            m.correct,
            m.false_positives,
            pct(m.precision()),
            m.unique_found
        );
    }
    if max_errors > 0 && !rep.errors.is_empty() {
        let _ = writeln!(s, "\nErrors ({} total, showing up to {max_errors}):", rep.errors.len());
        for e in rep.errors.iter().take(max_errors) {
            let kind = match e.kind {
                ErrorKind::FalsePositive => "FP",
                ErrorKind::FalseNegative => "FN",
                ErrorKind::WrongType => "TYPE",
            };
            let extra = match (&e.predicted, &e.recognizer) {
                (Some(p), Some(r)) => format!(" (predicted {p} by {r})"),
                (None, Some(r)) => format!(" (by {r})"),
                _ => String::new(),
            };
            let _ = writeln!(
                s,
                "  {kind:<4} {:<22} {}{extra}\n       [{}] {}",
                e.entity.as_str(),
                e.text,
                e.record_id,
                e.context
            );
        }
    }
    s
}

/// Dataset statistics.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    pub records: usize,
    pub complete: usize,
    pub synthetic: usize,
    pub by_verdict: BTreeMap<String, usize>,
    pub gold_by_entity: BTreeMap<EntityType, usize>,
    pub by_source: BTreeMap<String, usize>,
}

pub fn stats(records: &[Record]) -> Stats {
    let mut s = Stats { records: records.len(), ..Default::default() };
    for r in records {
        s.complete += usize::from(r.complete);
        s.synthetic += usize::from(r.synthetic);
        *s.by_source.entry(r.source.clone().unwrap_or_else(|| "unknown".into())).or_default() += 1;
        for sp in &r.spans {
            *s.by_verdict.entry(sp.verdict.as_str().into()).or_default() += 1;
            if sp.verdict.is_gold() {
                *s.gold_by_entity.entry(sp.entity).or_default() += 1;
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markup;
    use detect::EngineConfig;

    fn records(src: &str) -> Vec<Record> {
        markup::parse(src, "test", true).unwrap()
    }

    #[test]
    fn counts_tp_fp_fn() {
        let engine = Engine::new(EngineConfig::default()).unwrap();
        // Phone found (TP), e-mail found but not labeled in a complete record (FP),
        // the made-up ID is labeled but not found (FN).
        let recs = records("Tel [[PHONE|0171 1234567]], Mail a@b.de, Kunde [[ID_DOCUMENT|QQ-77]].");
        let rep = evaluate(&engine, &recs).unwrap();
        assert_eq!(rep.per_entity[&EntityType::Phone].overlap, Counts { tp: 1, fp: 0, fn_: 0 });
        assert_eq!(rep.per_entity[&EntityType::Email].overlap.fp, 1);
        assert_eq!(rep.per_entity[&EntityType::IdDocument].overlap.fn_, 1);
        assert_eq!(rep.micro.overlap, Counts { tp: 1, fp: 1, fn_: 1 });
        assert!(rep.errors.iter().any(|e| e.kind == ErrorKind::FalseNegative && e.text == "QQ-77"));
        assert!(rep.per_recognizer["phone.mobile_de"].correct == 1);
    }

    #[test]
    fn incomplete_records_do_not_count_unlabeled_predictions() {
        let engine = Engine::new(EngineConfig::default()).unwrap();
        let recs = records("@complete: false\nMail a@b.de");
        let rep = evaluate(&engine, &recs).unwrap();
        assert!(rep.per_entity.get(&EntityType::Email).is_none_or(|m| m.overlap.fp == 0));
    }

    #[test]
    fn special_document_level_and_regression_gate() {
        let engine = Engine::new(EngineConfig::default()).unwrap();
        let recs = records("Herr Weber ist [[HEALTH|krankgeschrieben]].\n---\nDas Wetter ist schön.");
        let rep = evaluate(&engine, &recs).unwrap();
        assert_eq!(rep.special_documents, Counts { tp: 1, fp: 0, fn_: 0 });
        assert!(compare_special_recall(&rep, &rep).is_empty());

        let mut worse = rep.clone();
        let m = worse.per_entity.get_mut(&EntityType::Health).unwrap();
        m.overlap.tp = 0;
        m.overlap.fn_ = 1;
        let regs = compare_special_recall(&rep, &worse);
        assert_eq!(regs.len(), 1);
        assert!(regs[0].to_string().contains("HEALTH"));
        assert_eq!(check_min_recall(&worse, &[(EntityType::Health, 0.9)]).len(), 1);
        assert!(render_text(&rep, 5).contains("HEALTH"));
    }
}
