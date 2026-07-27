#!/usr/bin/env bash
# Reject references that disclose private implementation names or source layout.
set -euo pipefail

patterns=(
  'medulla[-_ ]?v1'
  'workflow-medulla'
  'createAgentHarness'
  'TypeScript'
  'type[ -]?script'
  'src/(ports|agent|core)/[^[:space:]]+\.ts'
  '(budgetRoster|taskTools|memoryTools|taskTracker|harness-events)\.ts'
  '(senamakel|enamakel)'
)

failed=0
for pattern in "${patterns[@]}"; do
  if matches=$(git grep -n -I -i -E "$pattern" -- \
    ':!vendor/**' \
    ':!scripts/check-public-boundary.sh' \
    ':!Cargo.lock'); then
    printf 'Public-boundary violation matching /%s/:\n%s\n' "$pattern" "$matches" >&2
    failed=1
  fi
done

if ((failed)); then
  printf 'Remove private implementation provenance or explicitly document why the check must change.\n' >&2
  exit 1
fi

printf 'Public-boundary check passed.\n'
