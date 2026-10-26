# Architecture

## Goals and constraints

- Local only: no network code, no telemetry, no update checks. Enforced in CI (`scripts/check-offline.sh`).
- German first, English fallback.
- Detection accuracy before automation: the app stays in review mode until measured metrics reach targets set by the maintainer.
- GDPR Art. 9/10 data is never redacted, only blocked, and that is not configurable.
- Single Rust workspace, no Python runtime, small binary.

## Workspace

```
crates/
  detect   recognizers, NER trait, Engine           (no I/O, no UI)
  vault    placeholder vault, reveal(), zeroization  (independent of detect)
  dataset  JSONL/CSV/markup format, eval metrics     -> detect
  clip     clipboard watcher (arboard backend)
  ingest   text extraction: PDF, Office, ODF, e-mail, spreadsheets, CSV, JSON, HTML, RTF
  cli      `privatsphaere` binary: guard, scan, label, eval -> detect, vault, dataset, ingest, clip
  session  review-mode rules, audit log              -> detect, vault, clip
  app      desktop window + tray icon (egui)         -> session
dataset/   public (synthetic, in CI) and private (git-ignored) samples
scripts/   check-offline.sh, accuracy-gate.sh
```

Dependency direction is strictly towards `detect`/`vault`. Nothing depends on
`app`; CI fails if any other crate pulls in `app` or a UI toolkit.

## Data flow

```
clipboard change ─► clip::Watcher ─► detect::Engine::analyze
                                        │
             ┌──────────────────────────┼──────────────────────────┐
         Verdict::Clean           Verdict::Redact            Verdict::Block
         leave as is        vault.pseudonymize(spans)    replace clipboard with a
                            review: show diff, confirm   notice*, show warning naming
                            watch: replace clipboard     the category (Art. 9/10)

LLM answer copied ─► user triggers "Reveal" ─► vault.reveal() ─► clipboard
```

\* In both modes, without asking, so the text cannot be pasted by accident.
There is no override; the original is still in the application it was copied
from.

**Fail closed:** any detection error (for example a NER model failure) is
treated like a block. `Recognizer::detect` returns `Result` for that reason.

## detect

- `EntityType` is a closed enum. Its `Disposition` (`Redact` or `Block`) is a
  `const fn`; special categories are `Block` by construction.
- `Recognizer` trait: `id()`, `entities()`, `detect(text, &mut Vec<Detection>)`.
  Detections carry byte offsets, a score in `[0, 1]` and a fine-grained rule id
  (`iban.checksum`, `phone.mobile_de`) so eval can report per rule.
- Built-in recognizers emit candidates with graded scores instead of yes/no.
  Checksum-valid matches score ≈ 1; format-only matches (an IBAN with a wrong
  check digit, an ID number without context) are still emitted with low scores
  so that `label` mode shows them and they end up in the dataset as near misses.
- `Engine::candidates` returns everything (deduplicated); `Engine::analyze`
  applies thresholds, the allowlist (never for special categories), the
  denylist, and resolves overlaps greedily by score then length.
- Special categories use a fixed `SPECIAL_CATEGORY_THRESHOLD`. Configuring a
  threshold for them, allow-listing them or disabling their recognizer is a
  configuration error.

| Recognizer | Validation |
|---|---|
| IBAN | country length table + ISO 7064 MOD 97-10 |
| BIC | ISO 3166 country code, context-dependent score |
| Steuer-ID | ISO 7064 MOD 11,10 + § 139b AO digit distribution |
| Steuernummer | format + context |
| Sozialversicherungsnummer | date part + weighted check digit |
| Personalausweis / Reisepass | ICAO 9303 check digit (7-3-1) |
| Credit card | Luhn + issuer prefix |
| Kfz-Kennzeichen | district code list + format + context |
| Phone | DE mobile/landline, international, context words |
| Address / PLZ | street suffixes, prepositional names, PLZ+city with positive/negative context |
| Dates | German numeric and month names, ISO, English; birth-date context |
| IP | `std::net` parsing; RFC 1918/loopback/link-local/ULA scored as internal |
| Secrets | OpenAI, Anthropic, AWS, GitHub, GitLab, Stripe, Slack, Google, Hugging Face, SendGrid, npm, bearer tokens, private key blocks, JWT, DB connection strings, `.env` lines, inline passwords |
| Names | honorifics, greetings, signatures, first-name/surname gazetteer (case-insensitive) |
| Organisations | legal-form suffixes (GmbH, AG, e.V., …) |
| Art. 9/10 | German/English lexicons (strong/weak terms, case-sensitive party and union names) with personal and category cues, ICD-10 codes with context, criminal court file numbers |

### NER: recommendation

**GLiNER (multilingual PII checkpoint) on ONNX Runtime via `ort`.**

- One multilingual model covers German and English.
- Zero-shot labels: besides person/organisation/location we can prompt for
  "health condition", "religion", "political party", and so on, which gives a
  second, semantic line of defence for Art. 9/10 recall behind the lexicons.
- Span-based and less dependent on capitalisation than a cased BERT token
  classifier, which matters for lower-case chat text.

Trade-offs: GLiNER on an mDeBERTa-v3 base is larger and slower than a German
BERT NER model (expect a few hundred MB as int8 ONNX). Clipboard snippets are
short, so latency is acceptable; thresholds per label are tuned with `eval`.

The alternative, a German BERT token classifier (GermEval-style PER/LOC/ORG),
is smaller and faster but has no labels for special categories and is weaker on
lower-case text. Because NER sits behind the `NerModel` trait, both can be
measured on the same dataset before deciding for good.

Implementation notes:

- `ort`'s default `download-binaries` feature fetches ONNX Runtime at build
  time over HTTP and would fail the offline check. Use
  `default-features = false` with `load-dynamic` and ship the runtime library
  with the app.
- `candle` (pure Rust) would avoid the native runtime, but has no ready
  DeBERTa-v3/GLiNER implementation; revisit later.
- Models are never downloaded by the program. The user places the model file,
  and its SHA-256 is pinned in the configuration.
- Check the model card's license before bundling.

Implemented in `crates/gliner` (feature `ner` of `session`, on by default):

- Not `gline-rs`: it enables `ort`'s `download-binaries` and `tokenizers`'
  `http` feature, both network clients. Our crate uses `ort` with
  `load-dynamic` only and `tokenizers` without `http`/`hf-hub`.
- Span mode (`markerV0`) only: words via GLiNER's splitter regex, prompt
  `<<ENT>> label … <<SEP>> words`, `words_mask` marking each word's first
  sub-token, all spans up to `max_width` words, sigmoid, greedy flat
  decoding. Long texts run in overlapping word windows.
- `model.onnx` and `tokenizer.json` are hashed (SHA-256) and compared with
  the pins before ONNX Runtime is even loaded. `privatsphaere ner-config DIR`
  prints the section with the pins.
- Special-category prompts come from `default_gliner_labels()` and cannot be
  replaced or re-scored from the configuration; NER detections then go
  through the engine like any other, including the fixed special-category
  threshold.
- Tests: pre/post-processing unit tests, and an end-to-end run on ONNX Runtime
  with a tiny synthetic model in `tests/fixtures/tiny` that has GLiNER's
  signature (CI installs ONNX Runtime for it on Linux). Accuracy with a real
  checkpoint has to be measured with `eval`.

## vault

- Placeholders are `[KIND_n]`, numbered per kind in order of first appearance.
- Consistency: values are normalised per kind before lookup (names:
  whitespace/case; IBAN/IDs: alphanumerics only; phone: `+49`/`0049` → `0`), so
  `Thomas Müller` and `thomas  müller` share `[PERSON_1]`.
- `reveal()` accepts the variants LLMs produce: any case, inner spaces,
  Markdown-escaped brackets; `RevealMode::Lenient` also restores bare
  `PERSON_1`. Unknown placeholders are left in place and reported.
- Memory hygiene: originals and normalised keys are `SecretString` (wiped on
  drop); buffers are pre-sized so reallocation leaves no unwiped copies;
  `reveal()` returns `Zeroizing<String>`. An integration test installs a global
  allocator that scans every freed block for canary values and fails if one is
  freed unwiped.
- Not yet: `mlock` of vault pages, and encrypted persistence. Persistence will
  be opt-in (off by default) using `age` with a passphrase, pure Rust.

## ingest

Turns files into plain text with pure-Rust parsers: `pdf-extract` (text layer
only, no OCR), `zip` + `quick-xml` for `.docx`/`.pptx`/`.odt`/`.odp` (body,
headers, footers, footnotes, comments and speaker notes; tracked deletions and
field codes are skipped), `calamine` for spreadsheets, `mail-parser` for
`.eml`/`.mbox` (address headers are kept because they are personal data), and
small built-in converters for HTML and RTF. Each format is a cargo feature, so
the app can leave out what it does not need. Tabular inputs yield one text per
row. The offline check covers this crate's whole dependency tree.

## clip

Polling watcher over a `ClipboardBackend` trait with change detection and loop
protection (its own writes are ignored). `arboard` is the default backend;
platform change notifications (macOS change count, Windows
`AddClipboardFormatListener`, X11 XFixes, Wayland data-control) can replace
polling later.

## session

The review-mode decisions live in one place so the terminal and the window
cannot drift apart: `Session::poll` returns an `Event` (clean, review, blocked,
failed, reveal offer). Block and failure write the notice before any UI sees
the event; `preview` pseudonymizes and rolls back; `confirm` and
`accept_reveal` write only if the clipboard still holds what was analysed.

## app

**`egui`/`eframe` plus `tray-icon`, not Tauri.**

- No webview: no HTML/JS engine that could fetch remote resources, and no
  WebView2/WebKit runtime with its own update and telemetry behaviour. That
  makes the "nothing leaves the machine" claim much easier to audit.
- Single Rust binary with a smaller dependency tree and binary.
- The review UI (text with highlighted spans, per-span toggles, a diff of the
  pseudonymised result) fits immediate-mode UI well.

Costs: less polished look than web UI, and `tray-icon` needs GTK and
libappindicator on Linux.

Implementation:

- A background thread polls the clipboard and stores pending decisions in
  shared state, then raises the window. Blocking does not depend on frames
  being drawn, so it works while the window is minimised.
- eframe is built without `links` (would open a browser), `clipboard` (egui's
  own clipboard access) and `accesskit` (would expose clipboard text to the
  accessibility tree), and with glow instead of wgpu.
- On Linux the tray icon runs on its own GTK thread; elsewhere it is created on
  the UI thread.
- egui caches laid-out text (including originals) in memory it does not
  zeroize; this is a known gap of the window compared to `guard`.

## Review mode and watch mode

- Review mode (default): every clipboard change with findings opens the review
  window; nothing is replaced without confirmation.
- Watch mode (automatic replacement) can only be turned on once `eval` metrics
  on the full dataset meet targets the maintainer sets (for example
  `--min-recall HEALTH=0.98`), kept in `accuracy-targets.toml`. The targets and the report the build was checked
  against will be embedded at build time.

## guard (terminal review mode)

`privatsphaere guard` runs the data flow above in a terminal. Stdin is read on a background thread, so commands (`r` reveal,
`c` clear vault, `s` status, `q` quit) work while the clipboard is polled.
Findings are pseudonymized into the vault for the preview; if the user
declines, `Vault::rollback` removes those entries again. Writes use
`Watcher::replace_if_current`, so a confirmation never overwrites something the
user copied while the prompt was open. Reveal is offered automatically for
copied text with this session's placeholders, defaulting to no.

## Audit log

Opt-in (`--audit-log FILE` for `guard` and the app), local JSONL: timestamp, action (redacted,
blocked, revealed, allowed, error), counts per entity type, rule ids, text
length. Never original values, and no
hashes of values either, since hashes of short values like phone numbers can
be reversed by brute force.

## Threat model and limits

- Other programs and clipboard history managers can read the clipboard before
  this tool reacts.
- Placeholders keep the structure of the text; the LLM can still infer things
  from context.
- Text that is typed, dictated or shown in screenshots is not covered.
- Detection is heuristic. The dataset and eval exist to measure how often it
  fails, not to prove that it never does.
