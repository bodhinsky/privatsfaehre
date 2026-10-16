use crate::util::{before, lazy_regex};
use crate::{DetectError, Detection, EntityType, Recognizer};

lazy_regex!(NUMERIC_DE, r"\b(\d{1,2})\.\s?(\d{1,2})\.\s?(\d{4}|\d{2})\b");
lazy_regex!(
    MONTH_DE,
    r"(?i)\b(\d{1,2})\.?\s*(januar|jänner|februar|feber|märz|maerz|april|mai|juni|juli|august|september|oktober|november|dezember|jan|feb|mär|mrz|apr|jun|jul|aug|sep|sept|okt|nov|dez)\b\.?(?:\s*(\d{4}))?"
);
lazy_regex!(
    MONTH_EN,
    r"(?i)\b(january|february|march|april|may|june|july|august|september|october|november|december|jan|feb|mar|apr|jun|jul|aug|sep|sept|oct|nov|dec)\.?\s+(\d{1,2})(?:st|nd|rd|th)?,?\s+(\d{4})\b"
);
lazy_regex!(ISO, r"\b(\d{4})-(\d{2})-(\d{2})\b");
lazy_regex!(SLASH, r"\b(\d{1,2})/(\d{1,2})/(\d{4})\b");
lazy_regex!(
    BIRTH_CONTEXT,
    r"(?i)(?:\bgeb\.|geboren|geburtsdatum|geburtstag|geb\.-datum|\bdob\b|date of birth|\bborn\b|\*\s*$)"
);

fn valid_dm(d: &str, m: &str) -> bool {
    matches!((d.parse::<u8>(), m.parse::<u8>()), (Ok(d), Ok(m)) if (1..=31).contains(&d) && (1..=12).contains(&m))
}

/// German, ISO and English date formats. Birth-date context raises the score.
#[derive(Debug, Default, Clone, Copy)]
pub struct DateRecognizer;

impl Recognizer for DateRecognizer {
    fn id(&self) -> &str {
        "date"
    }
    fn entities(&self) -> &[EntityType] {
        &[EntityType::Date]
    }
    fn detect(&self, text: &str, out: &mut Vec<Detection>) -> Result<(), DetectError> {
        let mut push = |start: usize, end: usize, base: f32, id: &'static str| {
            let score = if BIRTH_CONTEXT.is_match(before(text, start, 30)) { 0.95 } else { base };
            out.push(Detection::new(start, end, EntityType::Date, score, id));
        };
        let bytes = text.as_bytes();
        for c in NUMERIC_DE.captures_iter(text) {
            let m = c.get(0).expect("match");
            // Reject parts of longer dotted sequences (versions, IPs).
            let prev_dot = m.start() > 0 && bytes[m.start() - 1] == b'.';
            let next_dot_digit =
                bytes.get(m.end()) == Some(&b'.') && bytes.get(m.end() + 1).is_some_and(u8::is_ascii_digit);
            if prev_dot || next_dot_digit || !valid_dm(&c[1], &c[2]) {
                continue;
            }
            push(m.start(), m.end(), 0.65, "date.numeric_de");
        }
        for c in MONTH_DE.captures_iter(text) {
            let m = c.get(0).expect("match");
            if !matches!(c[1].parse::<u8>(), Ok(1..=31)) {
                continue;
            }
            let base = if c.get(3).is_some() { 0.7 } else { 0.55 };
            push(m.start(), m.end(), base, "date.month_de");
        }
        for c in MONTH_EN.captures_iter(text) {
            let m = c.get(0).expect("match");
            if matches!(c[2].parse::<u8>(), Ok(1..=31)) {
                push(m.start(), m.end(), 0.7, "date.month_en");
            }
        }
        for c in ISO.captures_iter(text) {
            let m = c.get(0).expect("match");
            if valid_dm(&c[3], &c[2]) {
                push(m.start(), m.end(), 0.6, "date.iso");
            }
        }
        for c in SLASH.captures_iter(text) {
            let m = c.get(0).expect("match");
            if valid_dm(&c[1], &c[2]) || valid_dm(&c[2], &c[1]) {
                push(m.start(), m.end(), 0.55, "date.slash");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recognizers::test_util::{run, texts};

    #[test]
    fn formats() {
        let r = DateRecognizer;
        let t = "Am 3.5.2024, am 12. März 2023, 2024-01-31, March 5, 2024 und 05/03/2024.";
        assert_eq!(texts(&r, t, 0.5), ["3.5.2024", "12. März 2023", "March 5, 2024", "2024-01-31", "05/03/2024"]);
    }

    #[test]
    fn birth_context() {
        let r = DateRecognizer;
        let d = run(&r, "geb. 14.02.1987");
        assert_eq!(d.len(), 1);
        assert!(d[0].score > 0.9);
    }

    #[test]
    fn negatives() {
        let r = DateRecognizer;
        assert!(texts(&r, "Version 1.2.3.4 und 32.13.2020 und 10.1.2.2024", 0.5).is_empty());
    }
}
