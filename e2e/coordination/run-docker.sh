#!/usr/bin/env bash
# Build the coordination e2e image and run the harness (run.sh) fully inside a
# Linux container. Exits 0 iff run.sh prints PASS.
#
#   bash e2e/coordination/run-docker.sh
#
# Env passthrough:
#   E2E_KEEP=1    keep the container after exit (inspect with docker logs) and,
#                 inside the container, keep run.sh's run dir + tmux session
#   E2E_SMOKE=0   skip the interactive opencode TUI smoke leg
#   IMAGE=<tag>   override image tag (default: medulla-e2e)
#   NO_CACHE=1    build with --no-cache
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SDK_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
IMAGE="${IMAGE:-medulla-e2e}"
CONTAINER="medulla-e2e-run-$$"

log() { printf '[run-docker] %s\n' "$*" >&2; }

# Native arch build — host is Apple Silicon, so this is linux/arm64. Never force
# amd64 emulation (opencode + rust builds would run under qemu = slow/flaky).
PLATFORM="linux/$(uname -m | sed 's/aarch64/arm64/;s/x86_64/amd64/')"

log "building image '$IMAGE' for $PLATFORM (this takes a few minutes cold)…"
build_args=(build --platform "$PLATFORM" -t "$IMAGE" -f "$SCRIPT_DIR/Dockerfile")
[ "${NO_CACHE:-0}" = "1" ] && build_args+=(--no-cache)
build_args+=("$SDK_DIR")
docker "${build_args[@]}" >&2

log "running harness in container '$CONTAINER'…"
run_args=(run --name "$CONTAINER" --platform "$PLATFORM")
# tmux needs a writable /tmp and enough shared memory; defaults are fine.
run_args+=(-e "E2E_KEEP=${E2E_KEEP:-0}")
[ -n "${E2E_SMOKE:-}" ] && run_args+=(-e "E2E_SMOKE=$E2E_SMOKE")
run_args+=("$IMAGE")

rc=0
docker "${run_args[@]}" || rc=$?

if [ "$rc" -eq 0 ]; then
  log "PASS (container exited 0)"
  if [ "${E2E_KEEP:-0}" = "1" ]; then
    log "E2E_KEEP=1 — container '$CONTAINER' left in place; inspect: docker logs $CONTAINER"
  else
    docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
  fi
else
  log "FAIL (container exited $rc)"
  log "full logs:   docker logs $CONTAINER"
  if [ "${E2E_KEEP:-0}" = "1" ]; then
    log "container '$CONTAINER' kept (E2E_KEEP=1). Re-run with a shell:"
    log "  docker run --rm -it --entrypoint bash $IMAGE"
  else
    log "removing container (set E2E_KEEP=1 to keep it for debugging)"
    docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
  fi
fi

exit "$rc"
