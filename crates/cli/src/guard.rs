//! `guard`: clipboard guard in review mode, in the terminal.
//!
//! Every clipboard change is analysed. Findings are shown and the clipboard
//! is only replaced with the pseudonymized text after confirmation.
//! Special-category data (GDPR Art. 9/10) and detection errors replace the
//! clipboard with a notice right away, without asking. Text that contains
//! placeholders issued in this session can be restored ("reveal").
//!
//! The vault lives in memory for the lifetime of the command and is wiped on
//! exit or with the `c` command.

use crate::audit::{Action, AuditLog};
use crate::term::{self, Mark};
use clip::{ClipError, ClipboardBackend, Watcher};
use detect::{Detection, Engine, Verdict};
use std::collections::BTreeSet;
use std::io::{self, BufRead, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;
use vault::zeroize::Zeroizing;
use vault::{RevealMode, Vault};

pub const BLOCK_NOTICE: &str = "⛔ privatsphaere: Inhalt blockiert, enthält besondere Kategorien personenbezogener \
Daten (Art. 9/10 DSGVO). Content blocked: special-category personal data (GDPR Art. 9/10).";
pub const ERROR_NOTICE: &str = "⛔ privatsphaere: Inhalt blockiert, Prüfung fehlgeschlagen. \
Content blocked: detection failed.";

/// Terminal input and output, abstracted so sessions can be scripted in tests.
pub trait Console {
    /// Next input line, waiting at most `timeout` (`None` = wait forever).
    fn next_line(&mut self, timeout: Option<Duration>) -> Input;
    fn say(&mut self, line: &str);
    /// Draw attention (terminal bell).
    fn bell(&mut self) {}
}

#[derive(Debug, PartialEq, Eq)]
pub enum Input {
    Line(String),
    Timeout,
    Closed,
}

/// Reads stdin on a background thread so the clipboard can be polled while
/// waiting for commands.
pub struct Terminal {
    rx: Receiver<String>,
}

impl Terminal {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self { rx }
    }
}

impl Console for Terminal {
    fn next_line(&mut self, timeout: Option<Duration>) -> Input {
        let r = match timeout {
            Some(t) => self.rx.recv_timeout(t),
            None => self.rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match r {
            Ok(l) => Input::Line(l.trim_end_matches('\r').to_owned()),
            Err(RecvTimeoutError::Timeout) => Input::Timeout,
            Err(RecvTimeoutError::Disconnected) => Input::Closed,
        }
    }
    fn say(&mut self, line: &str) {
        println!("{line}");
    }
    fn bell(&mut self) {
        print!("\x07");
        let _ = io::stdout().flush();
    }
}

const HELP: &str = "Commands: r = reveal clipboard now   c = clear vault   s = status   h = help   q = quit";

pub struct Guard<'a, B, C> {
    pub engine: &'a Engine,
    pub watcher: Watcher<B>,
    pub console: C,
    pub vault: Vault,
    pub audit: Option<AuditLog>,
    pub color: bool,
    pub interval: Duration,
}

#[derive(PartialEq)]
enum Flow {
    Continue,
    Quit,
}

type Res<T> = Result<T, Box<dyn std::error::Error>>;

impl<B: ClipboardBackend, C: Console> Guard<'_, B, C> {
    pub fn run(&mut self) -> Res<()> {
        self.console.say("Watching the clipboard (review mode). Nothing leaves this machine.");
        self.console.say(HELP);
        let mut last_error: Option<String> = None;
        loop {
            match self.console.next_line(Some(self.interval)) {
                Input::Line(l) => {
                    if self.command(l.trim())? == Flow::Quit {
                        break;
                    }
                }
                Input::Closed => break,
                Input::Timeout => {}
            }
            match self.watcher.poll() {
                Ok(Some(ev)) => {
                    last_error = None;
                    if self.handle(&ev.text)? == Flow::Quit {
                        break;
                    }
                }
                Ok(None) => {}
                // Clipboard owners come and go; report once and keep polling.
                Err(e) => {
                    let msg = e.to_string();
                    if last_error.as_deref() != Some(&msg) {
                        self.console.say(&format!("⚠ {msg}"));
                        last_error = Some(msg);
                    }
                }
            }
        }
        self.vault.clear();
        self.console.say("Vault wiped. Bye.");
        Ok(())
    }

    fn ask(&mut self, prompt: &str) -> Option<String> {
        self.console.say(prompt);
        match self.console.next_line(None) {
            Input::Line(l) => Some(l.trim().to_owned()),
            _ => None,
        }
    }

    fn audit<'d>(&mut self, action: Action, dets: impl IntoIterator<Item = &'d Detection>, chars: usize) {
        if let Some(log) = &mut self.audit
            && let Err(e) = log.record(action, dets, chars)
        {
            self.console.say(&format!("⚠ audit log: {e}"));
        }
    }

    fn write(&mut self, current: &str, text: &str) -> Result<bool, ClipError> {
        let done = self.watcher.replace_if_current(current, text)?;
        if !done {
            self.console.say("  Clipboard changed in the meantime; left it alone.");
        }
        Ok(done)
    }

    fn command(&mut self, cmd: &str) -> Res<Flow> {
        match cmd {
            "" => {}
            "q" | "quit" | "exit" => return Ok(Flow::Quit),
            "h" | "help" | "?" => self.console.say(HELP),
            "s" | "status" => {
                let n = self.vault.len();
                let audit = if self.audit.is_some() { "on" } else { "off" };
                self.console.say(&format!("{n} placeholders in the vault; audit log {audit}; review mode."));
            }
            "c" | "clear" => {
                self.vault.clear();
                self.console.say("Vault wiped; placeholders issued so far can no longer be revealed.");
            }
            "r" | "reveal" => match self.watcher.backend_mut().read_text()? {
                Some(text) => {
                    let r = self.vault.reveal_with(&text, RevealMode::Lenient);
                    self.reveal(&text, r, false)?;
                }
                None => self.console.say("Clipboard holds no text."),
            },
            other => self.console.say(&format!("Unknown command `{other}`. {HELP}")),
        }
        Ok(Flow::Continue)
    }

    fn reveal(&mut self, text: &str, r: vault::Revealed, ask: bool) -> Res<()> {
        if !r.unknown.is_empty() {
            self.console.say(&format!("  Not from this session: {}", r.unknown.join(", ")));
        }
        if r.restored == 0 {
            self.console.say("  No placeholders from this session found.");
            return Ok(());
        }
        if ask {
            self.console.bell();
            self.console.say(&format!("\n↩ Copied text contains {} placeholder(s) from this session.", r.restored));
            // Default no: the text might be on its way back into a chat.
            let a = self.ask("  Restore the originals into the clipboard? [y/N]").unwrap_or_default();
            if !matches!(a.as_str(), "y" | "Y" | "j" | "J" | "yes" | "ja") {
                self.console.say("  Left as is.");
                return Ok(());
            }
        }
        if self.write(text, &r.text)? {
            self.console.say(&format!("  ✓ Restored {} placeholder(s). Only paste this locally.", r.restored));
            if let Some(log) = &mut self.audit
                && let Err(e) = log.revealed(r.restored, text.chars().count())
            {
                self.console.say(&format!("⚠ audit log: {e}"));
            }
        }
        Ok(())
    }

    fn handle(&mut self, text: &str) -> Res<Flow> {
        let chars = text.chars().count();
        if !self.vault.is_empty() {
            let r = self.vault.reveal(text);
            if r.restored > 0 {
                self.reveal(text, r, true)?;
                return Ok(Flow::Continue);
            }
        }
        let analysis = match self.engine.analyze(text) {
            Ok(a) => a,
            Err(e) => {
                // Fail closed.
                self.write(text, ERROR_NOTICE)?;
                self.console.bell();
                self.console.say(&format!("\n⛔ Detection failed ({e}); clipboard replaced with a notice."));
                self.audit(Action::Error, [], chars);
                return Ok(Flow::Continue);
            }
        };
        match analysis.verdict() {
            Verdict::Clean => {
                self.console.say(&format!("✓ clean ({chars} chars)"));
            }
            Verdict::Block => {
                self.write(text, BLOCK_NOTICE)?;
                self.console.bell();
                let mut marks: Vec<Mark> = analysis.blocking.iter().map(|d| Mark::new(d.start, d.end, true)).collect();
                marks.extend(analysis.detections.iter().map(|d| Mark::new(d.start, d.end, false)));
                let shown = Zeroizing::new(term::highlight(text, &marks, self.color));
                self.console.say(&format!("\n{}", *shown));
                let cats: BTreeSet<String> = analysis
                    .blocking
                    .iter()
                    .map(|d| format!("{} ({})", d.entity.as_str(), d.entity.gdpr_article().unwrap_or("Art. 9")))
                    .collect();
                self.console.say(&format!(
                    "⛔ BLOCKED: {}. The clipboard now holds a notice instead; this cannot be overridden.",
                    cats.into_iter().collect::<Vec<_>>().join(", ")
                ));
                self.audit(Action::Blocked, analysis.blocking.iter().chain(&analysis.detections), chars);
            }
            Verdict::Redact => return self.review(text, &analysis.detections),
        }
        Ok(Flow::Continue)
    }

    /// Show findings and replace the clipboard after confirmation.
    fn review(&mut self, text: &str, detections: &[Detection]) -> Res<Flow> {
        let chars = text.chars().count();
        let mut keep = vec![true; detections.len()];
        self.console.bell();
        loop {
            let marks: Vec<Mark> = detections
                .iter()
                .zip(&keep)
                .filter(|(_, k)| **k)
                .map(|(d, _)| Mark::new(d.start, d.end, false))
                .collect();
            let shown = Zeroizing::new(term::highlight(text, &marks, self.color));
            self.console.say(&format!("\n── Copied ──\n{}", *shown));
            for (i, (d, k)) in detections.iter().zip(&keep).enumerate() {
                let line = Zeroizing::new(format!(
                    "  {:>2} {} {:<20} {:.2} {:?}",
                    i + 1,
                    if *k { "✓" } else { "·" },
                    d.entity.as_str(),
                    d.score,
                    d.text(text)
                ));
                self.console.say(&line);
            }
            let mark = self.vault.len();
            let spans = detections.iter().zip(&keep).filter(|(_, k)| **k).map(|(d, _)| vault::Span {
                start: d.start,
                end: d.end,
                kind: d.entity.as_str(),
            });
            let pseudonymized = self.vault.pseudonymize(text, spans)?;
            self.console.say(&format!("── Pseudonymized ──\n{pseudonymized}"));
            let Some(a) = self.ask("Replace the clipboard? [Y/n], or numbers to toggle (e.g. 2,4), q = quit guard")
            else {
                self.vault.rollback(mark);
                return Ok(Flow::Quit);
            };
            match a.as_str() {
                "" | "y" | "Y" | "j" | "J" | "yes" | "ja" => {
                    let used: Vec<&Detection> =
                        detections.iter().zip(&keep).filter(|(_, k)| **k).map(|(d, _)| d).collect();
                    if used.is_empty() {
                        self.vault.rollback(mark);
                        self.console.say("  Nothing selected; clipboard unchanged.");
                        self.audit(Action::Allowed, detections, chars);
                    } else if self.write(text, &pseudonymized)? {
                        self.console.say(&format!("  ✓ Clipboard pseudonymized ({} spans).", used.len()));
                        self.audit(Action::Redacted, used, chars);
                    } else {
                        self.vault.rollback(mark);
                    }
                    return Ok(Flow::Continue);
                }
                "n" | "N" | "no" | "nein" => {
                    self.vault.rollback(mark);
                    self.console.say("  Clipboard unchanged.");
                    self.audit(Action::Allowed, detections, chars);
                    return Ok(Flow::Continue);
                }
                "q" => {
                    self.vault.rollback(mark);
                    return Ok(Flow::Quit);
                }
                list => {
                    self.vault.rollback(mark);
                    let mut ok = true;
                    for part in list.split([',', ' ']).filter(|p| !p.is_empty()) {
                        match part.parse::<usize>() {
                            Ok(n) if (1..=keep.len()).contains(&n) => keep[n - 1] = !keep[n - 1],
                            _ => ok = false,
                        }
                    }
                    if !ok {
                        self.console.say(&format!("  Expected y, n, q or numbers from 1 to {}.", keep.len()));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    type Shared = Rc<RefCell<Option<String>>>;

    struct SharedClipboard(Shared);

    impl ClipboardBackend for SharedClipboard {
        fn read_text(&mut self) -> Result<Option<Zeroizing<String>>, ClipError> {
            Ok(self.0.borrow().clone().map(Zeroizing::new))
        }
        fn write_text(&mut self, text: &str) -> Result<(), ClipError> {
            *self.0.borrow_mut() = Some(text.to_owned());
            Ok(())
        }
    }

    enum Step {
        /// The user copies this text (happens on the next idle poll).
        Copy(&'static str),
        /// The user types this line.
        Type(&'static str),
    }

    struct Script {
        clip: Shared,
        steps: VecDeque<Step>,
        out: Vec<String>,
    }

    impl Console for Script {
        fn next_line(&mut self, timeout: Option<Duration>) -> Input {
            match self.steps.pop_front() {
                None => Input::Closed,
                Some(Step::Type(l)) => Input::Line(l.to_owned()),
                Some(Step::Copy(t)) => {
                    assert!(timeout.is_some(), "copy step while a prompt waits for an answer");
                    *self.clip.borrow_mut() = Some(t.to_owned());
                    Input::Timeout
                }
            }
        }
        fn say(&mut self, line: &str) {
            self.out.push(line.to_owned());
        }
    }

    fn run(steps: Vec<Step>, audit: Option<AuditLog>) -> (Shared, Vec<String>) {
        let engine = Engine::new(Default::default()).unwrap();
        let clip: Shared = Rc::new(RefCell::new(Some("start".into())));
        let watcher = Watcher::new(SharedClipboard(clip.clone())).unwrap();
        let console = Script { clip: clip.clone(), steps: steps.into(), out: Vec::new() };
        let mut g = Guard {
            engine: &engine,
            watcher,
            console,
            vault: Vault::new(),
            audit,
            color: false,
            interval: Duration::from_millis(1),
        };
        g.run().unwrap();
        (clip, g.console.out)
    }

    const MAIL: &str = "Hallo Herr Thomas Müller, bitte überweisen Sie an DE89 3704 0044 0532 0130 00.";

    #[test]
    fn redact_then_reveal() {
        let (clip, out) = run(
            vec![
                Step::Copy(MAIL),
                Step::Type(""),
                Step::Copy("Sehr geehrter [PERSON_1], die IBAN [IBAN_1] ist korrekt."),
                Step::Type("y"),
            ],
            None,
        );
        let text = clip.borrow().clone().unwrap();
        assert_eq!(text, "Sehr geehrter Thomas Müller, die IBAN DE89 3704 0044 0532 0130 00 ist korrekt.");
        assert!(out.iter().any(|l| l.contains("Clipboard pseudonymized")), "{out:#?}");
        assert!(out.iter().any(|l| l.contains("[IBAN_1]")));
    }

    #[test]
    fn redacted_clipboard_has_no_originals() {
        let (clip, _) = run(vec![Step::Copy(MAIL), Step::Type("y")], None);
        let text = clip.borrow().clone().unwrap();
        assert!(!text.contains("Müller") && !text.contains("0532"), "{text}");
        assert!(text.contains("[PERSON_1]") && text.contains("[IBAN_1]"), "{text}");
    }

    #[test]
    fn declined_and_toggled() {
        let (clip, _) = run(vec![Step::Copy(MAIL), Step::Type("n")], None);
        assert_eq!(clip.borrow().as_deref(), Some(MAIL));

        // Toggle off the first finding, keep the rest.
        let (clip, out) = run(vec![Step::Copy(MAIL), Step::Type("1"), Step::Type("")], None);
        let text = clip.borrow().clone().unwrap();
        assert!(text.contains("Müller") && text.contains("[IBAN_1]"), "{text}\n{out:#?}");
    }

    #[test]
    fn reveal_defaults_to_no() {
        let (clip, _) = run(vec![Step::Copy(MAIL), Step::Type(""), Step::Copy("Re: [PERSON_1]"), Step::Type("")], None);
        assert_eq!(clip.borrow().as_deref(), Some("Re: [PERSON_1]"));
    }

    #[test]
    fn special_category_is_blocked_without_asking() {
        let t = "Frau Lea Wolf ist seit Montag wegen ihrer Depression krankgeschrieben.";
        let (clip, out) = run(vec![Step::Copy(t), Step::Type("y")], None);
        assert_eq!(clip.borrow().as_deref(), Some(BLOCK_NOTICE));
        assert!(out.iter().any(|l| l.contains("BLOCKED: HEALTH")), "{out:#?}");
    }

    #[test]
    fn clean_text_untouched_and_quit() {
        let (clip, out) = run(vec![Step::Copy("Das Wetter ist schön."), Step::Type("s"), Step::Type("q")], None);
        assert_eq!(clip.borrow().as_deref(), Some("Das Wetter ist schön."));
        assert!(out.iter().any(|l| l.starts_with("✓ clean")));
        assert!(out.iter().any(|l| l.starts_with("0 placeholders")));
    }

    #[test]
    fn audit_log_never_contains_values() {
        let dir = std::env::temp_dir().join(format!("privatsphaere-audit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("audit.jsonl");
        let _ = std::fs::remove_file(&path);
        let log = AuditLog::open(&path).unwrap();
        run(
            vec![
                Step::Copy(MAIL),
                Step::Type(""),
                Step::Copy("Danke [PERSON_1]"),
                Step::Type("y"),
                Step::Copy("Herr Jonas Weber hat Diabetes und ist krankgeschrieben."),
                Step::Copy("Ruf an: 0171 2345678"),
                Step::Type("n"),
            ],
            Some(log),
        );
        let s = std::fs::read_to_string(&path).unwrap();
        let actions: Vec<String> = s
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["action"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(actions, ["redacted", "revealed", "blocked", "allowed"], "{s}");
        for secret in ["Müller", "Thomas", "DE89", "0532", "Jonas", "Weber", "Diabetes", "0171", "2345678"] {
            assert!(!s.contains(secret), "audit log leaks {secret}: {s}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
