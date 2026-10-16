//! GDPR Art. 9 / Art. 10 special categories.
//!
//! Detection is lexicon based (German + English) with context boosting.
//! Detections of these types always **block** the paste; the threshold is the
//! fixed [`crate::SPECIAL_CATEGORY_THRESHOLD`].
//!
//! Scoring:
//! * strong terms (e.g. "krankgeschrieben", "vorbestraft"): 0.85, 0.95 with a
//!   personal cue in the same sentence;
//! * weak terms (e.g. "Krebs", "katholisch", "Depression"): 0.4 alone, 0.8 with
//!   a personal cue (first-person words, possessives, honorifics, role nouns)
//!   or a category cue;
//! * organisation terms (party and union names, case-sensitive): 0.4 alone,
//!   0.9 with a category cue ("Mitglied", "wähle", …).
//!
//! The lexicons are the starting point for the human-in-the-loop dataset;
//! eval results decide what moves between strong and weak.

use crate::util::{lazy_regex, sentence_window};
use crate::{DetectError, Detection, EntityType, Recognizer};
use regex::Regex;
use std::sync::LazyLock;

struct Lexicon {
    entity: EntityType,
    strong: &'static [&'static str],
    weak: &'static [&'static str],
    /// Case-sensitive organisation names (parties, unions).
    orgs: &'static [&'static str],
    /// Category-specific cue that raises weak and org terms.
    cue: &'static str,
}

const LEXICONS: &[Lexicon] = &[
    Lexicon {
        entity: EntityType::Health,
        strong: &[
            "diagnose*",
            "*diagnose",
            "diagnostiziert",
            "*erkrankung*",
            "erkrankt*",
            "krankgeschrieben",
            "krankschreibung",
            "krankmeldung",
            "arbeitsunfähig*",
            "au-bescheinigung",
            "schwanger*",
            "chemo*",
            "psychotherap*",
            "psychiat*",
            "antidepressiv*",
            "schwerbehindert*",
            "schwerbehinderung",
            "grad der behinderung",
            "hiv*",
            "adhs",
            "autistisch*",
            "epilep*",
            "diabet*",
            "demenz",
            "alzheimer",
            "parkinson",
            "multiple sklerose",
            "alkoholsucht",
            "drogensucht",
            "spielsucht",
            "magersucht",
            "alkoholabhängig*",
            "drogenabhängig*",
            "bulimie",
            "anorexie",
            "suizid*",
            "selbstmord*",
            "panikattacke*",
            "angststörung*",
            "essstörung*",
            "persönlichkeitsstörung*",
            "bipolar*",
            "schizophren*",
            "ärztliches attest",
            "positiv getestet",
            "diagnosed",
            "diagnosis",
            "chemotherapy",
            "pregnant",
            "pregnancy",
            "antidepressant*",
            "sick leave",
            "psychiatric",
            "adhd",
            "dementia",
            "epilepsy",
            "medical condition",
            "medical record*",
            "hiv-positive",
        ],
        weak: &[
            "*krebs*",
            "*tumor*",
            "depressi*",
            "burnout",
            "burn-out",
            "therapie",
            "*therapie",
            "sucht",
            "behinderung",
            "autismus",
            "asthma",
            "allergi*",
            "migräne",
            "medikament*",
            "klinik*",
            "krankenhaus",
            "reha",
            "befund*",
            "attest",
            "arzttermin",
            "patient*",
            "blutwerte",
            "aids",
            "cancer",
            "depression",
            "therapy",
            "disability",
            "disabled",
            "mental health",
            "autism",
            "medication",
            "prescription",
            "hospital*",
        ],
        orgs: &[],
        cue: r"(?i)\b(?:arzt|ärztin|hausarzt|diagnose|behandlung|symptom\w*|krank\w*|doctor|treatment)\b",
    },
    Lexicon {
        entity: EntityType::Religion,
        strong: &["religionszugehörigkeit", "konfession*", "konvertiert*", "kirchenaustritt", "zeugen jehovas"],
        weak: &[
            "katholi*",
            "evangelisch*",
            "protestant*",
            "muslim*",
            "moslem*",
            "islamisch*",
            "jüdisch*",
            "jude",
            "jüdin",
            "christ",
            "christin",
            "buddhist*",
            "hindu*",
            "atheist*",
            "orthodox*",
            "freikirch*",
            "moschee",
            "synagoge",
            "kirchensteuer",
            "ramadan",
            "gottesdienst",
            "catholic",
            "jewish",
            "christian",
            "religion",
            "religious",
            "religiös*",
        ],
        orgs: &[],
        cue: r"(?i)\b(?:glaube\w*|gläubig\w*|religion\w*|konfession\w*|bete\w*|praktizier\w*|faith|believer)\b",
    },
    Lexicon {
        entity: EntityType::SexualOrientation,
        strong: &[
            "schwul*",
            "lesbisch*",
            "lesbe",
            "bisexuell*",
            "homosexuell*",
            "pansexuell*",
            "asexuell*",
            "queer",
            "coming-out",
            "coming out",
            "transgender",
            "transsexuell*",
            "trans frau",
            "trans mann",
            "nichtbinär*",
            "non-binary",
            "gay",
            "lesbian",
            "bisexual",
            "homosexual",
            "sexuelle orientierung",
            "sexualleben",
            "sex life",
            "sexual orientation",
        ],
        weak: &["sexuell*", "sexual*"],
        orgs: &[],
        cue: r"(?i)\b(?:partner\w*|freund|freundin|beziehung|dating)\b",
    },
    Lexicon {
        entity: EntityType::UnionMembership,
        strong: &["gewerkschaftsmitglied*", "gewerkschaftlich organisiert", "union member*", "union membership"],
        weak: &["gewerkschaft*", "trade union*", "betriebsgruppe"],
        orgs: &[
            "ver.di",
            "Verdi",
            "IG Metall",
            "IG BCE",
            "IG BAU",
            "GEW",
            "EVG",
            "GDL",
            "NGG",
            "Marburger Bund",
            "DGB",
            "dbb",
            "GdP",
            "DPolG",
        ],
        cue: r"(?i)\b(?:mitglied\w*|beigetreten|eingetreten|organisiert|beitrag|vertrauens\w*|member\w*|joined)\b",
    },
    Lexicon {
        entity: EntityType::Ethnicity,
        strong: &[
            "ethnische herkunft",
            "ethnischer herkunft",
            "migrationshintergrund",
            "*stämmig",
            "*stämmige*",
            "abstammung",
            "hautfarbe",
            "ethnicity",
            "ethnic origin",
            "racial origin",
            "afrodeutsch*",
        ],
        weak: &["ethnisch*", "roma", "sinti", "ethnic", "racial"],
        orgs: &[],
        cue: r"(?i)\b(?:herkunft|wurzeln|eltern|familie|origin|background)\b",
    },
    Lexicon {
        entity: EntityType::PoliticalOpinion,
        strong: &[
            "politische einstellung",
            "politische gesinnung",
            "politische meinung",
            "parteimitglied*",
            "parteibuch",
            "political opinion*",
            "political views",
        ],
        weak: &[
            "rechtsextrem*",
            "linksextrem*",
            "rechtsradikal*",
            "linksradikal*",
            "reichsbürger*",
            "querdenker*",
            "kommunist*",
            "faschist*",
            "neonazi*",
            "anarchist*",
            "politisch aktiv",
        ],
        orgs: &[
            "CDU",
            "CSU",
            "SPD",
            "AfD",
            "FDP",
            "Grünen",
            "Grüne",
            "Linke",
            "Linken",
            "Linkspartei",
            "BSW",
            "NPD",
            "ÖDP",
            "Freie Wähler",
            "Freien Wählern",
            "Piratenpartei",
            "Volt",
            "DKP",
            "MLPD",
            "Die PARTEI",
        ],
        cue: r"(?i)\b(?:mitglied\w*|wähle|wählt|wählen|gewählt|wähler\w*|stimme für|stimmt für|unterstütze\w*|engagier\w*|sympathis\w*|eingetreten|ausgetreten|kandidier\w*|vote\w*|member\w*|supporter)\b",
    },
    Lexicon {
        entity: EntityType::Genetic,
        strong: &[
            "gentest*",
            "genetische*",
            "erbkrankheit*",
            "dna-test*",
            "dna-analyse*",
            "genom*",
            "brca*",
            "genetic test*",
            "genetic data",
        ],
        weak: &["genetic", "dna"],
        orgs: &[],
        cue: r"(?i)\b(?:ergebnis\w*|befund\w*|result\w*|test\w*)\b",
    },
    Lexicon {
        entity: EntityType::Biometric,
        strong: &["biometrische daten", "fingerabdruckdaten", "biometric data", "iris-scan*", "irisscan*"],
        weak: &["fingerabdr*", "gesichtserkennung", "biometrisch*", "fingerprint*", "face id", "biometric"],
        orgs: &[],
        cue: r"(?i)\b(?:gespeichert|erfasst|abgleich\w*|stored|enrol\w*|identifi\w*)\b",
    },
    Lexicon {
        entity: EntityType::CriminalRecord,
        strong: &[
            "vorstrafe*",
            "vorbestraft",
            "bewährungsstrafe",
            "auf bewährung",
            "polizeilich bekannt",
            "vorstrafenregister",
            "bundeszentralregister",
            "haftstrafe",
            "criminal record",
            "on probation",
            "felony",
            "convicted of",
        ],
        weak: &[
            "verurteilt",
            "verurteilung",
            "strafverfahren",
            "ermittlungsverfahren",
            "freiheitsstrafe",
            "angeklagt",
            "anklage",
            "inhaftiert",
            "jva",
            "gefängnis",
            "straftat*",
            "strafanzeige",
            "führungszeugnis",
            "convicted",
            "sentenced",
            "arrested",
            "prison",
        ],
        orgs: &[],
        cue: r"(?i)\b(?:gericht\w*|staatsanwalt\w*|polizei|urteil\w*|court|police)\b",
    },
];

lazy_regex!(
    PERSONAL_CUE,
    r"(?i)\b(?:ich|mein|meine|meinem|meinen|meiner|meines|mir|mich|er|ihm|ihn|seine|seinem|seinen|seiner|du|dein|deine|deinem|deinen|deiner|patient|patientin|mitarbeiter|mitarbeiterin|kollege|kollegin|kunde|kundin|bewerber|bewerberin|herr|herrn|frau|hr|fr|sohn|tochter|ehemann|ehefrau|mutter|vater|bruder|schwester|i|i'm|my|me|he|his|him|she|her|employee|colleague|client|customer|applicant|mr|mrs|ms)\b"
);
lazy_regex!(ICD10, r"\b[A-TV-Z][0-9]{2}(?:\.[0-9]{1,2})?\b");
lazy_regex!(ICD10_CONTEXT, r"(?i)(?:\bicd|diagnose|diagnosis|\bdg\.)");
// German criminal court file numbers: "123 Js 456/24".
lazy_regex!(COURT_FILE, r"\b\d{1,4}\s?(?:Js|Ds|Ls|KLs|Ns|Cs|Ss|VRs|BRs)\s?\d{1,6}/\d{2}\b");

fn term_pattern(term: &str) -> String {
    let (pre, rest) = term.strip_prefix('*').map_or(("", term), |r| (r"\w*", r));
    let (body, post) = rest.strip_suffix('*').map_or((rest, ""), |r| (r, r"\w*"));
    let body = regex::escape(body).replace(' ', r"\s+");
    format!("{pre}{body}{post}")
}

fn alternation(terms: &[&str], case_insensitive: bool) -> Option<Regex> {
    if terms.is_empty() {
        return None;
    }
    let alts: Vec<_> = terms.iter().map(|t| term_pattern(t)).collect();
    let flags = if case_insensitive { "(?i)" } else { "" };
    // `\b` does not work after a trailing '.', so use an explicit non-word check.
    Some(Regex::new(&format!(r"{flags}\b(?:{})(?:\b|$)", alts.join("|"))).expect("lexicon regex"))
}

struct Compiled {
    entity: EntityType,
    strong: Option<Regex>,
    weak: Option<Regex>,
    orgs: Option<Regex>,
    cue: Regex,
}

static COMPILED: LazyLock<Vec<Compiled>> = LazyLock::new(|| {
    LEXICONS
        .iter()
        .map(|l| Compiled {
            entity: l.entity,
            strong: alternation(l.strong, true),
            weak: alternation(l.weak, true),
            orgs: alternation(l.orgs, false),
            cue: Regex::new(l.cue).expect("cue regex"),
        })
        .collect()
});

/// Lexicon-based recognizer for GDPR Art. 9/10 special categories.
#[derive(Debug, Default, Clone, Copy)]
pub struct SpecialCategoryRecognizer;

impl Recognizer for SpecialCategoryRecognizer {
    fn id(&self) -> &str {
        "special"
    }
    fn entities(&self) -> &[EntityType] {
        use EntityType::*;
        &[
            Health,
            Religion,
            SexualOrientation,
            UnionMembership,
            Ethnicity,
            PoliticalOpinion,
            Genetic,
            Biometric,
            CriminalRecord,
        ]
    }
    fn detect(&self, text: &str, out: &mut Vec<Detection>) -> Result<(), DetectError> {
        for c in COMPILED.iter() {
            let window = |s, e| sentence_window(text, s, e, 60);
            if let Some(re) = &c.strong {
                for m in re.find_iter(text) {
                    let w = window(m.start(), m.end());
                    let score = if PERSONAL_CUE.is_match(w) { 0.95 } else { 0.85 };
                    out.push(Detection::new(m.start(), m.end(), c.entity, score, "special.strong"));
                }
            }
            if let Some(re) = &c.weak {
                for m in re.find_iter(text) {
                    let w = window(m.start(), m.end());
                    let score = if PERSONAL_CUE.is_match(w) || c.cue.is_match(w) { 0.8 } else { 0.4 };
                    out.push(Detection::new(m.start(), m.end(), c.entity, score, "special.weak"));
                }
            }
            if let Some(re) = &c.orgs {
                for m in re.find_iter(text) {
                    let w = window(m.start(), m.end());
                    let score = if c.cue.is_match(w) { 0.9 } else { 0.4 };
                    out.push(Detection::new(m.start(), m.end(), c.entity, score, "special.org"));
                }
            }
        }
        for m in ICD10.find_iter(text) {
            if ICD10_CONTEXT.is_match(sentence_window(text, m.start(), m.end(), 30)) {
                out.push(Detection::new(m.start(), m.end(), EntityType::Health, 0.9, "special.icd10"));
            }
        }
        for m in COURT_FILE.find_iter(text) {
            out.push(Detection::new(m.start(), m.end(), EntityType::CriminalRecord, 0.9, "special.court_file"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recognizers::test_util::found;

    const BLOCK: f32 = crate::SPECIAL_CATEGORY_THRESHOLD;

    fn blocks(text: &str) -> Vec<(String, EntityType)> {
        found(&SpecialCategoryRecognizer, text, BLOCK)
    }

    #[test]
    fn health() {
        assert_eq!(blocks("Frau Weber ist seit Montag krankgeschrieben.")[0].1, EntityType::Health);
        assert!(blocks("Bei ihm wurde Brustkrebs festgestellt").iter().any(|(t, _)| t == "Brustkrebs"));
        assert_eq!(blocks("ich hab seit wochen depressionen")[0].1, EntityType::Health);
        assert_eq!(blocks("Diagnose laut Arztbrief: F32.1")[1].0, "F32.1");
    }

    #[test]
    fn health_negatives_stay_below_threshold() {
        assert!(blocks("Die Große Depression begann 1929 an der Wall Street.").is_empty());
        assert!(blocks("Das Sternzeichen Krebs beginnt im Juni.").is_empty());
    }

    #[test]
    fn religion_union_politics() {
        assert_eq!(blocks("Er ist katholisch und zahlt Kirchensteuer.")[0].1, EntityType::Religion);
        assert_eq!(blocks("Sie ist seit 2019 Mitglied bei ver.di.")[0].1, EntityType::UnionMembership);
        assert_eq!(blocks("Ich bin Mitglied der SPD.")[0].0, "SPD");
        assert!(blocks("Die SPD hat heute ein Papier vorgestellt.").is_empty());
        assert!(blocks("Wir messen 220 Volt am Ausgang.").is_empty());
    }

    #[test]
    fn orientation_ethnicity_criminal() {
        assert_eq!(blocks("mein kollege ist schwul")[0].1, EntityType::SexualOrientation);
        assert_eq!(blocks("Der Bewerber ist türkischstämmig.")[0].1, EntityType::Ethnicity);
        assert_eq!(blocks("Herr K. ist vorbestraft.")[0].1, EntityType::CriminalRecord);
        assert_eq!(blocks("Az. 123 Js 4567/24")[0].1, EntityType::CriminalRecord);
    }

    #[test]
    fn english_fallback() {
        assert_eq!(blocks("She was diagnosed with diabetes last year.")[0].1, EntityType::Health);
        assert_eq!(blocks("He has a criminal record.")[0].1, EntityType::CriminalRecord);
    }

    #[test]
    fn term_patterns() {
        assert_eq!(term_pattern("*krebs*"), r"\w*krebs\w*");
        assert_eq!(term_pattern("grad der behinderung"), r"grad\s+der\s+behinderung");
    }
}
