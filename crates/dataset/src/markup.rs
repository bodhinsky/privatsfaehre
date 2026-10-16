//! Inline markup for hand-written samples.
//!
//! ```text
//! @source: email
//! @id: optional-explicit-id
//! @complete: true
//! Hallo [[PERSON|Anna Schmidt]], bitte an [[IBAN|DE89 3704 0044 0532 0130 00]] überweisen.
//! ---
//! Next record. Lines starting with `//` are comments.
//! ```
//!
//! Records are separated by a line containing only `---`. Directives
//! (`@key: value`) are only recognised at the top of a record. Spans become
//! `added` gold spans; `complete` defaults to true because the author is
//! expected to mark everything.

use crate::{LabeledSpan, Record, Verdict, content_id};
use detect::EntityType;

/// Parse markup into records. `default_source` is used when no `@source`.
pub fn parse(input: &str, default_source: &str, synthetic: bool) -> Result<Vec<Record>, String> {
    let mut out = Vec::new();
    let mut block: Vec<&str> = Vec::new();
    let mut line_no = 0;
    let mut block_start = 1;
    for line in input.lines().chain(std::iter::once("---")) {
        line_no += 1;
        if line.trim() == "---" {
            if let Some(r) = parse_block(&block, default_source, synthetic)
                .map_err(|e| format!("record at line {block_start}: {e}"))?
            {
                out.push(r);
            }
            block.clear();
            block_start = line_no + 1;
        } else if !line.trim_start().starts_with("//") {
            block.push(line);
        }
    }
    Ok(out)
}

fn parse_block(lines: &[&str], default_source: &str, synthetic: bool) -> Result<Option<Record>, String> {
    let mut source = default_source.to_owned();
    let mut id = None;
    let mut complete = true;
    let mut lang = None;
    let mut i = 0;
    while i < lines.len() && lines[i].trim().is_empty() {
        i += 1;
    }
    while i < lines.len() {
        let Some((k, v)) = lines[i].trim().strip_prefix('@').and_then(|d| d.split_once(':')) else { break };
        let v = v.trim();
        match k.trim() {
            "source" => source = v.to_owned(),
            "id" => id = Some(v.to_owned()),
            "lang" => lang = Some(v.to_owned()),
            "complete" => complete = v.parse().map_err(|_| format!("bad @complete `{v}`"))?,
            other => return Err(format!("unknown directive @{other}")),
        }
        i += 1;
    }
    let body_start = i;
    let body = lines[body_start..].join("\n");
    let body = body.trim_matches('\n');
    if body.trim().is_empty() {
        return Ok(None);
    }
    let (text, spans) = parse_spans(body)?;
    let mut r = Record::new(id.unwrap_or_else(|| content_id(&source, &text)), text);
    r.source = Some(source);
    r.lang = lang;
    r.complete = complete;
    r.synthetic = synthetic;
    r.spans = spans;
    r.validate()?;
    Ok(Some(r))
}

/// Strip `[[TYPE|text]]` markers, returning plain text and spans.
pub fn parse_spans(marked: &str) -> Result<(String, Vec<LabeledSpan>), String> {
    let mut text = String::with_capacity(marked.len());
    let mut spans = Vec::new();
    let mut rest = marked;
    let mut chars = 0usize;
    while let Some(open) = rest.find("[[") {
        let before = &rest[..open];
        text.push_str(before);
        chars += before.chars().count();
        let after = &rest[open + 2..];
        let close = after.find("]]").ok_or("unclosed `[[`")?;
        let inner = &after[..close];
        let (ty, value) = inner.split_once('|').ok_or_else(|| format!("missing `|` in [[{inner}]]"))?;
        let entity: EntityType = ty.parse().map_err(|e| format!("{e}"))?;
        if value.is_empty() {
            return Err(format!("empty span [[{inner}]]"));
        }
        let n = value.chars().count();
        spans.push(LabeledSpan {
            start: chars,
            end: chars + n,
            text: value.to_owned(),
            entity,
            verdict: Verdict::Added,
            recognizer: None,
            score: None,
            proposed: None,
            note: None,
        });
        text.push_str(value);
        chars += n;
        rest = &after[close + 2..];
    }
    text.push_str(rest);
    Ok((text, spans))
}

/// Render a record back to markup (gold spans only).
pub fn render(r: &Record) -> String {
    let mut gold: Vec<_> = r.gold().collect();
    gold.sort_by_key(|s| s.start);
    let mut out = String::new();
    let mut pos = 0;
    let chars: Vec<char> = r.text.chars().collect();
    for s in gold {
        if s.start < pos {
            continue; // overlapping gold cannot be expressed inline
        }
        out.extend(&chars[pos..s.start]);
        out.push_str(&format!("[[{}|{}]]", s.entity, s.text));
        pos = s.end;
    }
    out.extend(&chars[pos..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_records() {
        let input = "@source: email\n@complete: false\nHallo [[PERSON|Jürgen]],\nIBAN [[IBAN|DE89 3704 0044 0532 0130 00]].\n---\n// comment\nNur Text.\n";
        let recs = parse(input, "misc", true).unwrap();
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].text, "Hallo Jürgen,\nIBAN DE89 3704 0044 0532 0130 00.");
        assert_eq!(recs[0].source.as_deref(), Some("email"));
        assert!(!recs[0].complete);
        assert_eq!(recs[0].spans[0].start, 6);
        assert_eq!(recs[0].spans[1].entity, EntityType::Iban);
        assert!(recs[1].complete && recs[1].spans.is_empty());
        assert_eq!(render(&recs[0]), "Hallo [[PERSON|Jürgen]],\nIBAN [[IBAN|DE89 3704 0044 0532 0130 00]].");
    }

    #[test]
    fn errors() {
        assert!(parse_spans("[[NOPE|x]]").is_err());
        assert!(parse_spans("[[PERSON|x").is_err());
        assert!(parse_spans("[[PERSON x]]").is_err());
    }
}
