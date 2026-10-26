//! `privatsphaere-app`: review window and tray icon.
//!
//! Review mode only: on a clipboard change with findings the window comes to
//! the front with the highlighted text and the pseudonymized result, and the
//! clipboard is only replaced after confirmation. Special-category data
//! (GDPR Art. 9/10) is replaced by a notice immediately. Watch mode
//! (automatic replacement) stays off until the accuracy targets are met.

mod i18n;
#[cfg(feature = "tray")]
mod tray;
mod ui;

use clap::Parser;
use detect::{Engine, EngineConfig};
use eframe::egui;
use serde::Deserialize;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "privatsphaere-app", version, about = "Local-only clipboard guard (review mode).")]
struct Args {
    /// Engine configuration (TOML with a [detect] table, see privatsphaere.example.toml).
    #[arg(long)]
    config: Option<PathBuf>,
    /// Append decisions to this local JSONL file (never original values).
    #[arg(long)]
    audit_log: Option<PathBuf>,
    /// Polling interval in milliseconds.
    #[arg(long, default_value_t = 250)]
    interval_ms: u64,
}

#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    #[serde(default)]
    detect: EngineConfig,
}

fn engine(path: Option<&std::path::Path>) -> Result<Engine, Box<dyn std::error::Error>> {
    let cfg = match path {
        Some(p) => {
            let s = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
            toml::from_str::<FileConfig>(&s).map_err(|e| format!("{}: {e}", p.display()))?.detect
        }
        None => EngineConfig::default(),
    };
    Ok(Engine::new(cfg)?)
}

fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let t = i18n::detect();
    let audit = args.audit_log.as_deref().map(session::AuditLog::open).transpose()?;
    let session = session::Session::new(engine(args.config.as_deref())?, clip::SystemClipboard::new()?, audit)?;
    let state: ui::State =
        Arc::new(Mutex::new(ui::Shared { session, pending: None, status: String::new(), quitting: false }));
    let interval = Duration::from_millis(args.interval_ms.max(50));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(t.title)
            .with_app_id("privatsphaere")
            .with_inner_size([720.0, 560.0])
            .with_min_inner_size([420.0, 300.0]),
        ..Default::default()
    };
    eframe::run_native(
        t.title,
        options,
        Box::new(move |cc| {
            ui::spawn_poller(state.clone(), cc.egui_ctx.clone(), interval, t);
            #[cfg(feature = "tray")]
            let has_tray = tray::install(state.clone(), cc.egui_ctx.clone(), t);
            #[cfg(not(feature = "tray"))]
            let has_tray = false;
            Ok(Box::new(ui::App { state, t, has_tray }))
        }),
    )?;
    Ok(())
}

fn main() -> ExitCode {
    match run(Args::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}
