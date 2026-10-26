//! Review-mode clipboard session, shared by `privatsphaere guard` and the app.
//!
//! [`Session::poll`] analyses every clipboard change and returns an
//! [`Event`]. The decisions that must not depend on a user interface are made
//! here:
//!
//! * Special-category data (GDPR Art. 9/10) and detection errors replace the
//!   clipboard with a notice immediately, without asking (fail closed).
//! * Personal data is only pseudonymized after [`Session::confirm`]; the
//!   preview leaves nothing in the vault ([`vault::Vault::rollback`]).
//! * Writes never overwrite something the user copied in the meantime.
//! * Copied text with this session's placeholders is offered for reveal, and
//!   only restored after [`Session::accept_reveal`].
//!
//! The vault lives in memory only and is wiped on drop or
//! [`Session::clear_vault`].

pub mod audit;
pub mod config;
mod time;

pub use audit::{Action, AuditLog};
pub use time::now_rfc3339;

use clip::{ClipError, ClipboardBackend, Watcher};
use detect::{Detection, Engine, EntityType, Verdict};
use vault::zeroize::Zeroizing;
use vault::{RevealMode, Revealed, Vault, VaultError};

pub const BLOCK_NOTICE: &str = "⛔ privatsphaere: Inhalt blockiert, enthält besondere Kategorien personenbezogener \
Daten (Art. 9/10 DSGVO). Content blocked: special-category personal data (GDPR Art. 9/10).";
pub const ERROR_NOTICE: &str = "⛔ privatsphaere: Inhalt blockiert, Prüfung fehlgeschlagen. \
Content blocked: detection failed.";

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error(transparent)]
    Clip(#[from] ClipError),
    #[error(transparent)]
    Vault(#[from] VaultError),
}

/// What happened to a clipboard change.
#[derive(Debug)]
pub enum Event {
    /// Nothing found; clipboard left as is.
    Clean { chars: usize },
    /// Findings that need a decision: [`Session::confirm`] or [`Session::decline`].
    Review(Review),
    /// Special-category data; the clipboard already holds [`BLOCK_NOTICE`].
    Blocked(Blocked),
    /// Detection failed; the clipboard already holds [`ERROR_NOTICE`].
    Failed { error: String },
    /// Copied text contains placeholders from this session.
    Reveal(RevealOffer),
}

/// Copied text with findings, waiting for confirmation.
#[derive(Debug)]
pub struct Review {
    text: Zeroizing<String>,
    pub detections: Vec<Detection>,
    /// Which detections to pseudonymize; all `true` initially.
    pub keep: Vec<bool>,
}

impl Review {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn toggle(&mut self, i: usize) {
        if let Some(k) = self.keep.get_mut(i) {
            *k = !*k;
        }
    }

    pub fn selected(&self) -> impl Iterator<Item = &Detection> {
        self.detections.iter().zip(&self.keep).filter(|(_, k)| **k).map(|(d, _)| d)
    }
}

/// Blocked text, kept only so the UI can show where the findings are.
#[derive(Debug)]
pub struct Blocked {
    text: Zeroizing<String>,
    pub blocking: Vec<Detection>,
    pub detections: Vec<Detection>,
}

impl Blocked {
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Distinct special categories found, in a stable order.
    pub fn categories(&self) -> Vec<EntityType> {
        let mut out: Vec<EntityType> = self.blocking.iter().map(|d| d.entity).collect();
        out.sort_by_key(|e| e.as_str());
        out.dedup();
        out
    }
}

/// Placeholders from this session found in the clipboard.
#[derive(Debug)]
pub struct RevealOffer {
    current: Zeroizing<String>,
    revealed: Revealed,
}

impl RevealOffer {
    /// Number of placeholders that would be restored.
    pub fn restored(&self) -> usize {
        self.revealed.restored
    }

    /// Placeholder-looking tokens this session did not issue.
    pub fn unknown(&self) -> &[String] {
        &self.revealed.unknown
    }
}

/// Result of an action that writes the clipboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Replaced,
    /// The user copied something else meanwhile; nothing was written.
    ChangedMeanwhile,
    /// Nothing to write (no selected findings, no placeholders).
    Unchanged,
}

pub struct Session<B> {
    engine: Engine,
    watcher: Watcher<B>,
    vault: Vault,
    audit: Option<AuditLog>,
    warnings: Vec<String>,
}

impl<B> std::fmt::Debug for Session<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session").field("vault", &self.vault).field("audit", &self.audit.is_some()).finish()
    }
}

impl<B: ClipboardBackend> Session<B> {
    /// Start a session. Content already in the clipboard is not analysed.
    pub fn new(engine: Engine, backend: B, audit: Option<AuditLog>) -> Result<Self, ClipError> {
        Ok(Self { engine, watcher: Watcher::new(backend)?, vault: Vault::new(), audit, warnings: Vec::new() })
    }

    pub fn vault_len(&self) -> usize {
        self.vault.len()
    }

    pub fn audit_enabled(&self) -> bool {
        self.audit.is_some()
    }

    /// Wipe the vault; earlier placeholders can no longer be revealed.
    pub fn clear_vault(&mut self) {
        self.vault.clear();
    }

    /// Non-fatal problems since the last call (audit log write errors).
    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    pub fn backend_mut(&mut self) -> &mut B {
        self.watcher.backend_mut()
    }

    /// Check the clipboard for a change and analyse it.
    pub fn poll(&mut self) -> Result<Option<Event>, ClipError> {
        match self.watcher.poll()? {
            Some(ev) => self.handle(ev.text).map(Some),
            None => Ok(None),
        }
    }

    fn handle(&mut self, text: Zeroizing<String>) -> Result<Event, ClipError> {
        let chars = text.chars().count();
        if !self.vault.is_empty() {
            let revealed = self.vault.reveal(&text);
            if revealed.restored > 0 {
                return Ok(Event::Reveal(RevealOffer { current: text, revealed }));
            }
        }
        let analysis = match self.engine.analyze(&text) {
            Ok(a) => a,
            Err(e) => {
                self.watcher.replace_if_current(&text, ERROR_NOTICE)?;
                self.audit(Action::Error, [], chars);
                return Ok(Event::Failed { error: e.to_string() });
            }
        };
        Ok(match analysis.verdict() {
            Verdict::Clean => Event::Clean { chars },
            Verdict::Block => {
                self.watcher.replace_if_current(&text, BLOCK_NOTICE)?;
                self.audit(Action::Blocked, analysis.blocking.iter().chain(&analysis.detections), chars);
                Event::Blocked(Blocked { text, blocking: analysis.blocking, detections: analysis.detections })
            }
            Verdict::Redact => {
                let keep = vec![true; analysis.detections.len()];
                Event::Review(Review { text, detections: analysis.detections, keep })
            }
        })
    }

    fn pseudonymize(&mut self, r: &Review) -> Result<String, VaultError> {
        let spans = r.selected().map(|d| vault::Span { start: d.start, end: d.end, kind: d.entity.as_str() });
        self.vault.pseudonymize(r.text(), spans)
    }

    /// The text [`Session::confirm`] would write. Leaves the vault unchanged.
    pub fn preview(&mut self, r: &Review) -> Result<String, VaultError> {
        let mark = self.vault.len();
        let out = self.pseudonymize(r);
        self.vault.rollback(mark);
        out
    }

    /// Pseudonymize the selected findings and write the result.
    pub fn confirm(&mut self, r: &Review) -> Result<Outcome, SessionError> {
        let chars = r.text.chars().count();
        if r.selected().next().is_none() {
            self.audit(Action::Allowed, &r.detections, chars);
            return Ok(Outcome::Unchanged);
        }
        let mark = self.vault.len();
        let text = self.pseudonymize(r)?;
        if !self.watcher.replace_if_current(r.text(), &text)? {
            self.vault.rollback(mark);
            return Ok(Outcome::ChangedMeanwhile);
        }
        let used: Vec<&Detection> = r.selected().collect();
        self.audit(Action::Redacted, used, chars);
        Ok(Outcome::Replaced)
    }

    /// Keep the original; nothing is stored in the vault.
    pub fn decline(&mut self, r: &Review) {
        self.audit(Action::Allowed, &r.detections, r.text.chars().count());
    }

    /// Restore the originals into the clipboard.
    pub fn accept_reveal(&mut self, o: &RevealOffer) -> Result<Outcome, ClipError> {
        if o.revealed.restored == 0 {
            return Ok(Outcome::Unchanged);
        }
        if !self.watcher.replace_if_current(&o.current, &o.revealed.text)? {
            return Ok(Outcome::ChangedMeanwhile);
        }
        if let Some(log) = &mut self.audit
            && let Err(e) = log.revealed(o.revealed.restored, o.current.chars().count())
        {
            self.warnings.push(format!("audit log: {e}"));
        }
        Ok(Outcome::Replaced)
    }

    /// Reveal offer for the current clipboard content, also matching bare
    /// `PERSON_1`. `None` if the clipboard holds no text.
    pub fn reveal_clipboard(&mut self) -> Result<Option<RevealOffer>, ClipError> {
        let Some(current) = self.watcher.backend_mut().read_text()? else { return Ok(None) };
        let revealed = self.vault.reveal_with(&current, RevealMode::Lenient);
        Ok(Some(RevealOffer { current, revealed }))
    }

    fn audit<'d>(&mut self, action: Action, dets: impl IntoIterator<Item = &'d Detection>, chars: usize) {
        if let Some(log) = &mut self.audit
            && let Err(e) = log.record(action, dets, chars)
        {
            self.warnings.push(format!("audit log: {e}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clip::MemoryClipboard;

    const MAIL: &str = "Hallo Herr Thomas Müller, bitte überweisen Sie an DE89 3704 0044 0532 0130 00.";

    fn session() -> Session<MemoryClipboard> {
        let engine = Engine::new(Default::default()).unwrap();
        Session::new(engine, MemoryClipboard { content: Some("start".into()) }, None).unwrap()
    }

    fn copy(s: &mut Session<MemoryClipboard>, text: &str) -> Event {
        s.backend_mut().content = Some(text.to_owned());
        s.poll().unwrap().expect("event")
    }

    fn clipboard(s: &mut Session<MemoryClipboard>) -> String {
        s.backend_mut().content.clone().unwrap()
    }

    #[test]
    fn preview_leaves_vault_empty_and_matches_confirm() {
        let mut s = session();
        let Event::Review(r) = copy(&mut s, MAIL) else { panic!("review") };
        let p = s.preview(&r).unwrap();
        assert_eq!(s.vault_len(), 0);
        assert_eq!(clipboard(&mut s), MAIL, "preview writes nothing");
        assert_eq!(s.confirm(&r).unwrap(), Outcome::Replaced);
        assert_eq!(clipboard(&mut s), p);
        assert_eq!(s.vault_len(), 2);
    }

    #[test]
    fn confirm_does_not_overwrite_newer_copy() {
        let mut s = session();
        let Event::Review(r) = copy(&mut s, MAIL) else { panic!("review") };
        s.backend_mut().content = Some("etwas anderes".into());
        assert_eq!(s.confirm(&r).unwrap(), Outcome::ChangedMeanwhile);
        assert_eq!(clipboard(&mut s), "etwas anderes");
        assert_eq!(s.vault_len(), 0);
    }

    #[test]
    fn block_writes_notice_before_any_ui() {
        let mut s = session();
        let Event::Blocked(b) = copy(&mut s, "Er ist wegen Depression krankgeschrieben.") else { panic!("block") };
        assert_eq!(clipboard(&mut s), BLOCK_NOTICE);
        assert_eq!(b.categories(), [EntityType::Health]);
    }

    #[test]
    fn reveal_round_trip_and_unselected() {
        let mut s = session();
        let Event::Review(mut r) = copy(&mut s, MAIL) else { panic!("review") };
        r.toggle(0);
        s.confirm(&r).unwrap();
        let out = clipboard(&mut s);
        assert!(out.contains("Thomas Müller") && out.contains("[IBAN_1]"), "{out}");

        let Event::Reveal(o) = copy(&mut s, "IBAN [IBAN_1] und [PERSON_9]") else { panic!("reveal") };
        assert_eq!(o.restored(), 1);
        assert_eq!(o.unknown(), ["[PERSON_9]"]);
        assert_eq!(s.accept_reveal(&o).unwrap(), Outcome::Replaced);
        assert_eq!(clipboard(&mut s), "IBAN DE89 3704 0044 0532 0130 00 und [PERSON_9]");
        assert!(s.poll().unwrap().is_none(), "own write is not analysed again");

        s.backend_mut().content = Some("bare IBAN_1".into());
        let _ = s.poll();
        let o = s.reveal_clipboard().unwrap().unwrap();
        assert_eq!(o.restored(), 1, "lenient reveal on demand");
    }
}
