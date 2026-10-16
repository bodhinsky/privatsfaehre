use crate::util::lazy_regex;
use crate::{DetectError, Detection, EntityType, Recognizer};

lazy_regex!(
    ORG,
    r"\b(?:[\p{Lu}][\p{L}\p{N}&\-.]*\s){0,3}[\p{Lu}][\p{L}\p{N}&\-.]*\s(?:GmbH & Co\. KG|GmbH & Co\. OHG|gGmbH|GmbH|AG & Co\. KG|KGaA|AG|KG|OHG|UG \(haftungsbeschränkt\)|UG|e\. ?V\.|GbR|PartG mbB|PartG|SE|Ltd\.?|Inc\.?|LLC|B\.V\.|N\.V\.)"
);

/// Leading words that are captured by the capitalised-token pattern but are
/// not part of the name (sentence-initial articles and prepositions).
const LEADING_NOISE: &[&str] = &[
    "die", "der", "das", "den", "dem", "des", "the", "bei", "von", "vom", "zur", "zum", "mit", "für", "an", "auf",
    "aus", "unsere", "unser", "ihre", "ihr", "meine", "mein", "liebe", "sehr",
];

/// Organisations identified by a German or international legal-form suffix.
#[derive(Debug, Default, Clone, Copy)]
pub struct OrganizationRecognizer;

impl Recognizer for OrganizationRecognizer {
    fn id(&self) -> &str {
        "organization"
    }
    fn entities(&self) -> &[EntityType] {
        &[EntityType::Organization]
    }
    fn detect(&self, text: &str, out: &mut Vec<Detection>) -> Result<(), DetectError> {
        for m in ORG.find_iter(text) {
            if text[m.end()..].chars().next().is_some_and(char::is_alphanumeric) {
                continue;
            }
            let mut start = m.start();
            for w in m.as_str().split(' ') {
                if LEADING_NOISE.contains(&w.to_lowercase().as_str()) {
                    start += w.len() + 1;
                } else {
                    break;
                }
            }
            // At least one name word must remain before the suffix.
            if text[start..m.end()].split_whitespace().count() < 2 {
                continue;
            }
            out.push(Detection::new(start, m.end(), EntityType::Organization, 0.8, "organization.legal_form"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recognizers::test_util::texts;

    #[test]
    fn legal_forms() {
        let r = OrganizationRecognizer;
        assert_eq!(
            texts(&r, "Die Muster Software GmbH & Co. KG und der Sportverein Grün-Weiß e.V. sowie Acme Inc.", 0.5),
            ["Muster Software GmbH & Co. KG", "Sportverein Grün-Weiß e.V.", "Acme Inc."]
        );
        assert!(texts(&r, "Die AG tagt", 0.5).is_empty());
    }
}
