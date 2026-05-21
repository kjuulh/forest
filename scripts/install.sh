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
# the tmpdir. Pick whichever tool is on the host: `sha256sum` ships
# in coreutils on Linux, `shasum -a 256` ships with Perl on macOS
# (and most BSDs). Both produce identical `<hex>  <file>` output.
verify_sha256() {
    local sumfile="$1"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum -c "$sumfile"
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 -c "$sumfile"
    else
        err "Neither sha256sum nor shasum is on \$PATH; cannot verify download."
    fi
}
( cd "$tmp" && verify_sha256 "$checksum" ) \
    || err "Checksum verification failed for $asset."

# ── Extract + install ─────────────────────────────────────────────
tar -xzf "$tmp/$asset" -C "$tmp"

target_path="$PREFIX/bin/$BIN"
mkdir -p "$PREFIX/bin" 2>/dev/null || true
if [ -w "$PREFIX/bin" ]; then
    install -m 0755 "$tmp/$BIN" "$target_path"
else
    # /usr/local/bin (or any system path) needs sudo. When this script
    # is being piped from curl, stdin is the pipe — `sudo` would have
    # nowhere to read the password from. Reattach to /dev/tty if it
    # exists, otherwise fail with a clear message.
    echo "==> $PREFIX/bin is not writable; trying sudo"
    if [ -e /dev/tty ]; then
        sudo install -m 0755 "$tmp/$BIN" "$target_path" </dev/tty
    else
        err "Need sudo for $PREFIX/bin but no /dev/tty available. Re-run with PREFIX=\$HOME/.local (or any writable prefix), or download + run the script directly instead of piping."
    fi
fi

echo "==> $BIN $VERSION installed at $target_path"

# Friendly nudge if PREFIX/bin isn't on PATH (common when PREFIX is overridden).
# Suggest the concrete shell-rc edit so the user can act without guessing.
case ":$PATH:" in
    *":$PREFIX/bin:"*) ;;
    *)
        rc="your shell config"
        case "${SHELL##*/}" in
            bash) rc="~/.bashrc" ;;
            zsh)  rc="~/.zshrc"  ;;
            fish) rc="~/.config/fish/config.fish" ;;
        esac
        echo "    note: $PREFIX/bin is not in your PATH"
        if [ "${SHELL##*/}" = "fish" ]; then
            echo "    add to $rc:  fish_add_path $PREFIX/bin"
        else
            echo "    add to $rc:  export PATH=\"$PREFIX/bin:\$PATH\""
        fi
        ;;
esac

# ── Optional first-run context provisioning ───────────────────────
#
# If the operator sets FOREST_PROFILE=name=...,server=... in the
# environment, seed a context for the freshly-installed binary so the
# user doesn't have to run `forest context create` manually after
# install. If no context exists yet, the provisioned one becomes the
# active default; if some do, it is added without changing the
# active. Idempotent — re-running the installer with the same
# FOREST_PROFILE just updates the server URL.
#
# Renamed from FOREST_CONTEXT to avoid colliding with forest's
# runtime `FOREST_CONTEXT=<name>` env which selects WHICH context to
# use per-invocation.
if [ -n "${FOREST_PROFILE:-}" ]; then
    profile_name=""
    profile_server=""
    profile_web=""
    # Parse comma-separated key=value pairs. Tolerant: ignores empty
    # segments, unknown keys, and whitespace around `=`.
    IFS=','
    for pair in $FOREST_PROFILE; do
        unset IFS
        key="${pair%%=*}"
        val="${pair#*=}"
        # Trim whitespace from BOTH key and value. Users routinely paste
        # FOREST_PROFILE from docs / wikis with stray spaces around `=`
        # or `,`, and a server URL with a leading space would later
        # fail as an invalid URL with a confusing error.
        trim() { echo "$1" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//'; }
        key="$(trim "$key")"
        val="$(trim "$val")"
        case "$key" in
            name) profile_name="$val" ;;
            server) profile_server="$val" ;;
            web) profile_web="$val" ;;
            "") ;; # empty segment from trailing comma
            *) echo "==> FOREST_PROFILE: ignoring unknown key '$key'" >&2 ;;
        esac
        IFS=','
    done
    unset IFS

    if [ -z "$profile_name" ] || [ -z "$profile_server" ]; then
        echo "==> FOREST_PROFILE was set but missing name= or server=; skipping context provision." >&2
    else
        echo "==> Provisioning context '$profile_name' → $profile_server"
        provision_args="context provision --name $profile_name --server $profile_server"
        if [ -n "$profile_web" ]; then
            provision_args="$provision_args --web-url $profile_web"
        fi
        # shellcheck disable=SC2086
        if ! "$target_path" $provision_args; then
            echo "==> Context provision failed. The installed forest binary may be" >&2
            echo "    older than the version that introduced 'context provision'." >&2
            echo "    Try: $target_path self update, then re-run this installer." >&2
        fi
    fi
fi

# ── Next steps banner ────────────────────────────────────────────
#
# After a successful install the user is dropped at a prompt with no
# breadcrumbs. Print a short hint at the end so they know what to try
# next. This is the LAST thing the user sees from install.sh, so it
# should be brief and actionable.
echo
echo "Next:"
if [ -n "${FOREST_PROFILE:-}" ] && [ -n "${profile_name:-}" ]; then
    echo "  forest context active        # confirm '$profile_name' is the active context"
    echo "  forest init                  # scaffold a new project"
else
    echo "  forest context create <name> --server <url>   # point forest at a server"
    echo "  forest context active                         # see active context"
    echo "  forest init                                   # scaffold a new project"
fi
