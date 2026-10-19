#!/usr/bin/env bash

set -euo pipefail

REPO="understory-io/forest"
BIN="forest"
VERSION="${1:-}"

default_prefix() {
    if [ "$(id -u 2>/dev/null)" = "0" ] || [ -z "${HOME:-}" ]; then
        echo "/usr/local"
    else
        echo "$HOME/.local"
    fi
}
PREFIX="${PREFIX:-$(default_prefix)}"

err() { echo "install.sh: $*" >&2; exit 1; }

command -v gh >/dev/null 2>&1 \
  || err "gh CLI not found. Install from https://cli.github.com/ and run 'gh auth login'."

gh auth status >/dev/null 2>&1 \
  || err "gh CLI is not authenticated. Run 'gh auth login' first."

if [ -z "$VERSION" ]; then
    VERSION=$(gh release view --repo "$REPO" --json tagName --jq '.tagName') \
        || err "Failed to resolve latest release. Check repo access."
fi

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

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

gh release download "$VERSION" \
    --repo "$REPO" \
    --pattern "$asset" \
    --pattern "$checksum" \
    --dir "$tmp" \
    || err "Failed to download $asset from $REPO. Check the tag exists and you have access."

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

tar -xzf "$tmp/$asset" -C "$tmp"

target_path="$PREFIX/bin/$BIN"
mkdir -p "$PREFIX/bin" 2>/dev/null || true
if [ -w "$PREFIX/bin" ]; then
    install -m 0755 "$tmp/$BIN" "$target_path"
else
    echo "==> $PREFIX/bin is not writable; trying sudo"
    if [ -e /dev/tty ]; then
        sudo install -m 0755 "$tmp/$BIN" "$target_path" </dev/tty
    else
        err "Need sudo for $PREFIX/bin but no /dev/tty available. Re-run with PREFIX=\$HOME/.local (or any writable prefix), or download + run the script directly instead of piping."
    fi
fi

echo "==> $BIN $VERSION installed at $target_path"

case ":$PATH:" in
    *":$PREFIX/bin:"*)
        :
        ;;
    *)
        path_added=
        if [ -z "${FOREST_NO_SHELL_INTEGRATION:-}" ]; then
            case "${SHELL##*/}" in
                zsh)  path_rc="$HOME/.zshrc";  path_rc_display="~/.zshrc"  ;;
                bash) path_rc="$HOME/.bashrc"; path_rc_display="~/.bashrc" ;;
                *)    path_rc="";              path_rc_display=""          ;;
            esac

            if [ -n "$path_rc" ]; then
                path_marker="# forest install: prepend $PREFIX/bin to PATH"
                if [ -f "$path_rc" ] && grep -qF "$path_marker" "$path_rc"; then
                    echo "==> $PREFIX/bin PATH entry already present in $path_rc_display"
                    path_added=1
                else
                    path_block="
$path_marker (added by install.sh)
case \":\$PATH:\" in
    *\":$PREFIX/bin:\"*) ;;
    *) export PATH=\"$PREFIX/bin:\$PATH\" ;;
esac"
                    if printf '%s\n' "$path_block" >> "$path_rc" 2>/dev/null; then
                        echo "==> added $PREFIX/bin to PATH in $path_rc_display"
                        echo "    run 'source $path_rc_display' or open a new shell to activate"
                        path_added=1
                    fi
                fi
            fi
        fi

        if [ -z "${path_added:-}" ]; then
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
        fi
        ;;
esac

if [ -z "${FOREST_NO_SHELL_INTEGRATION:-}" ]; then
    case "${SHELL##*/}" in
        zsh)  rc_path="$HOME/.zshrc";  rc_display="~/.zshrc";  forest_shell_arg="zsh"  ;;
        bash) rc_path="$HOME/.bashrc"; rc_display="~/.bashrc"; forest_shell_arg="bash" ;;
        *)    rc_path="";              rc_display="";          forest_shell_arg=""     ;;
    esac

    if [ -n "$rc_path" ]; then
        if [ -f "$rc_path" ] && grep -qF 'forest shell' "$rc_path"; then
            echo "==> forest shell integration already present in $rc_display"
        else
            integration_block="
command -v forest >/dev/null 2>&1 && eval \"\$(forest shell $forest_shell_arg)\""

            if printf '%s\n' "$integration_block" >> "$rc_path" 2>/dev/null; then
                echo "==> added forest shell integration to $rc_display"
                echo "    run 'source $rc_display' or open a new shell to activate"
            else
                echo "==> could not write to $rc_path; add this to your rc file:"
                echo "    command -v forest >/dev/null 2>&1 && eval \"\$(forest shell $forest_shell_arg)\""
            fi
        fi
    else
        echo "    note: shell integration is auto-installed for zsh and bash only"
        echo "          (detected shell: ${SHELL##*/}); skip with FOREST_NO_SHELL_INTEGRATION=1"
    fi
fi

if [ -n "${FOREST_PROFILE:-}" ]; then
    profile_name=""
    profile_server=""
    profile_web=""
    IFS=','
    for pair in $FOREST_PROFILE; do
        unset IFS
        key="${pair%%=*}"
        val="${pair#*=}"
        trim() { echo "$1" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//'; }
        key="$(trim "$key")"
        val="$(trim "$val")"
        case "$key" in
            name) profile_name="$val" ;;
            server) profile_server="$val" ;;
            web) profile_web="$val" ;;
            "") ;;
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
