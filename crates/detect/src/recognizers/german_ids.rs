use crate::checksum::{icao_check_digit, steuer_id_valid, svnr_valid};
use crate::util::{around, before, digits, lazy_regex};
use crate::{DetectError, Detection, EntityType, Recognizer};

lazy_regex!(STEUER_ID, r"\b\d{2} ?\d{3} ?\d{3} ?\d{3}\b");
lazy_regex!(STEUER_ID_CONTEXT, r"(?i)(?:steuer[- ]?id|identifikationsnummer|\bidnr\b|\bid-nr\b|\btin\b|tax id)");
lazy_regex!(STEUERNUMMER, r"\b\d{2,3}/\d{3,4}/\d{4,5}\b");
lazy_regex!(STEUERNUMMER_CONTEXT, r"(?i)(?:steuernummer|steuer-nr|st\.?-?nr|\bstnr\b|finanzamt)");
lazy_regex!(SVNR, r"(?i)\b(\d{2}) ?(\d{6}) ?([A-Z]) ?(\d{2})(\d)\b");
lazy_regex!(
    SVNR_CONTEXT,
    r"(?i)(?:sozialversicherungs|sv-nummer|sv-nr|\bsvnr\b|rentenversicherungs|rv-nummer|versicherungsnummer|social security)"
);
lazy_regex!(ID_DOC, r"(?i)\b([CFGHJKLMNPRTVWXYZ][CFGHJKLMNPRTVWXYZ0-9]{8})(\d?)\b");
lazy_regex!(
    ID_DOC_CONTEXT,
    r"(?i)(?:personalausweis|ausweis|dokumentennummer|reisepass|passnummer|pass-nr|\bperso\b|id card|passport|document number)"
);
lazy_regex!(PLATE, r"\b([A-ZÄÖÜ]{1,3})([- ])([A-Z]{1,2}) ?([1-9][0-9]{0,3})([EH])?\b");
lazy_regex!(
    PLATE_CONTEXT,
    r"(?i)(?:kennzeichen|\bkfz\b|nummernschild|\bamtl|fahrzeug|\bpkw\b|\blkw\b|\bauto\b|\bwagen\b|license plate|geparkt|parkt)"
);

/// Steuer-ID (11 digits, MOD 11,10) and Steuernummer (`123/456/78901`).
#[derive(Debug, Default, Clone, Copy)]
pub struct TaxIdRecognizer;

impl Recognizer for TaxIdRecognizer {
    fn id(&self) -> &str {
        "tax_id"
    }
    fn entities(&self) -> &[EntityType] {
        &[EntityType::TaxId]
    }
    fn detect(&self, text: &str, out: &mut Vec<Detection>) -> Result<(), DetectError> {
        for m in STEUER_ID.find_iter(text) {
            let ctx = STEUER_ID_CONTEXT.is_match(before(text, m.start(), 50));
            if steuer_id_valid(&digits(m.as_str())) {
                let score = if ctx { 0.99 } else { 0.8 };
                out.push(Detection::new(m.start(), m.end(), EntityType::TaxId, score, "tax_id.steuer_id"));
            } else if ctx {
                out.push(Detection::new(m.start(), m.end(), EntityType::TaxId, 0.6, "tax_id.steuer_id_format"));
            }
        }
        for m in STEUERNUMMER.find_iter(text) {
            let score = if STEUERNUMMER_CONTEXT.is_match(before(text, m.start(), 50)) { 0.95 } else { 0.55 };
            out.push(Detection::new(m.start(), m.end(), EntityType::TaxId, score, "tax_id.steuernummer"));
        }
        Ok(())
    }
}

/// Sozialversicherungsnummer (`15 070649 C 103`).
#[derive(Debug, Default, Clone, Copy)]
pub struct SocialSecurityRecognizer;

impl Recognizer for SocialSecurityRecognizer {
    fn id(&self) -> &str {
        "svnr"
    }
    fn entities(&self) -> &[EntityType] {
        &[EntityType::SocialSecurityNumber]
    }
    fn detect(&self, text: &str, out: &mut Vec<Detection>) -> Result<(), DetectError> {
        for caps in SVNR.captures_iter(text) {
            let m = caps.get(0).expect("match");
            let area_date = digits(&format!("{}{}", &caps[1], &caps[2]));
            let letter = caps[3].chars().next().expect("letter");
            let serial = digits(&caps[4]);
            let check = digits(&caps[5])[0];
            let ctx = SVNR_CONTEXT.is_match(before(text, m.start(), 50));
            let (score, id) = if svnr_valid(&area_date, letter, &serial, check) {
                (if ctx { 0.99 } else { 0.95 }, "svnr.checksum")
            } else if ctx {
                (0.6, "svnr.format")
            } else {
                continue;
            };
            out.push(Detection::new(m.start(), m.end(), EntityType::SocialSecurityNumber, score, id));
        }
        Ok(())
    }
}

/// Personalausweis / Reisepass document numbers (ICAO 9303 check digit).
#[derive(Debug, Default, Clone, Copy)]
pub struct IdDocumentRecognizer;

impl Recognizer for IdDocumentRecognizer {
    fn id(&self) -> &str {
        "id_document"
    }
    fn entities(&self) -> &[EntityType] {
        &[EntityType::IdDocument]
    }
    fn detect(&self, text: &str, out: &mut Vec<Detection>) -> Result<(), DetectError> {
        for caps in ID_DOC.captures_iter(text) {
            let m = caps.get(0).expect("match");
            let body = &caps[1];
            if !body[1..].bytes().any(|b| b.is_ascii_digit()) {
                continue;
            }
            let ctx = ID_DOC_CONTEXT.is_match(before(text, m.start(), 50));
            let check = &caps[2];
            let (score, id) = if !check.is_empty() {
                let expected = icao_check_digit(body);
                if expected == check.parse().ok() {
                    (if ctx { 0.99 } else { 0.95 }, "id_document.checksum")
                } else if ctx {
                    (0.6, "id_document.format")
                } else {
                    continue;
                }
            } else if ctx {
                (0.85, "id_document.format")
            } else {
                (0.4, "id_document.format")
            };
            out.push(Detection::new(m.start(), m.end(), EntityType::IdDocument, score, id));
        }
        Ok(())
    }
}

/// German district codes (Unterscheidungszeichen). Not exhaustive; unknown
/// codes still match with a lower score.
const DISTRICTS: &str = "A AA AB ABG AC AE AIC AK AM AN AÖ AP AS AUR AW AZ B BA BAD BB BC BGL BI BIR BIT BK BL BM BN BO BOR BOT BRA BS BT BTF BÜS BZ C CB CE CHA CLP CO COC COE CUX CW D DA DAH DAN DAU DD DE DEG DEL DGF DH DL DLG DN DO DON DU DÜW E EA EB EBE ED EE EF EI EIC EL EM EMD EMS EN ER ERB ERH ES ESW EU F FB FD FDS FF FFB FG FL FN FO FR FRG FRI FS FT FÜ G GAP GE GER GF GG GI GL GM GÖ GP GR GS GT GTH GÜ GZ H HA HAL HAM HAS HB HBN HD HDH HE HEF HEI HER HF HG HGW HH HI HL HM HN HO HOL HOM HP HR HRO HS HSK HST HU HVL HWI HX HZ IGB IK IL IN IZ J JL K KA KB KC KE KEH KF KG KH KI KIB KL KLE KN KO KR KS KT KU KÜN KUS KYF L LA LAU LB LD LDK LDS LER LEV LG LI LIF LIP LL LM LÖ LOS LRO LU M MA MB MD ME MEI MG MI MIL MK MKK MM MN MOL MOS MR MS MSE MSH MSP MST MTK MÜ MÜR MW MYK MZ MZG N NB ND NDH NE NEA NES NEW NF NI NK NM NMS NOH NOL NOM NR NU NVP NW NWM OA OAL OB OD OE OF OG OH OHA OHV OHZ OL OPR OS OSL OVP P PA PAF PAN PB PCH PE PF PI PIR PLÖ PM PR PS R RA RD RE REG RO ROS ROW RP RS RT RÜD RÜG RV RW RZ S SAD SAW SB SC SDL SE SG SH SHA SHG SHK SHL SI SIG SIM SK SL SLF SLK SLS SM SN SO SOK SÖM SON SP SPN SR ST STA STD SU SÜW SW SZ TBB TF TIR TO TÖL TR TS TÜ TUT UE UH UL UM UN V VB VEC VER VIE VK VS W WAF WAK WB WE WEN WES WF WHV WI WIL WL WM WN WND WO WOB WST WT WTM WÜ WUG WUN WW WZ Z ZW";

/// Kfz-Kennzeichen (`M-AB 1234`, `HH X 99E`).
#[derive(Debug, Default, Clone, Copy)]
pub struct LicensePlateRecognizer;

impl Recognizer for LicensePlateRecognizer {
    fn id(&self) -> &str {
        "license_plate"
    }
    fn entities(&self) -> &[EntityType] {
        &[EntityType::LicensePlate]
    }
    fn detect(&self, text: &str, out: &mut Vec<Detection>) -> Result<(), DetectError> {
        for caps in PLATE.captures_iter(text) {
            let m = caps.get(0).expect("match");
            let district = &caps[1];
            let chars = district.chars().count() + caps[3].len() + caps[4].len();
            if chars > 8 {
                continue;
            }
            let known = DISTRICTS.split(' ').any(|d| d == district);
            let hyphen = &caps[2] == "-";
            let mut score: f32 = match (hyphen, known) {
                (true, true) => 0.85,
                (true, false) => 0.55,
                (false, true) => 0.6,
                (false, false) => 0.3,
            };
            if PLATE_CONTEXT.is_match(around(text, m.start(), m.end(), 40)) {
                score = (score + 0.3).min(0.97);
            }
            out.push(Detection::new(m.start(), m.end(), EntityType::LicensePlate, score, "license_plate"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recognizers::test_util::{run, texts};

    #[test]
    fn steuer_id() {
        let r = TaxIdRecognizer;
        let t = "Meine Steuer-ID ist 36 574 261 809, die Nummer 12345678901 ist keine.";
        let found = run(&r, t);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].text(t), "36 574 261 809");
        assert!(found[0].score > 0.95);
        // Invalid checksum near a context word is only a low-score candidate.
        assert!(found[1].score < 0.7);
        assert!(run(&r, "Bestellung 12345678901").is_empty());
        assert_eq!(texts(&r, "Steuernummer 143/815/08150", 0.9), ["143/815/08150"]);
    }

    #[test]
    fn svnr() {
        let r = SocialSecurityRecognizer;
        assert_eq!(texts(&r, "SV-Nummer: 15 070649 C 103", 0.9), ["15 070649 C 103"]);
        assert_eq!(texts(&r, "15070649c103", 0.9), ["15070649c103"]);
        assert!(texts(&r, "15 070649 C 104", 0.5).is_empty());
    }

    #[test]
    fn id_document() {
        let r = IdDocumentRecognizer;
        assert_eq!(texts(&r, "Ausweis T220001293", 0.9), ["T220001293"]);
        assert_eq!(texts(&r, "Personalausweisnummer: T22000129", 0.8), ["T22000129"]);
        assert!(texts(&r, "Code T22000129 im Text", 0.5).is_empty());
        assert!(texts(&r, "WXYZTRKLMN", 0.0).is_empty(), "letters only");
    }

    #[test]
    fn plates() {
        let r = LicensePlateRecognizer;
        assert_eq!(texts(&r, "Das Auto mit M-AB 1234 und HH-X 99E", 0.8), ["M-AB 1234", "HH-X 99E"]);
        assert!(texts(&r, "Das IT AG 2024 Projekt", 0.5).is_empty());
    }
}
