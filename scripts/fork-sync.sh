#!/usr/bin/env bash
# fork-sync.sh — merge upstream main into the fork, resolving what the fork
# has declared and stopping on anything it has not.
#
# Code moves one way: product changes land upstream
# (github.com/understory-io/forest) and this script brings them here. What the
# fork changes is declared in .fork/ (see .fork/README.md), which is what lets
# a merge resolve without a person:
#   - .fork/removed      deleted again after the merge, whatever upstream did
#   - .fork/prefer-ours  conflicting hunks resolve to the fork's side
#   - .fork/carries groups headed `## [take-upstream #N ...]`: once every
#                        listed PR's squash commit "(#N)" is in upstream, a
#                        conflict resolves to upstream's file (the PR landed,
#                        reworked in review). Before that, it is a conflict.
#   - */Cargo.lock       the fork's lock, then `cargo metadata` adds or drops
#                        only what the merged manifests now need
# Any other conflict aborts the merge and names the files. So does
# fork-check.sh failing afterwards. Stale carries are dropped from
# .fork/carries in the merge commit.
#
# Usage: scripts/fork-sync.sh [--upstream <ref>] [--base <ref>] [--branch <name>]
#   --upstream  upstream commit to merge (default: upstream/main)
#   --base      fork branch to start from (default: origin/kjuulh/gitea-fork)
#   --branch    branch to create for the result
#               (default: sync/upstream-<UTC date>)
#
# Leaves the result committed on --branch (checked out) and pushes nothing. On
# exit 2 or 3 it returns to the branch it was started from.
#
# Resolving by hand, when a carried file conflicts:
#   git switch -c sync/manual origin/kjuulh/gitea-fork
#   git merge --no-ff upstream/main    # resolve; keep upstream's product text
#   echo <upstream sha> > .fork/upstream-base && git add .fork/upstream-base
#   scripts/fork-check.sh              # must pass before committing
#   git commit                         # message: "merge: reconcile Gitea fork with Understory main"
# Exit: 0 merged, 3 already up to date, 2 unresolved conflict or undeclared
# change (merge aborted, branch left at --base), 1 other failure.

set -euo pipefail

upstream="upstream/main"
base="origin/kjuulh/gitea-fork"
branch="sync/upstream-$(date -u +%Y-%m-%d)"

while [ "$#" -gt 0 ]; do
    case "$1" in
        --upstream) upstream="${2:?}"; shift 2 ;;
        --base) base="${2:?}"; shift 2 ;;
        --branch) branch="${2:?}"; shift 2 ;;
        -h|--help) sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "fork-sync.sh: unknown argument: $1" >&2; exit 1 ;;
    esac
done

root=$(git rev-parse --show-toplevel)
cd "$root"

[ -z "$(git status --porcelain --untracked-files=no)" ] \
    || { echo "fork-sync.sh: working tree has changes; commit or discard them first" >&2; exit 1; }

upstream_sha=$(git rev-parse --verify "${upstream}^{commit}")
# Where to return on exit 2 or 3: a branch, or a commit (CI's detached HEAD).
if orig=$(git symbolic-ref --short -q HEAD); then orig_switch=(git switch -q "$orig")
else orig_switch=(git switch -q --detach "$(git rev-parse HEAD)"); fi
git switch -q -C "$branch" "$base"

if git merge-base --is-ancestor "$upstream_sha" HEAD; then
    echo "fork-sync: $base already contains upstream $(git rev-parse --short "$upstream_sha")"
    "${orig_switch[@]}"
    exit 3
fi

old_base=$(tr -d '[:space:]' < .fork/upstream-base)
merged_log=$(git log --no-merges --format='- %h %s' "HEAD..$upstream_sha")

globs() { sed -e 's/#.*//' -e 's/[[:space:]]*$//' -e '/^$/d' "$1"; }
matches_any() {
    local path=$1 g
    shift
    for g in "$@"; do
        # shellcheck disable=SC2053
        [[ $path == $g ]] && return 0
    done
    return 1
}
mapfile -t removed < <(globs .fork/removed)
mapfile -t ours < <(globs .fork/prefer-ours)
mapfile -t carries < <(globs .fork/carries)

# Globs in carries groups headed `## [take-upstream #N ...]`, active only once
# upstream contains every listed PR. Upstream squash-merges, so a merged PR is
# a commit whose subject contains "(#N)". A heading is a run of `##` lines; the
# marker on its first line covers the group. Checking merge state rather than
# trusting the marker matters: a *different* upstream change to the same lines
# would otherwise replace the fork's carry before its PR has landed.
take_upstream_globs() {
    local subjects glob prs pr ok
    subjects=$(git log --format=%s "$upstream_sha")
    while IFS=$'\t' read -r glob prs; do
        ok=1
        for pr in $prs; do
            grep -qF "(#$pr)" <<<"$subjects" || { ok=""; break; }
        done
        [ -n "$ok" ] && [ -n "$prs" ] && echo "$glob"
    done < <(awk '
        /^##/ {
            if (!in_heading) {
                prs = ""
                if (match($0, /^## \[take-upstream[^]]*\]/)) {
                    m = substr($0, RSTART, RLENGTH); gsub(/[^0-9 ]/, " ", m); prs = m
                }
            }
            in_heading = 1; next
        }
        { in_heading = 0 }
        /^[[:space:]]*(#|$)/ { next }
        prs != "" { sub(/[[:space:]]*#.*/, ""); print $0 "\t" prs }
    ' .fork/carries)
}
mapfile -t take_upstream < <(take_upstream_globs)

# Back to where the caller was, with --branch left at --base.
abort() {
    git merge --abort 2>/dev/null || git reset -q --hard "$base"
    "${orig_switch[@]}"
    exit 2
}

git -c merge.renames=false merge --no-ff --no-commit "$upstream_sha" >/dev/null 2>&1 || true
git rev-parse -q --verify MERGE_HEAD >/dev/null \
    || { echo "fork-sync.sh: git merge did not start" >&2; exit 1; }

resolved=()
unresolved=()
locks=()
while IFS= read -r path; do
    [ -n "$path" ] || continue
    if matches_any "$path" "${removed[@]}"; then
        git rm -q --cached -- "$path" 2>/dev/null || true
        rm -f -- "$path"
        resolved+=("$path (removed in the fork)")
    elif [[ $path == */Cargo.lock ]]; then
        git checkout -q --ours -- "$path"
        git add -- "$path"
        locks+=("$path")
        resolved+=("$path (fork's lock, re-resolved below)")
    elif matches_any "$path" "${ours[@]}" \
        && git cat-file -e ":1:$path" 2>/dev/null \
        && git cat-file -e ":2:$path" 2>/dev/null \
        && git cat-file -e ":3:$path" 2>/dev/null; then
        tmp=$(mktemp -d)
        git show ":1:$path" > "$tmp/base"
        git show ":2:$path" > "$tmp/ours"
        git show ":3:$path" > "$tmp/theirs"
        git merge-file --ours "$tmp/ours" "$tmp/base" "$tmp/theirs"
        cp "$tmp/ours" "$path"
        rm -rf "$tmp"
        git add -- "$path"
        resolved+=("$path (conflicting hunks: fork's side)")
    elif matches_any "$path" "${take_upstream[@]}"; then
        if git cat-file -e ":3:$path" 2>/dev/null; then
            git checkout -q --theirs -- "$path"
            git add -- "$path"
        else
            git rm -q -f -- "$path"
        fi
        resolved+=("$path (take-upstream carry: upstream's version)")
    elif matches_any "$path" "${carries[@]}"; then
        unresolved+=("$path (declared carry: upstream changed the same lines)")
    else
        unresolved+=("$path (not declared in .fork/)")
    fi
done < <(git diff --name-only --diff-filter=U)

if [ "${#unresolved[@]}" -gt 0 ]; then
    cat >&2 <<EOF
fork-sync: merging upstream $(git rev-parse --short "$upstream_sha") needs a person for:
$(printf '  %s\n' "${unresolved[@]}")

A carry conflicts when upstream edits the lines the fork changed: resolve it
by hand (scripts/fork-sync.sh --help, "Resolving by hand"), and prefer moving
fork-only content into a fork-owned file so it cannot conflict again. An
undeclared file is product code changed in the fork: land it upstream, or
declare it in .fork/.
Merge aborted; nothing was committed.
EOF
    abort
fi

# Upstream may have added files under a removed path without conflicting.
for g in "${removed[@]}"; do
    while IFS= read -r path; do
        [ -n "$path" ] || continue
        matches_any "$path" "$g" || continue
        git rm -q -f -- "$path"
        resolved+=("$path (removed in the fork)")
    done < <(git ls-files)
done

# A textual merge of a lock can be internally inconsistent even without a
# conflict, and a resolved one lacks whatever upstream's manifests added.
# `cargo metadata` makes the minimal change: it adds and drops entries the
# manifests require and moves nothing else.
for manifest in apps/forest/Cargo.toml apps/forage/Cargo.toml; do
    [ -f "$manifest" ] || continue
    lock="$(dirname "$manifest")/Cargo.lock"
    before=$(git hash-object "$lock")
    cargo metadata --format-version 1 --manifest-path "$manifest" >/dev/null
    if [ "$(git hash-object "$lock")" != "$before" ]; then
        git add -- "$lock"
        [[ " ${locks[*]:-} " == *" $lock "* ]] || resolved+=("$lock (re-resolved by cargo metadata)")
    fi
done

echo "$upstream_sha" > .fork/upstream-base
git add .fork/upstream-base

# Check the merged tree before committing it, and find carries the merge made
# identical to upstream.
check_rc=0
check_out=$(scripts/fork-check.sh --base "$upstream_sha" --worktree 2>&1) || check_rc=$?
mapfile -t stale < <(scripts/fork-check.sh --base "$upstream_sha" --worktree --list-stale)

if [ "$check_rc" -ne 0 ]; then
    echo "$check_out" >&2
    echo "fork-sync: the merged tree fails fork-check; merge aborted" >&2
    abort
fi

if [ "${#stale[@]}" -gt 0 ]; then
    for g in "${stale[@]}"; do
        # Delete the exact line; the glob is literal text in the file.
        grep -vxF -- "$g" .fork/carries > .fork/carries.tmp || true
        mv .fork/carries.tmp .fork/carries
    done
    git add .fork/carries
fi

msg=$(cat <<EOF
merge: reconcile Gitea fork with Understory main

Merges upstream $(git rev-parse --short "$old_base")..$(git rev-parse --short "$upstream_sha") with scripts/fork-sync.sh.

Upstream commits:
$merged_log

Resolved by .fork/ declarations:
$( [ "${#resolved[@]}" -gt 0 ] && printf -- '- %s\n' "${resolved[@]}" || echo "- nothing to resolve" )

Carries now identical to upstream, dropped from .fork/carries:
$( [ "${#stale[@]}" -gt 0 ] && printf -- '- %s\n' "${stale[@]}" || echo "- none" )

Verified: fork-check.sh passes on the merged tree. Tests run in the
upstream-sync pipeline (or by hand), not by this script.
EOF
)
git commit -q -m "$msg"
echo "fork-sync: merged upstream $(git rev-parse --short "$upstream_sha") on $branch as $(git rev-parse --short HEAD)"
