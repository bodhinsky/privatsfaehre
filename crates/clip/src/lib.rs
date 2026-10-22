//! Clipboard access and change detection.
//!
//! [`Watcher`] polls a [`ClipboardBackend`] and reports new text content. It
//! remembers what it wrote itself, so replacing the clipboard with a redacted
//! version does not trigger another round. Clipboard text is returned as
//! [`Zeroizing<String>`] because it may contain personal data.
//!
//! The watcher only reads; the decision what to write back belongs to the
//! caller (review mode in the app until `watch` mode is validated).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use zeroize::Zeroizing;

#[derive(Debug, thiserror::Error)]
pub enum ClipError {
    #[error("clipboard unavailable: {0}")]
    Unavailable(String),
    #[error("clipboard access failed: {0}")]
    Access(String),
}

/// Minimal text clipboard interface.
pub trait ClipboardBackend {
    /// Current text, or `None` if the clipboard holds no text.
    fn read_text(&mut self) -> Result<Option<Zeroizing<String>>, ClipError>;
    fn write_text(&mut self, text: &str) -> Result<(), ClipError>;
}

fn fingerprint(text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// A change seen by [`Watcher::poll`].
#[derive(Debug)]
pub struct ClipEvent {
    pub text: Zeroizing<String>,
}

/// Polling watcher with change detection.
#[derive(Debug)]
pub struct Watcher<B> {
    backend: B,
    last: Option<u64>,
    own_write: Option<u64>,
}

impl<B: ClipboardBackend> Watcher<B> {
    /// Start watching; content present at start-up is treated as already seen.
    pub fn new(mut backend: B) -> Result<Self, ClipError> {
        let last = backend.read_text()?.map(|t| fingerprint(&t));
        Ok(Self { backend, last, own_write: None })
    }

    /// Return new text if the clipboard changed since the last poll and the
    /// change was not made by [`Watcher::replace`].
    pub fn poll(&mut self) -> Result<Option<ClipEvent>, ClipError> {
        let Some(text) = self.backend.read_text()? else {
            self.last = None;
            return Ok(None);
        };
        let fp = fingerprint(&text);
        if self.last == Some(fp) {
            return Ok(None);
        }
        self.last = Some(fp);
        if self.own_write == Some(fp) {
            return Ok(None);
        }
        Ok(Some(ClipEvent { text }))
    }

    /// Write `text` and remember it so the next poll ignores it.
    pub fn replace(&mut self, text: &str) -> Result<(), ClipError> {
        self.backend.write_text(text)?;
        let fp = fingerprint(text);
        self.own_write = Some(fp);
        self.last = Some(fp);
        Ok(())
    }

    /// Like [`Watcher::replace`], but only if the clipboard still holds
    /// `current`. Returns `false` (and writes nothing) if the user copied
    /// something else in the meantime; the next poll reports that change.
    pub fn replace_if_current(&mut self, current: &str, text: &str) -> Result<bool, ClipError> {
        let now = self.backend.read_text()?;
        if now.as_deref().map(|t| t.as_str()) != Some(current) {
            return Ok(false);
        }
        self.replace(text)?;
        Ok(true)
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }
}

/// The real system clipboard.
#[cfg(feature = "system")]
pub struct SystemClipboard(arboard::Clipboard);

#[cfg(feature = "system")]
impl std::fmt::Debug for SystemClipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SystemClipboard")
    }
}

#[cfg(feature = "system")]
impl SystemClipboard {
    pub fn new() -> Result<Self, ClipError> {
        arboard::Clipboard::new().map(Self).map_err(|e| ClipError::Unavailable(e.to_string()))
    }
}

#[cfg(feature = "system")]
impl ClipboardBackend for SystemClipboard {
    fn read_text(&mut self) -> Result<Option<Zeroizing<String>>, ClipError> {
        match self.0.get_text() {
            Ok(t) => Ok(Some(Zeroizing::new(t))),
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(e) => Err(ClipError::Access(e.to_string())),
        }
    }
    fn write_text(&mut self, text: &str) -> Result<(), ClipError> {
        self.0.set_text(text).map_err(|e| ClipError::Access(e.to_string()))
    }
}

/// In-memory backend for tests and headless use.
#[derive(Debug, Default)]
pub struct MemoryClipboard {
    pub content: Option<String>,
}

impl ClipboardBackend for MemoryClipboard {
    fn read_text(&mut self) -> Result<Option<Zeroizing<String>>, ClipError> {
        Ok(self.content.clone().map(Zeroizing::new))
    }
    fn write_text(&mut self, text: &str) -> Result<(), ClipError> {
        self.content = Some(text.to_owned());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_changes_and_ignores_own_writes() {
        let mut w = Watcher::new(MemoryClipboard { content: Some("alt".into()) }).unwrap();
        assert!(w.poll().unwrap().is_none(), "start-up content is not an event");

        w.backend_mut().content = Some("IBAN DE89 3704 0044 0532 0130 00".into());
        let ev = w.poll().unwrap().expect("change");
        assert!(ev.text.starts_with("IBAN"));
        assert!(w.poll().unwrap().is_none(), "same content twice is one event");

        w.replace("IBAN [IBAN_1]").unwrap();
        assert!(w.poll().unwrap().is_none(), "own write is ignored");

        w.backend_mut().content = Some("neu".into());
        assert!(w.poll().unwrap().is_some());

        // Not overwritten when the user copied something else meanwhile.
        assert!(!w.replace_if_current("alt", "[X_1]").unwrap());
        assert_eq!(w.backend_mut().content.as_deref(), Some("neu"));
        assert!(w.replace_if_current("neu", "[X_1]").unwrap());
        assert!(w.poll().unwrap().is_none());
        w.backend_mut().content = Some("neu".into());
        assert!(w.poll().unwrap().is_some());

        // Copying the same text again after something else counts again.
        w.backend_mut().content = None;
        assert!(w.poll().unwrap().is_none());
        w.backend_mut().content = Some("neu".into());
        assert!(w.poll().unwrap().is_some());
    }
}
