# Labeled dataset

The dataset is the regression test suite for detection. CI evaluates
`dataset/public` (synthetic, safe to publish). Real samples live in
`dataset/private` and never leave your machine.

## JSONL format

One record per line. Later lines with the same `id` replace earlier ones, so
files can be appended to safely.

```json
{"id":"email-3f2a9c1b0d4e5f67","text":"Hallo Jens, Tel 0171 1234567","source":"email","complete":true,"synthetic":false,
 "spans":[
  {"start":6,"end":10,"text":"Jens","entity":"PERSON","verdict":"accepted","recognizer":"person.greeting","score":0.8},
  {"start":16,"end":28,"text":"0171 1234567","entity":"PHONE","verdict":"accepted","recognizer":"phone.mobile_de","score":0.9}
 ],
 "labeled_by":"tim","labeled_at":"2026-10-16T12:00:00Z"}
```

| Field | Meaning |
|---|---|
| `start`, `end` | **Character** offsets (Unicode scalar values), not bytes. `text` must equal that slice; files are validated on load. |
| `entity` | One of the entity types (`PERSON`, `IBAN`, `HEALTH`, …), see `crates/detect/src/entity.rs`. |
| `verdict` | `accepted` (proposal correct), `rejected` (proposal wrong, kept as an explicit negative), `corrected` (proposal with fixed boundaries/type; `proposed` keeps the original), `added` (missed by all recognizers). |
| `recognizer`, `score` | Which rule proposed the span and with what score. |
| `complete` | `true` if every sensitive span in the text is labeled. Unlabeled predictions only count as false positives in complete records (or when they overlap a `rejected` span). |
| `synthetic` | `true` for invented samples that may be published. |

Gold spans = `accepted` + `corrected` + `added`.

## Adding your samples

1. Put raw texts into `dataset/private/raw/` as `.txt` or `.md` files. Several
   texts per file are separated by a line containing only `---`. Optional
   directives at the top of a text: `@source: email`, `@id: …`, `@lang: de`.
   A `.jsonl` file with a `"text"` field per line also works.
2. Label them:
   ```sh
   privatsphaere label dataset/private/raw -d dataset/private/labeled.jsonl --labeler tim
   ```
   Every candidate from every recognizer is shown, including low scores. Answer
   `a` (accept), `r` (reject), `c` (correct type or boundaries), then type any
   missed spans. Each finished text is saved immediately; rerunning resumes.
3. Evaluate:
   ```sh
   privatsphaere eval dataset/public dataset/private
   ```

## Spreadsheet review

```sh
privatsphaere export-csv dataset/private/labeled.jsonl -o review.csv
# edit in Excel/LibreOffice: change entity/verdict, fix span_text, add rows, verdict "delete" removes
privatsphaere import-csv review.csv -d dataset/private/labeled.jsonl
```

The CSV uses `;` and a UTF-8 BOM so German Excel opens it directly
(`--delimiter ,` for others). Rows without `span_index` add new spans, located
by `span_text`. Editing `span_text` moves the span to the nearest occurrence.

## Hand-written samples (markup)

`dataset/synthetic-src/*.md` uses inline markup and is converted with
`import-markup`:

```text
@source: email
Hallo [[PERSON|Anna Schmidt]], IBAN [[IBAN|DE89 3704 0044 0532 0130 00]].
---
Nächster Text.
```

```sh
privatsphaere import-markup dataset/synthetic-src/positives.md --synthetic --source synthetic -o dataset/public/synthetic-positives.jsonl
```

## Why real samples are not in CI

Real emails and tickets are personal data. They stay in `dataset/private`
(git-ignored). For the CI recall gate to cover patterns from real data, add
synthetic look-alikes (same structure, invented values) to
`dataset/synthetic-src` and regenerate `dataset/public`.
