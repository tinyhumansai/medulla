#!/usr/bin/env bash
# Reject references that disclose private implementation names or source layout.
#
# WHAT THIS GUARD DOES NOT DO — read before trusting a green result
# -----------------------------------------------------------------
# This is a TEXTUAL check over tracked files. It is structurally blind to
# build-graph provenance, by construction:
#
#   * `':!vendor/**'` excludes the vendored submodules entirely, so anything
#     reachable through `vendor/` is invisible here.
#   * `':!Cargo.lock'` excludes the one tracked file that names every resolved
#     dependency and its source.
#   * `git grep` never descends into a gitlink, so adding a submodule pointing at
#     a private repository would add ZERO greppable text to this repository.
#
# Consequence: if this repo ever grows a dependency edge into another repo, this
# script passing tells you nothing about whether that was acceptable. Running it
# green over such a change would be actively misleading. The real check for that
# is a clean-clone build in a container with no credentials.
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
  # `git grep` exits 0 on match, 1 on no-match, and >=2 on ERROR (bad pattern,
  # bad pathspec, not a repo — 128 for several). The original form was
  #     if matches=$(git grep ...); then
  # which treats every non-zero exit as "no matches", so an error silently
  # PASSED the guard. Capture the status explicitly and treat >=2 as fatal.
  set +e
  matches=$(git grep -n -I -i -E "$pattern" -- \
    ':!vendor/**' \
    ':!scripts/check-public-boundary.sh' \
    ':!Cargo.lock')
  status=$?
  set -e

  case "$status" in
    0)
      printf 'Public-boundary violation matching /%s/:\n%s\n' "$pattern" "$matches" >&2
      failed=1
      ;;
    1)
      : # no matches — the good case
      ;;
    *)
      printf 'ERROR: git grep failed with status %d for pattern /%s/.\n' \
        "$status" "$pattern" >&2
      printf 'The guard could not evaluate this pattern, so its result is unknown — failing closed.\n' >&2
      failed=1
      ;;
  esac
done

if ((failed)); then
  printf 'Remove private implementation provenance or explicitly document why the check must change.\n' >&2
  exit 1
fi

printf 'Public-boundary check passed.\n'
