//! `guard`: clipboard guard in review mode, in the terminal.
//!
//! A terminal front end for [`session::Session`], which makes the decisions:
//! findings are only pseudonymized after confirmation, special-category data
//! (GDPR Art. 9/10) and detection errors replace the clipboard with a notice
//! right away, and copied text with this session's placeholders can be
//! restored ("reveal"). The vault is wiped on exit or with `c`.

use crate::term::{self, Mark};
use clip::{ClipError, ClipboardBackend};
use session::{Event, Outcome, RevealOffer, Review, Session};
use std::io::{self, BufRead, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;
use vault::zeroize::Zeroizing;

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

pub struct Guard<B, C> {
    pub session: Session<B>,
    pub console: C,
    pub color: bool,
    pub interval: Duration,
}

#[derive(PartialEq)]
enum Flow {
    Continue,
    Quit,
}

type Res<T> = Result<T, Box<dyn std::error::Error>>;

fn is_yes(a: &str) -> bool {
    matches!(a, "y" | "Y" | "j" | "J" | "yes" | "ja")
}

impl<B: ClipboardBackend, C: Console> Guard<B, C> {
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
            match self.session.poll() {
                Ok(Some(ev)) => {
                    last_error = None;
                    if self.handle(ev)? == Flow::Quit {
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
            for w in self.session.take_warnings() {
                self.console.say(&format!("⚠ {w}"));
            }
        }
        self.session.clear_vault();
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

    fn report(&mut self, outcome: Outcome, done: &str) {
        match outcome {
            Outcome::Replaced => self.console.say(done),
            Outcome::ChangedMeanwhile => self.console.say("  Clipboard changed in the meantime; left it alone."),
            Outcome::Unchanged => self.console.say("  Nothing to do; clipboard unchanged."),
        }
    }

    fn command(&mut self, cmd: &str) -> Res<Flow> {
        match cmd {
            "" => {}
            "q" | "quit" | "exit" => return Ok(Flow::Quit),
            "h" | "help" | "?" => self.console.say(HELP),
            "s" | "status" => {
                let n = self.session.vault_len();
                let audit = if self.session.audit_enabled() { "on" } else { "off" };
                self.console.say(&format!("{n} placeholders in the vault; audit log {audit}; review mode."));
            }
            "c" | "clear" => {
                self.session.clear_vault();
                self.console.say("Vault wiped; placeholders issued so far can no longer be revealed.");
            }
            "r" | "reveal" => match self.session.reveal_clipboard()? {
                Some(o) => self.reveal(&o, false)?,
                None => self.console.say("Clipboard holds no text."),
            },
            other => self.console.say(&format!("Unknown command `{other}`. {HELP}")),
        }
        Ok(Flow::Continue)
    }

    fn reveal(&mut self, o: &RevealOffer, ask: bool) -> Result<(), ClipError> {
        if !o.unknown().is_empty() {
            self.console.say(&format!("  Not from this session: {}", o.unknown().join(", ")));
        }
        if o.restored() == 0 {
            self.console.say("  No placeholders from this session found.");
            return Ok(());
        }
        if ask {
            self.console.bell();
            self.console.say(&format!("\n↩ Copied text contains {} placeholder(s) from this session.", o.restored()));
            // Default no: the text might be on its way back into a chat.
            let a = self.ask("  Restore the originals into the clipboard? [y/N]").unwrap_or_default();
            if !is_yes(&a) {
                self.console.say("  Left as is.");
                return Ok(());
            }
        }
        let outcome = self.session.accept_reveal(o)?;
        self.report(outcome, &format!("  ✓ Restored {} placeholder(s). Only paste this locally.", o.restored()));
        Ok(())
    }

    fn handle(&mut self, ev: Event) -> Res<Flow> {
        match ev {
            Event::Clean { chars } => self.console.say(&format!("✓ clean ({chars} chars)")),
            Event::Failed { error } => {
                self.console.bell();
                self.console.say(&format!("\n⛔ Detection failed ({error}); clipboard replaced with a notice."));
            }
            Event::Blocked(b) => {
                self.console.bell();
                let mut marks: Vec<Mark> = b.blocking.iter().map(|d| Mark::new(d.start, d.end, true)).collect();
                marks.extend(b.detections.iter().map(|d| Mark::new(d.start, d.end, false)));
                let shown = Zeroizing::new(term::highlight(b.text(), &marks, self.color));
                self.console.say(&format!("\n{}", *shown));
                let cats: Vec<String> = b
                    .categories()
                    .iter()
                    .map(|e| format!("{} ({})", e.as_str(), e.gdpr_article().unwrap_or("Art. 9")))
                    .collect();
                self.console.say(&format!(
                    "⛔ BLOCKED: {}. The clipboard now holds a notice instead; this cannot be overridden.",
                    cats.join(", ")
                ));
            }
            Event::Reveal(o) => self.reveal(&o, true)?,
            Event::Review(r) => return self.review(r),
        }
        Ok(Flow::Continue)
    }

    /// Show findings and replace the clipboard after confirmation.
    fn review(&mut self, mut r: Review) -> Res<Flow> {
        self.console.bell();
        loop {
            let marks: Vec<Mark> = r.selected().map(|d| Mark::new(d.start, d.end, false)).collect();
            let shown = Zeroizing::new(term::highlight(r.text(), &marks, self.color));
            self.console.say(&format!("\n── Copied ──\n{}", *shown));
            for (i, (d, k)) in r.detections.iter().zip(&r.keep).enumerate() {
                let line = Zeroizing::new(format!(
                    "  {:>2} {} {:<20} {:.2} {:?}",
                    i + 1,
                    if *k { "✓" } else { "·" },
                    d.entity.as_str(),
                    d.score,
                    d.text(r.text())
                ));
                self.console.say(&line);
            }
            let preview = self.session.preview(&r)?;
            self.console.say(&format!("── Pseudonymized ──\n{preview}"));
            let Some(a) = self.ask("Replace the clipboard? [Y/n], or numbers to toggle (e.g. 2,4), q = quit guard")
            else {
                return Ok(Flow::Quit);
            };
            match a.as_str() {
                "" => {}
                a if is_yes(a) => {}
                "n" | "N" | "no" | "nein" => {
                    self.session.decline(&r);
                    self.console.say("  Clipboard unchanged.");
                    return Ok(Flow::Continue);
                }
                "q" => return Ok(Flow::Quit),
                list => {
                    let mut ok = true;
                    for part in list.split([',', ' ']).filter(|p| !p.is_empty()) {
                        match part.parse::<usize>() {
                            Ok(n) if (1..=r.keep.len()).contains(&n) => r.toggle(n - 1),
                            _ => ok = false,
                        }
                    }
                    if !ok {
                        self.console.say(&format!("  Expected y, n, q or numbers from 1 to {}.", r.keep.len()));
                    }
                    continue;
                }
            }
            let n = r.selected().count();
            let outcome = self.session.confirm(&r)?;
            self.report(outcome, &format!("  ✓ Clipboard pseudonymized ({n} spans)."));
            return Ok(Flow::Continue);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use session::{AuditLog, BLOCK_NOTICE};
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
        let engine = detect::Engine::new(Default::default()).unwrap();
        let clip: Shared = Rc::new(RefCell::new(Some("start".into())));
        let session = Session::new(engine, SharedClipboard(clip.clone()), audit).unwrap();
        let console = Script { clip: clip.clone(), steps: steps.into(), out: Vec::new() };
        let mut g = Guard { session, console, color: false, interval: Duration::from_millis(1) };
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
