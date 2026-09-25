#!/usr/bin/env bash
# fork-check.sh — fail when the fork changes upstream code it has not declared.
#
# The fork is upstream (github.com/understory-io/forest) plus an overlay. Every
# file that differs from the upstream commit last merged (.fork/upstream-base)
# must be one of:
#   - fork-owned              .fork/overlay
#   - deleted on purpose      .fork/removed
#   - fork-owned lines        .fork/prefer-ours
#   - a declared carry        .fork/carries (with the reason and exit plan)
# Anything else is product code changed in the fork, which is what made every
# sync a hand-resolved merge. Make that change upstream instead, or declare it.
#
# Also reports carries that no longer differ from upstream (stale), so the
# inventory shrinks as upstream PRs land.
#
# Usage: scripts/fork-check.sh [--base <upstream-commit>] [--worktree] [--list-stale]
#   --base        compare against this upstream commit instead of
#                 .fork/upstream-base (fork-sync.sh passes the one it merged)
#   --worktree    compare the working tree instead of HEAD (fork-sync.sh runs
#                 this in the middle of its merge, with --base = MERGE_HEAD)
#   --list-stale  print only the stale carry globs, one per line, and exit 0
#
# Exit: 0 clean (stale carries only warn), 1 undeclared changes, 2 misuse.
# Needs full history (Woodpecker: clone depth 0) and nothing else: no network,
# no credentials.

set -euo pipefail

root=$(git rev-parse --show-toplevel)
cd "$root"

base=""
list_stale=0
worktree=0
while [ "$#" -gt 0 ]; do
    case "$1" in
        --base) base="${2:?--base needs a commit}"; shift 2 ;;
        --list-stale) list_stale=1; shift ;;
        --worktree) worktree=1; shift ;;
        -h|--help) sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "fork-check.sh: unknown argument: $1" >&2; exit 2 ;;
    esac
done

[ -n "$base" ] || base=$(tr -d '[:space:]' < .fork/upstream-base)
git cat-file -e "${base}^{commit}" 2>/dev/null \
    || { echo "fork-check.sh: upstream base $base is not in this clone (shallow clone?)" >&2; exit 2; }
merging=""
[ "$worktree" -eq 1 ] && [ "$(git rev-parse -q --verify MERGE_HEAD || true)" = "$(git rev-parse "$base")" ] && merging=1
[ -n "$merging" ] || git merge-base --is-ancestor "$base" HEAD \
    || { echo "fork-check.sh: upstream base $base is not an ancestor of HEAD; merge upstream with scripts/fork-sync.sh" >&2; exit 2; }

# Glob lists without comments or blank lines.
globs() { sed -e 's/#.*//' -e 's/[[:space:]]*$//' -e '/^$/d' "$1"; }

matches_any() { # <path> <glob>...
    local path=$1 g
    shift
    for g in "$@"; do
        # shellcheck disable=SC2053  # the glob is meant to match
        [[ $path == $g ]] && return 0
    done
    return 1
}

mapfile -t overlay < <(globs .fork/overlay)
mapfile -t removed < <(globs .fork/removed)
mapfile -t ours < <(globs .fork/prefer-ours)
mapfile -t carries < <(globs .fork/carries)

undeclared=()
declare -A carry_used=()
while IFS=$'\t' read -r status path; do
    if matches_any "$path" "${overlay[@]}"; then continue; fi
    if [ "$status" = D ] && matches_any "$path" "${removed[@]}"; then continue; fi
    if matches_any "$path" "${ours[@]}"; then continue; fi
    hit=""
    for g in "${carries[@]}"; do
        # shellcheck disable=SC2053
        if [[ $path == $g ]]; then carry_used[$g]=1; hit=1; fi
    done
    [ -n "$hit" ] && continue
    undeclared+=("$status $path")
done < <(if [ "$worktree" -eq 1 ]; then
    git diff --name-status --no-renames "$base"
else
    git diff --name-status --no-renames "$base" HEAD
fi)

stale=()
for g in "${carries[@]}"; do
    [ -n "${carry_used[$g]:-}" ] || stale+=("$g")
done

if [ "$list_stale" -eq 1 ]; then
    [ "${#stale[@]}" -eq 0 ] || printf '%s\n' "${stale[@]}"
    exit 0
fi

echo "fork-check: comparing $([ "$worktree" -eq 1 ] && echo "the working tree" || echo HEAD) with upstream $(git rev-parse --short "$base")"

if [ "${#stale[@]}" -gt 0 ]; then
    echo "fork-check: ${#stale[@]} carry entr$([ "${#stale[@]}" -eq 1 ] && echo y || echo ies) no longer differ from upstream; remove from .fork/carries:"
    printf '  %s\n' "${stale[@]}"
fi

if [ "${#undeclared[@]}" -gt 0 ]; then
    cat <<EOF
fork-check: ${#undeclared[@]} upstream-owned file(s) changed without being declared:
$(printf '  %s\n' "${undeclared[@]}")

Product changes belong upstream (github.com/understory-io/forest) first; the
daily sync brings them here. If this one must live in the fork, add it to
.fork/carries under a heading that says why and when it goes away.
EOF
    exit 1
fi

echo "fork-check: every difference from upstream is declared"
