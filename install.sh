#!/bin/sh
# Medulla TUI installer.
#
# Downloads the prebuilt `medulla` binary for your platform, verifies its
# SHA-256 against the signed release manifest, and installs it to
# ~/.medulla/bin (override with MEDULLA_HOME). If no prebuilt binary ships for
# your platform, it falls back to building from source with cargo.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/tinyhumansai/medulla/main/install.sh | sh
#   curl -fsSL .../install.sh | sh -s -- <version>   # latest (default) | X.Y.Z
#
# Environment:
#   MEDULLA_HOME       install prefix (default: $HOME/.medulla)
#   MEDULLA_UPDATE_URL override the release manifest URL (testing)
#   MEDULLA_NO_MODIFY_PATH=1  skip editing shell profiles
#
# POSIX sh; no bashisms.
set -eu

# ---- Constants ---------------------------------------------------------------

REPO="tinyhumansai/medulla"
DEFAULT_MANIFEST="https://github.com/${REPO}/releases/latest/download/latest.json"
BIN_NAME="medulla"

# ---- Output helpers ----------------------------------------------------------

if [ -t 2 ] && [ -z "${NO_COLOR:-}" ]; then
    C_RESET="$(printf '\033[0m')"
    C_BOLD="$(printf '\033[1m')"
    C_RED="$(printf '\033[31m')"
    C_GREEN="$(printf '\033[32m')"
    C_YELLOW="$(printf '\033[33m')"
    C_BLUE="$(printf '\033[34m')"
else
    C_RESET="" C_BOLD="" C_RED="" C_GREEN="" C_YELLOW="" C_BLUE=""
fi

info()  { printf '%s\n' "${C_BLUE}==>${C_RESET} $*" >&2; }
ok()    { printf '%s\n' "${C_GREEN}✓${C_RESET} $*" >&2; }
warn()  { printf '%s\n' "${C_YELLOW}warning:${C_RESET} $*" >&2; }
error() { printf '%s\n' "${C_RED}error:${C_RESET} $*" >&2; }
die()   { error "$@"; exit 1; }

# ---- Cleanup -----------------------------------------------------------------

WORKDIR=""
cleanup() { [ -n "$WORKDIR" ] && rm -rf "$WORKDIR"; }
trap cleanup EXIT INT TERM

# ---- Dependency detection ----------------------------------------------------

have() { command -v "$1" >/dev/null 2>&1; }

# Pick an HTTP client once.
if have curl; then
    DL_STDOUT() { curl -fsSL "$1"; }
    DL_FILE()   { curl -fsSL "$1" -o "$2"; }
elif have wget; then
    DL_STDOUT() { wget -qO- "$1"; }
    DL_FILE()   { wget -q "$1" -O "$2"; }
else
    die "need either curl or wget to download Medulla"
fi

have tar || die "need tar to extract the release archive"

# SHA-256 helper — prefer sha256sum, fall back to shasum / openssl.
sha256_of() {
    if have sha256sum; then
        sha256sum "$1" | awk '{print $1}'
    elif have shasum; then
        shasum -a 256 "$1" | awk '{print $1}'
    elif have openssl; then
        openssl dgst -sha256 "$1" | awk '{print $NF}'
    else
        return 1
    fi
}

# ---- Platform detection ------------------------------------------------------
# Keys match the release build matrix's Rust target triples (see
# src/sdk/src/update.rs::platform_key).

detect_target() {
    os="$(uname -s)"
    arch="$(uname -m)"
    case "$os" in
        Linux)  os_part="unknown-linux-gnu" ;;
        Darwin) os_part="apple-darwin" ;;
        *) die "unsupported OS '$os' — build from source: cargo install --path src/tui" ;;
    esac
    case "$arch" in
        x86_64 | amd64) arch_part="x86_64" ;;
        arm64 | aarch64) arch_part="aarch64" ;;
        *) die "unsupported architecture '$arch'" ;;
    esac
    # macOS ships only aarch64; x86_64 hosts run it under Rosetta 2.
    if [ "$os_part" = "apple-darwin" ] && [ "$arch_part" = "x86_64" ]; then
        warn "no native x86_64 macOS build; using the aarch64 binary via Rosetta 2"
        arch_part="aarch64"
    fi
    printf '%s-%s' "$arch_part" "$os_part"
}

# ---- Manifest parsing (jq, or a portable sh/sed fallback) --------------------
# Extract "url" and "sha256" for a given platform key from latest.json.
# Prints "<url> <sha256>" or nothing.

manifest_entry() {
    manifest="$1"
    key="$2"
    if have jq; then
        jq -r --arg k "$key" \
            '.platforms[$k] | if . == null then "" else "\(.url) \(.sha256)" end' \
            "$manifest"
        return
    fi
    # Fallback: flatten to one line, isolate the block for "<key>": { ... },
    # then pull url/sha256. Good enough for the well-formed manifest we emit.
    tr -d '\n' < "$manifest" \
        | sed 's/[[:space:]]\{1,\}/ /g' \
        | grep -o "\"${key}\"[[:space:]]*:[[:space:]]*{[^}]*}" \
        | {
            block="$(cat)"
            [ -n "$block" ] || exit 0
            url="$(printf '%s' "$block" | grep -o '"url"[^,}]*' | head -n1 \
                   | sed 's/.*"url"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/')"
            sha="$(printf '%s' "$block" | grep -o '"sha256"[^,}]*' | head -n1 \
                   | sed 's/.*"sha256"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/')"
            [ -n "$url" ] && printf '%s %s' "$url" "$sha"
        }
}

manifest_version() {
    manifest="$1"
    if have jq; then
        jq -r '.version // ""' "$manifest"
    else
        tr -d '\n' < "$manifest" \
            | grep -o '"version"[[:space:]]*:[[:space:]]*"[^"]*"' | head -n1 \
            | sed 's/.*"\([^"]*\)"$/\1/'
    fi
}

# ---- cargo source-build fallback ---------------------------------------------

build_from_source() {
    warn "falling back to building from source"
    have cargo || die "no prebuilt binary for this platform and cargo is not installed — install Rust from https://rustup.rs then re-run"
    if [ -f "src/tui/Cargo.toml" ]; then
        info "building from the local checkout (cargo install --path src/tui)"
        cargo install --path src/tui --root "$MEDULLA_HOME" --locked
    else
        info "building from git (cargo install --git https://github.com/${REPO})"
        cargo install --git "https://github.com/${REPO}" medulla-tui --root "$MEDULLA_HOME" --locked
    fi
    ok "built and installed medulla to ${BIN_DIR}/${BIN_NAME}"
}

# ---- PATH wiring -------------------------------------------------------------

add_to_path() {
    dir="$1"
    case ":${PATH}:" in
        *":${dir}:"*) return 0 ;;
    esac
    [ "${MEDULLA_NO_MODIFY_PATH:-0}" = "1" ] && return 0

    line="export PATH=\"${dir}:\$PATH\""
    changed=""
    for rc in "$HOME/.zshrc" "$HOME/.bashrc" "$HOME/.bash_profile" "$HOME/.profile"; do
        [ -f "$rc" ] || continue
        if ! grep -qF "$dir" "$rc" 2>/dev/null; then
            printf '\n# Added by the Medulla installer\n%s\n' "$line" >> "$rc"
            changed="${changed} ${rc}"
        fi
    done
    if [ -n "$changed" ]; then
        ok "added ${dir} to PATH in:${changed}"
        PATH_NEEDS_RELOAD=1
    else
        warn "could not update a shell profile automatically; add this line yourself:"
        printf '    %s\n' "$line" >&2
        PATH_NEEDS_RELOAD=1
    fi
}

# ---- Main --------------------------------------------------------------------

main() {
    VERSION="${1:-latest}"
    MEDULLA_HOME="${MEDULLA_HOME:-$HOME/.medulla}"
    BIN_DIR="${MEDULLA_HOME}/bin"
    PATH_NEEDS_RELOAD=0

    target="$(detect_target)"
    info "installing Medulla TUI for ${C_BOLD}${target}${C_RESET}"

    # Resolve the manifest URL for the requested version.
    if [ -n "${MEDULLA_UPDATE_URL:-}" ]; then
        manifest_url="$MEDULLA_UPDATE_URL"
    elif [ "$VERSION" = "latest" ] || [ "$VERSION" = "stable" ]; then
        manifest_url="$DEFAULT_MANIFEST"
    else
        v="${VERSION#v}"
        manifest_url="https://github.com/${REPO}/releases/download/v${v}/latest.json"
    fi

    WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/medulla-install.XXXXXX")"
    manifest="${WORKDIR}/latest.json"

    info "fetching release manifest"
    if ! DL_FILE "$manifest_url" "$manifest"; then
        warn "could not fetch the release manifest ($manifest_url)"
        mkdir -p "$MEDULLA_HOME"
        build_from_source
        finish
        return
    fi

    resolved_version="$(manifest_version "$manifest")"
    [ -n "$resolved_version" ] && info "latest release: ${C_BOLD}v${resolved_version}${C_RESET}"

    entry="$(manifest_entry "$manifest" "$target")"
    if [ -z "$entry" ]; then
        warn "the release ships no prebuilt binary for ${target}"
        mkdir -p "$MEDULLA_HOME"
        build_from_source
        finish
        return
    fi

    asset_url="${entry% *}"
    asset_sha="${entry#* }"

    archive="${WORKDIR}/asset.tar.gz"
    case "$asset_url" in *.zip) archive="${WORKDIR}/asset.zip" ;; esac

    info "downloading $(basename "$asset_url")"
    DL_FILE "$asset_url" "$archive" || die "download failed: $asset_url"

    # Verify the checksum against the manifest.
    if [ -n "$asset_sha" ]; then
        got="$(sha256_of "$archive" || true)"
        if [ -z "$got" ]; then
            warn "no SHA-256 tool available; skipping checksum verification"
        elif [ "$got" != "$asset_sha" ]; then
            die "checksum mismatch: expected $asset_sha, got $got"
        else
            ok "checksum verified"
        fi
    fi

    info "extracting"
    extract="${WORKDIR}/extract"
    mkdir -p "$extract"
    case "$archive" in
        *.zip) have unzip || die "need unzip to extract $archive"; unzip -oq "$archive" -d "$extract" ;;
        *)     tar -xzf "$archive" -C "$extract" ;;
    esac

    # Locate the binary anywhere in the extracted tree.
    binpath="$(find "$extract" -type f -name "$BIN_NAME" -print 2>/dev/null | head -n1)"
    [ -n "$binpath" ] || die "no '${BIN_NAME}' binary found in the downloaded archive"

    mkdir -p "$BIN_DIR"
    install -m 0755 "$binpath" "${BIN_DIR}/${BIN_NAME}" 2>/dev/null \
        || { cp "$binpath" "${BIN_DIR}/${BIN_NAME}" && chmod 0755 "${BIN_DIR}/${BIN_NAME}"; }
    ok "installed ${BIN_NAME} to ${C_BOLD}${BIN_DIR}/${BIN_NAME}${C_RESET}"

    finish
}

finish() {
    add_to_path "$BIN_DIR"

    installed_version="$("${BIN_DIR}/${BIN_NAME}" version 2>/dev/null | head -n1 || true)"

    printf '\n' >&2
    ok "${C_BOLD}Medulla TUI is installed.${C_RESET}"
    [ -n "$installed_version" ] && info "$installed_version"
    printf '\n' >&2
    printf '%s\n' "Next steps:" >&2
    if [ "$PATH_NEEDS_RELOAD" = "1" ]; then
        printf '  %s\n' "1. Reload your shell:   ${C_BOLD}exec \$SHELL${C_RESET}   (or open a new terminal)" >&2
        printf '  %s\n' "2. Log in:              ${C_BOLD}medulla login${C_RESET}" >&2
        printf '  %s\n' "3. Launch the TUI:      ${C_BOLD}medulla${C_RESET}" >&2
    else
        printf '  %s\n' "1. Log in:              ${C_BOLD}medulla login${C_RESET}" >&2
        printf '  %s\n' "2. Launch the TUI:      ${C_BOLD}medulla${C_RESET}" >&2
    fi
    printf '\n' >&2
    info "Without credentials, ${C_BOLD}medulla${C_RESET} runs against the mock runtime so you can look around."
    info "Update anytime with ${C_BOLD}medulla update${C_RESET}."
}

main "$@"
