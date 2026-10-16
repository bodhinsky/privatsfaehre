use crate::util::{after, before, lazy_regex};
use crate::{DetectError, Detection, EntityType, Recognizer};

const SUFFIXES: &str = "straße|strasse|str\\.|weg|gasse|allee|platz|ring|damm|ufer|chaussee|steig|pfad|markt|stieg|kamp|graben|promenade|zeile";
const HOUSE_NO: &str = r"\d{1,4}[a-zA-Z]?(?:\s?[-/]\s?\d{1,4}[a-zA-Z]?)?\b";

// Single compound token ("Hauptstraße 5", "Karl-Marx-Allee 12").
static STREET_COMPOUND: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(&format!(r"(?i)\b[\p{{L}}][\p{{L}}\-]*?(?:{SUFFIXES})\s+{HOUSE_NO}")).expect("regex")
});
// Adjective + street word ("Frankfurter Allee 12", "berliner str. 5").
static STREET_TWO_WORDS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(&format!(
        r"(?i)\b[\p{{L}}][\p{{L}}\-]+er\s(?:straße|strasse|str\.|weg|gasse|allee|platz|ring|damm|ufer|chaussee|markt|promenade)\s+{HOUSE_NO}"
    ))
    .expect("regex")
});
// Prepositional names ("Am Markt 3", "An der Alster 7"). Case-sensitive.
static STREET_PREP: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(&format!(
        r"\b(?:Am|An der|An den|Auf der|Auf dem|Im|In der|In den|Zum|Zur|Hinter der|Unter den|Alt-)\s?[A-ZÄÖÜ][\p{{L}}\-]+\s+{HOUSE_NO}"
    ))
    .expect("regex")
});
lazy_regex!(
    PLZ_CITY,
    r"\b(?:D[- ])?(?P<plz>\d{5})\s+(?P<city>(?:Bad |Sankt |St\. )?[A-ZÄÖÜa-zäöü][a-zäöüß]+(?:-[A-ZÄÖÜ]?[a-zäöüß]+)*(?: (?:am|an der|im|ob der|in der|bei|vor der) [A-ZÄÖÜ][a-zäöüß]+)?(?: ?\([A-ZÄÖÜ][a-zäöüß]+\))?)"
);
lazy_regex!(PLZ_JOIN, r"^\s*,?\s*$");
lazy_regex!(
    UNIT_AFTER,
    r"(?i)^\s*(?:minuten|min|km|m|meter|stunden|std|h|tage|wochen|monate|jahre|euro|eur|€|%|mal|prozent|stück|personen|leute|grad|°)\b"
);
lazy_regex!(
    PLZ_NEGATIVE,
    r"(?i)(?:\bnr\.?|\w*nummer|artikel\w*|bestell\w*|rechnung\w*|kunden\w*|auftrag\w*|ticket\w*|vorgang\w*|\bid\b|#|konto|menge|stück|€|eur\b|summe|betrag)\s*:?\s*$"
);
lazy_regex!(
    PLZ_POSITIVE,
    r"(?i)(?:\bplz\b|wohnhaft|wohne|wohnt|anschrift|adresse|\bin\b|\baus\b|\bnach\b|,|^)\s*:?\s*$"
);
lazy_regex!(
    ADDRESS_CONTEXT,
    r"(?i)(?:wohnhaft|wohne|wohnt|adresse|anschrift|lieferadresse|rechnungsadresse|address|lives at)"
);

/// Common compounds that end in a street suffix but are not streets.
const NOT_STREETS: &[&str] = &[
    "arbeitsplatz",
    "parkplatz",
    "sitzplatz",
    "stellplatz",
    "rückweg",
    "hinweg",
    "heimweg",
    "arbeitsweg",
    "schulweg",
    "umweg",
    "ausweg",
    "fußweg",
    "radweg",
    "arbeitsmarkt",
    "binnenmarkt",
    "wohnungsmarkt",
    "aktienmarkt",
    "immobilienmarkt",
    "supermarkt",
    "flohmarkt",
    "weihnachtsmarkt",
    "bahnsteig",
    "ehering",
    "schlüsselring",
    "dammweg",
];

/// German street addresses and PLZ + city.
#[derive(Debug, Default, Clone, Copy)]
pub struct AddressRecognizer;

impl Recognizer for AddressRecognizer {
    fn id(&self) -> &str {
        "address"
    }
    fn entities(&self) -> &[EntityType] {
        &[EntityType::Address, EntityType::PostalCode]
    }
    fn detect(&self, text: &str, out: &mut Vec<Detection>) -> Result<(), DetectError> {
        let mut streets: Vec<(usize, usize, f32)> = Vec::new();
        for (re, base) in [(&*STREET_COMPOUND, 0.75_f32), (&*STREET_TWO_WORDS, 0.8), (&*STREET_PREP, 0.7)] {
            for m in re.find_iter(text) {
                if streets.iter().any(|&(s, e, _)| m.start() < e && s < m.end()) {
                    continue;
                }
                let first_word = m.as_str().split_whitespace().next().unwrap_or("").to_lowercase();
                if NOT_STREETS.contains(&first_word.as_str()) || UNIT_AFTER.is_match(after(text, m.end(), 15)) {
                    continue;
                }
                let score = if ADDRESS_CONTEXT.is_match(before(text, m.start(), 40)) { base.max(0.9) } else { base };
                streets.push((m.start(), m.end(), score));
            }
        }

        let mut consumed: Vec<(usize, usize)> = Vec::new();
        for (start, end, score) in streets {
            // Extend over a directly following ", 80331 München".
            let tail = after(text, end, 40);
            let joined = PLZ_CITY.captures(tail).and_then(|c| {
                let m = c.get(0)?;
                (PLZ_JOIN.is_match(&tail[..m.start()]) && valid_plz(&c["plz"])).then(|| end + m.end())
            });
            match joined {
                Some(full_end) => {
                    consumed.push((start, full_end));
                    out.push(Detection::new(start, full_end, EntityType::Address, 0.95, "address.street_plz"));
                }
                None => out.push(Detection::new(start, end, EntityType::Address, score, "address.street")),
            }
        }

        for caps in PLZ_CITY.captures_iter(text) {
            let m = caps.get(0).expect("match");
            if consumed.iter().any(|&(s, e)| m.start() >= s && m.end() <= e) || !valid_plz(&caps["plz"]) {
                continue;
            }
            let lowercase_city = caps["city"].chars().next().is_some_and(char::is_lowercase);
            let prefix = before(text, m.start(), 25);
            let score = if PLZ_NEGATIVE.is_match(prefix) {
                0.25
            } else if PLZ_POSITIVE.is_match(prefix) {
                if lowercase_city { 0.7 } else { 0.9 }
            } else if lowercase_city {
                0.45
            } else {
                0.7
            };
            out.push(Detection::new(m.start(), m.end(), EntityType::PostalCode, score, "address.plz_city"));
        }
        Ok(())
    }
}

fn valid_plz(plz: &str) -> bool {
    !plz.starts_with("00")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recognizers::test_util::found;

    #[test]
    fn street_with_plz_is_one_span() {
        let r = AddressRecognizer;
        let t = "Lieferung an Hauptstraße 12a, 80331 München bitte.";
        assert_eq!(found(&r, t, 0.9), [("Hauptstraße 12a, 80331 München".to_owned(), EntityType::Address)]);
    }

    #[test]
    fn street_variants() {
        let r = AddressRecognizer;
        let texts: Vec<_> = found(&r, "Frankfurter Allee 5, Am Markt 3 und karl-marx-str. 7", 0.6)
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        assert_eq!(texts, ["karl-marx-str. 7", "Frankfurter Allee 5", "Am Markt 3"]);
    }

    #[test]
    fn plz_city() {
        let r = AddressRecognizer;
        assert_eq!(found(&r, "Ich wohne in 10115 Berlin.", 0.8), [("10115 Berlin".to_owned(), EntityType::PostalCode)]);
        assert_eq!(found(&r, "Frankfurt: 60311 Frankfurt am Main", 0.6)[0].0, "60311 Frankfurt am Main");
    }

    #[test]
    fn negatives() {
        let r = AddressRecognizer;
        assert!(found(&r, "Der Arbeitsweg 20 Minuten, Parkplatz 3", 0.5).is_empty());
        assert!(found(&r, "Bestellnummer 12345 Lieferung folgt", 0.5).is_empty());
        assert!(found(&r, "Die Bestellung 12345 Lieferung folgt", 0.5).is_empty());
    }
}
