//! Interactive `label` mode.
//!
//! For each input text, every candidate from every recognizer (down to
//! `--min-score`) is shown in context and the user accepts, rejects or
//! corrects it, then adds anything that was missed. Each finished record is
//! appended to the dataset immediately, so a session can be stopped and
//! resumed at any time.

use crate::term::{self, Mark};
use dataset::{LabeledSpan, Proposal, Record, Verdict, byte_to_char, char_to_byte, find_all, find_nearest};
use detect::{Engine, EntityType};
use std::collections::HashSet;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

/// Prompt/print abstraction so sessions can be scripted in tests.
pub trait Io {
    /// `None` on end of input.
    fn ask(&mut self, prompt: &str) -> io::Result<Option<String>>;
    fn say(&mut self, line: &str);
}

struct Terminal;

impl Io for Terminal {
    fn ask(&mut self, prompt: &str) -> io::Result<Option<String>> {
        print!("{prompt}");
        io::stdout().flush()?;
        let mut line = String::new();
        if io::stdin().lock().read_line(&mut line)? == 0 {
            return Ok(None);
        }
        Ok(Some(line.trim_end_matches(['\n', '\r']).to_owned()))
    }
    fn say(&mut self, line: &str) {
        println!("{line}");
    }
}

pub enum Outcome {
    Labeled(Record),
    Skipped,
    Quit,
}

pub struct Session<'a, I> {
    pub engine: &'a Engine,
    pub io: I,
    pub color: bool,
    pub min_score: f32,
    pub labeler: Option<String>,
}

const HELP: &str = "  a = accept   r = reject   c = correct type/boundaries   s = skip record   q = quit";

impl<I: Io> Session<'_, I> {
    fn ask(&mut self, prompt: &str) -> io::Result<Option<String>> {
        self.io.ask(prompt)
    }

    fn ask_entity(&mut self, default: Option<EntityType>) -> io::Result<Option<EntityType>> {
        loop {
            let hint = default.map(|d| format!(" [{d}]")).unwrap_or_default();
            let Some(a) = self.ask(&format!("  entity type{hint}: "))? else { return Ok(None) };
            if a.trim().is_empty() {
                if let Some(d) = default {
                    return Ok(Some(d));
                }
                continue;
            }
            match a.parse() {
                Ok(e) => return Ok(Some(e)),
                Err(_) => {
                    let names: Vec<_> = EntityType::ALL.iter().map(|e| e.as_str()).collect();
                    self.io.say(&format!("  unknown type; one of: {}", names.join(", ")));
                }
            }
        }
    }

    fn context_line(&self, text: &str, start: usize, end: usize) -> String {
        let a = byte_to_char(text, start).saturating_sub(60);
        let b = byte_to_char(text, end) + 60;
        let (ab, bb) = (char_to_byte(text, a), char_to_byte(text, b));
        let snippet = &text[ab..bb];
        let mark = Mark { start: start - ab, end: end - ab, block: false, focus: true };
        term::highlight(snippet, &[mark], self.color).replace('\n', "⏎")
    }

    /// Label one record. Pre-existing spans (e.g. from markup) are kept.
    pub fn label(&mut self, input: Record, position: (usize, usize)) -> io::Result<Outcome> {
        let text = input.text.clone();
        let mut rec = input;
        let candidates = self
            .engine
            .candidates(&text)
            .map_err(io::Error::other)?
            .into_iter()
            .filter(|d| d.score >= self.min_score)
            .collect::<Vec<_>>();

        let marks: Vec<Mark> =
            candidates.iter().map(|d| Mark::new(d.start, d.end, d.entity.is_special_category())).collect();
        self.io.say(&format!(
            "\n━━ [{}/{}] {} ({}) ━━ {} candidate(s)",
            position.0,
            position.1,
            rec.id,
            rec.source.as_deref().unwrap_or("?"),
            candidates.len()
        ));
        self.io.say(&term::highlight(&text, &marks, self.color));
        self.io.say(HELP);

        let existing: HashSet<(usize, usize, EntityType)> =
            rec.spans.iter().map(|s| (s.start, s.end, s.entity)).collect();
        for (i, d) in candidates.iter().enumerate() {
            let (cs, ce) = (byte_to_char(&text, d.start), byte_to_char(&text, d.end));
            if existing.contains(&(cs, ce, d.entity)) {
                continue;
            }
            let block = if d.entity.is_special_category() { " ⛔ BLOCK" } else { "" };
            self.io.say(&format!(
                "\n  #{}/{} {}{block}  score {:.2}  ({})\n  {}",
                i + 1,
                candidates.len(),
                d.entity,
                d.score,
                d.recognizer,
                self.context_line(&text, d.start, d.end)
            ));
            loop {
                let Some(ans) = self.ask("  [a/r/c/s/q] > ")? else { return Ok(Outcome::Quit) };
                let base = LabeledSpan {
                    start: cs,
                    end: ce,
                    text: text[d.start..d.end].to_owned(),
                    entity: d.entity,
                    verdict: Verdict::Accepted,
                    recognizer: Some(d.recognizer.to_string()),
                    score: Some(d.score),
                    proposed: None,
                    note: None,
                };
                match ans.trim().to_ascii_lowercase().as_str() {
                    "a" | "y" => {
                        rec.spans.push(base);
                        break;
                    }
                    "r" | "n" => {
                        rec.spans.push(LabeledSpan { verdict: Verdict::Rejected, ..base });
                        break;
                    }
                    "c" => {
                        let Some(entity) = self.ask_entity(Some(d.entity))? else { return Ok(Outcome::Quit) };
                        let (start, end, span_text) = loop {
                            let Some(t) = self.ask(&format!("  exact text [{}]: ", base.text))? else {
                                return Ok(Outcome::Quit);
                            };
                            let t = if t.is_empty() { base.text.clone() } else { t };
                            match find_nearest(&text, &t, cs) {
                                Some((s, e)) => break (s, e, t),
                                None => self.io.say("  not found in text, try again"),
                            }
                        };
                        rec.spans.push(LabeledSpan {
                            start,
                            end,
                            text: span_text,
                            entity,
                            verdict: Verdict::Corrected,
                            proposed: Some(Proposal { start: cs, end: ce, entity: d.entity }),
                            ..base
                        });
                        break;
                    }
                    "s" => return Ok(Outcome::Skipped),
                    "q" => return Ok(Outcome::Quit),
                    _ => self.io.say(HELP),
                }
            }
        }

        self.io.say("\n  Anything missed? Type the exact text of each missing span.");
        loop {
            let Some(t) = self.ask("  missed span (empty = done): ")? else { return Ok(Outcome::Quit) };
            if t.is_empty() {
                break;
            }
            let hits = find_all(&text, &t);
            let chosen: Vec<(usize, usize)> = match hits.len() {
                0 => {
                    self.io.say("  not found in text");
                    continue;
                }
                1 => hits,
                n => {
                    let Some(a) = self.ask(&format!("  {n} occurrences; which one? [1-{n}, a = all] (a): "))? else {
                        return Ok(Outcome::Quit);
                    };
                    match a.trim().parse::<usize>() {
                        Ok(k) if (1..=n).contains(&k) => vec![hits[k - 1]],
                        _ => hits,
                    }
                }
            };
            let Some(entity) = self.ask_entity(None)? else { return Ok(Outcome::Quit) };
            for (start, end) in chosen {
                rec.spans.push(LabeledSpan {
                    start,
                    end,
                    text: t.clone(),
                    entity,
                    verdict: Verdict::Added,
                    recognizer: None,
                    score: None,
                    proposed: None,
                    note: None,
                });
            }
        }

        let Some(c) = self.ask("  Is every sensitive span in this text labeled? [Y/n] ")? else {
            return Ok(Outcome::Quit);
        };
        rec.complete = !c.trim().eq_ignore_ascii_case("n");
        rec.spans.sort_by_key(|s| (s.start, s.end));
        rec.labeled_by.clone_from(&self.labeler);
        rec.labeled_at = Some(term::now_rfc3339());
        Ok(Outcome::Labeled(rec))
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    engine: &Engine,
    inputs: &[PathBuf],
    dataset_path: &Path,
    source: &str,
    labeler: Option<String>,
    min_score: f32,
    relabel: bool,
    color: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let done: HashSet<String> = if dataset_path.exists() && !relabel {
        dataset::load(&[dataset_path.to_owned()])?.into_iter().map(|r| r.id).collect()
    } else {
        HashSet::new()
    };
    let todo: Vec<Record> =
        crate::inputs::load(inputs, source)?.into_iter().filter(|r| !done.contains(&r.id)).collect();
    if todo.is_empty() {
        println!("Nothing to label ({} already in {}).", done.len(), dataset_path.display());
        return Ok(());
    }
    let total = todo.len();
    let mut session = Session { engine, io: Terminal, color, min_score, labeler };
    let (mut labeled, mut skipped) = (0, 0);
    for (i, r) in todo.into_iter().enumerate() {
        match session.label(r, (i + 1, total))? {
            Outcome::Labeled(rec) => {
                dataset::append(dataset_path, &rec)?;
                labeled += 1;
            }
            Outcome::Skipped => skipped += 1,
            Outcome::Quit => break,
        }
    }
    println!("\nLabeled {labeled}, skipped {skipped}; dataset: {}", dataset_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use detect::EngineConfig;
    use std::collections::VecDeque;

    struct Script(VecDeque<&'static str>, Vec<String>);
    impl Io for Script {
        fn ask(&mut self, p: &str) -> io::Result<Option<String>> {
            self.1.push(p.to_owned());
            Ok(self.0.pop_front().map(str::to_owned))
        }
        fn say(&mut self, l: &str) {
            self.1.push(l.to_owned());
        }
    }

    #[test]
    fn scripted_session() {
        let engine = Engine::new(EngineConfig::default()).unwrap();
        let text = "Hallo Jens, meine Nummer ist 0171 1234567, Kundennr. K-4711.";
        // Candidates in order: "Jens" (PERSON), phone. Accept the name, correct
        // the phone to drop nothing but retype nothing, then add the missed
        // customer number and confirm completeness.
        let cands = engine.candidates(text).unwrap();
        assert_eq!(cands.len(), 2, "{cands:?}");
        let script =
            Script(VecDeque::from(["a", "c", "", "0171 1234567", "K-4711", "ID_DOCUMENT", "", "y"]), Vec::new());
        let mut s = Session { engine: &engine, io: script, color: false, min_score: 0.0, labeler: Some("tb".into()) };
        let Outcome::Labeled(r) = s.label(Record::new("t1", text), (1, 1)).unwrap() else { panic!("not labeled") };
        assert!(r.complete);
        r.validate().unwrap();
        let v: Vec<_> = r.spans.iter().map(|s| (s.text.as_str(), s.entity, s.verdict)).collect();
        assert_eq!(
            v,
            [
                ("Jens", EntityType::Person, Verdict::Accepted),
                ("0171 1234567", EntityType::Phone, Verdict::Corrected),
                ("K-4711", EntityType::IdDocument, Verdict::Added),
            ]
        );
        assert_eq!(r.labeled_by.as_deref(), Some("tb"));
    }

    #[test]
    fn quit_on_eof() {
        let engine = Engine::new(EngineConfig::default()).unwrap();
        let mut s = Session {
            engine: &engine,
            io: Script(VecDeque::new(), Vec::new()),
            color: false,
            min_score: 0.0,
            labeler: None,
        };
        assert!(matches!(s.label(Record::new("t", "Hallo Jens"), (1, 1)).unwrap(), Outcome::Quit));
    }
}
