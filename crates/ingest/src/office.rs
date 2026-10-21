//! Office Open XML (`.docx`, `.pptx`) and OpenDocument (`.odt`, `.odp`).

use crate::Format;
use quick_xml::Reader;
use quick_xml::events::Event;
use std::io::{Cursor, Read};

/// How to turn one XML part into text.
struct Dialect {
    /// Only text inside these elements counts (`None` = all text).
    text_in: Option<&'static [&'static str]>,
    /// Elements whose end is a paragraph break.
    paragraph: &'static [&'static str],
    newline: &'static [&'static str],
    tab: &'static [&'static str],
    /// ODF `<text:s text:c="n"/>`: n spaces.
    space: Option<&'static str>,
}

const WORD: Dialect =
    Dialect { text_in: Some(&["t"]), paragraph: &["p"], newline: &["br", "cr"], tab: &["tab"], space: None };
const DRAWING: Dialect = Dialect { text_in: Some(&["t"]), paragraph: &["p"], newline: &["br"], tab: &[], space: None };
const ODF: Dialect = Dialect {
    text_in: Some(&["p", "h"]),
    paragraph: &["p", "h"],
    newline: &["line-break"],
    tab: &["tab"],
    space: Some("s"),
};

fn xml_to_text(xml: &str, d: &Dialect, out: &mut String) -> Result<(), String> {
    let mut reader = Reader::from_str(xml);
    let mut depth_in_text = 0usize;
    // ODF: skip annotations / tracked deletions.
    let mut skip_depth = 0usize;
    loop {
        let ev = reader.read_event().map_err(|e| format!("XML error at {}: {e}", reader.error_position()))?;
        match ev {
            Event::Start(e) => {
                let name = e.local_name();
                let n: &str = name.as_ref();
                if skip_depth > 0 || matches!(n, "annotation" | "tracked-changes" | "delText" | "instrText") {
                    skip_depth += 1;
                    continue;
                }
                if d.text_in.is_some_and(|t| t.contains(&n)) {
                    depth_in_text += 1;
                }
            }
            Event::Empty(e) => {
                if skip_depth > 0 {
                    continue;
                }
                let name = e.local_name();
                let n: &str = name.as_ref();
                if d.newline.contains(&n) || d.paragraph.contains(&n) {
                    out.push('\n');
                } else if d.tab.contains(&n) {
                    out.push('\t');
                } else if d.space == Some(n) {
                    let count = e
                        .attributes()
                        .flatten()
                        .find(|a| AsRef::<str>::as_ref(&a.key.local_name()) == "c")
                        .and_then(|a| a.value.parse::<usize>().ok())
                        .unwrap_or(1);
                    out.extend(std::iter::repeat_n(' ', count.min(100)));
                }
            }
            Event::End(e) => {
                if skip_depth > 0 {
                    skip_depth -= 1;
                    continue;
                }
                let name = e.local_name();
                let n: &str = name.as_ref();
                if d.text_in.is_some_and(|t| t.contains(&n)) {
                    depth_in_text = depth_in_text.saturating_sub(1);
                }
                if d.paragraph.contains(&n) {
                    out.push('\n');
                }
            }
            Event::Text(t) if skip_depth == 0 && (d.text_in.is_none() || depth_in_text > 0) => {
                out.push_str(&t);
            }
            Event::CData(t) if skip_depth == 0 && (d.text_in.is_none() || depth_in_text > 0) => {
                out.push_str(&t);
            }
            Event::GeneralRef(r) if skip_depth == 0 && (d.text_in.is_none() || depth_in_text > 0) => {
                let c = match r.resolve_char_ref().ok().flatten() {
                    Some(c) => Some(c),
                    None => match &*r {
                        "amp" => Some('&'),
                        "lt" => Some('<'),
                        "gt" => Some('>'),
                        "quot" => Some('"'),
                        "apos" => Some('\''),
                        _ => None,
                    },
                };
                out.extend(c);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(())
}

fn part_number(name: &str) -> u32 {
    name.chars().filter(char::is_ascii_digit).collect::<String>().parse().unwrap_or(0)
}

pub(crate) fn extract(bytes: &[u8], format: Format) -> Result<String, String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("not a valid {format:?} file: {e}"))?;
    let names: Vec<String> = zip.file_names().filter_map(|n| n.ok().map(|n| n.into_owned())).collect();
    let (dialect, mut parts): (&Dialect, Vec<String>) = match format {
        Format::Docx => {
            let mut p = vec!["word/document.xml".to_owned()];
            for prefix in ["word/header", "word/footer"] {
                let mut h: Vec<_> =
                    names.iter().filter(|n| n.starts_with(prefix) && n.ends_with(".xml")).cloned().collect();
                h.sort_by_key(|n| part_number(n));
                p.extend(h);
            }
            p.extend(["word/footnotes.xml", "word/endnotes.xml", "word/comments.xml"].map(str::to_owned));
            (&WORD, p)
        }
        Format::Pptx => {
            let mut slides: Vec<_> =
                names.iter().filter(|n| n.starts_with("ppt/slides/slide") && n.ends_with(".xml")).cloned().collect();
            slides.sort_by_key(|n| part_number(n));
            let mut notes: Vec<_> = names
                .iter()
                .filter(|n| n.starts_with("ppt/notesSlides/notesSlide") && n.ends_with(".xml"))
                .cloned()
                .collect();
            notes.sort_by_key(|n| part_number(n));
            slides.extend(notes);
            (&DRAWING, slides)
        }
        Format::Odt | Format::Odp => (&ODF, vec!["content.xml".to_owned()]),
        _ => return Err("not an office format".into()),
    };
    parts.retain(|p| names.contains(p));
    if parts.is_empty() {
        return Err(format!("no text parts found in {format:?} file"));
    }
    let mut out = String::new();
    for p in parts {
        let mut xml = String::new();
        zip.by_name(&p).map_err(|e| e.to_string())?.read_to_string(&mut xml).map_err(|e| format!("{p}: {e}"))?;
        xml_to_text(&xml, dialect, &mut out).map_err(|e| format!("{p}: {e}"))?;
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    pub fn zip_with(files: &[(&str, &str)]) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            for (name, content) in files {
                z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
                z.write_all(content.as_bytes()).unwrap();
            }
            z.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn docx() {
        let doc = r#"<?xml version="1.0"?><w:document xmlns:w="w"><w:body>
            <w:p><w:r><w:t>Sehr geehrte Frau </w:t></w:r><w:r><w:t>M&#252;ller &amp; Co,</w:t></w:r></w:p>
            <w:p><w:r><w:t>IBAN</w:t><w:tab/><w:t>DE89 3704</w:t><w:delText>alt</w:delText></w:r></w:p>
            <w:p><w:r><w:instrText>HYPERLINK x</w:instrText><w:t>Ende</w:t></w:r></w:p></w:body></w:document>"#;
        let bytes = zip_with(&[
            ("word/document.xml", doc),
            ("word/footer1.xml", r#"<w:ftr xmlns:w="w"><w:p><w:r><w:t>Fußzeile</w:t></w:r></w:p></w:ftr>"#),
        ]);
        let t = extract(&bytes, Format::Docx).unwrap();
        assert_eq!(crate::normalize(&t), "Sehr geehrte Frau Müller & Co,\nIBAN\tDE89 3704\nEnde\n\nFußzeile");
    }

    #[test]
    fn odt_and_pptx() {
        let content = r#"<office:document-content xmlns:office="o" xmlns:text="t"><office:body><office:text>
            <text:h>Vertrag</text:h><text:p>Herr<text:s/>Becker,<text:s text:c="2"/>geb. 1981</text:p>
            <office:annotation><text:p>interner Kommentar</text:p></office:annotation></office:text></office:body></office:document-content>"#;
        let t = extract(&zip_with(&[("content.xml", content)]), Format::Odt).unwrap();
        assert_eq!(crate::normalize(&t), "Vertrag\nHerr Becker,  geb. 1981");

        let slide = |s: &str| format!(r#"<p:sld xmlns:p="p" xmlns:a="a"><a:p><a:r><a:t>{s}</a:t></a:r></a:p></p:sld>"#);
        let (s1, s2) = (slide("Folie eins"), slide("Folie zwei"));
        let t = extract(&zip_with(&[("ppt/slides/slide10.xml", &s2), ("ppt/slides/slide2.xml", &s1)]), Format::Pptx)
            .unwrap();
        assert_eq!(crate::normalize(&t), "Folie eins\n\nFolie zwei");
    }

    #[test]
    fn rejects_garbage() {
        assert!(extract(b"not a zip", Format::Docx).is_err());
    }
}
