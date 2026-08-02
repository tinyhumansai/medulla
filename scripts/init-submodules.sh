#!/usr/bin/env bash
#
# init-submodules.sh — initialize exactly the submodules this workspace builds.
#
# Why not `git submodule update --init --recursive`:
#
# `vendor/openhuman` carries submodules of its own, and two of them belong to
# the OpenHuman *desktop* app rather than to anything medulla compiles:
#
#   app/src-tauri/vendor/tauri-cef                  a Tauri fork bundling CEF
#   app/src-tauri/vendor/tauri-plugin-notification
#
# `--recursive` descends unconditionally and would clone both. The Cargo
# workspace excludes `vendor/`, so `app/src-tauri` is not a workspace member and
# nothing in medulla's dependency graph references either tree. It is pure cost.
#
# Also skipped: the `wiki` documentation submodules, and
# `vendor/openhuman/vendor/tinycortex/vendor/tinyagents` — the root
# `[patch.crates-io]` table redirects `tinyagents` to
# `vendor/openhuman/vendor/tinyagents`, so tinycortex's own nested copy is never
# resolved. (tinycortex ships a `.cargo/config.toml` patching it, but that file
# is CWD-scoped and is ignored when tinycortex is a path dependency of this
# workspace.) If a build ever does stat that path, add it to the list below —
# it is cheap.
#
# Note a git-dependency on openhuman would NOT avoid the Tauri clone: Cargo
# updates git-dependency submodules recursively with no opt-out. The submodule
# plus this explicit list is the only way to keep the CEF fork out.

set -euo pipefail

cd "$(dirname "$0")/.."

# The embedded OpenHuman core.
git submodule update --init --depth 1 vendor/openhuman

# Its vendored crates, which the root patch table redirects Cargo to. This list
# must stay in lockstep with the `[patch.crates-io]` table in Cargo.toml.
git -C vendor/openhuman submodule update --init --depth 1 \
  vendor/tinyagents \
  vendor/tinychannels \
  vendor/tinycortex \
  vendor/tinyflows \
  vendor/tinyhumans-sdk \
  vendor/tinyjuice \
  vendor/tinyplace

echo "Submodules initialized (OpenHuman core + its seven vendored crates)."
