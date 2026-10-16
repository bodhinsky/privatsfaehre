//! Built-in regex and checksum recognizers.
//!
//! Each recognizer emits candidates with a score; the [`crate::Engine`]
//! applies thresholds. Low-scoring candidates (e.g. an IBAN with a bad
//! checksum) are still emitted on purpose: `label` mode shows them so that
//! near-misses end up in the dataset.

mod address;
mod banking;
mod contact;
mod date;
mod german_ids;
mod network;
mod org;
mod person;
mod secrets;
mod special;

pub use address::AddressRecognizer;
pub use banking::{BicRecognizer, CreditCardRecognizer, IbanRecognizer};
pub use contact::{EmailRecognizer, PhoneRecognizer, UrlRecognizer};
pub use date::DateRecognizer;
pub use german_ids::{IdDocumentRecognizer, LicensePlateRecognizer, SocialSecurityRecognizer, TaxIdRecognizer};
pub use network::IpRecognizer;
pub use org::OrganizationRecognizer;
pub use person::PersonRecognizer;
pub use secrets::SecretsRecognizer;
pub use special::SpecialCategoryRecognizer;

use crate::Recognizer;

/// All built-in recognizers, in a stable order.
pub fn builtin() -> Vec<Box<dyn Recognizer>> {
    vec![
        Box::new(SpecialCategoryRecognizer),
        Box::new(SecretsRecognizer),
        Box::new(EmailRecognizer),
        Box::new(UrlRecognizer),
        Box::new(PhoneRecognizer),
        Box::new(IbanRecognizer),
        Box::new(BicRecognizer),
        Box::new(CreditCardRecognizer),
        Box::new(TaxIdRecognizer),
        Box::new(SocialSecurityRecognizer),
        Box::new(IdDocumentRecognizer),
        Box::new(LicensePlateRecognizer),
        Box::new(AddressRecognizer),
        Box::new(DateRecognizer),
        Box::new(IpRecognizer),
        Box::new(PersonRecognizer),
        Box::new(OrganizationRecognizer),
    ]
}

#[cfg(test)]
pub(crate) mod test_util {
    use crate::{Detection, EntityType, Recognizer};

    pub fn run(r: &dyn Recognizer, text: &str) -> Vec<Detection> {
        let mut out = Vec::new();
        r.detect(text, &mut out).unwrap();
        out
    }

    /// `(matched text, entity)` pairs at or above `min_score`.
    pub fn found(r: &dyn Recognizer, text: &str, min_score: f32) -> Vec<(String, EntityType)> {
        run(r, text).into_iter().filter(|d| d.score >= min_score).map(|d| (d.text(text).to_owned(), d.entity)).collect()
    }

    pub fn texts(r: &dyn Recognizer, text: &str, min_score: f32) -> Vec<String> {
        found(r, text, min_score).into_iter().map(|(t, _)| t).collect()
    }
}
