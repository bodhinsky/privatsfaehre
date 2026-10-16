#!/usr/bin/env bash
# Art. 9/10 recall gate.
#
# Usage: scripts/accuracy-gate.sh [BASE_REF]
#
# Evaluates the current code on dataset/public. With BASE_REF, also builds the
# base revision and evaluates *its* code on the *same* (current) dataset, then
# fails if recall on any GDPR special category drops. It also fails if the PR
# removes special-category gold spans from the public dataset, so the gate
# cannot be passed by deleting hard examples.
set -euo pipefail
cd "$(dirname "$0")/.."
BASE_REF="${1:-}"
DATA=dataset/public
OUT=target/accuracy
mkdir -p "$OUT"

cargo build --release -q -p cli
HEAD_BIN=target/release/privatsphaere
"$HEAD_BIN" validate "$DATA"
"$HEAD_BIN" --no-color eval "$DATA" --json-out "$OUT/head.json" --errors 50

if [ -z "$BASE_REF" ]; then
  exit 0
fi

BASE_DIR=$(mktemp -d)
trap 'git worktree remove --force "$BASE_DIR" >/dev/null 2>&1 || true' EXIT
git worktree add --detach "$BASE_DIR" "$BASE_REF" >/dev/null

if [ ! -f "$BASE_DIR/crates/cli/Cargo.toml" ]; then
  echo "base revision has no CLI yet; skipping comparison"
  exit 0
fi

(cd "$BASE_DIR" && cargo build --release -q -p cli)
BASE_BIN="$BASE_DIR/target/release/privatsphaere"

# Base code, current data.
"$BASE_BIN" --no-color eval "$DATA" --json-out "$OUT/base.json" --errors 0 >/dev/null || {
  echo "::error::base revision could not evaluate the current dataset (format change?); review manually"
  exit 1
}
"$HEAD_BIN" --no-color eval "$DATA" --compare "$OUT/base.json" --errors 0 >/dev/null

# Special-category gold spans must not be removed.
special='["HEALTH","RELIGION","SEXUAL_ORIENTATION","UNION_MEMBERSHIP","ETHNICITY","POLITICAL_OPINION","GENETIC","BIOMETRIC","CRIMINAL_RECORD"]'
count() { jq --argjson s "$special" '[.gold_by_entity | to_entries[] | select(.key as $k | $s | index($k)) | .value] | add // 0'; }
base_n=$("$HEAD_BIN" stats "$BASE_DIR/$DATA" --json | count)
head_n=$("$HEAD_BIN" stats "$DATA" --json | count)
if [ "$head_n" -lt "$base_n" ]; then
  echo "::error::special-category gold spans dropped from $base_n to $head_n"
  exit 1
fi
echo "accuracy gate passed (special-category gold spans: $base_n -> $head_n)"
