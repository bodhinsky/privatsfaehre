//! Tray / menu-bar icon with a small menu.
//!
//! On Linux the icon lives on its own GTK thread (`tray-icon` needs a running
//! GTK main loop, which winit does not provide). On Windows and macOS it is
//! created on the UI thread once the event loop runs.

use crate::i18n::T;
use crate::ui::{self, State, lock};
use eframe::egui;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

struct Ids {
    show: MenuId,
    reveal: MenuId,
    clear: MenuId,
    quit: MenuId,
}

/// A 32×32 shield in the app's yellow, drawn in code so no image files are needed.
fn icon() -> Icon {
    const N: u32 = 32;
    let mut rgba = Vec::with_capacity((N * N * 4) as usize);
    for y in 0..N {
        for x in 0..N {
            let (fx, fy) = (x as f32 - 15.5, y as f32);
            // Flat top, sides narrowing into a point at the bottom.
            let half = if fy < 16.0 { 13.0 } else { 13.0 * (1.0 - (fy - 16.0) / 15.0) };
            let inside = (2.0..=30.0).contains(&fy) && fx.abs() <= half;
            let border = inside && (fx.abs() > half - 2.5 || fy < 4.5);
            let px = match (inside, border) {
                (true, true) => [0x33, 0x33, 0x33, 0xff],
                (true, false) => [0xff, 0xd5, 0x4f, 0xff],
                _ => [0, 0, 0, 0],
            };
            rgba.extend_from_slice(&px);
        }
    }
    Icon::from_rgba(rgba, N, N).expect("valid icon")
}

fn build(t: &T) -> Result<(TrayIcon, Ids), Box<dyn std::error::Error>> {
    let show = MenuItem::new(t.show_window, true, None);
    let reveal = MenuItem::new(t.reveal_clipboard, true, None);
    let clear = MenuItem::new(t.clear_vault, true, None);
    let quit = MenuItem::new(t.quit, true, None);
    let menu = Menu::new();
    menu.append_items(&[&show, &reveal, &clear, &PredefinedMenuItem::separator(), &quit])?;
    let ids = Ids {
        show: show.id().clone(),
        reveal: reveal.id().clone(),
        clear: clear.id().clone(),
        quit: quit.id().clone(),
    };
    let tray = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip(t.title).with_icon(icon()).build()?;
    Ok((tray, ids))
}

fn handle(ids: &Ids, ev: &MenuEvent, state: &State, ctx: &egui::Context, t: &T) {
    if ev.id == ids.show {
        ui::attention(ctx);
    } else if ev.id == ids.reveal {
        let mut s = lock(state);
        match s.session.reveal_clipboard() {
            Ok(Some(o)) if o.restored() > 0 => s.pending = Some(ui::Pending::Reveal(o)),
            Ok(_) => s.status = t.no_placeholders.to_owned(),
            Err(e) => s.status = format!("! {e}"),
        }
        drop(s);
        ui::attention(ctx);
    } else if ev.id == ids.clear {
        let mut s = lock(state);
        s.session.clear_vault();
        if matches!(s.pending, Some(ui::Pending::Reveal(_))) {
            s.pending = None;
        }
        s.status = t.vault_cleared.to_owned();
        ctx.request_repaint();
    } else if ev.id == ids.quit {
        lock(state).quitting = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        ctx.request_repaint();
    }
}

/// Install the tray icon. Returns `false` if it could not be created (for
/// example a Linux desktop without an AppIndicator host); the window then
/// quits on close instead of minimising.
pub fn install(state: State, ctx: egui::Context, t: &'static T) -> bool {
    #[cfg(target_os = "linux")]
    {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            if gtk::init().is_err() {
                let _ = tx.send(false);
                return;
            }
            match build(t) {
                Ok((_tray, ids)) => {
                    let _ = tx.send(true);
                    MenuEvent::set_event_handler(Some(move |ev: MenuEvent| handle(&ids, &ev, &state, &ctx, t)));
                    gtk::main();
                }
                Err(_) => {
                    let _ = tx.send(false);
                }
            }
        });
        rx.recv().unwrap_or(false)
    }
    #[cfg(not(target_os = "linux"))]
    {
        match build(t) {
            Ok((tray, ids)) => {
                // The icon must outlive the window; it is removed at process exit.
                std::mem::forget(tray);
                MenuEvent::set_event_handler(Some(move |ev: MenuEvent| handle(&ids, &ev, &state, &ctx, t)));
                true
            }
            Err(_) => false,
        }
    }
}
