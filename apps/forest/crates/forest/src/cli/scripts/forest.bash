# forest shell bash — interactive helpers

forest-tmp() {
  echo "creating tmp dir"
  dir=$(forest tmp)

  echo "moving into: $dir"
  cd "$dir"
}

# ── forest-defer-aggregate: load component-declared integrations late ────────
#
# Called by the block `forest shell bash` emits when the aggregate of
# component-declared integrations doesn't exist yet (fresh install, cold cache).
# A background warm is already running by then; this watches for the aggregate to
# appear and sources it before a prompt, so the integrations arrive in this shell
# rather than only in the next one. Not something you call yourself.

forest-defer-aggregate() {
  _forest_aggregate_path="${XDG_CACHE_HOME:-$HOME/.cache}/forest/global/shell/bash.sh"
  case ";$PROMPT_COMMAND;" in
    *";_forest_aggregate_drain;"*) ;;
    *) PROMPT_COMMAND="_forest_aggregate_drain${PROMPT_COMMAND:+;$PROMPT_COMMAND}" ;;
  esac
}

_forest_aggregate_drain() {
  [ -r "$_forest_aggregate_path" ] || return 0
  . "$_forest_aggregate_path"
  # One shot: the aggregate is loaded, so stop checking.
  PROMPT_COMMAND="${PROMPT_COMMAND/_forest_aggregate_drain;/}"
  PROMPT_COMMAND="${PROMPT_COMMAND/#_forest_aggregate_drain/}"
  unset _forest_aggregate_path
}

# ── forest-init: the escape hatch for tools forest can't discover ────────────
#
# You should not normally need this. A forest component declares its own shell
# integration via `include.shell.init.<shell>` in its manifest and the block
# above loads all of them from one cached file. forest-init is for tools that
# aren't forest components (cargo/brew installs) or forest tools whose component
# hasn't declared `include.shell` yet.
#
# It replaces `eval "$(<tool> <args…>)"` without blocking a cold shell:
# FOREST_GLOBAL_NO_FETCH=1 makes forest answer "not cached yet" (exit 75) and
# warm in the background, and skipped integrations are retried from
# PROMPT_COMMAND so they load in this shell once the download lands.
#
#   forest-init kignore init bash    # cargo-installed, not a forest component
#
# Set FOREST_NO_GLOBAL_WARM=1 to opt out of background warming entirely.

_forest_init_pending=()

forest-init() {
  [ "$#" -gt 0 ] || return 0
  command -v -- "$1" >/dev/null 2>&1 || return 0

  local out ret
  out=$(FOREST_GLOBAL_NO_FETCH=1 "$@" 2>/dev/null)
  ret=$?

  if [ "$ret" -eq 0 ]; then
    [ -n "$out" ] && eval "$out"
    return 0
  fi

  # 75 == EX_TEMPFAIL: not cached yet, background warm started. Queue it.
  if [ "$ret" -eq 75 ]; then
    _forest_init_pending+=("$(printf '%q ' "$@")")
    case ";$PROMPT_COMMAND;" in
      *";_forest_init_drain;"*) ;;
      *) PROMPT_COMMAND="_forest_init_drain${PROMPT_COMMAND:+;$PROMPT_COMMAND}" ;;
    esac
  fi
  return 0
}

# Retry queued integrations before each prompt; unhook once the queue drains so
# a warm shell pays nothing.
_forest_init_drain() {
  local -a still=()
  local entry out ret
  for entry in "${_forest_init_pending[@]}"; do
    out=$(eval "FOREST_GLOBAL_NO_FETCH=1 $entry" 2>/dev/null)
    ret=$?
    if [ "$ret" -eq 0 ]; then
      [ -n "$out" ] && eval "$out"
    elif [ "$ret" -eq 75 ]; then
      still+=("$entry")
    fi
  done
  _forest_init_pending=("${still[@]}")
  if [ "${#_forest_init_pending[@]}" -eq 0 ]; then
    PROMPT_COMMAND="${PROMPT_COMMAND/_forest_init_drain;/}"
    PROMPT_COMMAND="${PROMPT_COMMAND/#_forest_init_drain/}"
  fi
}
