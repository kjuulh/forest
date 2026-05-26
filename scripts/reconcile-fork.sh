#!/usr/bin/env bash
# reconcile-fork.sh — rebase a local fork bookmark onto an upstream bookmark.
#
# Defaults match this repository's regular workflow:
#   scripts/reconcile-fork.sh --push
#
# First-time/rawpotion-stream cutover, when kjuulh/gitea-fork still sits on
# main@origin instead of main@github:
#   scripts/reconcile-fork.sh --from main@origin --push
#
# Routine use after the cutover:
#   scripts/reconcile-fork.sh --push
#
# The script creates a timestamped backup bookmark before rewriting and uses
# jj's force-with-lease-style push safety checks.

set -euo pipefail

branch="kjuulh/gitea-fork"
onto="main@github"
from=""
push_remote="origin"
push=0
fetch=1
dry_run=0

usage() {
    cat <<'EOF'
Usage: reconcile-fork.sh [options]

Rebase a fork bookmark stack onto an upstream bookmark with a backup bookmark,
verification, and optional push.

Options:
  --branch REVSET      Bookmark/revset to move (default: kjuulh/gitea-fork)
  --onto REVSET        Destination base (default: main@github)
  --from REVSET        Old base used to select the fork stack.
                       Default: same as --onto. Use main@origin for the
                       first cutover from rawpotion's old stream.
  --remote NAME        Remote to push to when --push is set (default: origin)
  --push               Push --branch to --remote after verification
  --dry-run            Show the plan and run jj rebase with --no-integrate-operation
  --no-fetch           Skip fetching github and origin before rebasing
  -h, --help           Show this help

Examples:
  reconcile-fork.sh --push
  reconcile-fork.sh --from main@origin --push
  reconcile-fork.sh --branch kjuulh/gitea-fork --onto main@github --dry-run
EOF
}

err() {
    echo "reconcile-fork.sh: $*" >&2
    exit 1
}

quote_revset() {
    # Single-quote for display only; revsets are still passed as argv values.
    printf "'%s'" "$1"
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --branch)
            [ "$#" -ge 2 ] || err "--branch needs a value"
            branch="$2"
            shift 2
            ;;
        --onto|--base)
            [ "$#" -ge 2 ] || err "$1 needs a value"
            onto="$2"
            shift 2
            ;;
        --from)
            [ "$#" -ge 2 ] || err "--from needs a value"
            from="$2"
            shift 2
            ;;
        --remote)
            [ "$#" -ge 2 ] || err "--remote needs a value"
            push_remote="$2"
            shift 2
            ;;
        --push)
            push=1
            shift
            ;;
        --dry-run)
            dry_run=1
            shift
            ;;
        --no-fetch)
            fetch=0
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            err "unknown argument: $1"
            ;;
    esac
done

[ -n "$from" ] || from="$onto"

command -v jj >/dev/null 2>&1 || err "jj not found on PATH"
jj root >/dev/null || err "not inside a jj repository"

if [ "$fetch" -eq 1 ]; then
    echo "==> Fetching github"
    jj git fetch --remote github
    echo "==> Fetching origin"
    jj git fetch --remote origin
fi

source_roots="roots(${from}..${branch})"
stack="${from}..${branch}"

# Validate revsets before creating a backup or rebasing.
jj log -r "$onto" --limit 1 --no-graph --template 'commit_id.short() ++ " " ++ description.first_line() ++ "\n"' >/dev/null \
    || err "cannot resolve --onto $(quote_revset "$onto")"
jj log -r "$branch" --limit 1 --no-graph --template 'commit_id.short() ++ " " ++ description.first_line() ++ "\n"' >/dev/null \
    || err "cannot resolve --branch $(quote_revset "$branch")"

root_count=$(jj log -r "$source_roots" --no-graph --template 'commit_id ++ "\n"' | wc -l | tr -d ' ')
[ "$root_count" -gt 0 ] || err "no revisions selected by $(quote_revset "$source_roots")"

backup_name="backup/${branch//\//-}-before-${onto//@/-}-$(date -u +%Y%m%dT%H%M%SZ)"

cat <<EOF
==> Reconcile plan
    branch:      $branch
    onto:        $onto
    from:        $from
    source root: $source_roots ($root_count root(s))
    backup:      $backup_name
    push:        $([ "$push" -eq 1 ] && echo "$push_remote" || echo "no")
EOF

echo "==> Selected stack"
jj log -r "$stack" --limit 30

if [ "$dry_run" -eq 1 ]; then
    echo "==> Dry-running rebase"
    jj rebase -s "$source_roots" -d "$onto" --no-integrate-operation
    echo "==> Dry-run complete; repository operation was not integrated and no backup was created"
    exit 0
fi

echo "==> Creating backup bookmark"
jj bookmark create "$backup_name" -r "$branch"

echo "==> Rebasing"
jj rebase -s "$source_roots" -d "$onto"

if conflicts=$(jj resolve --list 2>/dev/null); then
    if [ -n "$conflicts" ]; then
        cat <<EOF
==> Conflicts remain
$conflicts

Resolve them, then run:
  jj status
  jj diff
  jj squash

If jj reports another conflicted commit, follow its "jj new <change>" instruction
and repeat until "jj resolve --list" reports no conflicts.

Backup bookmark: $backup_name
EOF
        exit 2
    fi
fi

echo "==> Verifying ancestry"
jj log -r "${onto}::${branch}" --limit 20

if jj resolve --list >/tmp/reconcile-fork-conflicts.$$ 2>/dev/null; then
    if [ -s /tmp/reconcile-fork-conflicts.$$ ]; then
        cat /tmp/reconcile-fork-conflicts.$$
        rm -f /tmp/reconcile-fork-conflicts.$$
        err "conflicts remain after rebase"
    fi
fi
rm -f /tmp/reconcile-fork-conflicts.$$

if [ "$push" -eq 1 ]; then
    echo "==> Push dry-run"
    jj git push --remote "$push_remote" --bookmark "$branch" --dry-run
    echo "==> Pushing"
    jj git push --remote "$push_remote" --bookmark "$branch"
    echo "==> Fetching $push_remote to verify pushed bookmark"
    jj git fetch --remote "$push_remote"
    jj bookmark list --all-remotes "$branch"
else
    cat <<EOF
==> Rebase complete; not pushed
Push with:
  jj git push --remote $push_remote --bookmark $branch

Backup bookmark: $backup_name
EOF
fi
