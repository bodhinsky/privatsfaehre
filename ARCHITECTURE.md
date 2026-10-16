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
  cli      `privatsphaere` binary: scan, label, eval -> detect, vault, dataset
  app      tray UI (placeholder)                     -> detect, vault, clip
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
         leave as is        vault.pseudonymize(spans)    leave clipboard untouched*,
                            review: show diff, confirm   show warning naming the
                            watch: replace clipboard     category (Art. 9/10)

LLM answer copied ─► user triggers "Reveal" ─► vault.reveal() ─► clipboard
```

\* In watch mode the clipboard is replaced by a short notice so the text cannot
be pasted by accident; the user can restore the original locally from the tray.

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

## clip

Polling watcher over a `ClipboardBackend` trait with change detection and loop
protection (its own writes are ignored). `arboard` is the default backend;
platform change notifications (macOS change count, Windows
`AddClipboardFormatListener`, X11 XFixes, Wayland data-control) can replace
polling later.

## app: recommendation

**`egui`/`eframe` plus `tray-icon`, not Tauri.**

- No webview: no HTML/JS engine that could fetch remote resources, and no
  WebView2/WebKit runtime with its own update and telemetry behaviour. That
  makes the "nothing leaves the machine" claim much easier to audit.
- Single Rust binary with a smaller dependency tree and binary.
- The review UI (text with highlighted spans, per-span toggles, a diff of the
  pseudonymised result) fits immediate-mode UI well.

Costs: less polished look than web UI, and `tray-icon` needs GTK and
libappindicator on Linux.

## Review mode and watch mode

- Review mode (default): every clipboard change with findings opens the review
  window; nothing is replaced without confirmation.
- Watch mode (automatic replacement) can only be turned on once `eval` metrics
  on the full dataset meet targets the maintainer sets (for example
  `--min-recall HEALTH=0.98`). The targets and the report the build was checked
  against will be embedded at build time.

## Audit log (planned)

Opt-in, local JSONL: timestamp, action (redacted, blocked, revealed, allowed),
counts per entity type, rule ids, text length. Never original values, and no
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
