use crate::checksum::{iban_length, iban_valid, is_country_code, luhn_valid};
use crate::util::{around, before, digits, lazy_regex};
use crate::{DetectError, Detection, EntityType, Recognizer};

lazy_regex!(IBAN, r"(?i)\b[A-Z]{2}\d{2}(?:[ \-]?[A-Z0-9]){11,30}");
lazy_regex!(IBAN_CONTEXT, r"(?i)\b(?:iban|konto|kontonummer|bankverbindung|account|überweis\w*)\b");
lazy_regex!(BIC, r"\b[A-Z]{4}([A-Z]{2})[A-Z2-9][A-NP-Z0-9](?:[A-Z0-9]{3})?\b");
lazy_regex!(BIC_CONTEXT, r"(?i)\b(?:bic|swift|iban|bank|bankverbindung|kreditinstitut)\b");
lazy_regex!(CARD, r"\b(?:\d[ \-]?){12,18}\d\b");

/// IBANs, validated with the country length table and MOD 97-10.
#[derive(Debug, Default, Clone, Copy)]
pub struct IbanRecognizer;

impl Recognizer for IbanRecognizer {
    fn id(&self) -> &str {
        "iban"
    }
    fn entities(&self) -> &[EntityType] {
        &[EntityType::Iban]
    }
    fn detect(&self, text: &str, out: &mut Vec<Detection>) -> Result<(), DetectError> {
        for m in IBAN.find_iter(text) {
            let s = m.as_str();
            let country = s[..2].to_ascii_uppercase();
            let Some(len) = iban_length(&country) else { continue };
            // Collect alphanumerics until the country's IBAN length is reached;
            // the regex is deliberately greedy and may run into following words.
            let mut compact = String::with_capacity(len);
            let mut end = m.start();
            for (i, c) in s.char_indices() {
                if compact.len() == len {
                    break;
                }
                if c.is_ascii_alphanumeric() {
                    compact.push(c.to_ascii_uppercase());
                    end = m.start() + i + c.len_utf8();
                }
            }
            if compact.len() < len {
                continue;
            }
            // A longer run of alphanumerics without separator is some other number.
            if text[end..].chars().next().is_some_and(|c| c.is_alphanumeric()) {
                continue;
            }
            let (score, id) = if iban_valid(&compact) {
                (1.0, "iban.checksum")
            } else if IBAN_CONTEXT.is_match(before(text, m.start(), 40)) {
                (0.75, "iban.format")
            } else {
                (0.3, "iban.format")
            };
            out.push(Detection::new(m.start(), end, EntityType::Iban, score, id));
        }
        Ok(())
    }
}

/// BIC / SWIFT codes. Without context these are easily confused with
/// upper-case words, so the score depends heavily on context.
#[derive(Debug, Default, Clone, Copy)]
pub struct BicRecognizer;

impl Recognizer for BicRecognizer {
    fn id(&self) -> &str {
        "bic"
    }
    fn entities(&self) -> &[EntityType] {
        &[EntityType::Bic]
    }
    fn detect(&self, text: &str, out: &mut Vec<Detection>) -> Result<(), DetectError> {
        for caps in BIC.captures_iter(text) {
            let m = caps.get(0).expect("match");
            if !is_country_code(&caps[1]) {
                continue;
            }
            let s = m.as_str();
            let mut score: f32 = if BIC_CONTEXT.is_match(around(text, m.start(), m.end(), 60)) {
                0.9
            } else if s.bytes().any(|b| b.is_ascii_digit()) {
                0.55
            } else {
                0.3
            };
            if s.len() == 11 && s.ends_with("XXX") {
                score += 0.1;
            }
            out.push(Detection::new(m.start(), m.end(), EntityType::Bic, score, "bic"));
        }
        Ok(())
    }
}

/// Payment card numbers (Luhn + issuer prefix).
#[derive(Debug, Default, Clone, Copy)]
pub struct CreditCardRecognizer;

impl Recognizer for CreditCardRecognizer {
    fn id(&self) -> &str {
        "credit_card"
    }
    fn entities(&self) -> &[EntityType] {
        &[EntityType::CreditCard]
    }
    fn detect(&self, text: &str, out: &mut Vec<Detection>) -> Result<(), DetectError> {
        for m in CARD.find_iter(text) {
            let d = digits(m.as_str());
            if !(13..=19).contains(&d.len()) || d.iter().all(|&x| x == d[0]) || !luhn_valid(&d) {
                continue;
            }
            let p2 = d[0] * 10 + d[1];
            let known_issuer = d[0] == 4
                || (51..=55).contains(&p2)
                || (22..=27).contains(&p2)
                || p2 == 34
                || p2 == 37
                || p2 == 35
                || p2 == 36
                || p2 == 30
                || p2 == 38
                || p2 == 65
                || (d[0] == 6 && d[1] == 0);
            if known_issuer {
                out.push(Detection::new(m.start(), m.end(), EntityType::CreditCard, 0.9, "credit_card.luhn"));
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
    fn iban_valid_with_spaces_and_lowercase() {
        let r = IbanRecognizer;
        let t = "IBAN: DE89 3704 0044 0532 0130 00 Bitte überweisen. Alt: de89370400440532013000.";
        let found = run(&r, t);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].text(t), "DE89 3704 0044 0532 0130 00");
        assert_eq!(found[0].score, 1.0);
        assert_eq!(found[1].text(t), "de89370400440532013000");
    }

    #[test]
    fn iban_bad_checksum_low_score() {
        let r = IbanRecognizer;
        let found = run(&r, "Referenz DE12 3456 7890 1234 5678 90");
        assert_eq!(found.len(), 1);
        assert!(found[0].score < 0.5);
        // Longer digit runs are not IBANs.
        assert!(run(&r, "DE893704004405320130001234").is_empty());
    }

    #[test]
    fn bic_needs_context_for_high_score() {
        let r = BicRecognizer;
        assert_eq!(texts(&r, "BIC: COBADEFFXXX", 0.8), ["COBADEFFXXX"]);
        assert!(texts(&r, "Das FEEDBACK war gut", 0.5).is_empty());
    }

    #[test]
    fn credit_cards() {
        let r = CreditCardRecognizer;
        assert_eq!(texts(&r, "Karte 4111 1111 1111 1111 gültig", 0.5), ["4111 1111 1111 1111"]);
        assert!(texts(&r, "Karte 4111 1111 1111 1112", 0.5).is_empty());
    }
}
