//! Reversible pseudonymization vault.
//!
//! The vault assigns typed, numbered placeholders (`[PERSON_1]`, `[IBAN_1]`)
//! to original values and can restore them in text that comes back from an
//! LLM ([`Vault::reveal`]).
//!
//! * Consistency: the same value (after per-type normalisation) always gets
//!   the same placeholder within one vault, so `Thomas Müller` and
//!   `thomas  müller` are both `[PERSON_1]`.
//! * Memory hygiene: every original value and every normalised lookup key is
//!   held in a [`SecretString`] that is zeroized on drop, and buffers are
//!   sized up front so no unzeroized copies are left behind by reallocation.
//!   Text returned by [`Vault::reveal`] contains originals and is returned as
//!   [`Zeroizing<String>`].
//! * The vault is independent of the detection crate: it only sees spans and
//!   kind strings.
//!
//! Encrypted persistence is intentionally not implemented yet; it will be an
//! explicit opt-in feature (see `ARCHITECTURE.md`).

use regex::Regex;
use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::LazyLock;
use zeroize::{Zeroize, Zeroizing};

pub use zeroize;

/// A string that is wiped (full capacity) when dropped. `Debug` never prints
/// the content.
pub struct SecretString(String);

impl SecretString {
    /// Copy `s` into an exactly-sized buffer.
    pub fn new(s: &str) -> Self {
        let mut buf = String::with_capacity(s.len());
        buf.push_str(s);
        Self(buf)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl Drop for SecretString {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SecretString(<{} bytes>)", self.0.len())
    }
}

impl PartialEq for SecretString {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for SecretString {}
impl Hash for SecretString {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

/// A span to pseudonymize. Byte offsets; `kind` becomes the placeholder
/// prefix and must match `[A-Z][A-Z0-9_]*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span<'a> {
    pub start: usize,
    pub end: usize,
    pub kind: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VaultError {
    #[error("span {start}..{end} is out of bounds or not on a character boundary")]
    InvalidSpan { start: usize, end: usize },
    #[error("spans overlap or are not sorted at {start}..{end}")]
    Overlap { start: usize, end: usize },
    #[error("invalid placeholder kind `{0}`")]
    InvalidKind(String),
}

/// How to compare values for placeholder reuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Normalization {
    /// Trim, collapse whitespace, lower-case (names, addresses, free text).
    Text,
    /// Keep only letters and digits, lower-case (IBAN, IDs, plates, cards).
    Compact,
    /// Digits only; `+49`/`0049` become a leading `0`.
    Phone,
}

/// Normalisation used for a placeholder kind.
pub fn normalization_for(kind: &str) -> Normalization {
    match kind {
        "IBAN" | "BIC" | "CREDIT_CARD" | "TAX_ID" | "SOCIAL_SECURITY_NUMBER" | "ID_DOCUMENT" | "LICENSE_PLATE" => {
            Normalization::Compact
        }
        "PHONE" => Normalization::Phone,
        _ => Normalization::Text,
    }
}

/// Normalise into a pre-sized secret buffer (no reallocation copies).
fn normalize(kind: &str, value: &str) -> SecretString {
    let mode = normalization_for(kind);
    let capacity: usize = value.chars().map(|c| c.to_lowercase().map(char::len_utf8).sum::<usize>()).sum::<usize>() + 1;
    let mut out = String::with_capacity(capacity);
    match mode {
        Normalization::Text => {
            let mut pending_space = false;
            for c in value.trim().chars() {
                if c.is_whitespace() {
                    pending_space = true;
                    continue;
                }
                if pending_space {
                    out.push(' ');
                    pending_space = false;
                }
                out.extend(c.to_lowercase());
            }
        }
        Normalization::Compact => {
            for c in value.chars().filter(|c| c.is_alphanumeric()) {
                out.extend(c.to_lowercase());
            }
        }
        Normalization::Phone => {
            let v = value.trim_start();
            let rest = if let Some(r) = v.strip_prefix("+49") {
                out.push('0');
                r
            } else if let Some(r) = v.strip_prefix("0049") {
                out.push('0');
                r
            } else {
                v
            };
            // "+49 (0)30" -> "030", not "0030".
            let rest = rest.trim_start().strip_prefix("(0)").unwrap_or(rest);
            out.extend(rest.chars().filter(char::is_ascii_digit));
        }
    }
    debug_assert!(out.capacity() == capacity, "normalisation must not reallocate");
    SecretString(out)
}

struct Entry {
    token: String,
    original: SecretString,
}

static KIND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Z][A-Z0-9_]*$").expect("regex"));
// `[PERSON_1]`, `[ PERSON_1 ]`, `\[PERSON_1\]`, `[person_1]`.
static BRACKETED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\\?\[\s*([A-Z][A-Z0-9_]*_\d+)\s*\\?\]").expect("regex"));
static BARE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b([A-Z][A-Z0-9_]*_\d+)\b").expect("regex"));

/// How tolerant [`Vault::reveal_with`] is with placeholder spellings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RevealMode {
    /// Bracketed placeholders, any case, optional inner whitespace or
    /// Markdown-escaped brackets.
    #[default]
    Bracketed,
    /// Additionally restore bare `PERSON_1` (upper case only).
    Lenient,
}

/// Output of [`Vault::reveal`].
#[derive(Debug)]
pub struct Revealed {
    /// Text with originals restored. Zeroized on drop.
    pub text: Zeroizing<String>,
    /// Number of placeholders replaced.
    pub restored: usize,
    /// Placeholder-looking tokens this vault did not issue.
    pub unknown: Vec<String>,
}

/// Session vault. Drop it (or call [`Vault::clear`]) to forget all values.
#[derive(Default)]
pub struct Vault {
    entries: Vec<Entry>,
    by_value: HashMap<(String, SecretString), usize>,
    by_token: HashMap<String, usize>,
    counters: HashMap<String, u32>,
}

impl fmt::Debug for Vault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Vault").field("entries", &self.entries.len()).finish_non_exhaustive()
    }
}

impl Vault {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Placeholder for `original`, creating one if needed.
    pub fn placeholder(&mut self, kind: &str, original: &str) -> Result<&str, VaultError> {
        if !KIND.is_match(kind) {
            return Err(VaultError::InvalidKind(kind.to_owned()));
        }
        let key = (kind.to_owned(), normalize(kind, original));
        if let Some(&i) = self.by_value.get(&key) {
            return Ok(&self.entries[i].token);
        }
        let n = self.counters.entry(kind.to_owned()).or_insert(0);
        *n += 1;
        let token = format!("[{kind}_{n}]");
        let i = self.entries.len();
        self.entries.push(Entry { token: token.clone(), original: SecretString::new(original) });
        self.by_token.insert(token.to_ascii_uppercase(), i);
        self.by_value.insert(key, i);
        Ok(&self.entries[i].token)
    }

    /// Replace `spans` (sorted, non-overlapping) in `text` with placeholders.
    pub fn pseudonymize<'a>(
        &mut self,
        text: &str,
        spans: impl IntoIterator<Item = Span<'a>>,
    ) -> Result<String, VaultError> {
        let mut out = String::with_capacity(text.len());
        let mut pos = 0;
        for s in spans {
            let valid = s.start < s.end
                && s.end <= text.len()
                && text.is_char_boundary(s.start)
                && text.is_char_boundary(s.end);
            if !valid {
                return Err(VaultError::InvalidSpan { start: s.start, end: s.end });
            }
            if s.start < pos {
                return Err(VaultError::Overlap { start: s.start, end: s.end });
            }
            out.push_str(&text[pos..s.start]);
            out.push_str(self.placeholder(s.kind, &text[s.start..s.end])?);
            pos = s.end;
        }
        out.push_str(&text[pos..]);
        Ok(out)
    }

    /// Original value for a placeholder token such as `[PERSON_1]`.
    pub fn lookup(&self, token: &str) -> Option<&str> {
        self.by_token.get(&token.to_ascii_uppercase()).map(|&i| self.entries[i].original.expose())
    }

    /// Restore originals in `text` (default [`RevealMode::Bracketed`]).
    pub fn reveal(&self, text: &str) -> Revealed {
        self.reveal_with(text, RevealMode::default())
    }

    pub fn reveal_with(&self, text: &str, mode: RevealMode) -> Revealed {
        // Collect replacements first, then build the output in a buffer of the
        // exact final size so no partial copies are reallocated away.
        let mut hits: Vec<(usize, usize, usize)> = Vec::new();
        let mut unknown = Vec::new();
        for c in BRACKETED.captures_iter(text) {
            let m = c.get(0).expect("match");
            match self.by_token.get(&format!("[{}]", c[1].to_ascii_uppercase())) {
                Some(&i) => hits.push((m.start(), m.end(), i)),
                None => unknown.push(m.as_str().to_owned()),
            }
        }
        if mode == RevealMode::Lenient {
            for c in BARE.captures_iter(text) {
                let m = c.get(1).expect("group");
                if hits.iter().any(|&(s, e, _)| m.start() < e && s < m.end()) {
                    continue;
                }
                if let Some(&i) = self.by_token.get(&format!("[{}]", &c[1])) {
                    hits.push((m.start(), m.end(), i));
                }
            }
            hits.sort_by_key(|h| h.0);
        }
        let len = hits.iter().fold(text.len(), |acc, &(s, e, i)| acc - (e - s) + self.entries[i].original.0.len());
        let mut out = Zeroizing::new(String::with_capacity(len));
        let mut pos = 0;
        for &(s, e, i) in &hits {
            out.push_str(&text[pos..s]);
            out.push_str(self.entries[i].original.expose());
            pos = e;
        }
        out.push_str(&text[pos..]);
        Revealed { text: out, restored: hits.len(), unknown }
    }

    /// Wipe and forget all values. Counters restart at 1.
    pub fn clear(&mut self) {
        // Dropping the entries zeroizes every SecretString.
        self.entries.clear();
        self.by_value.clear();
        self.by_token.clear();
        self.counters.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: usize, end: usize, kind: &str) -> Span<'_> {
        Span { start, end, kind }
    }

    fn spans_of<'a>(text: &str, parts: &[(&str, &'a str)]) -> Vec<Span<'a>> {
        let mut from = 0;
        parts
            .iter()
            .map(|(needle, kind)| {
                let start = from + text[from..].find(needle).expect("needle");
                from = start + needle.len();
                Span { start, end: from, kind }
            })
            .collect()
    }

    #[test]
    fn consistent_placeholders() {
        let mut v = Vault::new();
        let text = "Thomas Müller (0171 1234567) schrieb an thomas  müller und +49 171 1234567. Anna Schmidt auch.";
        let spans = spans_of(
            text,
            &[
                ("Thomas Müller", "PERSON"),
                ("0171 1234567", "PHONE"),
                ("thomas  müller", "PERSON"),
                ("+49 171 1234567", "PHONE"),
                ("Anna Schmidt", "PERSON"),
            ],
        );
        let out = v.pseudonymize(text, spans).unwrap();
        assert_eq!(out, "[PERSON_1] ([PHONE_1]) schrieb an [PERSON_1] und [PHONE_1]. [PERSON_2] auch.");
        assert_eq!(v.len(), 3);
        // Consistency also holds across calls in the same session.
        let again = v.pseudonymize("Anna Schmidt", [span(0, 12, "PERSON")]).unwrap();
        assert_eq!(again, "[PERSON_2]");
    }

    #[test]
    fn compact_normalisation() {
        let mut v = Vault::new();
        let a = v.placeholder("IBAN", "DE89 3704 0044 0532 0130 00").unwrap().to_owned();
        let b = v.placeholder("IBAN", "de89370400440532013000").unwrap().to_owned();
        assert_eq!(a, b);
        // Same value under a different kind is a different placeholder.
        assert_ne!(v.placeholder("SECRET", "de89370400440532013000").unwrap(), a);
    }

    #[test]
    fn reveal_roundtrip_and_variants() {
        let mut v = Vault::new();
        let text = "Bitte Anna Schmidt anrufen: 0171 1234567";
        let p = v.pseudonymize(text, spans_of(text, &[("Anna Schmidt", "PERSON"), ("0171 1234567", "PHONE")])).unwrap();
        assert_eq!(v.reveal(&p).text.as_str(), text);

        let llm = "Sehr geehrte [person_1], Ihre Nummer \\[PHONE_1\\] ist notiert. [ PERSON_1 ] und [PERSON_9].";
        let r = v.reveal(llm);
        assert_eq!(
            r.text.as_str(),
            "Sehr geehrte Anna Schmidt, Ihre Nummer 0171 1234567 ist notiert. Anna Schmidt und [PERSON_9]."
        );
        assert_eq!(r.restored, 3);
        assert_eq!(r.unknown, ["[PERSON_9]"]);

        let bare = v.reveal_with("Antwort an PERSON_1.", RevealMode::Lenient);
        assert_eq!(bare.text.as_str(), "Antwort an Anna Schmidt.");
        assert_eq!(v.reveal("Antwort an PERSON_1.").restored, 0);
    }

    #[test]
    fn rejects_bad_spans() {
        let mut v = Vault::new();
        assert!(matches!(v.pseudonymize("abc", [span(1, 5, "X")]), Err(VaultError::InvalidSpan { .. })));
        assert!(matches!(v.pseudonymize("äb", [span(1, 2, "X")]), Err(VaultError::InvalidSpan { .. })));
        assert!(matches!(
            v.pseudonymize("abcdef", [span(0, 3, "X"), span(2, 4, "X")]),
            Err(VaultError::Overlap { .. })
        ));
        assert!(matches!(v.placeholder("person", "x"), Err(VaultError::InvalidKind(_))));
    }

    #[test]
    fn clear_forgets_everything() {
        let mut v = Vault::new();
        v.placeholder("PERSON", "Anna").unwrap();
        v.clear();
        assert!(v.is_empty());
        assert_eq!(v.lookup("[PERSON_1]"), None);
        assert_eq!(v.placeholder("PERSON", "Bert").unwrap(), "[PERSON_1]");
    }

    #[test]
    fn debug_does_not_leak() {
        let mut v = Vault::new();
        v.placeholder("PERSON", "Geheimname").unwrap();
        assert!(!format!("{v:?}").contains("Geheimname"));
        assert!(!format!("{:?}", SecretString::new("Geheimname")).contains("Geheimname"));
    }
}
