# Dockerized coordination e2e harness.
#
# Build context MUST be the sdk/ crate root:
#   docker build -f e2e/coordination/Dockerfile -t medulla-e2e .
# (run-docker.sh does this for you.)
#
# Two stages and a prebuilt base. Stage 1 compiles the release `medulla` binary
# plus the two examples the harness drives. Stage 2 layers those and the harness
# scripts onto `Dockerfile.base` — the published tools image carrying tmux,
# python3, node and all three real coding CLIs — so nothing is recompiled or
# re-downloaded inside the container.
#
# The CLIs live in the base rather than here because they are ~600 MB that does
# not change when the source does: a CI run with a cold layer cache used to
# re-fetch every one of them before it could compile a line of Rust. Now it pulls
# a pinned image instead. See `Dockerfile.base`.
#
# One image serves all three harness legs; pick one at run time:
#   docker run --rm --network none -e E2E_HARNESS=claude medulla-e2e

# The published tools image stage 2 builds on. Declared here, before the first
# `FROM`, because that is the only scope a `FROM` line can read an ARG from.
#
# Pinned to a version tag rather than `latest`, for the same reason every CLI
# inside it is pinned: a base that moves under a green branch is a harness that
# tests something other than what it last reported. Point this at a new tag when
# a CLI version is bumped.
#
# Building against a locally-built base instead (no GHCR access, or trying a CLI
# bump before publishing):
#   bash e2e/coordination/build-image.sh --base
#   BASE_IMAGE=ghcr.io/tinyhumansai/medulla_e2e_base:latest \
#     bash e2e/coordination/build-image.sh
ARG BASE_IMAGE=ghcr.io/tinyhumansai/medulla_e2e_base:oc1.17.18-cc2.1.226-cx0.147.0-node22

# ---------------------------------------------------------------------------
# Stage 1: compile medulla + examples (release)
# ---------------------------------------------------------------------------
# Debian (glibc) base so the resulting binaries match the glibc runtime stage.
FROM rust:1.96.1-bookworm AS build

WORKDIR /build

# System deps for the vendored OpenHuman core's native builds. Needed only when
# the `openhuman-core` feature is on, but installed unconditionally so enabling
# it never turns into a confusing link failure here.
#   - libssl-dev + pkg-config: openssl-sys links the SYSTEM libssl
#     (-lssl -lcrypto), reached via native-tls <- hyper-tls <- reqwest, which an
#     OpenHuman dependency turns on. It is NOT vendored.
#   - X11 set: `rdev`, `enigo` and `arboard` are NON-optional OpenHuman
#     dependencies (input events, synthetic input, clipboard), so they are in
#     the graph even on a headless build and their build scripts fail without
#     these. Not needed by medulla itself.
#   - libgit2 needs nothing: libgit2-sys builds its own C (libgit2_vendored)
#     using cc, which the rust image already provides.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      pkg-config libssl-dev libx11-dev libxi-dev libxtst-dev libxdo-dev \
 && rm -rf /var/lib/apt/lists/*

# The workspace's vendored path deps all live under vendor/openhuman/vendor/*
# now (see docs/vendoring.md). src/ carries every member crate (src/sdk,
# src/tui, src/link) including their tests/ and examples/ trees.
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY vendor ./vendor

# One shot; path-dep crates make the dummy-main cache trick brittle, so just
# build. Cold compile is ~25 min. A BuildKit cache mount keeps the downloaded
# crate registry across rebuilds so only compilation reruns, not the fetch.
# (target/ is NOT cache-mounted — the runtime stage COPYs binaries out of it.)
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --release \
    --bin medulla \
    --example mock_link_forwarder \
    --example coordination_owner

# ---------------------------------------------------------------------------
# Stage 2: runtime — the published tools base + this build's binaries
# ---------------------------------------------------------------------------
FROM ${BASE_IMAGE}

# Prebuilt binaries from the build stage, onto PATH.
COPY --from=build /build/target/release/medulla                      /usr/local/bin/medulla
COPY --from=build /build/target/release/examples/mock_link_forwarder /usr/local/bin/mock_link_forwarder
COPY --from=build /build/target/release/examples/coordination_owner  /usr/local/bin/coordination_owner

# The harness scripts (run.sh, mock_llm.py, the per-harness configs, fixtures).
WORKDIR /app
COPY e2e/coordination /app/e2e/coordination

# Hand run.sh its prebuilt binaries so it never shells out to cargo. The coding
# CLIs' own `*_BIN` variables come from the base image. `E2E_HARNESS` chooses
# which of them a run actually drives.
ENV MEDULLA_BIN=/usr/local/bin/medulla \
    FORWARDER_BIN=/usr/local/bin/mock_link_forwarder \
    OWNER_BIN=/usr/local/bin/coordination_owner \
    E2E_HARNESS=opencode

# Last, so every COPY above still ran as root. See `Dockerfile.base` for why the
# harness must not run as root.
USER e2e
ENV HOME=/home/e2e

CMD ["bash", "/app/e2e/coordination/run.sh"]
