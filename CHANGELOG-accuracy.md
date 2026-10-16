# Accuracy changelog

Per-release detection metrics from `privatsphaere eval`. Overlap matching is
the primary metric (a prediction counts if it overlaps a gold span of the same
type); strict = identical boundaries. Art. 9/10 document recall is the share of
records containing special-category data that were blocked.

Every entry states what the dataset contained. Metrics on data the rules were
tuned on are not evidence of real-world accuracy.

## 0.1.0 (unreleased)

**Dataset:** `dataset/public` only: 51 synthetic records (26 positives, 25
negatives), 67 gold spans, 12 records with special-category data. No real
samples yet. The rules were developed against this set, so the numbers below
only show that the pipeline works end to end.

| Metric | Before iteration | After |
|---|---|---|
| Micro precision (overlap) | 89.9 % | 100 % |
| Micro recall (overlap) | 96.9 % | 100 % |
| Art. 9/10 document recall | 100 % (12/12) | 100 % (12/12) |
| Art. 9/10 document false blocks | 0 / 25 negatives | 0 / 25 negatives |

Changes during iteration:

- `address.plz_city`: "Bestellung 12345 Lieferung" was a postcode false positive. The negative-context rule now matches word stems (`bestell…`, `…nummer`, `rechnung…`).
- `secret.env_assignment`: `Authorization: Bearer …` flagged the word "Bearer" as a secret; the bearer-token rule owns that case now.
- Labels: OpenAI key in an `.env` line relabeled `API_KEY` (was `SECRET`); bearer JWT relabeled `JWT`; "Patient", "Diagnose" and "Gentest" added as special-category gold spans.

Next entry: first metrics on Tim's labeled real samples (`dataset/private`).
