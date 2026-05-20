#!/usr/bin/env bash
# install.sh — install the forest CLI from a private GitHub release.
#
# The understory-io/forest repository is private, so unauthenticated
# downloads return 404. This script wraps `gh release download` so
# end users can use the GitHub CLI's existing auth (which they need
# for day-to-day repo access anyway) instead of standing up a token
# in $HOMEBREW_GITHUB_API_TOKEN or similar.
#
# ── Prereqs ───────────────────────────────────────────────────────
#   gh CLI installed and authenticated:
#       gh auth login                # one-time
#       gh auth status               # verify
#   The signed-in user must have read access to understory-io/forest.
#
# ── Usage ─────────────────────────────────────────────────────────
#   ./install.sh                     # install latest release
#   ./install.sh v0.2.1              # install a specific tag
#   PREFIX=$HOME/.local ./install.sh # install under PREFIX/bin (default /usr/local)
#
# ── Bootstrap ─────────────────────────────────────────────────────
#   gh release download --repo understory-io/forest --pattern install.sh
#   bash install.sh

set -euo pipefail

REPO="understory-io/forest"
BIN="forest"
PREFIX="${PREFIX:-/usr/local}"
VERSION="${1:-}"

err() { echo "install.sh: $*" >&2; exit 1; }

command -v gh >/dev/null 2>&1 \
  || err "gh CLI not found. Install from https://cli.github.com/ and run 'gh auth login'."

gh auth status >/dev/null 2>&1 \
  || err "gh CLI is not authenticated. Run 'gh auth login' first."

# ── Resolve target tag ────────────────────────────────────────────
# Empty -> latest release on the repo. `gh release view` follows the
# repo's notion of "latest" (not strictly highest semver — release
# marked as latest by GH, which release-please always sets).
if [ -z "$VERSION" ]; then
    VERSION=$(gh release view --repo "$REPO" --json tagName --jq '.tagName') \
        || err "Failed to resolve latest release. Check repo access."
fi

# ── Detect platform ───────────────────────────────────────────────
uname_s=$(uname -s)
uname_m=$(uname -m)

case "$uname_s" in
    Darwin)
        case "$uname_m" in
            arm64|aarch64) target="aarch64-apple-darwin" ;;
            *) err "Unsupported macOS architecture: $uname_m (only Apple silicon ships today)." ;;
        esac
        ;;
    Linux)
        case "$uname_m" in
            x86_64|amd64) target="x86_64-unknown-linux-gnu" ;;
            aarch64|arm64) target="aarch64-unknown-linux-gnu" ;;
            *) err "Unsupported Linux architecture: $uname_m." ;;
        esac
        ;;
    *)
        err "Unsupported OS: $uname_s. forest ships for macOS and Linux only."
        ;;
esac

asset="${BIN}-${VERSION}-${target}.tar.gz"
checksum="${asset}.sha256"

echo "==> Installing $BIN $VERSION ($target) to $PREFIX/bin"

# ── Download tarball + checksum ───────────────────────────────────
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

gh release download "$VERSION" \
    --repo "$REPO" \
    --pattern "$asset" \
    --pattern "$checksum" \
    --dir "$tmp" \
    || err "Failed to download $asset from $REPO. Check the tag exists and you have access."

# ── Verify checksum ───────────────────────────────────────────────
# The sha256 file's path-form was generated server-side relative to
# the working directory at build time, so the check has to run in
# the tmpdir. `-c` cross-checks the listed file against the digest.
( cd "$tmp" && shasum -a 256 -c "$checksum" ) \
    || err "Checksum verification failed for $asset."

# ── Extract + install ─────────────────────────────────────────────
tar -xzf "$tmp/$asset" -C "$tmp"

target_path="$PREFIX/bin/$BIN"
if [ -w "$PREFIX/bin" ]; then
    install -m 0755 "$tmp/$BIN" "$target_path"
else
    # Need sudo to write to /usr/local/bin on most setups.
    echo "==> $PREFIX/bin is not writable; using sudo"
    sudo install -m 0755 "$tmp/$BIN" "$target_path"
fi

echo "==> $BIN $VERSION installed at $target_path"

# Friendly nudge if PREFIX/bin isn't on PATH (common when PREFIX is overridden).
case ":$PATH:" in
    *":$PREFIX/bin:"*) ;;
    *) echo "    note: $PREFIX/bin is not in your PATH" ;;
esac
