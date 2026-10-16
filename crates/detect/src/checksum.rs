//! Checksum algorithms for German and international identifiers.

/// IBAN length per country (ISO 13616 registry, subset covering SEPA and
/// common non-SEPA countries).
pub(crate) fn iban_length(country: &str) -> Option<usize> {
    let len = match country {
        "AD" => 24,
        "AE" => 23,
        "AL" => 28,
        "AT" => 20,
        "AZ" => 28,
        "BA" => 20,
        "BE" => 16,
        "BG" => 22,
        "BH" => 22,
        "BR" => 29,
        "BY" => 28,
        "CH" => 21,
        "CR" => 22,
        "CY" => 28,
        "CZ" => 24,
        "DE" => 22,
        "DK" => 18,
        "DO" => 28,
        "EE" => 20,
        "EG" => 29,
        "ES" => 24,
        "FI" => 18,
        "FO" => 18,
        "FR" => 27,
        "GB" => 22,
        "GE" => 22,
        "GI" => 23,
        "GL" => 18,
        "GR" => 27,
        "GT" => 28,
        "HR" => 21,
        "HU" => 28,
        "IE" => 22,
        "IL" => 23,
        "IQ" => 23,
        "IS" => 26,
        "IT" => 27,
        "JO" => 30,
        "KW" => 30,
        "KZ" => 20,
        "LB" => 28,
        "LC" => 32,
        "LI" => 21,
        "LT" => 20,
        "LU" => 20,
        "LV" => 21,
        "MC" => 27,
        "MD" => 24,
        "ME" => 22,
        "MK" => 19,
        "MR" => 27,
        "MT" => 31,
        "MU" => 30,
        "NL" => 18,
        "NO" => 15,
        "PK" => 24,
        "PL" => 28,
        "PS" => 29,
        "PT" => 25,
        "QA" => 29,
        "RO" => 24,
        "RS" => 22,
        "SA" => 24,
        "SC" => 31,
        "SE" => 24,
        "SI" => 19,
        "SK" => 24,
        "SM" => 27,
        "ST" => 25,
        "SV" => 28,
        "TL" => 23,
        "TN" => 24,
        "TR" => 26,
        "UA" => 29,
        "VA" => 22,
        "VG" => 24,
        "XK" => 20,
        _ => return None,
    };
    Some(len)
}

/// ISO 7064 MOD 97-10 check on a compact, upper-case IBAN.
pub(crate) fn iban_valid(compact: &str) -> bool {
    let b = compact.as_bytes();
    if b.len() < 15 || !b.iter().all(u8::is_ascii_alphanumeric) {
        return false;
    }
    let mut rem: u32 = 0;
    for &c in b[4..].iter().chain(&b[..4]) {
        if c.is_ascii_digit() {
            rem = (rem * 10 + u32::from(c - b'0')) % 97;
        } else {
            let v = u32::from(c.to_ascii_uppercase() - b'A') + 10;
            rem = (rem * 100 + v) % 97;
        }
    }
    rem == 1
}

/// Steuerliche Identifikationsnummer (11 digits): ISO 7064 MOD 11,10 check
/// digit plus the digit distribution rule of § 139b AO.
pub(crate) fn steuer_id_valid(d: &[u8]) -> bool {
    if d.len() != 11 || d[0] == 0 {
        return false;
    }
    let mut counts = [0u8; 10];
    for &x in &d[..10] {
        counts[usize::from(x)] += 1;
    }
    // Exactly one digit occurs twice or three times; all others at most once.
    if counts.iter().filter(|&&c| c > 1).count() != 1 || counts.iter().any(|&c| c > 3) {
        return false;
    }
    // A digit must not occur three times in a row.
    if d[..10].windows(3).any(|w| w[0] == w[1] && w[1] == w[2]) {
        return false;
    }
    let mut product: u32 = 10;
    for &x in &d[..10] {
        let mut sum = (u32::from(x) + product) % 10;
        if sum == 0 {
            sum = 10;
        }
        product = (sum * 2) % 11;
    }
    let mut check = 11 - product;
    if check == 10 {
        check = 0;
    }
    check == u32::from(d[10])
}

/// Sozialversicherungsnummer: `AA DDMMYY L SS P`, where the letter is
/// replaced by its two-digit alphabet position before weighting.
pub(crate) fn svnr_valid(area_and_date: &[u8], letter: char, serial: &[u8], check: u8) -> bool {
    if area_and_date.len() != 8 || serial.len() != 2 || !letter.is_ascii_alphabetic() {
        return false;
    }
    let day = area_and_date[2] * 10 + area_and_date[3];
    let month = area_and_date[4] * 10 + area_and_date[5];
    if !(1..=31).contains(&day) || !(1..=12).contains(&month) {
        return false;
    }
    let pos = (letter.to_ascii_uppercase() as u8) - b'A' + 1;
    let mut digits = Vec::with_capacity(12);
    digits.extend_from_slice(area_and_date);
    digits.push(pos / 10);
    digits.push(pos % 10);
    digits.extend_from_slice(serial);
    const W: [u32; 12] = [2, 1, 2, 5, 7, 1, 2, 1, 2, 1, 2, 1];
    let sum: u32 = digits
        .iter()
        .zip(W)
        .map(|(&d, w)| {
            let p = u32::from(d) * w;
            p / 10 + p % 10
        })
        .sum();
    sum % 10 == u32::from(check)
}

/// ICAO 9303 check digit (weights 7, 3, 1), used by German ID cards and passports.
pub(crate) fn icao_check_digit(s: &str) -> Option<u8> {
    const W: [u32; 3] = [7, 3, 1];
    let mut sum = 0u32;
    for (i, c) in s.chars().enumerate() {
        let v = match c {
            '0'..='9' => c as u32 - '0' as u32,
            'A'..='Z' => c as u32 - 'A' as u32 + 10,
            'a'..='z' => c as u32 - 'a' as u32 + 10,
            '<' => 0,
            _ => return None,
        };
        sum += v * W[i % 3];
    }
    Some((sum % 10) as u8)
}

/// Luhn check for payment card numbers.
pub(crate) fn luhn_valid(d: &[u8]) -> bool {
    if d.len() < 12 {
        return false;
    }
    let sum: u32 = d
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &x)| {
            let x = u32::from(x);
            if i % 2 == 1 {
                let y = x * 2;
                if y > 9 { y - 9 } else { y }
            } else {
                x
            }
        })
        .sum();
    sum % 10 == 0
}

/// ISO 3166-1 alpha-2 codes (plus XK), used to validate BIC country codes.
pub(crate) fn is_country_code(cc: &str) -> bool {
    const CODES: &str = "AD AE AF AG AI AL AM AO AQ AR AS AT AU AW AX AZ BA BB BD BE BF BG BH BI BJ BL BM BN BO BQ BR BS BT BV BW BY BZ CA CC CD CF CG CH CI CK CL CM CN CO CR CU CV CW CX CY CZ DE DJ DK DM DO DZ EC EE EG EH ER ES ET FI FJ FK FM FO FR GA GB GD GE GF GG GH GI GL GM GN GP GQ GR GS GT GU GW GY HK HM HN HR HT HU ID IE IL IM IN IO IQ IR IS IT JE JM JO JP KE KG KH KI KM KN KP KR KW KY KZ LA LB LC LI LK LR LS LT LU LV LY MA MC MD ME MF MG MH MK ML MM MN MO MP MQ MR MS MT MU MV MW MX MY MZ NA NC NE NF NG NI NL NO NP NR NU NZ OM PA PE PF PG PH PK PL PM PN PR PS PT PW PY QA RE RO RS RU RW SA SB SC SD SE SG SH SI SJ SK SL SM SN SO SR SS ST SV SX SY SZ TC TD TF TG TH TJ TK TL TM TN TO TR TT TV TW TZ UA UG UM US UY UZ VA VC VE VG VI VN VU WF WS XK YE YT ZA ZM ZW";
    cc.len() == 2 && CODES.split(' ').any(|c| c == cc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Vec<u8> {
        crate::util::digits(s)
    }

    #[test]
    fn iban() {
        assert!(iban_valid("DE89370400440532013000"));
        assert!(iban_valid("GB82WEST12345698765432"));
        assert!(iban_valid("AT611904300234573201"));
        assert!(!iban_valid("DE89370400440532013001"));
        assert_eq!(iban_length("DE"), Some(22));
    }

    #[test]
    fn steuer_id() {
        // Examples from the public test sets used by python-stdnum and the BZSt docs.
        assert!(steuer_id_valid(&d("36574261809")));
        assert!(steuer_id_valid(&d("86095742719")));
        assert!(!steuer_id_valid(&d("36574261808")), "wrong check digit");
        assert!(!steuer_id_valid(&d("06574261809")), "leading zero");
        assert!(!steuer_id_valid(&d("12345678901")), "no repeated digit");
    }

    #[test]
    fn svnr() {
        // Example from the German Wikipedia article "Versicherungsnummer".
        assert!(svnr_valid(&d("15070649"), 'C', &d("10"), 3));
        assert!(!svnr_valid(&d("15070649"), 'C', &d("10"), 4));
        assert!(!svnr_valid(&d("15071349"), 'C', &d("10"), 3), "month 13");
    }

    #[test]
    fn icao() {
        // Specimen ID card "Erika Mustermann": T22000129 + check digit 3.
        assert_eq!(icao_check_digit("T22000129"), Some(3));
    }

    #[test]
    fn luhn() {
        assert!(luhn_valid(&d("4111111111111111")));
        assert!(luhn_valid(&d("5500000000000004")));
        assert!(!luhn_valid(&d("4111111111111112")));
    }

    #[test]
    fn countries() {
        assert!(is_country_code("DE"));
        assert!(!is_country_code("WO"));
    }
}
