# forest shell zsh — interactive helpers

function forest-tmp() {
  echo "creating tmp dir"
  dir=$(forest tmp)

  echo "moving into: $dir"
  cd "$dir"
}

autoload -Uz add-zsh-hook

# ── forest-defer-aggregate: load component-declared integrations late ────────
#
# Called by the block `forest shell zsh` emits when the aggregate of
# component-declared integrations doesn't exist yet (fresh install, cold cache).
# A background warm is already running by then; this watches for the aggregate
# to appear and sources it before a prompt, so the integrations arrive in this
# shell instead of only in the next one.
#
# Not something you call yourself.

typeset -g _forest_aggregate_path

function forest-defer-aggregate() {
  _forest_aggregate_path="${XDG_CACHE_HOME:-$HOME/.cache}/forest/global/shell/zsh.sh"
  add-zsh-hook precmd _forest_aggregate_drain
}

function _forest_aggregate_drain() {
  [[ -r $_forest_aggregate_path ]] || return 0
  source "$_forest_aggregate_path"
  # One shot: the aggregate is loaded, so stop checking.
  add-zsh-hook -d precmd _forest_aggregate_drain
  unset _forest_aggregate_path
}

# ── forest-init: the escape hatch for tools forest can't discover ───────────
#
# You should not normally need this. A forest component declares its own shell
# integration via `include.shell.init.<shell>` in its manifest, and the block
# above loads all of them from one cached file — nothing per tool in your .zshrc.
#
# forest-init is for the two cases that can't work:
#
#   * a tool that isn't a forest component at all (installed via cargo, brew, …)
#   * a forest tool whose component hasn't declared `include.shell` yet
#
# It is a drop-in replacement for `eval "$(<tool> <args…>)"` that never blocks a
# cold shell: it runs the tool with FOREST_GLOBAL_NO_FETCH=1, so forest answers
# "not cached yet" (exit 75) instead of downloading, and starts a quiet
# background warm. The skipped integration is queued and retried from a precmd
# hook, so it loads in *this* shell as soon as the download lands.
#
# Already-cached tools take the path they always did: one exec, eval, done.
# Non-forest commands ignore the env var and get eval'd normally, so one form
# covers both cases:
#
#   forest-init kignore init zsh     # cargo-installed, not a forest component
#
# Set FOREST_NO_GLOBAL_WARM=1 to opt out of background warming entirely.

# Pending integrations, each entry a `${(q)}`-quoted argv.
typeset -ga _forest_init_pending

function forest-init() {
  (( $# )) || return 0
  # Not installed at all (a tool you removed, a machine where it was never
  # added) — silently do nothing, exactly as an unguarded eval of a missing
  # command would after its "command not found".
  command -v -- "$1" >/dev/null 2>&1 || return 0

  local out ret
  out=$(FOREST_GLOBAL_NO_FETCH=1 "$@" 2>/dev/null)
  ret=$?

  if (( ret == 0 )); then
    [[ -n $out ]] && eval "$out"
    return 0
  fi

  # 75 == EX_TEMPFAIL: forest deliberately declined to download and has
  # started a background warm. Queue for retry. Any other code is a real
  # failure (bad subcommand, crashing tool) — don't retry it forever, and
  # don't let it fail the rc file.
  if (( ret == 75 )); then
    _forest_init_pending+=("${(j: :)${(q)@}}")
    add-zsh-hook precmd _forest_init_drain
  fi
  return 0
}

# Retry the queued integrations before each prompt, dropping the hook once the
# queue empties. Runs only while something is pending, so a warm shell never
# pays for it.
function _forest_init_drain() {
  local -a still
  local entry out ret
  local -a cmd

  for entry in $_forest_init_pending; do
    # (z) re-splits the stored line into words honouring the (q) quoting from
    # forest-init, (Q) then strips that quoting. Deliberately unquoted: inside
    # "…" the whole expansion would collapse into a single word and we'd try to
    # exec a command literally named "gitnow init zsh".
    cmd=( ${(Q)${(z)entry}} )
    out=$(FOREST_GLOBAL_NO_FETCH=1 "${cmd[@]}" 2>/dev/null)
    ret=$?
    if (( ret == 0 )); then
      [[ -n $out ]] && eval "$out"
    elif (( ret == 75 )); then
      still+=("$entry")   # still downloading — try again next prompt
    fi
  done

  _forest_init_pending=("${still[@]}")
  (( $#_forest_init_pending )) || add-zsh-hook -d precmd _forest_init_drain
}
