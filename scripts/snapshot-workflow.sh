#!/usr/bin/env bash
# Snapshot the Workflows tab for one workflow, as text, at several sizes.
#
#   scripts/snapshot-workflow.sh issue-implement            # by id, from the real store
#   scripts/snapshot-workflow.sh path/to/workflow.json      # or from a file
#   OUT=/tmp/snap scripts/snapshot-workflow.sh issue-implement
#
# A workflow id is resolved to the newest revision the operator's Medulla home
# holds for it, read-only — nothing is launched and the real home is never
# written to. The rendering itself is `--example workflow_snapshot`, which draws
# the tab onto ratatui's TestBackend; this script is only the part that finds
# the workflow, picks the views worth looking at, and writes them side by side
# so two sizes of the same screen can be diffed.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="$1"
OUT="${OUT:-$ROOT/target/workflow-snapshots}"
SIZES="${SIZES:-100x30 140x40 200x50}"
MEDULLA_HOME="${MEDULLA_HOME:-$HOME/.medulla}"

resolve() {
  # A path is taken as given. An id is looked up in the live store first — the
  # store's own current definition, at `<home>/workflows/<id>.json` — because
  # the revisions directory holds only *superseded* graphs (see
  # `src/sdk/src/workflows/store/mod.rs`): the current graph after any edit is
  # the live definition, not a revision, and a workflow that has never been
  # edited has no revision at all. Only when there is no live definition does
  # this fall back to the newest revision, whose filenames sort lexicographically
  # by mint time — so the last one is current.
  if [ -f "$TARGET" ]; then printf '%s' "$TARGET"; return; fi
  local live="$MEDULLA_HOME/workflows/$TARGET.json"
  if [ -f "$live" ]; then printf '%s' "$live"; return; fi
  local newest
  newest=$(find "$MEDULLA_HOME" -type d -name "$TARGET" -path '*/revisions/*' -prune \
             -exec sh -c 'ls "$1"/*.json 2>/dev/null | sort | tail -n 1' _ {} \; 2>/dev/null \
           | sort | tail -n 1)
  [ -n "$newest" ] || { echo "no workflow '$TARGET' under $MEDULLA_HOME (and no such file)" >&2; exit 1; }
  printf '%s' "$newest"
}

WORKFLOW="$(resolve)"
echo "workflow: $WORKFLOW" >&2

cargo build --quiet --manifest-path "$ROOT/Cargo.toml" -p medulla-tui --example workflow_snapshot
BIN="$ROOT/target/debug/examples/workflow_snapshot"

mkdir -p "$OUT"
# The views an operator actually passes through: the catalogue, the graph, the
# selected node's declaration, and the copilot that edits it.
snap() { # name keys size
  local name="$1" keys="$2" size="$3"
  local file="$OUT/$name.$size.txt"
  "$BIN" --workflow "$WORKFLOW" --size "$size" --keys "$keys" > "$file"
  echo "  $file"
}

for size in $SIZES; do
  snap rail    ""            "$size"
  snap graph   "enter"       "$size"
  snap node    "enter,i"     "$size"
  snap deep    "enter,right,right,i" "$size"
  snap copilot "c"           "$size"
done

echo "snapshots in $OUT" >&2
