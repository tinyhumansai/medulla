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

# Its vendored crates and direct path dependencies. The crates patched by the
# root manifest must stay in lockstep with its `[patch.crates-io]` table;
# tinybus and tinymemory are unpublished direct paths from OpenHuman's manifest.
#
# `vendor/motosan-ai-oauth` is NOT in this list: OpenHuman vendors it as a
# plain tracked directory rather than a submodule, so it comes along for free
# with the `vendor/openhuman` checkout above. `vendor/tinyjuice` came back: a
# later OpenHuman bump reintroduced both the submodule and `tinyjuice-bus` as
# a dependency, so it is cloned again here (see the root `[patch.crates-io]`
# table for the matching redirect).
#
# `vendor/tinymcp` arrived the same way: OpenHuman extracted its Model Context
# Protocol client into its own repository and now takes a path dependency on
# `vendor/tinymcp/crates/tinymcp`. Because this list is an allowlist rather than
# a recursive init, an OpenHuman bump that adds a path dependency fails here
# *at resolution* — `cargo metadata` cannot read the missing manifest, so every
# lane dies before it compiles a line. When a bump breaks with "failed to load
# source for dependency <name>", the fix is almost always a new entry here.
git -C vendor/openhuman submodule update --init --depth 1 \
  vendor/tinyagents \
  vendor/tinybus \
  vendor/tinychannels \
  vendor/tinycortex \
  vendor/tinyflows \
  vendor/tinyhumans-sdk \
  vendor/tinyjuice \
  vendor/tinymcp \
  vendor/tinymemory \
  vendor/tinyplace

# Deliberately NOT recursive. Three of these vendor crates of their own —
# tinyflows has its own `tinyagents`, tinymemory its own `tinyagents`, `tinybus`
# and `tinycortex` — and none of those nested copies is ever resolved: the root
# patch table redirects each name to the copy beside OpenHuman, so the graph
# holds one of each. Initializing them recursively would clone several hundred
# megabytes that nothing links, and would put two checkouts of one crate on
# disk for anyone reading the tree.

echo "Submodules initialized (OpenHuman core + its ten vendored crates)."
