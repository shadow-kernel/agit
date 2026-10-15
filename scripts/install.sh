#!/bin/sh
# agit installer.
#
# Downloads a release tarball from https://github.com/shadow-kernel/agit/releases,
# verifies its SHA-256 against the published .sha256 file and installs the
# `agit` and `agit-server` binaries.
#
#   curl -fsSL https://raw.githubusercontent.com/shadow-kernel/agit/main/scripts/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/shadow-kernel/agit/main/scripts/install.sh | sh -s -- --version v0.1.0
#   curl -fsSL https://raw.githubusercontent.com/shadow-kernel/agit/main/scripts/install.sh | sudo sh -s -- --prefix /usr/local
#
# Options (each also has an environment variable for curl | sh use):
#   --version <tag|X.Y.Z>  AGIT_VERSION   release to install (default: latest)
#   --prefix <dir>         AGIT_PREFIX    install root; binaries go to <dir>/bin
#                                         (default: ~/.local, or /usr/local when root)
#   --bin-dir <dir>        AGIT_BIN_DIR   exact directory for the binaries (overrides --prefix)
#   --dry-run                             print what would happen; download nothing
#   --no-verify                           skip checksum verification (not recommended)
#   -h, --help
#
# Supported targets: macOS arm64 and x86_64, Linux x86_64 and aarch64 (glibc).
# Windows users: download the .zip from the releases page.

set -eu

REPO="${AGIT_REPO:-shadow-kernel/agit}"
BASE_URL="https://github.com/${REPO}/releases"
VERSION="${AGIT_VERSION:-}"
PREFIX="${AGIT_PREFIX:-}"
BIN_DIR="${AGIT_BIN_DIR:-}"
DRY_RUN=0
VERIFY=1
BINARIES="agit agit-server"

usage() {
    cat <<'USAGE'
agit installer: downloads a release from https://github.com/shadow-kernel/agit/releases,
verifies its SHA-256 and installs `agit` and `agit-server`.

  curl -fsSL https://raw.githubusercontent.com/shadow-kernel/agit/main/scripts/install.sh | sh
  curl -fsSL https://raw.githubusercontent.com/shadow-kernel/agit/main/scripts/install.sh | sh -s -- --version v0.1.0

Options (each also has an environment variable for curl | sh use):
  --version <tag|X.Y.Z>  AGIT_VERSION   release to install (default: latest)
  --prefix <dir>         AGIT_PREFIX    install root; binaries go to <dir>/bin
                                        (default: ~/.local, or /usr/local when root)
  --bin-dir <dir>        AGIT_BIN_DIR   exact directory for the binaries (overrides --prefix)
  --dry-run                             print what would happen; download nothing
  --no-verify                           skip checksum verification (not recommended)
  -h, --help

Supported: macOS arm64/x86_64, Linux x86_64/aarch64 (glibc). Windows: download the .zip.
USAGE
}

say()  { printf 'agit-install: %s\n' "$*" >&2; }
die()  { printf 'agit-install: error: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --version)   [ $# -ge 2 ] || die "--version needs a value"; VERSION="$2"; shift 2 ;;
        --version=*) VERSION="${1#*=}"; shift ;;
        --prefix)    [ $# -ge 2 ] || die "--prefix needs a value"; PREFIX="$2"; shift 2 ;;
        --prefix=*)  PREFIX="${1#*=}"; shift ;;
        --bin-dir)   [ $# -ge 2 ] || die "--bin-dir needs a value"; BIN_DIR="$2"; shift 2 ;;
        --bin-dir=*) BIN_DIR="${1#*=}"; shift ;;
        --dry-run)   DRY_RUN=1; shift ;;
        --no-verify) VERIFY=0; shift ;;
        -h|--help)   usage; exit 0 ;;
        *)           die "unknown option: $1 (try --help)" ;;
    esac
done

# ---------------------------------------------------------------------------
# Target detection
# ---------------------------------------------------------------------------
detect_target() {
    os=$(uname -s 2>/dev/null || echo unknown)
    arch=$(uname -m 2>/dev/null || echo unknown)
    case "$arch" in
        x86_64|amd64)  arch=x86_64 ;;
        arm64|aarch64) arch=aarch64 ;;
        *) die "unsupported architecture: $arch" ;;
    esac
    case "$os" in
        Darwin)
            # x86_64 reported under Rosetta: prefer the native arm64 build.
            if [ "$arch" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ]; then
                arch=aarch64
            fi
            TARGET="${arch}-apple-darwin"
            ;;
        Linux)
            if have ldd && ldd --version 2>&1 | grep -qi musl; then
                say "warning: musl libc detected; the release binaries are built against glibc and may not run here"
            fi
            TARGET="${arch}-unknown-linux-gnu"
            ;;
        MINGW*|MSYS*|CYGWIN*|Windows_NT)
            die "Windows is not supported by this script; download agit-<version>-x86_64-pc-windows-msvc.zip from ${BASE_URL}"
            ;;
        *) die "unsupported operating system: $os" ;;
    esac
}

# ---------------------------------------------------------------------------
# HTTP helpers (curl preferred, wget fallback)
# ---------------------------------------------------------------------------
fetch() { # fetch <url> <out-file>
    if have curl; then
        curl -fsSL --retry 3 --retry-delay 1 -o "$2" "$1"
    elif have wget; then
        wget -q -O "$2" "$1"
    else
        die "need curl or wget"
    fi
}

resolve_latest() {
    # GitHub redirects /releases/latest to /releases/tag/<tag>; read the final URL.
    final=""
    if have curl; then
        final=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "${BASE_URL}/latest" 2>/dev/null || true)
    elif have wget; then
        final=$(wget -q --max-redirect=0 -S -O /dev/null "${BASE_URL}/latest" 2>&1 | sed -n 's/^ *[Ll]ocation: *//p' | tail -1 | tr -d '\r')
    fi
    case "$final" in
        */tag/*) printf '%s\n' "${final##*/tag/}" ;;
        *) return 1 ;;
    esac
}

sha256_of() { # sha256_of <file> -> hex
    if have sha256sum; then
        sha256sum "$1" | cut -d' ' -f1
    elif have shasum; then
        shasum -a 256 "$1" | cut -d' ' -f1
    elif have openssl; then
        openssl dgst -sha256 "$1" | sed 's/^.* //'
    else
        return 1
    fi
}

# ---------------------------------------------------------------------------
# Resolve version, destination
# ---------------------------------------------------------------------------
detect_target

if [ -z "$VERSION" ] || [ "$VERSION" = latest ]; then
    if tag=$(resolve_latest); then
        VERSION="$tag"
    elif [ "$DRY_RUN" = 1 ]; then
        say "warning: could not resolve the latest release (offline?); showing a placeholder"
        VERSION="v<latest>"
    else
        die "could not resolve the latest release from ${BASE_URL}/latest; pass --version"
    fi
fi
case "$VERSION" in v*) TAG="$VERSION" ;; *) TAG="v$VERSION" ;; esac
VER="${TAG#v}"

if [ -z "$BIN_DIR" ]; then
    if [ -z "$PREFIX" ]; then
        if [ "$(id -u)" = 0 ]; then PREFIX=/usr/local; else PREFIX="${HOME}/.local"; fi
    fi
    BIN_DIR="${PREFIX%/}/bin"
fi

ASSET="agit-${VER}-${TARGET}.tar.gz"
ASSET_URL="${BASE_URL}/download/${TAG}/${ASSET}"
SUM_URL="${ASSET_URL}.sha256"

say "release:  ${TAG}"
say "target:   ${TARGET}"
say "tarball:  ${ASSET_URL}"
say "checksum: ${SUM_URL}"
say "install:  ${BIN_DIR}/{agit,agit-server}"

if [ "$DRY_RUN" = 1 ]; then
    say "dry run: nothing downloaded or installed"
    exit 0
fi

# ---------------------------------------------------------------------------
# Download, verify, extract
# ---------------------------------------------------------------------------
TMP=$(mktemp -d 2>/dev/null || mktemp -d -t agit-install)
trap 'rm -rf "$TMP"' EXIT INT TERM

say "downloading ${ASSET}"
fetch "$ASSET_URL" "$TMP/$ASSET" || die "download failed: $ASSET_URL (does release $TAG have a $TARGET build?)"

if [ "$VERIFY" = 1 ]; then
    fetch "$SUM_URL" "$TMP/$ASSET.sha256" || die "checksum download failed: $SUM_URL (use --no-verify to skip)"
    expected=$(awk 'NR==1 {print $1}' "$TMP/$ASSET.sha256" | tr 'A-F' 'a-f')
    actual=$(sha256_of "$TMP/$ASSET") || die "no sha256sum, shasum or openssl available to verify the download"
    [ -n "$expected" ] || die "empty checksum file"
    if [ "$expected" != "$actual" ]; then
        die "checksum mismatch for $ASSET: expected $expected, got $actual"
    fi
    say "checksum ok"
else
    say "warning: checksum verification skipped"
fi

mkdir -p "$TMP/x"
tar -xzf "$TMP/$ASSET" -C "$TMP/x"

# ---------------------------------------------------------------------------
# Install
# ---------------------------------------------------------------------------
if ! mkdir -p "$BIN_DIR" 2>/dev/null || [ ! -w "$BIN_DIR" ]; then
    die "cannot write to $BIN_DIR; re-run with sudo, or choose --prefix \$HOME/.local"
fi

for b in $BINARIES; do
    src=$(find "$TMP/x" -type f -name "$b" | head -1)
    [ -n "$src" ] || die "$b not found in $ASSET"
    install -m 0755 "$src" "$BIN_DIR/$b"
    say "installed $BIN_DIR/$b"
done

if "$BIN_DIR/agit" --version >/dev/null 2>&1; then
    say "$("$BIN_DIR/agit" --version)"
else
    say "warning: $BIN_DIR/agit --version failed; the binary may not match this platform"
fi

# ---------------------------------------------------------------------------
# PATH advice
# ---------------------------------------------------------------------------
case ":${PATH}:" in
    *":${BIN_DIR}:"*) ;;
    *)
        shell_name=$(basename "${SHELL:-sh}")
        case "$shell_name" in
            zsh)  rc="$HOME/.zshrc";  line="export PATH=\"${BIN_DIR}:\$PATH\"" ;;
            bash) rc="$HOME/.bashrc"; line="export PATH=\"${BIN_DIR}:\$PATH\"" ;;
            fish) rc="$HOME/.config/fish/config.fish"; line="fish_add_path ${BIN_DIR}" ;;
            *)    rc="your shell profile"; line="export PATH=\"${BIN_DIR}:\$PATH\"" ;;
        esac
        say ""
        say "${BIN_DIR} is not on your PATH. Add it to ${rc}:"
        say "    ${line}"
        say "then open a new shell."
        ;;
esac

say "done. Next: agit --help, agit-server --help, https://github.com/${REPO}/blob/main/docs/DEPLOYMENT.md"
