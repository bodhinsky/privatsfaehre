//! Opt-in local audit log (JSONL).
//!
//! One line per decision: timestamp, action, number of spans per entity type,
//! rule ids and text length. Original values are never written, and neither
//! are hashes of them: hashes of short values such as phone numbers can be
//! reversed by brute force.

use detect::Detection;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Clipboard replaced with the pseudonymized text.
    Redacted,
    /// Special-category data: clipboard replaced with a notice.
    Blocked,
    /// Placeholders restored.
    Revealed,
    /// Findings shown, the user kept the original.
    Allowed,
    /// Detection failed; handled like a block.
    Error,
}

#[derive(Serialize)]
struct Line<'a> {
    ts: String,
    action: Action,
    entities: BTreeMap<&'a str, usize>,
    rules: BTreeSet<&'a str>,
    chars: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored: Option<usize>,
}

#[derive(Debug)]
pub struct AuditLog {
    file: File,
}

impl AuditLog {
    pub fn open(path: &Path) -> io::Result<Self> {
        Ok(Self { file: OpenOptions::new().create(true).append(true).open(path)? })
    }

    /// Log a decision about `detections` in a text of `chars` characters.
    pub fn record<'a>(
        &mut self,
        action: Action,
        detections: impl IntoIterator<Item = &'a Detection>,
        chars: usize,
    ) -> io::Result<()> {
        let mut entities = BTreeMap::new();
        let mut rules = BTreeSet::new();
        for d in detections {
            *entities.entry(d.entity.as_str()).or_insert(0) += 1;
            rules.insert(d.recognizer.as_ref());
        }
        self.write(&Line { ts: crate::now_rfc3339(), action, entities, rules, chars, restored: None })
    }

    pub fn revealed(&mut self, restored: usize, chars: usize) -> io::Result<()> {
        let line = Line {
            ts: crate::now_rfc3339(),
            action: Action::Revealed,
            entities: BTreeMap::new(),
            rules: BTreeSet::new(),
            chars,
            restored: Some(restored),
        };
        self.write(&line)
    }

    fn write(&mut self, line: &Line<'_>) -> io::Result<()> {
        let mut s = serde_json::to_string(line).map_err(io::Error::other)?;
        s.push('\n');
        self.file.write_all(s.as_bytes())?;
        self.file.flush()
    }
}
