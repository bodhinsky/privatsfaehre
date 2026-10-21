//! Minimal HTML-to-text conversion: drops tags, `script`/`style`/`head`
//! content and comments, turns block elements into line breaks and decodes
//! entities. Good enough for e-mails and saved web pages.

const BLOCK: &[&str] = &[
    "p",
    "div",
    "br",
    "li",
    "tr",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "table",
    "ul",
    "ol",
    "blockquote",
    "pre",
    "section",
    "article",
    "header",
    "footer",
    "hr",
    "dt",
    "dd",
];
const SKIP: &[&str] = &["script", "style", "head", "noscript", "template", "svg"];

pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut rest = html;
    let mut skip_until: Option<String> = None;
    while !rest.is_empty() {
        if let Some(tail) = rest.strip_prefix("<!--") {
            rest = tail.find("-->").map_or("", |i| &tail[i + 3..]);
            continue;
        }
        if rest.starts_with('<') {
            let end = rest.find('>').map_or(rest.len(), |i| i + 1);
            let tag = &rest[1..end.saturating_sub(1).max(1)];
            rest = &rest[end..];
            let closing = tag.starts_with('/');
            let name: String = tag
                .trim_start_matches('/')
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
                .to_ascii_lowercase();
            if let Some(s) = &skip_until {
                if closing && &name == s {
                    skip_until = None;
                }
                continue;
            }
            if !closing && SKIP.contains(&name.as_str()) && !tag.ends_with('/') {
                skip_until = Some(name);
                continue;
            }
            if name == "td" || name == "th" {
                if !closing && !out.is_empty() && !out.ends_with(['\n', '\t']) {
                    out.push('\t');
                }
            } else if BLOCK.contains(&name.as_str()) {
                out.push('\n');
            }
            continue;
        }
        let next = rest.find('<').unwrap_or(rest.len());
        if skip_until.is_none() {
            let chunk = &rest[..next];
            // Collapse source whitespace like a browser.
            let mut last_space = out.ends_with([' ', '\n', '\t']);
            for c in decode_entities(chunk).chars() {
                if c.is_whitespace() && c != '\u{a0}' {
                    if !last_space {
                        out.push(' ');
                        last_space = true;
                    }
                } else {
                    out.push(c);
                    last_space = false;
                }
            }
        }
        rest = &rest[next..];
    }
    let mut text = out.lines().map(|l| l.trim_matches(' ')).collect::<Vec<_>>().join("\n");
    while text.contains("\n\n\n") {
        text = text.replace("\n\n\n", "\n\n");
    }
    text.trim_matches('\n').to_owned()
}

/// Decode named (common subset) and numeric character references.
pub(crate) fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(semi) = rest[..rest.len().min(12)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..semi];
        let decoded = if let Some(num) = name.strip_prefix('#') {
            let v = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok(),
                None => num.parse().ok(),
            };
            v.and_then(char::from_u32)
        } else {
            match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some('\u{a0}'),
                "auml" => Some('ä'),
                "ouml" => Some('ö'),
                "uuml" => Some('ü'),
                "Auml" => Some('Ä'),
                "Ouml" => Some('Ö'),
                "Uuml" => Some('Ü'),
                "szlig" => Some('ß'),
                "euro" => Some('€'),
                "ndash" => Some('–'),
                "mdash" => Some('—'),
                "hellip" => Some('…'),
                "bdquo" => Some('„'),
                "ldquo" => Some('“'),
                "rdquo" => Some('”'),
                "sbquo" => Some('‚'),
                "lsquo" => Some('‘'),
                "rsquo" => Some('’'),
                "copy" => Some('©'),
                "eacute" => Some('é'),
                _ => None,
            }
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_markup() {
        let html = "<html><head><title>x</title><style>p{}</style></head><body>\
                    <p>Hallo <b>Frau&nbsp;M&uuml;ller</b>,</p><!-- c --><script>alert(1)</script>\
                    <table><tr><td>IBAN</td><td>DE89&#32;3704</td></tr></table>Gr&uuml;&szlig;e</body></html>";
        assert_eq!(html_to_text(html), "Hallo Frau\u{a0}Müller,\n\nIBAN\tDE89 3704\n\nGrüße");
    }

    #[test]
    fn keeps_bare_ampersands() {
        assert_eq!(decode_entities("A & B &unknown; &#x41;"), "A & B &unknown; A");
    }
}
