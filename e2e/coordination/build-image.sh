#!/usr/bin/env bash
# Build (and optionally push) the coordination e2e harness images.
#
#   bash e2e/coordination/build-image.sh            # the harness image
#   bash e2e/coordination/build-image.sh --base     # the tools base image
#
# There are two, and the split is what keeps CI fast:
#
#   base     `Dockerfile.base` — debian + tmux + python3 + node + all three
#            coding CLIs + the primed ACP npm cache. ~600 MB of downloads that
#            do not change when the source does, so it is built when a CLI
#            version moves and pulled the rest of the time. Published as
#            ghcr.io/tinyhumansai/medulla_e2e_base.
#   harness  `Dockerfile` — the release `medulla` binary, the two link examples,
#            and the harness scripts, layered onto that base. This is the one a
#            source change rebuilds.
#
# `run-docker.sh` builds the harness image inline for a one-off local run; this
# script exists for when the build and the run are separate steps — CI, or a
# shared image other checkouts pull.
#
# Env:
#   IMAGE=<repo>        image name without tag (default depends on --base)
#   TAGS="a b c"        tags to apply (default: latest, plus the version tag
#                       for --base)
#   BASE_IMAGE=<ref>    harness only: the base to build against, overriding the
#                       pinned default in `Dockerfile`
#   PLATFORM=<p>        target platform (default: the host's)
#   PUSH=1              push after building (requires a prior `docker login`)
#   NO_CACHE=1          build with --no-cache
#   CACHE_FROM/CACHE_TO BuildKit cache specs passed straight through
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SDK_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

BASE=0
[ "${1:-}" = "--base" ] && BASE=1

log() { printf '[build-image] %s\n' "$*" >&2; }

# The version tag for the base image, derived from the CLI versions its
# Dockerfile pins. Read out of the file rather than restated here, so the tag and
# the contents cannot disagree.
base_version_tag() {
  "${PYTHON_BIN:-python3}" - "$SCRIPT_DIR/Dockerfile.base" <<'PY'
import re, sys
text = open(sys.argv[1], encoding="utf-8").read()


def arg(name):
    match = re.search(rf"^ARG {name}=(\S+)", text, re.M)
    if not match:
        raise SystemExit(f"Dockerfile.base has no ARG {name}")
    return match.group(1)


# Node moves for reasons unrelated to the harness, so only its major is in the
# tag: a patch bump should not orphan an otherwise identical base.
print(
    "oc{}-cc{}-cx{}-node{}".format(
        arg("OPENCODE_VERSION"),
        arg("CLAUDE_VERSION"),
        arg("CODEX_VERSION"),
        arg("NODE_VERSION").split(".")[0],
    )
)
PY
}

if [ "$BASE" = "1" ]; then
  IMAGE="${IMAGE:-ghcr.io/tinyhumansai/medulla_e2e_base}"
  TAGS="${TAGS:-latest $(base_version_tag)}"
  DOCKERFILE="$SCRIPT_DIR/Dockerfile.base"
  # The base needs no source: its whole content is fetched, so the context is a
  # directory holding only the Dockerfile.
  CONTEXT="$SCRIPT_DIR"
else
  IMAGE="${IMAGE:-medulla-e2e}"
  TAGS="${TAGS:-latest}"
  DOCKERFILE="$SCRIPT_DIR/Dockerfile"
  CONTEXT="$SDK_DIR"
fi

# Native arch by default. Never force amd64 emulation: the Rust stage and the
# CLIs would run under qemu, which is slow enough to look like a hang.
PLATFORM="${PLATFORM:-linux/$(uname -m | sed 's/aarch64/arm64/;s/x86_64/amd64/')}"

args=(buildx build --platform "$PLATFORM" -f "$DOCKERFILE")
for tag in $TAGS; do
  args+=(-t "$IMAGE:$tag")
done
[ "$BASE" = "0" ] && [ -n "${BASE_IMAGE:-}" ] && args+=(--build-arg "BASE_IMAGE=$BASE_IMAGE")
[ "${NO_CACHE:-0}" = "1" ] && args+=(--no-cache)
[ -n "${CACHE_FROM:-}" ] && args+=(--cache-from "$CACHE_FROM")
[ -n "${CACHE_TO:-}" ] && args+=(--cache-to "$CACHE_TO")
# Provenance attestations turn a single-platform build into a manifest list,
# which `docker run` on the same host then refuses to load.
args+=(--provenance=false)
if [ "${PUSH:-0}" = "1" ]; then
  args+=(--push)
else
  args+=(--load)
fi
args+=("$CONTEXT")

log "building $IMAGE ($TAGS) for $PLATFORM${PUSH:+ and pushing}…"
docker "${args[@]}" >&2

if [ "$BASE" = "1" ]; then
  log "done. Point the harness at it with:"
  log "  BASE_IMAGE=$IMAGE:${TAGS%% *} bash e2e/coordination/build-image.sh"
  log "…or update the BASE_IMAGE default in e2e/coordination/Dockerfile."
else
  log "done. Run a leg with:"
  for harness in opencode claude codex; do
    log "  docker run --rm --network none -e E2E_HARNESS=$harness $IMAGE:${TAGS%% *} bash /app/e2e/coordination/run.sh"
  done
fi
