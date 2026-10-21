//! Extract plain text from the file formats people actually copy from.
//!
//! Everything is pure Rust and offline. Each input file yields one or more
//! [`Document`]s: one per file for prose formats (PDF, Word, HTML, e-mail),
//! one per message for mailboxes, and one per row for tabular data when a
//! text-like column exists (CSV/TSV, Excel, ODS, JSON arrays, JSONL).
//!
//! | Format | Extensions |
//! |---|---|
//! | Plain text, Markdown, logs | `txt md markdown text log` |
//! | JSON / JSON Lines | `json jsonl ndjson` |
//! | CSV / TSV | `csv tsv` |
//! | HTML | `html htm xhtml` |
//! | E-mail | `eml mbox` (feature `email`) |
//! | Word, PowerPoint, OpenDocument | `docx pptx odt odp` (feature `office`) |
//! | Spreadsheets | `xlsx xlsm xls ods` (feature `spreadsheet`) |
//! | PDF (text layer only, no OCR) | `pdf` (feature `pdf`) |
//! | RTF | `rtf` |

mod html;
#[cfg(feature = "office")]
mod office;
mod rtf;
mod table;

use std::path::{Path, PathBuf};

pub use html::html_to_text;
pub use rtf::rtf_to_text;

/// Format detected from the file extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Text,
    Json,
    JsonLines,
    Csv,
    Tsv,
    Html,
    Rtf,
    Eml,
    Mbox,
    Docx,
    Pptx,
    Odt,
    Odp,
    Spreadsheet,
    Pdf,
}

impl Format {
    pub fn from_path(path: &Path) -> Option<Format> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "txt" | "md" | "markdown" | "text" | "log" => Format::Text,
            "json" => Format::Json,
            "jsonl" | "ndjson" => Format::JsonLines,
            "csv" => Format::Csv,
            "tsv" | "tab" => Format::Tsv,
            "html" | "htm" | "xhtml" => Format::Html,
            "rtf" => Format::Rtf,
            "eml" => Format::Eml,
            "mbox" => Format::Mbox,
            "docx" => Format::Docx,
            "pptx" => Format::Pptx,
            "odt" => Format::Odt,
            "odp" => Format::Odp,
            "xlsx" | "xlsm" | "xls" | "ods" => Format::Spreadsheet,
            "pdf" => Format::Pdf,
            _ => return None,
        })
    }

    /// Default `source` label for records from this format.
    pub fn source_label(self) -> &'static str {
        match self {
            Format::Text => "text",
            Format::Json | Format::JsonLines => "json",
            Format::Csv | Format::Tsv | Format::Spreadsheet => "table",
            Format::Html => "html",
            Format::Rtf | Format::Docx | Format::Odt => "document",
            Format::Pptx | Format::Odp => "slides",
            Format::Eml | Format::Mbox => "email",
            Format::Pdf => "pdf",
        }
    }
}

/// All supported extensions (for directory scans).
pub const EXTENSIONS: &[&str] = &[
    "txt", "md", "markdown", "text", "log", "json", "jsonl", "ndjson", "csv", "tsv", "tab", "html", "htm", "xhtml",
    "rtf", "eml", "mbox", "docx", "pptx", "odt", "odp", "xlsx", "xlsm", "xls", "ods", "pdf",
];

/// One extracted text.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    /// Where it came from, e.g. `tickets.csv#row-12` or `inbox.mbox#3`.
    pub origin: String,
    pub format: Format,
    pub text: String,
    /// Explicit id or source from the input (JSON fields `id` / `source`).
    pub id: Option<String>,
    pub source: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{path}: unsupported file type")]
    Unsupported { path: PathBuf },
    #[error("{path}: support for this format was not compiled in (feature `{feature}`)")]
    Disabled { path: PathBuf, feature: &'static str },
    #[error("{path}: {message}")]
    Parse { path: PathBuf, message: String },
}

/// Options for tabular inputs.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Column holding the text (CSV, spreadsheets, JSON). If unset, a column
    /// named like `text`, `body`, `message`, `nachricht`, … is used; if none
    /// exists, all cells of a row are joined.
    pub column: Option<String>,
}

/// Extract documents from one file.
pub fn extract(path: &Path, opts: &Options) -> Result<Vec<Document>, IngestError> {
    let format = Format::from_path(path).ok_or_else(|| IngestError::Unsupported { path: path.to_owned() })?;
    let parse = |message: String| IngestError::Parse { path: path.to_owned(), message };
    let read = || std::fs::read(path).map_err(|source| IngestError::Io { path: path.to_owned(), source });
    let read_str = || read().map(|b| String::from_utf8_lossy(&b).into_owned());
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("input").to_owned();
    let single = |text: String| -> Vec<Document> {
        vec![Document { origin: name.clone(), format, text, id: None, source: None }]
    };
    #[allow(unused_variables)]
    let disabled = |feature| IngestError::Disabled { path: path.to_owned(), feature };

    let docs = match format {
        Format::Text => single(read_str()?),
        Format::Html => single(html_to_text(&read_str()?)),
        Format::Rtf => single(rtf_to_text(&read_str()?)),
        Format::Json => table::json(&read_str()?, &name, opts).map_err(parse)?,
        Format::JsonLines => table::json_lines(&read_str()?, &name, opts).map_err(parse)?,
        Format::Csv => table::delimited(&read()?, b',', &name, opts).map_err(parse)?,
        Format::Tsv => table::delimited(&read()?, b'\t', &name, opts).map_err(parse)?,
        Format::Eml | Format::Mbox => {
            #[cfg(feature = "email")]
            {
                let bytes = read()?;
                let messages = if format == Format::Mbox { split_mbox(&bytes) } else { vec![bytes.as_slice()] };
                let mut out = Vec::new();
                for (i, m) in messages.into_iter().enumerate() {
                    let text =
                        email_to_text(m).ok_or_else(|| parse(format!("message {} is not a valid e-mail", i + 1)))?;
                    let origin = if format == Format::Mbox { format!("{name}#{}", i + 1) } else { name.clone() };
                    out.push(Document { origin, format, text, id: None, source: None });
                }
                out
            }
            #[cfg(not(feature = "email"))]
            return Err(disabled("email"));
        }
        Format::Docx | Format::Pptx | Format::Odt | Format::Odp => {
            #[cfg(feature = "office")]
            {
                single(office::extract(&read()?, format).map_err(parse)?)
            }
            #[cfg(not(feature = "office"))]
            return Err(disabled("office"));
        }
        Format::Spreadsheet => {
            #[cfg(feature = "spreadsheet")]
            {
                table::spreadsheet(path, &name, opts).map_err(parse)?
            }
            #[cfg(not(feature = "spreadsheet"))]
            return Err(disabled("spreadsheet"));
        }
        Format::Pdf => {
            #[cfg(feature = "pdf")]
            {
                let text = pdf_extract::extract_text_from_mem(&read()?).map_err(|e| parse(e.to_string()))?;
                if text.trim().is_empty() {
                    return Err(parse("no text layer (scanned PDF?); OCR is not supported".into()));
                }
                single(text)
            }
            #[cfg(not(feature = "pdf"))]
            return Err(disabled("pdf"));
        }
    };
    Ok(docs
        .into_iter()
        .map(|mut d| {
            d.text = normalize(&d.text);
            d
        })
        .filter(|d| !d.text.trim().is_empty())
        .collect())
}

/// Normalise line endings and trim trailing whitespace on each line.
fn normalize(text: &str) -> String {
    let t = text.replace("\r\n", "\n").replace('\r', "\n").replace('\u{a0}', " ");
    let lines: Vec<&str> = t.lines().map(str::trim_end).collect();
    let mut out = lines.join("\n");
    // Collapse runs of more than two blank lines (common in PDF/HTML output).
    while out.contains("\n\n\n\n") {
        out = out.replace("\n\n\n\n", "\n\n\n");
    }
    out.trim_matches('\n').to_owned()
}

#[cfg(feature = "email")]
fn split_mbox(bytes: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    if bytes.starts_with(b"From ") {
        starts.push(0);
    }
    for i in 0..bytes.len().saturating_sub(6) {
        if bytes[i] == b'\n' && bytes[i + 1..].starts_with(b"From ") {
            starts.push(i + 1);
        }
    }
    let mut out = Vec::new();
    for (k, &s) in starts.iter().enumerate() {
        let e = starts.get(k + 1).copied().unwrap_or(bytes.len());
        // Skip the "From " envelope line itself.
        let body_start = bytes[s..e].iter().position(|&b| b == b'\n').map_or(e, |p| s + p + 1);
        out.push(&bytes[body_start..e]);
    }
    if out.is_empty() {
        out.push(bytes);
    }
    out
}

/// Headers that carry personal data plus the body, as plain text.
#[cfg(feature = "email")]
pub fn email_to_text(raw: &[u8]) -> Option<String> {
    let msg = mail_parser::MessageParser::default().parse(raw)?;
    let mut out = String::new();
    let fmt_addr = |a: Option<&mail_parser::Address<'_>>| -> Option<String> {
        let list: Vec<String> = a?
            .iter()
            .map(|x| match (x.name(), x.address()) {
                (Some(n), Some(m)) => format!("{n} <{m}>"),
                (None, Some(m)) => m.to_owned(),
                (Some(n), None) => n.to_owned(),
                (None, None) => String::new(),
            })
            .filter(|s| !s.is_empty())
            .collect();
        (!list.is_empty()).then(|| list.join(", "))
    };
    for (label, value) in [("Von", fmt_addr(msg.from())), ("An", fmt_addr(msg.to())), ("Cc", fmt_addr(msg.cc()))] {
        if let Some(v) = value {
            out.push_str(&format!("{label}: {v}\n"));
        }
    }
    if let Some(s) = msg.subject() {
        out.push_str(&format!("Betreff: {s}\n"));
    }
    out.push('\n');
    match msg.body_text(0) {
        Some(t) => out.push_str(&t),
        None => {
            if let Some(h) = msg.body_html(0) {
                out.push_str(&html_to_text(&h));
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_by_extension() {
        assert_eq!(Format::from_path(Path::new("a/B.DOCX")), Some(Format::Docx));
        assert_eq!(Format::from_path(Path::new("x.ndjson")), Some(Format::JsonLines));
        assert_eq!(Format::from_path(Path::new("x.exe")), None);
        for e in EXTENSIONS {
            assert!(Format::from_path(Path::new(&format!("f.{e}"))).is_some(), "{e}");
        }
    }

    #[test]
    fn normalizes_whitespace() {
        assert_eq!(normalize("a  \r\nb\r\n\n\n\n\nc\u{a0}d\n"), "a\nb\n\n\nc d");
    }

    #[cfg(feature = "email")]
    #[test]
    fn email_headers_and_body() {
        let raw = b"From: Anna Schmidt <anna@example.org>\r\nTo: support@firma.de\r\nSubject: =?utf-8?q?R=C3=BCckfrage?=\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHallo, meine IBAN ist DE89 3704 0044 0532 0130 00.\r\n";
        let t = email_to_text(raw).unwrap();
        assert!(t.contains("Von: Anna Schmidt <anna@example.org>"), "{t}");
        assert!(t.contains("Betreff: Rückfrage"));
        assert!(t.contains("DE89 3704"));
    }

    #[cfg(feature = "email")]
    #[test]
    fn mbox_split() {
        let mbox = b"From a@b Mon Jan 1 00:00:00 2024\nSubject: eins\n\nErster\n\nFrom c@d Mon Jan 1 00:00:00 2024\nSubject: zwei\n\nZweiter\n";
        let parts = split_mbox(mbox);
        assert_eq!(parts.len(), 2);
        assert!(email_to_text(parts[1]).unwrap().contains("Zweiter"));
    }
}
