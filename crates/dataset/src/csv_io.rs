//! Span-per-row CSV for spreadsheet review.
//!
//! Columns: `record_id, span_index, start, end, span_text, entity, verdict,
//! recognizer, score, note, context`. The default delimiter is `;` and files
//! start with a UTF-8 BOM so German Excel opens them correctly.
//!
//! Import rules (applied to an existing dataset):
//! * a row with `span_index` updates that span (entity, verdict, note,
//!   boundaries). If `span_text` changed, the new text is located near the old
//!   position and the span becomes `corrected`;
//! * a row with an empty `span_index` adds a span located by `span_text`
//!   (nearest to `start` if given), default verdict `added`;
//! * verdict `delete` removes the span;
//! * `context` and `score` are informational and ignored on import.

use crate::{DatasetError, LabeledSpan, Proposal, Record, Verdict, find_nearest};
use detect::EntityType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Row {
    record_id: String,
    span_index: Option<usize>,
    start: Option<usize>,
    end: Option<usize>,
    span_text: String,
    entity: String,
    verdict: String,
    recognizer: Option<String>,
    score: Option<f32>,
    note: Option<String>,
    context: Option<String>,
}

const CONTEXT_CHARS: usize = 40;

fn context(text: &str, start: usize, end: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let a = start.saturating_sub(CONTEXT_CHARS);
    let b = (end + CONTEXT_CHARS).min(chars.len());
    let mut s: String = chars[a..start].iter().collect();
    s.push('»');
    s.extend(&chars[start..end]);
    s.push('«');
    s.extend(&chars[end..b]);
    s.replace(['\n', '\r'], " ")
}

/// Write all spans of `records` as CSV.
pub fn export<W: Write>(records: &[Record], mut w: W, delimiter: u8) -> Result<(), DatasetError> {
    w.write_all("\u{feff}".as_bytes()).map_err(csv::Error::from)?;
    let mut wr = csv::WriterBuilder::new().delimiter(delimiter).from_writer(w);
    for r in records {
        for (i, s) in r.spans.iter().enumerate() {
            wr.serialize(Row {
                record_id: r.id.clone(),
                span_index: Some(i),
                start: Some(s.start),
                end: Some(s.end),
                span_text: s.text.clone(),
                entity: s.entity.to_string(),
                verdict: s.verdict.as_str().to_owned(),
                recognizer: s.recognizer.clone(),
                score: s.score,
                note: s.note.clone(),
                context: Some(context(&r.text, s.start, s.end)),
            })?;
        }
    }
    wr.flush().map_err(csv::Error::from)?;
    Ok(())
}

/// Summary of an import.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ImportStats {
    pub updated: usize,
    pub added: usize,
    pub deleted: usize,
}

/// Apply edited CSV rows to `records` in place.
pub fn import<R: Read>(records: &mut [Record], r: R, delimiter: u8) -> Result<ImportStats, DatasetError> {
    let mut rd = csv::ReaderBuilder::new().delimiter(delimiter).flexible(true).from_reader(r);
    let index: HashMap<String, usize> = records.iter().enumerate().map(|(i, r)| (r.id.clone(), i)).collect();
    let mut stats = ImportStats::default();
    let mut deletions: Vec<(usize, usize)> = Vec::new();
    for (line, row) in rd.deserialize::<Row>().enumerate() {
        let row = row?;
        let err = |m: String| DatasetError::Invalid(format!("csv row {}: {m}", line + 2));
        let &ri = index
            .get(row.record_id.trim_start_matches('\u{feff}'))
            .ok_or_else(|| err(format!("unknown record `{}`", row.record_id)))?;
        let rec = &mut records[ri];
        if row.verdict.trim().eq_ignore_ascii_case("delete") {
            let si = row.span_index.ok_or_else(|| err("delete needs span_index".into()))?;
            deletions.push((ri, si));
            continue;
        }
        let entity: EntityType = row.entity.parse().map_err(|e| err(format!("{e}")))?;
        let verdict: Option<Verdict> =
            if row.verdict.trim().is_empty() { None } else { Some(row.verdict.parse().map_err(err)?) };
        match row.span_index {
            Some(si) => {
                let text = rec.text.clone();
                let span = rec.spans.get_mut(si).ok_or_else(|| err(format!("span_index {si} out of range")))?;
                let old = Proposal { start: span.start, end: span.end, entity: span.entity };
                let (start, end) = locate(&text, &row, span.start)
                    .ok_or_else(|| err(format!("`{}` not found in record", row.span_text)))?;
                let moved = (start, end) != (span.start, span.end);
                let retyped = entity != span.entity;
                span.start = start;
                span.end = end;
                span.text = row.span_text.clone();
                span.entity = entity;
                span.note = row.note.filter(|n| !n.is_empty());
                span.verdict = match verdict {
                    Some(v) => v,
                    None if (moved || retyped) && span.verdict != Verdict::Added => Verdict::Corrected,
                    None => span.verdict,
                };
                if span.verdict == Verdict::Corrected && span.proposed.is_none() {
                    span.proposed = Some(old);
                }
                stats.updated += 1;
            }
            None => {
                let near = row.start.unwrap_or(0);
                let (start, end) = locate(&rec.text, &row, near)
                    .ok_or_else(|| err(format!("`{}` not found in record", row.span_text)))?;
                rec.spans.push(LabeledSpan {
                    start,
                    end,
                    text: row.span_text.clone(),
                    entity,
                    verdict: verdict.unwrap_or(Verdict::Added),
                    recognizer: None,
                    score: None,
                    proposed: None,
                    note: row.note.filter(|n| !n.is_empty()),
                });
                stats.added += 1;
            }
        }
    }
    deletions.sort_unstable_by(|a, b| b.cmp(a));
    deletions.dedup();
    for (ri, si) in deletions {
        if si < records[ri].spans.len() {
            records[ri].spans.remove(si);
            stats.deleted += 1;
        }
    }
    for r in records.iter() {
        r.validate().map_err(DatasetError::Invalid)?;
    }
    Ok(stats)
}

/// Use explicit offsets if they match `span_text`, else search near `near`.
fn locate(text: &str, row: &Row, near: usize) -> Option<(usize, usize)> {
    if let (Some(s), Some(e)) = (row.start, row.end) {
        if s < e && e <= text.chars().count() && crate::char_slice(text, s, e) == row.span_text {
            return Some((s, e));
        }
    }
    find_nearest(text, &row.span_text, near)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markup;

    #[test]
    fn export_edit_import() {
        let mut recs = markup::parse("Hallo [[PERSON|Anna]] Schmidt, Tel [[PHONE|0171 1234567]].", "t", true).unwrap();
        let mut buf = Vec::new();
        export(&recs, &mut buf, b';').unwrap();
        let csv = String::from_utf8(buf).unwrap();
        assert!(csv.starts_with('\u{feff}'));
        assert!(csv.contains("»Anna«"));
        let id = &recs[0].id;
        // Widen the name, delete the phone, add the missed "Hallo"-less name variant.
        let edited = format!(
            "record_id;span_index;start;end;span_text;entity;verdict;recognizer;score;note;context\n\
             {id};0;;;Anna Schmidt;PERSON;;;;full name;\n\
             {id};1;;;;PHONE;delete;;;;\n\
             {id};;;;Hallo;ORGANIZATION;rejected;;;;\n"
        );
        let stats = import(&mut recs, edited.as_bytes(), b';').unwrap();
        assert_eq!(stats, ImportStats { updated: 1, added: 1, deleted: 1 });
        let s = &recs[0].spans;
        assert_eq!((s[0].start, s[0].end, s[0].text.as_str()), (6, 18, "Anna Schmidt"));
        assert_eq!(s[0].note.as_deref(), Some("full name"));
        assert_eq!(s[1].verdict, Verdict::Rejected);
        assert_eq!(s.len(), 2);
    }
}
