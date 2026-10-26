//! UI strings: German, with English when the locale is not German.

#[derive(Debug)]
pub struct T {
    pub title: &'static str,
    pub watching: &'static str,
    pub offline: &'static str,
    pub vault: &'static str,
    pub reveal_clipboard: &'static str,
    pub clear_vault: &'static str,
    pub vault_cleared: &'static str,
    #[cfg_attr(not(feature = "tray"), allow(dead_code))]
    pub show_window: &'static str,
    pub quit: &'static str,
    pub found: &'static str,
    pub copied: &'static str,
    pub pseudonymized: &'static str,
    pub confirm: &'static str,
    pub keep_original: &'static str,
    pub replaced: &'static str,
    pub kept: &'static str,
    pub changed_meanwhile: &'static str,
    pub nothing_selected: &'static str,
    pub reveal_offer: &'static str,
    pub reveal_hint: &'static str,
    pub reveal_yes: &'static str,
    pub reveal_no: &'static str,
    pub revealed: &'static str,
    pub no_placeholders: &'static str,
    pub unknown_placeholders: &'static str,
    pub blocked: &'static str,
    pub blocked_hint: &'static str,
    pub failed: &'static str,
    pub ok: &'static str,
    pub clean: &'static str,
    pub audit_on: &'static str,
    pub review_mode: &'static str,
}

pub const DE: T = T {
    title: "Privatsphäre",
    watching: "Überwacht die Zwischenablage.",
    offline: "Nichts verlässt diesen Rechner. Pseudonymisierte Daten bleiben personenbezogene Daten.",
    vault: "Platzhalter im Tresor",
    reveal_clipboard: "Zwischenablage aufdecken",
    clear_vault: "Tresor leeren",
    vault_cleared: "Tresor geleert.",
    show_window: "Fenster zeigen",
    quit: "Beenden",
    found: "Personenbezogene Daten oder Geheimnisse gefunden",
    copied: "Kopiert",
    pseudonymized: "Pseudonymisiert",
    confirm: "Pseudonymisieren (Enter)",
    keep_original: "Original behalten (Esc)",
    replaced: "Zwischenablage pseudonymisiert.",
    kept: "Zwischenablage unverändert.",
    changed_meanwhile: "Die Zwischenablage wurde inzwischen geändert und nicht überschrieben.",
    nothing_selected: "Nichts ausgewählt, Zwischenablage unverändert.",
    reveal_offer: "Der kopierte Text enthält Platzhalter aus dieser Sitzung",
    reveal_hint: "Nur aufdecken, wenn der Text lokal eingefügt wird, nicht zurück in einen Chat.",
    reveal_yes: "Originale wiederherstellen",
    reveal_no: "So lassen (Esc)",
    revealed: "Originale wiederhergestellt. Nur lokal einfügen.",
    no_placeholders: "Keine Platzhalter aus dieser Sitzung gefunden.",
    unknown_placeholders: "Nicht aus dieser Sitzung",
    blocked: "Blockiert",
    blocked_hint: "Besondere Kategorien personenbezogener Daten (Art. 9/10 DSGVO). Die Zwischenablage enthält \
                   jetzt einen Hinweis. Das lässt sich nicht übersteuern.",
    failed: "Prüfung fehlgeschlagen, Zwischenablage durch einen Hinweis ersetzt",
    ok: "OK",
    clean: "Nichts gefunden",
    audit_on: "Audit-Log an",
    review_mode: "Prüfmodus",
};

pub const EN: T = T {
    title: "Privatsphäre",
    watching: "Watching the clipboard.",
    offline: "Nothing leaves this machine. Pseudonymized data is still personal data.",
    vault: "placeholders in the vault",
    reveal_clipboard: "Reveal clipboard",
    clear_vault: "Clear vault",
    vault_cleared: "Vault cleared.",
    show_window: "Show window",
    quit: "Quit",
    found: "Personal data or secrets found",
    copied: "Copied",
    pseudonymized: "Pseudonymized",
    confirm: "Pseudonymize (Enter)",
    keep_original: "Keep original (Esc)",
    replaced: "Clipboard pseudonymized.",
    kept: "Clipboard unchanged.",
    changed_meanwhile: "The clipboard changed in the meantime and was not overwritten.",
    nothing_selected: "Nothing selected; clipboard unchanged.",
    reveal_offer: "The copied text contains placeholders from this session",
    reveal_hint: "Only reveal when pasting locally, not back into a chat.",
    reveal_yes: "Restore originals",
    reveal_no: "Leave as is (Esc)",
    revealed: "Originals restored. Only paste this locally.",
    no_placeholders: "No placeholders from this session found.",
    unknown_placeholders: "Not from this session",
    blocked: "Blocked",
    blocked_hint: "Special-category personal data (GDPR Art. 9/10). The clipboard now holds a notice. This cannot be \
                   overridden.",
    failed: "Detection failed; clipboard replaced with a notice",
    ok: "OK",
    clean: "Nothing found",
    audit_on: "audit log on",
    review_mode: "review mode",
};

/// German unless `LC_ALL`/`LC_MESSAGES`/`LANG` names another language.
pub fn detect() -> &'static T {
    let lang = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .find(|v| !v.is_empty() && v != "C" && v != "POSIX");
    match lang {
        Some(l) if !l.to_ascii_lowercase().starts_with("de") => &EN,
        _ => &DE,
    }
}
