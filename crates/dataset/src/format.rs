use detect::EntityType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

/// Human decision on a candidate span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Recognizer proposal confirmed as is.
    Accepted,
    /// Recognizer proposal is wrong (kept as an explicit negative).
    Rejected,
    /// Recognizer proposal with fixed boundaries and/or type.
    Corrected,
    /// Span no recognizer proposed (a false negative of the engine).
    Added,
}

impl Verdict {
    /// Accepted, corrected and added spans form the gold standard.
    pub fn is_gold(self) -> bool {
        !matches!(self, Verdict::Rejected)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Accepted => "accepted",
            Verdict::Rejected => "rejected",
            Verdict::Corrected => "corrected",
            Verdict::Added => "added",
        }
    }
}

impl std::str::FromStr for Verdict {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "accepted" | "accept" | "a" | "yes" | "y" | "ok" => Ok(Verdict::Accepted),
            "rejected" | "reject" | "r" | "no" | "n" => Ok(Verdict::Rejected),
            "corrected" | "correct" | "c" => Ok(Verdict::Corrected),
            "added" | "add" | "+" => Ok(Verdict::Added),
            other => Err(format!("unknown verdict `{other}`")),
        }
    }
}

/// The recognizer's original proposal before a correction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    pub start: usize,
    pub end: usize,
    pub entity: EntityType,
}

/// A labeled span. `start`/`end` are character offsets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabeledSpan {
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub entity: EntityType,
    pub verdict: Verdict,
    /// Recognizer that proposed the span (`None` for added spans).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recognizer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
    /// For corrected spans: what was proposed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed: Option<Proposal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One labeled text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    pub text: String,
    /// Origin category, e.g. `email`, `support_ticket`, `contract`, `chat`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// True if every sensitive span in `text` is labeled. Unmatched
    /// predictions only count as false positives in complete records.
    #[serde(default)]
    pub complete: bool,
    /// True for synthetic samples (safe to publish).
    #[serde(default)]
    pub synthetic: bool,
    #[serde(default)]
    pub spans: Vec<LabeledSpan>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labeled_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labeled_at: Option<String>,
}

impl Record {
    pub fn new(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            source: None,
            lang: None,
            complete: false,
            synthetic: false,
            spans: Vec::new(),
            tags: Vec::new(),
            labeled_by: None,
            labeled_at: None,
        }
    }

    pub fn gold(&self) -> impl Iterator<Item = &LabeledSpan> {
        self.spans.iter().filter(|s| s.verdict.is_gold())
    }

    /// Check offsets and that each span's `text` matches the record text.
    pub fn validate(&self) -> Result<(), String> {
        let chars = self.text.chars().count();
        for (i, s) in self.spans.iter().enumerate() {
            if s.start >= s.end || s.end > chars {
                return Err(format!("record `{}` span {i}: offsets {}..{} out of range", self.id, s.start, s.end));
            }
            let actual = char_slice(&self.text, s.start, s.end);
            if actual != s.text {
                return Err(format!(
                    "record `{}` span {i}: text {:?} does not match offsets {}..{} ({actual:?})",
                    self.id, s.text, s.start, s.end
                ));
            }
        }
        Ok(())
    }
}

/// Stable record id from content (FNV-1a 64), e.g. `email-3f2a9c1b0d4e5f67`.
pub fn content_id(prefix: &str, text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{prefix}-{h:016x}")
}

/// Byte offset of character index `ci` (or `text.len()` at the end).
pub fn char_to_byte(text: &str, ci: usize) -> usize {
    text.char_indices().nth(ci).map_or(text.len(), |(b, _)| b)
}

/// Character index of byte offset `bi` (must be a char boundary).
pub fn byte_to_char(text: &str, bi: usize) -> usize {
    text[..bi].chars().count()
}

pub fn char_slice(text: &str, start: usize, end: usize) -> &str {
    &text[char_to_byte(text, start)..char_to_byte(text, end)]
}

#[derive(Debug, thiserror::Error)]
pub enum DatasetError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("{path}:{line}: {message}")]
    Parse { path: PathBuf, line: usize, message: String },
    #[error("{0}")]
    Invalid(String),
    #[error("csv: {0}")]
    Csv(#[from] csv::Error),
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> DatasetError + '_ {
    move |source| DatasetError::Io { path: path.to_owned(), source }
}

/// Read one JSONL file without de-duplication.
pub fn read_jsonl(path: &Path) -> Result<Vec<Record>, DatasetError> {
    let file = File::open(path).map_err(io_err(path))?;
    let mut out = Vec::new();
    for (n, line) in BufReader::new(file).lines().enumerate() {
        let line = line.map_err(io_err(path))?;
        if line.trim().is_empty() {
            continue;
        }
        let rec: Record = serde_json::from_str(&line).map_err(|e| DatasetError::Parse {
            path: path.to_owned(),
            line: n + 1,
            message: e.to_string(),
        })?;
        rec.validate().map_err(|message| DatasetError::Parse { path: path.to_owned(), line: n + 1, message })?;
        out.push(rec);
    }
    Ok(out)
}

/// All `*.jsonl` files under `paths` (files or directories, recursively),
/// sorted for determinism.
pub fn jsonl_files(paths: &[PathBuf]) -> Result<Vec<PathBuf>, DatasetError> {
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            let mut entries: Vec<_> =
                fs::read_dir(p).map_err(io_err(p))?.filter_map(|e| e.ok().map(|e| e.path())).collect();
            entries.sort();
            for e in entries {
                if e.is_dir() {
                    out.extend(jsonl_files(&[e])?);
                } else if e.extension().is_some_and(|x| x == "jsonl") {
                    out.push(e);
                }
            }
        } else {
            out.push(p.clone());
        }
    }
    Ok(out)
}

/// Load records from files/directories. Later records with the same id
/// replace earlier ones; order of first appearance is kept.
pub fn load(paths: &[PathBuf]) -> Result<Vec<Record>, DatasetError> {
    let mut order: Vec<String> = Vec::new();
    let mut by_id: HashMap<String, Record> = HashMap::new();
    for f in jsonl_files(paths)? {
        for r in read_jsonl(&f)? {
            if !by_id.contains_key(&r.id) {
                order.push(r.id.clone());
            }
            by_id.insert(r.id.clone(), r);
        }
    }
    Ok(order.into_iter().filter_map(|id| by_id.remove(&id)).collect())
}

/// Append one record (creates the file if needed).
pub fn append(path: &Path, record: &Record) -> Result<(), DatasetError> {
    record.validate().map_err(DatasetError::Invalid)?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(io_err(dir))?;
    }
    let mut f = OpenOptions::new().create(true).append(true).open(path).map_err(io_err(path))?;
    let line = serde_json::to_string(record).map_err(|e| DatasetError::Invalid(e.to_string()))?;
    writeln!(f, "{line}").map_err(io_err(path))
}

/// Write records to `path` atomically (temp file + rename).
pub fn write_all(path: &Path, records: &[Record]) -> Result<(), DatasetError> {
    let tmp = path.with_extension("jsonl.tmp");
    {
        let mut f = File::create(&tmp).map_err(io_err(&tmp))?;
        for r in records {
            r.validate().map_err(DatasetError::Invalid)?;
            let line = serde_json::to_string(r).map_err(|e| DatasetError::Invalid(e.to_string()))?;
            writeln!(f, "{line}").map_err(io_err(&tmp))?;
        }
        f.sync_all().map_err(io_err(&tmp))?;
    }
    fs::rename(&tmp, path).map_err(io_err(path))
}

/// Character-offset occurrences of `needle` in `text`.
pub fn find_all(text: &str, needle: &str) -> Vec<(usize, usize)> {
    if needle.is_empty() {
        return Vec::new();
    }
    let n = needle.chars().count();
    text.match_indices(needle)
        .map(|(b, _)| {
            let s = byte_to_char(text, b);
            (s, s + n)
        })
        .collect()
}

/// Occurrence of `needle` closest to character offset `near`.
pub fn find_nearest(text: &str, needle: &str, near: usize) -> Option<(usize, usize)> {
    find_all(text, needle).into_iter().min_by_key(|&(s, _)| s.abs_diff(near))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(text: &str, needle: &str, entity: EntityType, verdict: Verdict) -> LabeledSpan {
        let (start, end) = find_all(text, needle)[0];
        LabeledSpan {
            start,
            end,
            text: needle.into(),
            entity,
            verdict,
            recognizer: None,
            score: None,
            proposed: None,
            note: None,
        }
    }

    #[test]
    fn char_offsets_with_umlauts() {
        let text = "Grüße von Jürgen Müller";
        let s = span(text, "Jürgen Müller", EntityType::Person, Verdict::Added);
        assert_eq!((s.start, s.end), (10, 23));
        let mut r = Record::new("x", text);
        r.spans.push(s);
        r.validate().unwrap();
        r.spans[0].start = 11;
        assert!(r.validate().is_err());
    }

    #[test]
    fn roundtrip_and_last_wins() {
        let dir = std::env::temp_dir().join(format!("ds-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.jsonl");
        let _ = fs::remove_file(&path);
        let mut r = Record::new("r1", "Anna ruft an");
        append(&path, &r).unwrap();
        r.spans.push(span(&r.text.clone(), "Anna", EntityType::Person, Verdict::Accepted));
        r.complete = true;
        append(&path, &r).unwrap();
        append(&path, &Record::new("r2", "nichts")).unwrap();
        let loaded = load(std::slice::from_ref(&dir)).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0], r);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ids_are_stable() {
        assert_eq!(content_id("t", "abc"), content_id("t", "abc"));
        assert_ne!(content_id("t", "abc"), content_id("t", "abd"));
    }
}
