#!/usr/bin/env bash
#
# init-submodules.sh — initialize exactly the submodules this workspace builds.
#
# Why not `git submodule update --init --recursive`:
#
# `vendor/tinyagents` carries a `wiki` submodule that nothing here compiles, and
# `--recursive` descends unconditionally. That is the whole of the difference
# now, so this script is close to trivial — worth keeping anyway, because it is
# also the one place the vendored set is written down, and it must stay in
# lockstep with the root manifest's `[patch.crates-io]` table.
#
# This used to be a long file. `vendor/openhuman` carried sixteen submodules of
# its own, two of them a Tauri fork bundling CEF, and every crate the root
# manifest patched had to be listed here by hand or resolution failed before a
# line compiled — with cargo reporting one missing entry at a time, so each fix
# cost a CI round trip. Removing the embedded core removed all of it.

set -euo pipefail

cd "$(dirname "$0")/.."

# Initialize the direct vendored dependencies first. Keep this explicit rather
# than using --recursive: tinyagents also carries a wiki submodule that is not
# needed to build medulla.
git submodule update --init --depth 1 \
  vendor/tinyagents \
  vendor/tinyflows \
  vendor/tinyhumans-sdk

# tinyagents' buildable crates are nested submodules, so initialize those while
# deliberately leaving its unrelated wiki checkout untouched.
git -C vendor/tinyagents submodule update --init --depth 1 \
  vendor/tinyinference \
  vendor/tinytools

echo "Submodules initialized (tinyagents, tinyinference, tinytools, tinyflows, tinyhumans-sdk)."
