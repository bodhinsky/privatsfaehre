//! Review window.
//!
//! A background thread polls the clipboard through [`session::Session`] and
//! stores what needs a decision in [`Shared::pending`]; the window shows it.
//! Polling does not depend on the window being drawn, so blocking Art. 9/10
//! data works while the window is minimised.

use crate::i18n::T;
use clip::SystemClipboard;
use detect::Detection;
use eframe::egui::{self, Color32, RichText, ScrollArea, TextFormat, text::LayoutJob};
use session::{Blocked, Event, Outcome, RevealOffer, Review, Session};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// Something the user has to look at or decide.
pub enum Pending {
    Review { review: Review, preview: String },
    Reveal(RevealOffer),
    Blocked(Blocked),
    Failed(String),
}

pub struct Shared {
    pub session: Session<SystemClipboard>,
    pub pending: Option<Pending>,
    /// Last status line (never contains clipboard content).
    pub status: String,
    pub quitting: bool,
}

pub type State = Arc<Mutex<Shared>>;

pub fn lock(state: &State) -> MutexGuard<'_, Shared> {
    // A panic while holding the lock leaves nothing half-written that matters
    // more than continuing to block special-category data.
    state.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    fn set_event(&mut self, ev: Event, t: &T) -> bool {
        if let Some(Pending::Review { review, .. }) = &self.pending {
            // A newer copy replaces an undecided review.
            self.session.decline(review);
        }
        let pending = match ev {
            Event::Clean { chars } => {
                self.status = format!("{} ({chars})", t.clean);
                if matches!(self.pending, Some(Pending::Review { .. } | Pending::Reveal(_))) {
                    self.pending = None;
                }
                return false;
            }
            Event::Review(review) => match self.session.preview(&review) {
                Ok(preview) => Pending::Review { review, preview },
                Err(e) => Pending::Failed(e.to_string()),
            },
            Event::Reveal(o) => Pending::Reveal(o),
            Event::Blocked(b) => Pending::Blocked(b),
            Event::Failed { error } => Pending::Failed(error),
        };
        self.pending = Some(pending);
        true
    }
}

/// Bring the window to the front.
pub fn attention(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Critical));
    ctx.request_repaint();
}

pub fn spawn_poller(state: State, ctx: egui::Context, interval: Duration, t: &'static T) {
    std::thread::spawn(move || {
        let mut last_error: Option<String> = None;
        loop {
            std::thread::sleep(interval);
            let mut s = lock(&state);
            if s.quitting {
                break;
            }
            let show = match s.session.poll() {
                Ok(Some(ev)) => {
                    last_error = None;
                    s.set_event(ev, t)
                }
                Ok(None) => false,
                Err(e) => {
                    // Clipboard owners come and go; show the error once.
                    let msg = e.to_string();
                    if last_error.as_deref() != Some(&msg) {
                        s.status = format!("! {msg}");
                        last_error = Some(msg);
                        ctx.request_repaint();
                    }
                    false
                }
            };
            for w in s.session.take_warnings() {
                s.status = format!("! {w}");
            }
            drop(s);
            if show {
                attention(&ctx);
            } else {
                ctx.request_repaint_after(Duration::from_millis(500));
            }
        }
    });
}

pub struct App {
    pub state: State,
    pub t: &'static T,
    pub has_tray: bool,
}

const FIND: Color32 = Color32::from_rgb(0xff, 0xd5, 0x4f);
const BLOCK: Color32 = Color32::from_rgb(0xc6, 0x28, 0x28);

/// `text` with `marks` (byte ranges) highlighted.
fn highlighted(text: &str, marks: &[(usize, usize, bool)], ui: &egui::Ui) -> LayoutJob {
    let mut marks = marks.to_vec();
    marks.sort_by_key(|m| (m.0, std::cmp::Reverse(m.1)));
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let plain = TextFormat { font_id: font.clone(), color: ui.visuals().text_color(), ..Default::default() };
    let mut job = LayoutJob::default();
    let mut pos = 0;
    for (start, end, block) in marks {
        if start < pos {
            continue;
        }
        job.append(&text[pos..start], 0.0, plain.clone());
        let (bg, fg) = if block { (BLOCK, Color32::WHITE) } else { (FIND, Color32::BLACK) };
        job.append(
            &text[start..end],
            0.0,
            TextFormat { font_id: font.clone(), color: fg, background: bg, ..Default::default() },
        );
        pos = end;
    }
    job.append(&text[pos..], 0.0, plain);
    job.wrap.max_width = ui.available_width();
    job
}

fn text_box(ui: &mut egui::Ui, id: &str, job: LayoutJob, max_height: f32) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ScrollArea::vertical().id_salt(id).max_height(max_height).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(job);
        });
    });
}

fn detection_line(d: &Detection, text: &str) -> String {
    let art = d.entity.gdpr_article().map(|a| format!(" · {a}")).unwrap_or_default();
    format!("{:<20} {:.2}  {}{art}", d.entity.as_str(), d.score, d.text(text))
}

enum Action {
    None,
    Confirm,
    Decline,
    Toggle(usize),
    RevealYes,
    Dismiss,
}

impl App {
    fn header(&self, ui: &mut egui::Ui, s: &mut Shared, ctx: &egui::Context) {
        let t = self.t;
        ui.horizontal_wrapped(|ui| {
            ui.heading(t.title);
            let audit = if s.session.audit_enabled() { format!(" · {}", t.audit_on) } else { String::new() };
            ui.label(format!("{} · {} {}{audit}", t.review_mode, s.session.vault_len(), t.vault));
        });
        ui.horizontal_wrapped(|ui| {
            if ui.button(t.reveal_clipboard).clicked() {
                match s.session.reveal_clipboard() {
                    Ok(Some(o)) if o.restored() > 0 => s.pending = Some(Pending::Reveal(o)),
                    Ok(_) => s.status = t.no_placeholders.to_owned(),
                    Err(e) => s.status = format!("! {e}"),
                }
            }
            if ui.button(t.clear_vault).clicked() {
                if let Some(Pending::Reveal(_)) = s.pending {
                    s.pending = None;
                }
                s.session.clear_vault();
                s.status = t.vault_cleared.to_owned();
            }
            if ui.button(t.quit).clicked() {
                s.quitting = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
        ui.separator();
    }

    fn body(&self, ui: &mut egui::Ui, s: &mut Shared) -> Action {
        let t = self.t;
        let mut action = Action::None;
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
        match &s.pending {
            None => {
                ui.label(t.watching);
                ui.label(RichText::new(t.offline).weak());
            }
            Some(Pending::Review { review, preview }) => {
                ui.label(RichText::new(t.found).strong().size(16.0));
                ui.label(t.copied);
                let marks: Vec<_> = review.selected().map(|d| (d.start, d.end, false)).collect();
                text_box(ui, "copied", highlighted(review.text(), &marks, ui), 160.0);
                ScrollArea::vertical().id_salt("findings").max_height(140.0).show(ui, |ui| {
                    for (i, (d, keep)) in review.detections.iter().zip(&review.keep).enumerate() {
                        let mut k = *keep;
                        let line = RichText::new(detection_line(d, review.text())).monospace();
                        if ui.checkbox(&mut k, line).changed() {
                            action = Action::Toggle(i);
                        }
                    }
                });
                ui.label(t.pseudonymized);
                text_box(ui, "preview", highlighted(preview, &[], ui), 160.0);
                ui.horizontal(|ui| {
                    if ui.button(RichText::new(t.confirm).strong()).clicked() || enter {
                        action = Action::Confirm;
                    }
                    if ui.button(t.keep_original).clicked() || esc {
                        action = Action::Decline;
                    }
                });
            }
            Some(Pending::Reveal(o)) => {
                ui.label(RichText::new(format!("{}: {}", t.reveal_offer, o.restored())).strong().size(16.0));
                ui.label(t.reveal_hint);
                if !o.unknown().is_empty() {
                    ui.label(format!("{}: {}", t.unknown_placeholders, o.unknown().join(", ")));
                }
                // No Enter shortcut: revealing must be a deliberate click.
                ui.horizontal(|ui| {
                    if ui.button(t.reveal_yes).clicked() {
                        action = Action::RevealYes;
                    }
                    if ui.button(t.reveal_no).clicked() || esc {
                        action = Action::Dismiss;
                    }
                });
            }
            Some(Pending::Blocked(b)) => {
                let cats: Vec<String> = b
                    .categories()
                    .iter()
                    .map(|e| format!("{} ({})", e.as_str(), e.gdpr_article().unwrap_or("Art. 9")))
                    .collect();
                ui.label(RichText::new(format!("{}: {}", t.blocked, cats.join(", "))).color(BLOCK).strong().size(16.0));
                ui.label(t.blocked_hint);
                let mut marks: Vec<_> = b.blocking.iter().map(|d| (d.start, d.end, true)).collect();
                marks.extend(b.detections.iter().map(|d| (d.start, d.end, false)));
                text_box(ui, "blocked", highlighted(b.text(), &marks, ui), 220.0);
                if ui.button(t.ok).clicked() || enter || esc {
                    action = Action::Dismiss;
                }
            }
            Some(Pending::Failed(e)) => {
                ui.label(RichText::new(format!("{}: {e}", t.failed)).color(BLOCK).strong());
                if ui.button(t.ok).clicked() || enter || esc {
                    action = Action::Dismiss;
                }
            }
        }
        action
    }

    fn apply(&self, s: &mut Shared, action: Action) {
        let t = self.t;
        let outcome_text = |o: Outcome, done: &str| match o {
            Outcome::Replaced => done.to_owned(),
            Outcome::ChangedMeanwhile => t.changed_meanwhile.to_owned(),
            Outcome::Unchanged => t.nothing_selected.to_owned(),
        };
        match action {
            Action::None => {}
            Action::Toggle(i) => {
                if let Some(Pending::Review { review, preview }) = &mut s.pending {
                    review.toggle(i);
                    if let Ok(p) = s.session.preview(review) {
                        *preview = p;
                    }
                }
            }
            Action::Confirm => {
                if let Some(Pending::Review { review, .. }) = s.pending.take() {
                    s.status = match s.session.confirm(&review) {
                        Ok(o) => outcome_text(o, t.replaced),
                        Err(e) => format!("! {e}"),
                    };
                }
            }
            Action::Decline => {
                if let Some(Pending::Review { review, .. }) = s.pending.take() {
                    s.session.decline(&review);
                    s.status = t.kept.to_owned();
                }
            }
            Action::RevealYes => {
                if let Some(Pending::Reveal(o)) = s.pending.take() {
                    s.status = match s.session.accept_reveal(&o) {
                        Ok(Outcome::Unchanged) => t.no_placeholders.to_owned(),
                        Ok(o) => outcome_text(o, t.revealed),
                        Err(e) => format!("! {e}"),
                    };
                }
            }
            Action::Dismiss => {
                if matches!(s.pending, Some(Pending::Reveal(_))) {
                    s.status = t.kept.to_owned();
                }
                s.pending = None;
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        let state = self.state.clone();
        let mut s = lock(&state);
        if ctx.input(|i| i.viewport().close_requested()) && !s.quitting && self.has_tray {
            // Closing the window keeps the guard running in the tray.
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
        egui::Panel::bottom("status").show(root, |ui| {
            ui.label(RichText::new(&s.status).weak());
        });
        egui::CentralPanel::default().show(root, |ui| {
            self.header(ui, &mut s, &ctx);
            let action = self.body(ui, &mut s);
            self.apply(&mut s, action);
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let mut s = lock(&self.state);
        s.quitting = true;
        s.pending = None;
        s.session.clear_vault();
    }
}
