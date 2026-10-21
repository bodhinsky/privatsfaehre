//! Reading raw texts for `label` and `scan`.
//!
//! `.txt` and `.md` files use the markup format (records separated by `---`,
//! optional inline `[[TYPE|text]]` pre-labels). Every other supported format
//! (PDF, Word, OpenDocument, e-mail, spreadsheets, CSV, JSON, HTML, RTF) is
//! converted to plain text by the `ingest` crate.

use dataset::{Record, content_id, markup};
use ingest::{Format, Options};
use std::fs;
use std::path::{Path, PathBuf};

fn supported(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ingest::EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

fn collect(p: &Path, top_level: bool, out: &mut Vec<PathBuf>) -> Result<(), String> {
    if p.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(p)
            .map_err(|e| format!("{}: {e}", p.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        entries.sort();
        for e in entries {
            if e.is_dir() || supported(&e) {
                collect(&e, false, out)?;
            }
        }
    } else if supported(p) {
        out.push(p.to_owned());
    } else if top_level {
        return Err(format!("{}: unsupported file type (supported: {})", p.display(), ingest::EXTENSIONS.join(", ")));
    }
    Ok(())
}

/// Load inputs as (possibly pre-labeled) records. `source` overrides the
/// per-format default source label.
pub fn load(
    paths: &[PathBuf],
    source: Option<&str>,
    opts: &Options,
) -> Result<Vec<Record>, Box<dyn std::error::Error>> {
    let mut files = Vec::new();
    for p in paths {
        collect(p, true, &mut files)?;
    }
    let mut out = Vec::new();
    for f in files {
        let format = Format::from_path(&f).expect("filtered to supported formats");
        if format == Format::Text {
            let content = fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?;
            let src = source.unwrap_or("text");
            out.extend(markup::parse(&content, src, false).map_err(|e| format!("{}: {e}", f.display()))?);
            continue;
        }
        for doc in ingest::extract(&f, opts)? {
            let src = source.map(str::to_owned).or(doc.source).unwrap_or_else(|| format.source_label().to_owned());
            let id = doc.id.unwrap_or_else(|| content_id(&src, &doc.text));
            let mut r = Record::new(id, doc.text);
            r.source = Some(src);
            r.tags.push(format!("origin:{}", doc.origin));
            out.push(r);
        }
    }
    Ok(out)
}

/// Texts to scan from one file, each with its origin.
pub fn scan_texts(path: &Path, opts: &Options) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    match Format::from_path(path) {
        Some(Format::Text) | None => Ok(vec![(path.display().to_string(), fs::read_to_string(path)?)]),
        Some(_) => Ok(ingest::extract(path, opts)?.into_iter().map(|d| (d.origin, d.text)).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_directory() {
        let dir = std::env::temp_dir().join(format!("inputs-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.txt"), "Hallo [[PERSON|Jens]]\n---\nZweiter Text").unwrap();
        fs::write(dir.join("b.csv"), "id;Nachricht\nT-1;Ruf 0171 1234567 an\n").unwrap();
        fs::write(dir.join("c.html"), "<p>Gr&uuml;&szlig;e</p>").unwrap();
        fs::write(dir.join("ignored.bin"), "x").unwrap();
        let recs = load(std::slice::from_ref(&dir), None, &Options::default()).unwrap();
        let summary: Vec<_> = recs.iter().map(|r| (r.source.as_deref().unwrap(), r.text.as_str())).collect();
        assert_eq!(
            summary,
            [("text", "Hallo Jens"), ("text", "Zweiter Text"), ("table", "Ruf 0171 1234567 an"), ("html", "Grüße")]
        );
        assert_eq!(recs[2].id, "T-1");
        assert!(load(&[dir.join("ignored.bin")], None, &Options::default()).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }
}
