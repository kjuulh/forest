# forest shell fish — interactive helpers

function forest-tmp
    echo "creating tmp dir"
    set dir (forest tmp)

    echo "moving into: $dir"
    cd "$dir"
end

# ── forest-defer-aggregate: load component-declared integrations late ────────
#
# Called by the block `forest shell fish` emits when the aggregate of
# component-declared integrations doesn't exist yet (fresh install, cold cache).
# A background warm is already running by then; this watches for the aggregate to
# appear and sources it on a prompt, so the integrations arrive in this shell
# rather than only in the next one. Not something you call yourself.

set -g _forest_aggregate_path

function forest-defer-aggregate
    set -g _forest_aggregate_path $HOME/.cache/forest/global/shell/fish.sh
    if test -n "$XDG_CACHE_HOME"
        set -g _forest_aggregate_path $XDG_CACHE_HOME/forest/global/shell/fish.sh
    end
end

function _forest_aggregate_drain --on-event fish_prompt
    test -n "$_forest_aggregate_path"; or return 0
    test -r "$_forest_aggregate_path"; or return 0
    source "$_forest_aggregate_path"
    # One shot: the aggregate is loaded, so stop checking.
    set -e _forest_aggregate_path
end

# ── forest-init: the escape hatch for tools forest can't discover ─────────────
#
# You should not normally need this. A forest component declares its own shell
# integration via `include.shell.init.<shell>` in its manifest and the block
# above loads all of them from one cached file. forest-init is for tools that
# aren't forest components (cargo/brew installs) or forest tools whose component
# hasn't declared `include.shell` yet.
#
# It replaces `<tool> <args…> | source` without blocking a cold shell:
# FOREST_GLOBAL_NO_FETCH=1 makes forest answer "not cached yet" (exit 75) and
# warm in the background, and skipped integrations are retried on each prompt so
# they load in this shell once the download lands.
#
#   forest-init kignore init fish    # cargo-installed, not a forest component
#
# Set FOREST_NO_GLOBAL_WARM=1 to opt out of background warming entirely.

set -g _forest_init_pending

function forest-init
    test (count $argv) -gt 0; or return 0
    # Not installed at all — silently do nothing.
    command -q -- $argv[1]; or return 0

    # Piped straight into `source` rather than captured: fish command
    # substitution splits on newlines and would drop the blank lines out of the
    # tool's script. $pipestatus[1] keeps the tool's exit code, which is the
    # whole signal here.
    FOREST_GLOBAL_NO_FETCH=1 $argv 2>/dev/null | source
    set -l ret $pipestatus[1]

    test $ret -eq 0; and return 0

    # 75 == EX_TEMPFAIL: forest declined to download and started a background
    # warm. Queue for retry. Any other code is a real failure — don't retry it
    # forever, and don't let it fail the config file.
    if test $ret -eq 75
        set -a _forest_init_pending (string escape -- $argv | string join ' ')
    end
    return 0
end

# Retry queued integrations on each prompt. A no-op once the queue drains, so a
# warm shell pays nothing for it.
function _forest_init_drain --on-event fish_prompt
    test (count $_forest_init_pending) -gt 0; or return 0
    set -l still
    for entry in $_forest_init_pending
        # The entry is already `string escape`d, so eval re-splits it back into
        # the original argv — splitting on spaces here would break any arg that
        # contained one.
        eval "FOREST_GLOBAL_NO_FETCH=1 $entry" 2>/dev/null | source
        set -l ret $pipestatus[1]
        if test $ret -eq 75
            set -a still $entry   # still downloading — try again next prompt
        end
    end
    set -g _forest_init_pending $still
end
