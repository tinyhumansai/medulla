# Dockerized coordination e2e harness.
#
# Build context MUST be the sdk/ crate root:
#   docker build -f e2e/coordination/Dockerfile -t medulla-e2e .
# (run-docker.sh does this for you.)
#
# Stage 1 bakes the release `medulla` binary plus the two examples the harness
# drives. Stage 2 is a slim Debian runtime with tmux + python3 + the real
# opencode CLI, and it hands run.sh prebuilt binaries via the *_BIN env vars so
# nothing is recompiled inside the container.

# ---------------------------------------------------------------------------
# Stage 1: compile medulla + examples (release)
# ---------------------------------------------------------------------------
# Debian (glibc) base so the resulting binaries match the glibc runtime stage.
FROM rust:1.96-bookworm AS build

WORKDIR /build

# The workspace has path deps: vendor/tinyplace/sdk/rust, vendor/tinycortex,
# and tinycortex's own vendor/tinyagents. src/ carries both member crates
# (src/sdk, src/tui) including their tests/ and examples/ trees — the examples
# pull src/sdk/tests/support/ via #[path], so it must be present.
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
    --example mock_signal_server \
    --example coordination_owner

# ---------------------------------------------------------------------------
# Stage 2: runtime — tmux + python3 + opencode + baked binaries
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim

# Pinned opencode CLI version (sst/opencode, published under anomalyco/opencode).
# glibc tarball to match the debian runtime. BuildKit sets TARGETARCH only when
# --platform is passed; otherwise fall back to the native dpkg architecture
# (never default it — a wrong-arch opencode binary is an Exec format error).
ARG OPENCODE_VERSION=1.17.18
ARG TARGETARCH

RUN set -eux; \
    apt-get update; \
    DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        tmux python3 curl bash ca-certificates git; \
    rm -rf /var/lib/apt/lists/*; \
    ARCH="${TARGETARCH:-$(dpkg --print-architecture)}"; \
    case "$ARCH" in \
        arm64) OC_ARCH=arm64 ;; \
        amd64) OC_ARCH=x64 ;; \
        *) echo "unsupported arch=$ARCH" >&2; exit 1 ;; \
    esac; \
    curl -fsSL --http1.1 --retry 5 --retry-delay 3 --retry-all-errors \
        -C - -o /tmp/opencode.tar.gz \
        "https://github.com/anomalyco/opencode/releases/download/v${OPENCODE_VERSION}/opencode-linux-${OC_ARCH}.tar.gz"; \
    tar -xzf /tmp/opencode.tar.gz -C /usr/local/bin; \
    rm /tmp/opencode.tar.gz; \
    chmod +x /usr/local/bin/opencode; \
    /usr/local/bin/opencode --version

# Prebuilt binaries from the build stage, onto PATH.
COPY --from=build /build/target/release/medulla                      /usr/local/bin/medulla
COPY --from=build /build/target/release/examples/mock_signal_server  /usr/local/bin/mock_signal_server
COPY --from=build /build/target/release/examples/coordination_owner  /usr/local/bin/coordination_owner

# The harness scripts (run.sh, mock_llm.py, opencode.json).
WORKDIR /app
COPY e2e/coordination /app/e2e/coordination

# Hand run.sh its prebuilt binaries so it never shells out to cargo.
ENV MEDULLA_BIN=/usr/local/bin/medulla \
    MOCK_SIGNAL_BIN=/usr/local/bin/mock_signal_server \
    OWNER_BIN=/usr/local/bin/coordination_owner \
    OPENCODE_BIN=/usr/local/bin/opencode \
    OPENCODE_DISABLE_AUTOUPDATE=1 \
    TERM=xterm-256color

CMD ["bash", "/app/e2e/coordination/run.sh"]
