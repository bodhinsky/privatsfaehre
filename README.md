# Privatsphäre

A local-only clipboard guardian for German and EU users. It detects personal
data and secrets in copied text and replaces them with typed placeholders
(`[PERSON_1]`, `[IBAN_1]`) before the text is pasted into an LLM chat. The
mapping stays in an in-memory vault, so the LLM's answer can be restored
locally. Text containing GDPR Art. 9/10 data is blocked, not redacted.

[Deutsche Version weiter unten](#deutsch)

> **Status: pre-alpha.** The detection engine, vault and the `label`/`eval`
> tooling work, and so does review mode in the terminal (`guard`) and in the
> desktop app (`privatsphaere-app`). Automatic replacement
> ("watch mode") stays disabled until detection has been validated on real,
> labeled data.

## Important: what this tool is and is not

- **This is pseudonymization, not anonymization.** The placeholders can be
  reversed with the local vault, and the remaining text still carries context.
- **Pseudonymized data is still personal data under the GDPR** (Art. 4(5) and
  Recital 26). Sending it to an LLM provider is still processing of personal
  data.
- **This tool reduces risk. It does not replace a data processing agreement
  (Art. 28 GDPR) with the LLM provider**, nor a legal basis, nor a check of
  third-country transfers.
- Detection is heuristic and will miss things. Blocking of special categories
  (health, religion, sexual orientation, union membership, ethnic origin,
  political opinions, genetic and biometric data, criminal records) is a
  safety net, not a guarantee.
- This is not legal advice.

## Privacy guarantees

- No network code at all: no telemetry, no update checks, no model downloads.
  CI fails if any HTTP, TLS, WebSocket or DNS client crate appears anywhere in
  the dependency tree, and if workspace code uses socket APIs
  (`scripts/check-offline.sh`).
- Original values live only in memory and are wiped when the vault is dropped.
- Real samples for the dataset stay in `dataset/private/`, which git ignores.

## What it detects

Names (also in lower-case chat text), addresses and PLZ, phone numbers
(`0171…`, `+49…`), e-mail, URLs, IBAN/BIC (checksum), credit cards (Luhn),
Steuer-ID (checksum) and Steuernummer, Sozialversicherungsnummer (checksum),
Personalausweis/Reisepass numbers (check digit), Kfz-Kennzeichen, dates, IP
addresses, organisations with a legal-form suffix, API keys (OpenAI,
Anthropic, AWS, GitHub, GitLab, Stripe, Slack, Google, …), private key
blocks, JWTs, database connection strings, `.env` secrets, and the GDPR
Art. 9/10 categories listed above. Optionally, a local GLiNER model on ONNX
Runtime adds names, organisations, places and a second line of defence for
Art. 9/10 (see "Local NER" below).

## Local NER (optional)

The rules miss names they have no cue for ("anna hat angerufen"). A GLiNER
model fixes much of that. Nothing is downloaded by the program; you provide:

1. ONNX Runtime 1.17 or newer as a shared library (`libonnxruntime.so`,
   `libonnxruntime.dylib` or `onnxruntime.dll`, from the official release or
   e.g. `pip install onnxruntime`, which ships it in `onnxruntime/capi/`).
2. A GLiNER span model exported to ONNX, in one directory with
   `model.onnx`, `tokenizer.json` and `gliner_config.json` (for example
   `gliner_multi_pii-v1`, multilingual; check the model card's license).

```sh
$B ner-config /opt/models/gliner_multi_pii-v1 >> privatsphaere.toml   # pins SHA-256
$B scan --config privatsphaere.toml notes.txt
$B eval --config privatsphaere.toml dataset/public dataset/private        # measure before relying on it
```

The model and tokenizer must match the pinned hashes, otherwise nothing
starts. If a configured model cannot be loaded, or fails on a text, the text
is treated as blocked (fail closed). Prompts for the Art. 9/10 categories are
fixed; `[ner] labels` can only change the ordinary ones.

## Quick start

```sh
cargo build --release
B=target/release/privatsphaere

echo "Hallo Frau Schneider, IBAN DE89 3704 0044 0532 0130 00, Tel 0171 1234567" | $B scan
$B scan --candidates notes.txt          # every raw candidate with score and rule
$B label dataset/private/raw -d dataset/private/labeled.jsonl --labeler me
$B eval dataset/public dataset/private --min-recall HEALTH=0.95
```

`scan` exits with code 4 when the text would be blocked. Configuration
(thresholds, allowlist, denylist) is described in
[privatsphaere.example.toml](privatsphaere.example.toml); special categories
cannot be configured.

## Clipboard guard

```sh
$B guard                                 # review mode, in a terminal next to your browser
$B guard --audit-log ~/privatsphaere-audit.jsonl
```

`guard` watches the clipboard. When copied text contains personal data or
secrets, it beeps, shows the findings and the pseudonymized version, and only
replaces the clipboard after you confirm (Enter). Typing numbers such as `2,4`
toggles single findings. Text with GDPR Art. 9/10 data is replaced by a notice
right away, without asking, and this cannot be overridden; the same happens
when detection fails. When you copy an LLM answer that contains placeholders
from this session, `guard` offers to restore the originals (default: no, so a
pseudonymized text on its way back into a chat is not revealed by accident).
The vault exists only in memory and is wiped when `guard` exits or on `c`.

The audit log is off by default. When enabled it stores, per decision, the
time, the action, the number of findings per type, rule ids and the text
length, never the values themselves or hashes of them.

On Linux (X11) a pseudonymized clipboard is owned by the `guard` process and
is gone once `guard` exits.

## Desktop app

```sh
cargo build --release -p app
target/release/privatsphaere-app [--config privatsphaere.toml] [--audit-log FILE]
```

The same review mode as `guard`, with a window and a tray icon. When the
clipboard has findings, the window comes to the front with the copied text
(findings highlighted, one checkbox each) and the pseudonymized version;
Enter pseudonymizes, Esc keeps the original. Blocked text is shown with the
special-category terms marked in red while the clipboard already holds the
notice. Reveal needs a click; Enter does not reveal. Closing the window keeps
the app running in the tray (menu: show window, reveal clipboard, clear vault,
quit). The UI is German, or English when the locale is not German.

Linux needs GTK 3 and libayatana-appindicator for the tray icon
(`libgtk-3-dev libxdo-dev libayatana-appindicator3-dev` to build); build with
`--no-default-features` for a window without a tray. On GNOME the icon only
appears with the AppIndicator extension. Without a tray, closing the window
quits.

## Supported input formats

`scan` and `label` read plain text and Markdown, PDF (text layer, no OCR),
Word (`.docx`), PowerPoint (`.pptx`), OpenDocument (`.odt`, `.odp`), RTF,
HTML, e-mail (`.eml`, `.mbox`, including From/To/Cc/Subject headers), Excel
and OpenDocument spreadsheets (`.xlsx`, `.xlsm`, `.xls`, `.ods`), CSV/TSV
(`;` is detected automatically), JSON and JSON Lines. Tables and JSON arrays
become one text per row: the column named like `text`, `body`, `nachricht`,
`inhalt`, … is used, or `--column NAME`; without one, all cells of a row are
joined as `header: value` lines. All parsing is pure Rust and offline.

## Accuracy process

1. `label`: shows every candidate from every recognizer; you accept, reject or
   correct each one and add what was missed. Results go to a JSONL dataset
   ([format](dataset/README.md)), which can also be exported to CSV for
   spreadsheet review.
2. `eval`: precision, recall and F1 per entity type and per rule, document
   level blocking for Art. 9/10, and a list of false positives and negatives
   with context.
3. Rules and thresholds are tuned against the dataset; metrics per release go
   into [CHANGELOG-accuracy.md](CHANGELOG-accuracy.md).
4. In CI, a pull request fails if it lowers recall on any Art. 9/10 category
   on `dataset/public` (base code and PR code are evaluated on the same data)
   or removes special-category examples (`scripts/accuracy-gate.sh`).

Watch mode may only be enabled once the targets in
[accuracy-targets.toml](accuracy-targets.toml) are met on the full dataset
(currently HEALTH recall ≥ 0.98): `privatsphaere eval dataset/public dataset/private --targets accuracy-targets.toml`.

## Workspace

| Crate | Purpose |
|---|---|
| `detect` | recognizers, NER trait, engine |
| `gliner` | optional GLiNER backend on ONNX Runtime (loaded at run time) |
| `vault` | placeholders, `reveal()`, zeroization |
| `dataset` | JSONL/CSV/markup format, eval metrics |
| `clip` | clipboard watcher |
| `ingest` | text extraction from PDF, Office, e-mail, spreadsheets, CSV, JSON, HTML, RTF |
| `session` | review-mode rules shared by `guard` and the app, audit log |
| `cli` | `privatsphaere` binary |
| `app` | `privatsphaere-app` desktop window and tray icon; nothing depends on it |

## License

Apache-2.0, see [LICENSE](LICENSE).

---

<a id="deutsch"></a>

# Privatsphäre (Deutsch)

Ein rein lokal arbeitender Zwischenablage-Wächter für Nutzer in Deutschland
und der EU. Er erkennt personenbezogene Daten und Geheimnisse in kopiertem
Text und ersetzt sie durch typisierte Platzhalter (`[PERSON_1]`, `[IBAN_1]`),
bevor der Text in einen LLM-Chat eingefügt wird. Die Zuordnung bleibt in einem
Tresor im Arbeitsspeicher, sodass die Antwort des LLM lokal wiederhergestellt
werden kann. Texte mit Daten nach Art. 9/10 DSGVO werden blockiert, nicht
geschwärzt.

> **Status: Pre-Alpha.** Erkennungs-Engine, Tresor sowie die Werkzeuge
> `label` und `eval` funktionieren, ebenso der Prüfmodus im Terminal
> (`guard`) und in der Desktop-App (`privatsphaere-app`). Das
> automatische Ersetzen („Watch-Modus“) bleibt abgeschaltet, bis die Erkennung
> an echten, gelabelten Daten validiert ist.

## Wichtig: Was dieses Werkzeug ist und was nicht

- **Es handelt sich um Pseudonymisierung, nicht um Anonymisierung.** Die
  Platzhalter lassen sich mit dem lokalen Tresor umkehren, und der übrige Text
  enthält weiterhin Kontext.
- **Pseudonymisierte Daten sind nach der DSGVO weiterhin personenbezogene
  Daten** (Art. 4 Nr. 5 und Erwägungsgrund 26). Die Übermittlung an einen
  LLM-Anbieter bleibt eine Verarbeitung personenbezogener Daten.
- **Das Werkzeug verringert das Risiko. Es ersetzt keinen
  Auftragsverarbeitungsvertrag (Art. 28 DSGVO) mit dem LLM-Anbieter**, keine
  Rechtsgrundlage und keine Prüfung von Drittlandübermittlungen.
- Die Erkennung ist heuristisch und übersieht Dinge. Das Blockieren besonderer
  Kategorien (Gesundheit, Religion, sexuelle Orientierung,
  Gewerkschaftszugehörigkeit, ethnische Herkunft, politische Meinungen,
  genetische und biometrische Daten, Straftaten) ist ein Sicherheitsnetz,
  keine Garantie.
- Dies ist keine Rechtsberatung.

## Datenschutz-Zusagen

- Keinerlei Netzwerkcode: keine Telemetrie, keine Update-Prüfung, keine
  Modell-Downloads. Die CI schlägt fehl, sobald irgendein HTTP-, TLS-,
  WebSocket- oder DNS-Client-Crate im Abhängigkeitsbaum auftaucht oder eigener
  Code Socket-APIs nutzt (`scripts/check-offline.sh`).
- Originalwerte liegen nur im Arbeitsspeicher und werden beim Verwerfen des
  Tresors überschrieben.
- Echte Beispieltexte für den Datensatz bleiben in `dataset/private/`, das von
  git ignoriert wird.

## Was erkannt wird

Namen (auch in kleingeschriebenem Chat-Text), Adressen und PLZ,
Telefonnummern (`0171…`, `+49…`), E-Mail, URLs, IBAN/BIC (Prüfsumme),
Kreditkarten (Luhn), Steuer-ID (Prüfziffer) und Steuernummer,
Sozialversicherungsnummer (Prüfziffer), Personalausweis- und Passnummern
(Prüfziffer), Kfz-Kennzeichen, Datumsangaben, IP-Adressen, Organisationen mit
Rechtsformzusatz, API-Schlüssel (OpenAI, Anthropic, AWS, GitHub, GitLab,
Stripe, Slack, Google, …), private Schlüssel, JWTs, Datenbank-Verbindungs-
strings, `.env`-Geheimnisse sowie die oben genannten Kategorien nach
Art. 9/10. Optional ergänzt ein lokales GLiNER-Modell auf ONNX Runtime Namen,
Organisationen, Orte und eine zweite Verteidigungslinie für Art. 9/10 (siehe
„Lokale NER“).

## Lokale NER (optional)

Die Regeln übersehen Namen ohne Hinweiswort („anna hat angerufen“). Ein
GLiNER-Modell schließt einen großen Teil dieser Lücke. Das Programm lädt nichts
herunter; Sie stellen bereit:

1. ONNX Runtime ab 1.17 als Bibliothek (`libonnxruntime.so`,
   `libonnxruntime.dylib` oder `onnxruntime.dll`, aus dem offiziellen Release
   oder z. B. per `pip install onnxruntime` in `onnxruntime/capi/`).
2. Ein nach ONNX exportiertes GLiNER-Span-Modell in einem Verzeichnis mit
   `model.onnx`, `tokenizer.json` und `gliner_config.json` (z. B.
   `gliner_multi_pii-v1`, mehrsprachig; Lizenz auf der Modellkarte prüfen).

```sh
$B ner-config /opt/models/gliner_multi_pii-v1 >> privatsphaere.toml   # fixiert SHA-256
$B scan --config privatsphaere.toml notizen.txt
$B eval --config privatsphaere.toml dataset/public dataset/private        # erst messen, dann verlassen
```

Modell und Tokenizer müssen zu den fixierten Hashes passen, sonst startet
nichts. Lässt sich ein konfiguriertes Modell nicht laden oder schlägt es bei
einem Text fehl, gilt der Text als blockiert (fail closed). Die Prompts für die
Kategorien nach Art. 9/10 sind fest; `[ner] labels` ändert nur die übrigen.

## Schnellstart

```sh
cargo build --release
B=target/release/privatsphaere

echo "Hallo Frau Schneider, IBAN DE89 3704 0044 0532 0130 00, Tel 0171 1234567" | $B scan
$B scan --candidates notizen.txt        # alle Kandidaten mit Score und Regel
$B label dataset/private/raw -d dataset/private/labeled.jsonl --labeler ich
$B eval dataset/public dataset/private --min-recall HEALTH=0.95
```

`scan` endet mit Code 4, wenn der Text blockiert würde. Die Konfiguration
(Schwellenwerte, Allowlist, Denylist) ist in
[privatsphaere.example.toml](privatsphaere.example.toml) beschrieben;
besondere Kategorien sind nicht konfigurierbar.

## Zwischenablage-Wächter

```sh
$B guard                                 # Prüfmodus, im Terminal neben dem Browser
$B guard --audit-log ~/privatsphaere-audit.jsonl
```

`guard` überwacht die Zwischenablage. Enthält kopierter Text personenbezogene
Daten oder Geheimnisse, piept es, zeigt die Funde und die pseudonymisierte
Fassung und ersetzt die Zwischenablage erst nach Bestätigung (Enter). Mit
Nummern wie `2,4` lassen sich einzelne Funde an- und abwählen. Text mit Daten
nach Art. 9/10 DSGVO wird sofort und ohne Rückfrage durch einen Hinweis
ersetzt; das lässt sich nicht übersteuern. Dasselbe passiert, wenn die
Erkennung fehlschlägt. Kopieren Sie eine LLM-Antwort mit Platzhaltern aus
dieser Sitzung, bietet `guard` an, die Originale wiederherzustellen (Vorgabe:
nein, damit ein pseudonymisierter Text auf dem Weg zurück in einen Chat nicht
versehentlich aufgedeckt wird). Der Tresor existiert nur im Arbeitsspeicher und
wird beim Beenden oder mit `c` gelöscht.

Das Audit-Log ist standardmäßig aus. Eingeschaltet speichert es pro
Entscheidung Zeit, Aktion, Anzahl der Funde je Typ, Regel-IDs und Textlänge,
niemals die Werte selbst oder Hashes davon.

Unter Linux (X11) gehört eine pseudonymisierte Zwischenablage dem
`guard`-Prozess und ist nach dessen Ende leer.

## Desktop-App

```sh
cargo build --release -p app
target/release/privatsphaere-app [--config privatsphaere.toml] [--audit-log DATEI]
```

Derselbe Prüfmodus wie `guard`, mit Fenster und Tray-Symbol. Hat die
Zwischenablage Funde, kommt das Fenster nach vorne und zeigt den kopierten Text
(Funde markiert, je eine Checkbox) und die pseudonymisierte Fassung; Enter
pseudonymisiert, Esc behält das Original. Blockierter Text wird mit rot
markierten Begriffen der besonderen Kategorien angezeigt, während die
Zwischenablage bereits den Hinweis enthält. Aufdecken braucht einen Klick;
Enter deckt nicht auf. Schließen des Fensters lässt die App im Tray weiterlaufen
(Menü: Fenster zeigen, Zwischenablage aufdecken, Tresor leeren, Beenden). Die
Oberfläche ist deutsch, bei nicht-deutscher Spracheinstellung englisch.

Unter Linux braucht das Tray-Symbol GTK 3 und libayatana-appindicator (zum
Bauen `libgtk-3-dev libxdo-dev libayatana-appindicator3-dev`); mit
`--no-default-features` entsteht ein Fenster ohne Tray. Unter GNOME erscheint
das Symbol nur mit der AppIndicator-Erweiterung. Ohne Tray beendet das
Schließen des Fensters die App.

## Unterstützte Eingabeformate

`scan` und `label` lesen Text und Markdown, PDF (Textebene, keine
Texterkennung), Word (`.docx`), PowerPoint (`.pptx`), OpenDocument (`.odt`,
`.odp`), RTF, HTML, E-Mails (`.eml`, `.mbox`, inklusive Von/An/Cc/Betreff),
Excel- und OpenDocument-Tabellen (`.xlsx`, `.xlsm`, `.xls`, `.ods`), CSV/TSV
(`;` wird automatisch erkannt), JSON und JSON Lines. Tabellen und JSON-Arrays
ergeben einen Text pro Zeile: verwendet wird die Spalte namens `text`,
`body`, `nachricht`, `inhalt` usw. oder `--column NAME`; gibt es keine, werden
alle Zellen einer Zeile als `Spalte: Wert` zusammengefügt. Alles wird rein in
Rust und offline verarbeitet.

## Vorgehen zur Erkennungsgenauigkeit

1. `label`: zeigt jeden Kandidaten jedes Erkenners; du akzeptierst, verwirfst
   oder korrigierst ihn und ergänzt Übersehenes. Die Entscheidungen landen in
   einem JSONL-Datensatz ([Format](dataset/README.md)), der sich auch als CSV
   für die Prüfung in einer Tabellenkalkulation exportieren lässt.
2. `eval`: Precision, Recall und F1 je Entitätstyp und je Regel, Blockierung
   auf Dokumentebene für Art. 9/10 sowie eine Liste der Fehlalarme und
   übersehenen Stellen mit Kontext.
3. Regeln und Schwellenwerte werden am Datensatz verbessert; die Kennzahlen je
   Release stehen in [CHANGELOG-accuracy.md](CHANGELOG-accuracy.md).
4. In der CI schlägt ein Pull Request fehl, wenn er den Recall einer
   Art.-9/10-Kategorie auf `dataset/public` senkt (alter und neuer Code werden
   auf denselben Daten gemessen) oder Beispiele besonderer Kategorien entfernt
   (`scripts/accuracy-gate.sh`).

Der Watch-Modus darf erst eingeschaltet werden, wenn die Ziele in
[accuracy-targets.toml](accuracy-targets.toml) auf dem gesamten Datensatz
erreicht sind (derzeit HEALTH-Recall ≥ 0,98).

## Lizenz

Apache-2.0, siehe [LICENSE](LICENSE).
