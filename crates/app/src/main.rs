//! Tray application. Not implemented yet.
//!
//! Plan (see ARCHITECTURE.md): `egui`/`eframe` window plus `tray-icon`.
//! Until the accuracy targets are met the app runs in **review mode**: on a
//! clipboard change it shows the text with highlighted spans and the
//! pseudonymized result, and only replaces the clipboard after the user
//! confirms. `watch` mode (automatic replacement) stays disabled by default.

fn main() {
    eprintln!(
        "privatsphaere-app: the tray UI is not implemented yet.\n\
         Use `privatsphaere scan` to preview detection and redaction, and\n\
         `privatsphaere label` / `privatsphaere eval` to build and validate the dataset."
    );
    std::process::exit(2);
}
