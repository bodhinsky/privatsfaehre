//! Reading raw texts for `label`.

use dataset::{Record, content_id, markup};
use std::fs;
use std::path::{Path, PathBuf};

fn collect(p: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if p.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(p)?.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort();
        for e in entries {
            if e.is_dir() || e.extension().is_some_and(|x| x == "txt" || x == "md" || x == "jsonl") {
                collect(&e, out)?;
            }
        }
    } else {
        out.push(p.to_owned());
    }
    Ok(())
}

/// Load inputs as (possibly pre-labeled) records.
pub fn load(paths: &[PathBuf], default_source: &str) -> Result<Vec<Record>, Box<dyn std::error::Error>> {
    let mut files = Vec::new();
    for p in paths {
        collect(p, &mut files)?;
    }
    let mut out = Vec::new();
    for f in files {
        let content = fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?;
        if f.extension().is_some_and(|x| x == "jsonl") {
            for (n, line) in content.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
                let v: serde_json::Value =
                    serde_json::from_str(line).map_err(|e| format!("{}:{}: {e}", f.display(), n + 1))?;
                let text = v["text"].as_str().ok_or_else(|| format!("{}:{}: missing \"text\"", f.display(), n + 1))?;
                let source = v["source"].as_str().unwrap_or(default_source).to_owned();
                let id = v["id"].as_str().map_or_else(|| content_id(&source, text), str::to_owned);
                let mut r = Record::new(id, text);
                r.source = Some(source);
                out.push(r);
            }
        } else {
            out.extend(markup::parse(&content, default_source, false).map_err(|e| format!("{}: {e}", f.display()))?);
        }
    }
    Ok(out)
}
