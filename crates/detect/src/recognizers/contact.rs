use crate::util::{before, lazy_regex, trim_end};
use crate::{DetectError, Detection, EntityType, Recognizer};

lazy_regex!(
    EMAIL,
    r"[\p{L}\p{N}][\p{L}\p{N}._%+\-]*@[\p{L}\p{N}](?:[\p{L}\p{N}\-]*[\p{L}\p{N}])?(?:\.[\p{L}\p{N}](?:[\p{L}\p{N}\-]*[\p{L}\p{N}])?)*\.\p{L}{2,24}\b"
);
lazy_regex!(URL, r#"(?i)\b(?:https?://|ftp://|www\.)[^\s<>"'`]+"#);
// Group `p` holds the number; the leading group stands in for a lookbehind.
lazy_regex!(
    PHONE,
    r"(?:^|[^\p{L}\p{N}_+./\-])(?P<p>(?:\+|00)[1-9](?:[ \-/.]?\(?\d\)?){6,16}|\(?0[1-9](?:[ \-/.]?\(?\d\)?){5,13})"
);
lazy_regex!(DATE_LIKE, r"^\d{1,2}\.\d{1,2}\.\d{2,4}$");
lazy_regex!(
    PHONE_CONTEXT,
    r"(?i)\b(?:tel|telefon|telefonnummer|fon|phone|mobil|mobile|handy|handynummer|fax|rufnummer|anrufen|ruf|erreichbar|durchwahl|call|whatsapp|signal)\b"
);

/// E-mail addresses (Unicode local parts and IDN domains).
#[derive(Debug, Default, Clone, Copy)]
pub struct EmailRecognizer;

impl Recognizer for EmailRecognizer {
    fn id(&self) -> &str {
        "email"
    }
    fn entities(&self) -> &[EntityType] {
        &[EntityType::Email]
    }
    fn detect(&self, text: &str, out: &mut Vec<Detection>) -> Result<(), DetectError> {
        for m in EMAIL.find_iter(text) {
            // Skip `user@host` inside URLs with credentials; the secrets
            // recognizer handles those.
            if before(text, m.start(), 3).ends_with("://") {
                continue;
            }
            out.push(Detection::new(m.start(), m.end(), EntityType::Email, 0.97, "email"));
        }
        Ok(())
    }
}

/// http(s)/ftp URLs and bare `www.` hosts.
#[derive(Debug, Default, Clone, Copy)]
pub struct UrlRecognizer;

impl Recognizer for UrlRecognizer {
    fn id(&self) -> &str {
        "url"
    }
    fn entities(&self) -> &[EntityType] {
        &[EntityType::Url]
    }
    fn detect(&self, text: &str, out: &mut Vec<Detection>) -> Result<(), DetectError> {
        for m in URL.find_iter(text) {
            let mut end = trim_end(text, m.start(), m.end(), &['.', ',', ';', ':', '!', '?', '"', '\'']);
            // Drop a trailing ')' or ']' that has no partner inside the URL.
            for (open, close) in [('(', ')'), ('[', ']')] {
                let s = &text[m.start()..end];
                if s.ends_with(close) && s.matches(open).count() < s.matches(close).count() {
                    end -= 1;
                }
            }
            out.push(Detection::new(m.start(), end, EntityType::Url, 0.85, "url"));
        }
        Ok(())
    }
}

/// German national and international phone numbers.
#[derive(Debug, Default, Clone, Copy)]
pub struct PhoneRecognizer;

impl Recognizer for PhoneRecognizer {
    fn id(&self) -> &str {
        "phone"
    }
    fn entities(&self) -> &[EntityType] {
        &[EntityType::Phone]
    }
    fn detect(&self, text: &str, out: &mut Vec<Detection>) -> Result<(), DetectError> {
        for caps in PHONE.captures_iter(text) {
            let m = caps.name("p").expect("group p");
            let start = m.start();
            let mut end = m.end();
            let raw = &text[start..end];
            if raw.ends_with(')') && raw.matches('(').count() < raw.matches(')').count() {
                end -= 1;
            }
            let s = &text[start..end];
            if DATE_LIKE.is_match(s) {
                continue;
            }
            let digits: String = s.chars().filter(char::is_ascii_digit).collect();
            let intl = s.starts_with('+') || s.starts_with("00");
            let (score, id) = if intl {
                let national = digits.strip_prefix("00").unwrap_or(&digits);
                if !(8..=15).contains(&national.len()) {
                    continue;
                }
                if national.starts_with("49") { (0.9, "phone.intl_de") } else { (0.8, "phone.intl") }
            } else {
                if !(7..=12).contains(&digits.len()) {
                    continue;
                }
                let has_sep = s.contains([' ', '-', '/', '(']);
                if digits.starts_with("015") || digits.starts_with("016") || digits.starts_with("017") {
                    (0.9, "phone.mobile_de")
                } else if has_sep {
                    (0.65, "phone.landline_de")
                } else {
                    (0.5, "phone.landline_de")
                }
            };
            let score =
                if PHONE_CONTEXT.is_match(before(text, start, 40)) { (score + 0.3_f32).min(0.97) } else { score };
            out.push(Detection::new(start, end, EntityType::Phone, score, id));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recognizers::test_util::texts;

    #[test]
    fn emails() {
        let r = EmailRecognizer;
        assert_eq!(
            texts(&r, "Schreib an jürgen.müller@beispiel-firma.de oder max+test@example.com.", 0.5),
            ["jürgen.müller@beispiel-firma.de", "max+test@example.com"]
        );
        assert!(texts(&r, "Kein Treffer: foo@bar, @handle, a@b.c", 0.5).is_empty());
    }

    #[test]
    fn urls() {
        let r = UrlRecognizer;
        assert_eq!(
            texts(&r, "Siehe https://example.org/a?b=c. Oder (www.beispiel.de/x).", 0.5),
            ["https://example.org/a?b=c", "www.beispiel.de/x"]
        );
    }

    #[test]
    fn phones() {
        let r = PhoneRecognizer;
        let t = "Ruf mich an: 0171 1234567, Büro +49 (0)30 12345678 oder 0049 89 1234567. Fax 030/1234567";
        assert_eq!(texts(&r, t, 0.5), ["0171 1234567", "+49 (0)30 12345678", "0049 89 1234567", "030/1234567"]);
        // Dates, prices and versions are not phone numbers.
        assert!(texts(&r, "am 01.02.2024 für 12,50 €, Version 1.2.3", 0.5).is_empty());
    }

    #[test]
    fn phone_context_raises_score() {
        let r = PhoneRecognizer;
        let plain = crate::recognizers::test_util::run(&r, "Nummer 0301234567");
        let ctx = crate::recognizers::test_util::run(&r, "Telefon: 0301234567");
        assert!(ctx[0].score > plain[0].score);
    }
}
