//! Minimal RTF-to-text: drops control words and destination groups
//! (`\fonttbl`, `\stylesheet`, `\*\…`), keeps `\par` as line breaks and
//! decodes `\'xx` (Windows-1252) and `\uN` escapes.

const SKIP_DESTINATIONS: &[&str] = &[
    "fonttbl",
    "colortbl",
    "stylesheet",
    "info",
    "pict",
    "header",
    "footer",
    "listtable",
    "listoverridetable",
    "generator",
    "themedata",
    "colorschememapping",
    "datastore",
    "latentstyles",
];

fn cp1252(b: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}', '\u{90}', '‘',
        '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
    ];
    if (0x80..0xa0).contains(&b) { HIGH[usize::from(b - 0x80)] } else { char::from(b) }
}

pub fn rtf_to_text(rtf: &str) -> String {
    let b = rtf.as_bytes();
    let mut out = String::new();
    // Stack of "skip this group" flags.
    let mut stack: Vec<bool> = vec![false];
    let mut skip_chars = 0usize; // characters to skip after \uN
    let mut i = 0;
    while i < b.len() {
        let skipping = *stack.last().unwrap_or(&false);
        match b[i] {
            b'{' => {
                stack.push(skipping);
                i += 1;
            }
            b'}' => {
                stack.pop();
                if stack.is_empty() {
                    stack.push(false);
                }
                i += 1;
            }
            b'\\' => {
                i += 1;
                let Some(&c) = b.get(i) else { break };
                if c == b'\'' {
                    if let Some(hex) = rtf.get(i + 1..i + 3) {
                        if let Ok(v) = u8::from_str_radix(hex, 16) {
                            if !skipping && skip_chars == 0 {
                                out.push(cp1252(v));
                            }
                            skip_chars = skip_chars.saturating_sub(1);
                        }
                    }
                    i += 3;
                    continue;
                }
                if c == b'*' {
                    if let Some(s) = stack.last_mut() {
                        *s = true;
                    }
                    i += 1;
                    continue;
                }
                if !c.is_ascii_alphabetic() {
                    // Control symbol: \\ \{ \} \~ etc.
                    if !skipping {
                        match c {
                            b'\\' | b'{' | b'}' => out.push(char::from(c)),
                            b'~' => out.push(' '),
                            b'\n' | b'\r' => out.push('\n'),
                            _ => {}
                        }
                    }
                    i += 1;
                    continue;
                }
                let start = i;
                while i < b.len() && b[i].is_ascii_alphabetic() {
                    i += 1;
                }
                let word = &rtf[start..i];
                let num_start = i;
                if i < b.len() && (b[i] == b'-' || b[i].is_ascii_digit()) {
                    i += 1;
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                let num: Option<i32> = rtf[num_start..i].parse().ok();
                if i < b.len() && b[i] == b' ' {
                    i += 1;
                }
                if SKIP_DESTINATIONS.contains(&word) {
                    if let Some(s) = stack.last_mut() {
                        *s = true;
                    }
                    continue;
                }
                if skipping {
                    continue;
                }
                match word {
                    "par" | "line" | "sect" | "page" => out.push('\n'),
                    "tab" | "cell" => out.push('\t'),
                    "row" => out.push('\n'),
                    "u" => {
                        if let Some(n) = num {
                            let code = if n < 0 { (n + 65_536) as u32 } else { n as u32 };
                            if let Some(ch) = char::from_u32(code) {
                                out.push(ch);
                            }
                            skip_chars = 1;
                        }
                    }
                    _ => {}
                }
            }
            b'\r' | b'\n' => i += 1,
            c => {
                if !skipping {
                    if skip_chars > 0 {
                        skip_chars -= 1;
                    } else {
                        // Plain text is ASCII in valid RTF; push the full char otherwise.
                        let ch = rtf[i..].chars().next().unwrap_or(char::from(c));
                        out.push(ch);
                        i += ch.len_utf8();
                        continue;
                    }
                }
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_rtf() {
        let rtf = r"{\rtf1\ansi{\fonttbl{\f0 Arial;}}{\*\generator Word;}\f0 Sehr geehrte Frau M\'fcller,\par IBAN DE89\tab 3704 \u8364? 5\par}";
        assert_eq!(rtf_to_text(rtf), "Sehr geehrte Frau Müller,\nIBAN DE89\t3704 € 5\n");
    }
}
