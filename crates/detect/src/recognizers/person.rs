//! Rule-based person-name recognizer.
//!
//! This is a high-precision baseline that works on lower-case text; broad
//! recall for names is the job of the NER backend (see [`crate::ner`]).

use crate::util::{is_capitalized, lazy_regex};
use crate::{DetectError, Detection, EntityType, Recognizer};
use std::collections::HashSet;
use std::sync::LazyLock;

const FIRST_NAMES: &str = "anna maria marie sophie sofia emma mia hannah hanna emilia lena lea leonie lina laura julia sarah sara lisa lara johanna clara klara charlotte amelie luisa louisa ella nele paula greta frieda ida mila nina katharina christina kristina stefanie stephanie sabine susanne petra andrea claudia monika ursula renate karin gabriele birgit heike martina nicole sandra melanie jessica jennifer katrin kathrin anja silke simone tanja daniela manuela ute ulrike elke ingrid helga gisela brigitte erika inge christa hildegard margarete irene barbara eva angelika beate carina jana jasmin vanessa franziska theresa teresa miriam natalie nathalie alina carolin caroline annika isabel isabell marlene magdalena elisabeth anne antje doris edith elena olga natalia irina svetlana tatjana fatma ayse ayşe elif zeynep emine merve aylin leyla yasmin amina fatima aisha alexander maximilian paul leon lukas lucas luca luka felix jonas elias noah ben finn fynn henry emil anton theo jakob jacob david niklas nico moritz julian tim tom philipp phillip simon fabian florian tobias sebastian daniel michael thomas andreas stefan stephan christian markus marcus martin peter frank jürgen juergen klaus wolfgang uwe dieter manfred werner gerhard horst helmut günter guenter günther heinz hans karl walter rolf bernd ralf ralph jörg joerg dirk holger matthias mathias oliver sven torsten thorsten jens lars kai jan jannik marco mario patrick dennis kevin marcel benjamin johannes jonathan lennard lennart leonard luis louis valentin vincent samuel oskar oscar mats matteo ole erik eric robert richard georg josef joseph franz friedrich wilhelm otto heinrich hermann ernst rudolf norbert reinhard rainer detlef volker harald hartmut ulrich wilfried siegfried bernhard gregor konstantin dominik benedikt hendrik henrik carsten karsten kurt alfred mehmet mustafa ahmet ahmed ali hasan hassan hüseyin ibrahim murat emre can burak mohammed mohammad muhammad omar yusuf ivan dmitri sergej andrej piotr tomasz krzysztof max mark august john james william george charles mike chris alex sam kim robin";

/// First names that are also common words; only matched with a surname or honorific.
const AMBIGUOUS: &str = "max mark frank ernst kai jan can august will art mai rose ben";

const SURNAMES: &str = "müller mueller schmidt schneider fischer weber meyer meier mayer maier wagner becker schulz hoffmann schäfer schaefer koch bauer richter klein wolf schröder schroeder neumann schwarz zimmermann braun krüger krueger hofmann hartmann lange schmitt werner schmitz krause lehmann schmid schulze köhler koehler herrmann könig koenig walter huber kaiser fuchs peters lang scholz möller moeller weiß weiss jung hahn schubert vogel friedrich keller günther frank berger winkler roth beck lorenz baumann franke albrecht schuster simon ludwig böhm boehm winter kraus martin schumacher krämer kraemer vogt stein jäger jaeger otto sommer groß gross seidel heinrich brandt haas schreiber graf schulte dietrich ziegler kuhn kühn pohl engel horn busch bergmann thomas voigt sauer arnold wolff pfeiffer mustermann musterfrau yılmaz yilmaz kaya demir şahin sahin çelik celik yıldız yildiz öztürk ozturk aydın aydin özdemir ozdemir arslan doğan dogan kılıç kilic nowak kowalski wiśniewski wisniewski wójcik wojcik kowalczyk kamiński kaminski lewandowski zieliński zielinski nguyen tran ivanov petrov popescu smith johnson williams brown jones miller davis wilson taylor";

const HONORIFICS: &[&str] = &["herr", "herrn", "frau", "hr", "fr", "mr", "mrs", "ms", "miss", "kollege", "kollegin"];
const TITLES: &[&str] = &["dr", "prof", "med", "dipl", "ing", "rer", "nat"];
const GREETINGS: &[&str] = &["hallo", "hi", "hey", "liebe", "lieber", "liebes", "servus", "moin", "dear", "hello"];
/// Words that follow a greeting but are not names.
const NOT_NAMES: &[&str] = &[
    "grüße",
    "grüsse",
    "gruß",
    "gruss",
    "team",
    "zusammen",
    "alle",
    "leute",
    "kollegen",
    "kolleginnen",
    "freunde",
    "mitglieder",
    "kunden",
    "damen",
    "herren",
    "everyone",
    "all",
    "there",
    "guys",
    "folks",
    "ihr",
    "du",
    "sie",
    "wir",
    "nochmal",
    "again",
    "und",
    "bescheid",
    "dank",
    "danke",
    "oder",
];

static FIRST: LazyLock<HashSet<&'static str>> = LazyLock::new(|| FIRST_NAMES.split(' ').collect());
static AMBIG: LazyLock<HashSet<&'static str>> = LazyLock::new(|| AMBIGUOUS.split(' ').collect());
static LAST: LazyLock<HashSet<&'static str>> = LazyLock::new(|| SURNAMES.split(' ').collect());

lazy_regex!(WORD, r"\p{L}[\p{L}'’\-]*");
lazy_regex!(
    SIGNATURE,
    r"(?m)(?:[Gg]rüße|[Gg]rüßen|[Gg]ruß|[Gg]rüsse|[Gg]russ|[Rr]egards|[Cc]heers|[Bb]est|VG|LG|MfG|vg|lg|mfg|[Dd]anke)[ \t]*[,!.]?[ \t]*\r?\n[ \t]*(?P<name>\p{L}[\p{L}\-]+(?:[ \t]\p{L}[\p{L}\-]+)?)[ \t]*$"
);

struct Tok<'a> {
    start: usize,
    end: usize,
    word: &'a str,
    lower: String,
}

/// Names after honorifics/greetings, in signatures, and from a first-name /
/// surname gazetteer (case-insensitive, so lower-case chat text works).
#[derive(Debug, Default, Clone, Copy)]
pub struct PersonRecognizer;

impl Recognizer for PersonRecognizer {
    fn id(&self) -> &str {
        "person"
    }
    fn entities(&self) -> &[EntityType] {
        &[EntityType::Person]
    }
    fn detect(&self, text: &str, out: &mut Vec<Detection>) -> Result<(), DetectError> {
        let toks: Vec<Tok<'_>> = WORD
            .find_iter(text)
            .map(|m| Tok { start: m.start(), end: m.end(), word: m.as_str(), lower: m.as_str().to_lowercase() })
            .collect();
        // Tokens are adjacent if separated only by spaces/tabs (optionally a '.').
        let adjacent = |a: &Tok<'_>, b: &Tok<'_>| {
            let gap = &text[a.end..b.start];
            !gap.is_empty() && gap.len() <= 3 && gap.trim_start_matches('.').chars().all(|c| c == ' ' || c == '\t')
        };
        let followed_by_dot = |t: &Tok<'_>| text[t.end..].starts_with('.');
        let is_name_word = |t: &Tok<'_>| {
            !NOT_NAMES.contains(&t.lower.as_str())
                && (is_capitalized(t.word) || FIRST.contains(t.lower.as_str()) || LAST.contains(t.lower.as_str()))
        };

        let mut covered_until = 0usize;
        let mut i = 0;
        while i < toks.len() {
            let t = &toks[i];
            if t.start < covered_until {
                i += 1;
                continue;
            }
            let lw = t.lower.as_str();
            let abbrev = matches!(lw, "hr" | "fr");
            let honorific = HONORIFICS.contains(&lw) && (!abbrev || followed_by_dot(t));
            let greeting = GREETINGS.contains(&lw);

            if honorific || greeting {
                let mut j = i + 1;
                while j < toks.len() && TITLES.contains(&toks[j].lower.as_str()) && followed_by_dot(&toks[j]) {
                    j += 1;
                }
                // A greeting followed by an honorific is handled on the next iteration.
                let next_is_honorific = j < toks.len() && HONORIFICS.contains(&toks[j].lower.as_str());
                if j < toks.len() && !next_is_honorific && adjacent(&toks[j - 1], &toks[j]) && is_name_word(&toks[j]) {
                    let first = &toks[j];
                    let mut end = first.end;
                    if let Some(next) = toks.get(j + 1) {
                        let extend = adjacent(first, next)
                            && !NOT_NAMES.contains(&next.lower.as_str())
                            && (LAST.contains(next.lower.as_str())
                                || (FIRST.contains(first.lower.as_str()) && is_capitalized(next.word)));
                        if extend {
                            end = next.end;
                        }
                    }
                    let score = match (honorific, is_capitalized(first.word)) {
                        (true, true) => 0.9,
                        (true, false) => 0.85,
                        (false, _) => 0.8,
                    };
                    let id = if honorific { "person.honorific" } else { "person.greeting" };
                    out.push(Detection::new(first.start, end, EntityType::Person, score, id));
                    covered_until = end;
                }
                i += 1;
                continue;
            }

            if FIRST.contains(lw) {
                let next = toks.get(i + 1).filter(|n| adjacent(t, n) && !NOT_NAMES.contains(&n.lower.as_str()));
                let with_surname = next
                    .filter(|n| LAST.contains(n.lower.as_str()) || (is_capitalized(t.word) && is_capitalized(n.word)));
                if let Some(n) = with_surname {
                    let score = if LAST.contains(n.lower.as_str()) { 0.92 } else { 0.75 };
                    out.push(Detection::new(t.start, n.end, EntityType::Person, score, "person.gazetteer"));
                    covered_until = n.end;
                    i += 2;
                    continue;
                }
                if !AMBIG.contains(lw) {
                    let score = if is_capitalized(t.word) { 0.65 } else { 0.55 };
                    out.push(Detection::new(t.start, t.end, EntityType::Person, score, "person.gazetteer"));
                    covered_until = t.end;
                }
            }
            i += 1;
        }

        for caps in SIGNATURE.captures_iter(text) {
            let m = caps.name("name").expect("group name");
            let first_word = m.as_str().split_whitespace().next().unwrap_or("");
            let lower = first_word.to_lowercase();
            if NOT_NAMES.contains(&lower.as_str()) || !(is_capitalized(first_word) || FIRST.contains(lower.as_str())) {
                continue;
            }
            if let Some(d) = out.iter_mut().find(|d| d.start == m.start() && d.end == m.end()) {
                d.score = d.score.max(0.85);
                continue;
            }
            out.push(Detection::new(m.start(), m.end(), EntityType::Person, 0.85, "person.signature"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recognizers::test_util::texts;

    #[test]
    fn honorifics() {
        let r = PersonRecognizer;
        assert_eq!(
            texts(&r, "Sehr geehrte Frau Dr. Schneider, Herr Thomas Müller hat angerufen.", 0.8),
            ["Schneider", "Thomas Müller"]
        );
        assert_eq!(texts(&r, "frau müller hat gesagt", 0.8), ["müller"]);
        assert!(texts(&r, "meine Frau hat gekocht", 0.5).is_empty());
    }

    #[test]
    fn lowercase_chat() {
        let r = PersonRecognizer;
        assert_eq!(
            texts(&r, "hab grad mit thomas müller telefoniert, anna kommt später", 0.5),
            ["thomas müller", "anna"]
        );
    }

    #[test]
    fn greetings_and_signatures() {
        let r = PersonRecognizer;
        assert_eq!(
            texts(&r, "Hallo Jens,\nbitte melde dich.\n\nViele Grüße\nSandra Gröning", 0.8),
            ["Jens", "Sandra Gröning"]
        );
        assert!(texts(&r, "Hallo zusammen, Liebe Grüße", 0.5).is_empty());
    }

    #[test]
    fn ambiguous_first_names_need_surname() {
        let r = PersonRecognizer;
        assert!(texts(&r, "max. 5 Personen, im Ernst, frank und frei", 0.5).is_empty());
        assert_eq!(texts(&r, "Max Mustermann", 0.5), ["Max Mustermann"]);
    }
}
