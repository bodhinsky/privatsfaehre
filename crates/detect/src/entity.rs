use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// What happens to text that contains an entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    /// Replace the span with a typed, numbered placeholder.
    Redact,
    /// GDPR Art. 9/10 special category: the whole paste is blocked with a
    /// warning. Never configurable.
    Block,
}

macro_rules! entities {
    ($( $(#[$m:meta])* $variant:ident => $name:literal, $disp:ident; )*) => {
        /// Closed set of entity types. The string form is used for placeholders
        /// (`[PERSON_1]`) and in the dataset format.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub enum EntityType {
            $( $(#[$m])* #[serde(rename = $name)] $variant, )*
        }

        impl EntityType {
            /// Every entity type, in declaration order.
            pub const ALL: &'static [EntityType] = &[$(EntityType::$variant),*];

            /// Canonical upper-case name, e.g. `"PERSON"`.
            pub const fn as_str(self) -> &'static str {
                match self { $(EntityType::$variant => $name),* }
            }

            /// Fixed disposition of this type. Not configurable by design.
            pub const fn disposition(self) -> Disposition {
                match self { $(EntityType::$variant => Disposition::$disp),* }
            }
        }
    };
}

entities! {
    /// Name of a natural person.
    Person => "PERSON", Redact;
    /// Company, authority, association.
    Organization => "ORGANIZATION", Redact;
    /// City, region or other place (mostly from NER).
    Location => "LOCATION", Redact;
    /// Street address, optionally including postcode and city.
    Address => "ADDRESS", Redact;
    /// German postcode (PLZ) together with a city name.
    PostalCode => "POSTAL_CODE", Redact;
    Phone => "PHONE", Redact;
    Email => "EMAIL", Redact;
    Url => "URL", Redact;
    Iban => "IBAN", Redact;
    Bic => "BIC", Redact;
    CreditCard => "CREDIT_CARD", Redact;
    /// Steuer-ID (11 digits) or Steuernummer.
    TaxId => "TAX_ID", Redact;
    /// Sozialversicherungsnummer / Rentenversicherungsnummer.
    SocialSecurityNumber => "SOCIAL_SECURITY_NUMBER", Redact;
    /// Personalausweis or passport document number.
    IdDocument => "ID_DOCUMENT", Redact;
    /// Kfz-Kennzeichen.
    LicensePlate => "LICENSE_PLATE", Redact;
    Date => "DATE", Redact;
    IpAddress => "IP_ADDRESS", Redact;
    /// Provider API key or bearer token.
    ApiKey => "API_KEY", Redact;
    PrivateKey => "PRIVATE_KEY", Redact;
    Jwt => "JWT", Redact;
    ConnectionString => "CONNECTION_STRING", Redact;
    /// Password or other secret value (e.g. from `.env` lines).
    Secret => "SECRET", Redact;
    /// Art. 9: data concerning health.
    Health => "HEALTH", Block;
    /// Art. 9: religious or philosophical beliefs.
    Religion => "RELIGION", Block;
    /// Art. 9: sex life or sexual orientation.
    SexualOrientation => "SEXUAL_ORIENTATION", Block;
    /// Art. 9: trade union membership.
    UnionMembership => "UNION_MEMBERSHIP", Block;
    /// Art. 9: racial or ethnic origin.
    Ethnicity => "ETHNICITY", Block;
    /// Art. 9: political opinions.
    PoliticalOpinion => "POLITICAL_OPINION", Block;
    /// Art. 9: genetic data.
    Genetic => "GENETIC", Block;
    /// Art. 9: biometric data for identification.
    Biometric => "BIOMETRIC", Block;
    /// Art. 10: criminal convictions and offences.
    CriminalRecord => "CRIMINAL_RECORD", Block;
}

impl EntityType {
    /// True for GDPR Art. 9/10 special categories.
    pub const fn is_special_category(self) -> bool {
        matches!(self.disposition(), Disposition::Block)
    }

    /// The GDPR article that makes this a special category, if any.
    pub const fn gdpr_article(self) -> Option<&'static str> {
        match self {
            EntityType::CriminalRecord => Some("Art. 10"),
            t if t.is_special_category() => Some("Art. 9"),
            _ => None,
        }
    }

    /// All special-category types.
    pub fn special_categories() -> impl Iterator<Item = EntityType> {
        Self::ALL.iter().copied().filter(|t| t.is_special_category())
    }
}

impl fmt::Display for EntityType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when parsing an unknown entity name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown entity type `{0}`")]
pub struct ParseEntityError(pub String);

impl FromStr for EntityType {
    type Err = ParseEntityError;

    /// Case-insensitive; accepts the canonical name plus a few common aliases
    /// (`PER`, `ORG`, `LOC`, `PLZ`, `STEUER_ID`, `SVNR`, …).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let norm = s.trim().to_ascii_uppercase().replace(['-', ' '], "_");
        if let Some(t) = Self::ALL.iter().find(|t| t.as_str() == norm) {
            return Ok(*t);
        }
        let t = match norm.as_str() {
            "PER" | "NAME" => EntityType::Person,
            "ORG" => EntityType::Organization,
            "LOC" | "CITY" => EntityType::Location,
            "PLZ" | "POSTCODE" | "ZIP" => EntityType::PostalCode,
            "TEL" | "TELEFON" | "PHONE_NUMBER" => EntityType::Phone,
            "MAIL" | "E_MAIL" => EntityType::Email,
            "STEUER_ID" | "STEUERID" | "STEUERNUMMER" | "TIN" => EntityType::TaxId,
            "SVNR" | "SV_NUMMER" | "RVNR" | "SSN" => EntityType::SocialSecurityNumber,
            "PERSONALAUSWEIS" | "AUSWEIS" | "PASSPORT" | "ID_CARD" => EntityType::IdDocument,
            "KFZ" | "KENNZEICHEN" | "PLATE" => EntityType::LicensePlate,
            "IP" => EntityType::IpAddress,
            "PASSWORD" | "TOKEN" => EntityType::Secret,
            _ => return Err(ParseEntityError(s.to_owned())),
        };
        Ok(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_name_matches_as_str() {
        for t in EntityType::ALL {
            let json = serde_json::to_string(t).unwrap();
            assert_eq!(json, format!("\"{}\"", t.as_str()));
            let back: EntityType = serde_json::from_str(&json).unwrap();
            assert_eq!(back, *t);
            assert_eq!(t.as_str().parse::<EntityType>().unwrap(), *t);
        }
    }

    #[test]
    fn special_categories_block() {
        let special: Vec<_> = EntityType::special_categories().collect();
        assert_eq!(special.len(), 9);
        for t in special {
            assert_eq!(t.disposition(), Disposition::Block);
            assert!(t.gdpr_article().is_some());
        }
        assert_eq!(EntityType::CriminalRecord.gdpr_article(), Some("Art. 10"));
        assert_eq!(EntityType::Iban.disposition(), Disposition::Redact);
    }

    #[test]
    fn aliases() {
        assert_eq!("plz".parse::<EntityType>().unwrap(), EntityType::PostalCode);
        assert_eq!("Steuer-ID".parse::<EntityType>().unwrap(), EntityType::TaxId);
        assert!("nope".parse::<EntityType>().is_err());
    }
}
