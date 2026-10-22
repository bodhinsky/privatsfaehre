//! Tabular and JSON inputs.

use crate::{Document, Format, Options};
use serde_json::Value;

/// Column names that usually hold free text (lower-case).
const TEXT_COLUMNS: &[&str] = &[
    "text",
    "body",
    "message",
    "content",
    "nachricht",
    "inhalt",
    "beschreibung",
    "description",
    "kommentar",
    "comment",
    "notiz",
    "notes",
    "anliegen",
    "freitext",
    "mail_body",
    "email_body",
];

fn pick_column(headers: &[String], opts: &Options) -> Result<Option<usize>, String> {
    if let Some(c) = &opts.column {
        return headers
            .iter()
            .position(|h| h.trim().eq_ignore_ascii_case(c))
            .map(Some)
            .ok_or_else(|| format!("column `{c}` not found (columns: {})", headers.join(", ")));
    }
    Ok(TEXT_COLUMNS.iter().find_map(|t| headers.iter().position(|h| h.trim().eq_ignore_ascii_case(t))))
}

fn col(headers: &[String], name: &str) -> Option<usize> {
    headers.iter().position(|h| h.trim().eq_ignore_ascii_case(name))
}

/// One document per row: the text column if there is one, otherwise all
/// non-empty cells as `header: value` lines.
pub(crate) fn rows_to_docs(
    headers: &[String],
    rows: impl Iterator<Item = (String, Vec<String>)>,
    format: Format,
    opts: &Options,
) -> Result<Vec<Document>, String> {
    let text_col = pick_column(headers, opts)?;
    let id_col = col(headers, "id");
    let source_col = col(headers, "source");
    let mut out = Vec::new();
    for (origin, row) in rows {
        let cell = |i: usize| row.get(i).map(|s| s.trim()).filter(|s| !s.is_empty());
        let text = match text_col {
            Some(c) => cell(c).unwrap_or("").to_owned(),
            None => headers
                .iter()
                .enumerate()
                .filter_map(|(i, h)| {
                    cell(i).map(|v| if h.trim().is_empty() { v.to_owned() } else { format!("{}: {v}", h.trim()) })
                })
                .collect::<Vec<_>>()
                .join("\n"),
        };
        if text.trim().is_empty() {
            continue;
        }
        out.push(Document {
            origin,
            format,
            text,
            id: id_col.and_then(cell).map(str::to_owned),
            source: source_col.and_then(cell).map(str::to_owned),
        });
    }
    Ok(out)
}

pub(crate) fn delimited(bytes: &[u8], delimiter: u8, name: &str, opts: &Options) -> Result<Vec<Document>, String> {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim_start_matches('\u{feff}');
    // German Excel writes `;`-separated "CSV"; detect it from the header line.
    let delimiter = if delimiter == b',' {
        let header = text.lines().next().unwrap_or("");
        if header.matches(';').count() > header.matches(',').count() { b';' } else { b',' }
    } else {
        delimiter
    };
    let mut rd = csv::ReaderBuilder::new().delimiter(delimiter).flexible(true).from_reader(text.as_bytes());
    let headers: Vec<String> = rd.headers().map_err(|e| e.to_string())?.iter().map(str::to_owned).collect();
    let mut rows = Vec::new();
    for (i, r) in rd.records().enumerate() {
        let r = r.map_err(|e| e.to_string())?;
        rows.push((format!("{name}#row-{}", i + 2), r.iter().map(str::to_owned).collect()));
    }
    let format = if delimiter == b'\t' { Format::Tsv } else { Format::Csv };
    rows_to_docs(&headers, rows.into_iter(), format, opts)
}

fn scalar(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn value_to_doc(v: &Value, origin: String, format: Format, opts: &Options) -> Result<Option<Document>, String> {
    let doc = |text: String, id: Option<String>, source: Option<String>| Document {
        origin: origin.clone(),
        format,
        text,
        id,
        source,
    };
    Ok(match v {
        Value::String(s) => Some(doc(s.clone(), None, None)),
        Value::Object(map) => {
            let key = match &opts.column {
                Some(c) => Some(
                    map.keys()
                        .find(|k| k.eq_ignore_ascii_case(c))
                        .ok_or_else(|| format!("{origin}: field `{c}` not found"))?,
                ),
                None => TEXT_COLUMNS.iter().find_map(|t| map.keys().find(|k| k.eq_ignore_ascii_case(t))),
            };
            let text = match key {
                Some(k) => scalar(&map[k]).unwrap_or_default(),
                None => map
                    .iter()
                    .filter_map(|(k, v)| scalar(v).filter(|s| !s.is_empty()).map(|s| format!("{k}: {s}")))
                    .collect::<Vec<_>>()
                    .join("\n"),
            };
            (!text.trim().is_empty()).then(|| {
                doc(text, map.get("id").and_then(scalar), map.get("source").and_then(Value::as_str).map(str::to_owned))
            })
        }
        _ => None,
    })
}

pub(crate) fn json(text: &str, name: &str, opts: &Options) -> Result<Vec<Document>, String> {
    let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    match &v {
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                out.extend(value_to_doc(item, format!("{name}#{}", i + 1), Format::Json, opts)?);
            }
        }
        other => out.extend(value_to_doc(other, name.to_owned(), Format::Json, opts)?),
    }
    Ok(out)
}

pub(crate) fn json_lines(text: &str, name: &str, opts: &Options) -> Result<Vec<Document>, String> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
        let v: Value =
            serde_json::from_str(line.trim_start_matches('\u{feff}')).map_err(|e| format!("line {}: {e}", i + 1))?;
        out.extend(value_to_doc(&v, format!("{name}#line-{}", i + 1), Format::JsonLines, opts)?);
    }
    Ok(out)
}

#[cfg(feature = "spreadsheet")]
pub(crate) fn spreadsheet(path: &std::path::Path, name: &str, opts: &Options) -> Result<Vec<Document>, String> {
    use calamine::{Reader, open_workbook_auto};
    let mut wb = open_workbook_auto(path).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for sheet in wb.sheet_names() {
        let range = wb.worksheet_range(&sheet).map_err(|e| format!("sheet `{sheet}`: {e}"))?;
        let mut rows = range.rows().map(|r| r.iter().map(|c| c.to_string()).collect::<Vec<String>>());
        let Some(headers) = rows.next() else { continue };
        let rows = rows.enumerate().map(|(i, r)| (format!("{name}#{sheet}!{}", i + 2), r)).collect::<Vec<_>>();
        out.extend(rows_to_docs(&headers, rows.into_iter(), Format::Spreadsheet, opts)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_with_text_column_and_semicolons() {
        let csv = "\u{feff}id;Kunde;Nachricht\nT-1;Anna;\"Hallo, IBAN DE89 3704\"\nT-2;Bert;\n";
        let docs = delimited(csv.as_bytes(), b',', "t.csv", &Options::default()).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].text, "Hallo, IBAN DE89 3704");
        assert_eq!(docs[0].id.as_deref(), Some("T-1"));
        assert_eq!(docs[0].origin, "t.csv#row-2");
    }

    #[test]
    fn csv_without_text_column_joins_cells() {
        let csv = "Name,Telefon\nAnna Schmidt,0171 1234567\n";
        let docs = delimited(csv.as_bytes(), b',', "k.csv", &Options::default()).unwrap();
        assert_eq!(docs[0].text, "Name: Anna Schmidt\nTelefon: 0171 1234567");
        let opts = Options { column: Some("telefon".into()) };
        assert_eq!(delimited(csv.as_bytes(), b',', "k.csv", &opts).unwrap()[0].text, "0171 1234567");
        assert!(delimited(csv.as_bytes(), b',', "k.csv", &Options { column: Some("x".into()) }).is_err());
    }

    #[test]
    fn json_shapes() {
        let o = Options::default();
        assert_eq!(json(r#"["a", {"body": "b", "id": 7}]"#, "j", &o).unwrap()[1].id.as_deref(), Some("7"));
        assert_eq!(json(r#"{"name": "Anna", "plz": "10115"}"#, "j", &o).unwrap()[0].text, "name: Anna\nplz: 10115");
        let l = json_lines("{\"text\":\"x\",\"source\":\"chat\"}\n\n\"y\"\n", "l", &o).unwrap();
        assert_eq!((l.len(), l[0].source.as_deref(), l[1].origin.as_str()), (2, Some("chat"), "l#line-3"));
    }
}
