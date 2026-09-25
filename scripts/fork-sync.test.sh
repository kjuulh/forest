#!/usr/bin/env bash
# Tests for fork-sync.sh and fork-check.sh against real git repositories: a
# throwaway "upstream" and a "fork" built in a temp dir, so every rule is
# exercised through an actual merge rather than asserted about.
#
# Usage: scripts/fork-sync.test.sh     (needs git and bash only)

set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1

failures=0
pass() { echo "ok   $1"; }
fail() { echo "FAIL $1"; failures=$((failures + 1)); }

# upstream: a product file, a versioned manifest, and a workflow dir.
setup() {
    rm -rf "$work/up" "$work/fork"
    git init -q -b main "$work/up"
    cd "$work/up"
    mkdir -p src .github/workflows
    printf 'fn a() {}\nfn b() {}\n' > src/lib.rs
    printf '[package]\nname = "forest"\nversion = "0.3.13"\n\n[deps]\nserde = "1"\n' > Cargo.toml
    echo "on: push" > .github/workflows/ci.yaml
    git add -A && git commit -qm "upstream: initial"

    git clone -q "$work/up" "$work/fork"
    cd "$work/fork"
    git remote rename origin upstream
    git switch -q -c gitea-fork
    mkdir -p .fork scripts .woodpecker
    cp "$here/fork-check.sh" "$here/fork-sync.sh" scripts/
    git rev-parse main > .fork/upstream-base
    printf '.fork/*\n.woodpecker/*\nscripts/fork-check.sh\nscripts/fork-sync.sh\n' > .fork/overlay
    printf '.github/*\n' > .fork/removed
    printf 'Cargo.toml\n' > .fork/prefer-ours
    printf '# carries\n' > .fork/carries
    echo "steps: {}" > .woodpecker/ci.yaml
    git rm -q -r .github
    sed -i 's/0.3.13/0.3.15/' Cargo.toml
    git add -A && git commit -qm "fork: overlay"
}

upstream_commit() { # <message> <command...>
    local msg=$1
    shift
    (cd "$work/up" && "$@" && git add -A && git commit -qm "$msg")
    (cd "$work/fork" && git fetch -q upstream)
}

sync() { (cd "$work/fork" && scripts/fork-sync.sh --upstream upstream/main --base gitea-fork --branch sync "$@"); }

# 1. An overlay-only fork takes upstream changes with no person involved,
#    including edits to a file the fork deletes and to its version line.
setup
upstream_commit "upstream: product and workflow" bash -c \
    'printf "fn a() {}\nfn b() { 1 }\n" > src/lib.rs; echo "on: [push, pull_request]" > .github/workflows/ci.yaml; echo "x" > .github/workflows/new.yaml'
if sync >/dev/null 2>&1; then
    cd "$work/fork"
    if grep -q 'fn b() { 1 }' src/lib.rs && [ ! -e .github ] \
        && [ "$(cat .fork/upstream-base)" = "$(git rev-parse upstream/main)" ]; then
        pass "overlay-only fork merges upstream unattended; removed paths stay removed"
    else
        fail "merge result wrong (product change, .github or upstream-base)"
    fi
else
    fail "overlay-only merge did not succeed"
fi

# 2. Upstream bumps the version and adds a dependency in the same file:
#    the conflicting version hunk keeps the fork's, the dependency lands.
setup
upstream_commit "upstream: release and dep" bash -c \
    'sed -i "s/0.3.13/0.3.14/" Cargo.toml; printf "tokio = \"1\"\n" >> Cargo.toml'
if sync >/dev/null 2>&1; then
    cd "$work/fork"
    if grep -q 'version = "0.3.15"' Cargo.toml && grep -q 'tokio' Cargo.toml && ! grep -q '<<<<' Cargo.toml; then
        pass "prefer-ours keeps the fork's version hunk and upstream's other changes"
    else
        fail "prefer-ours resolution wrong: $(tr '\n' ' ' < Cargo.toml)"
    fi
else
    fail "prefer-ours merge did not succeed"
fi

# 3. The fork changed product code without declaring it, and upstream changes
#    the same lines: the sync stops, names the file, and commits nothing.
setup
(cd "$work/fork" && sed -i 's/fn a() {}/fn a() { fork }/' src/lib.rs && git commit -qam "fork: product change")
upstream_commit "upstream: same lines" sed -i 's/fn a() {}/fn a() { upstream }/' src/lib.rs
before=$(cd "$work/fork" && git rev-parse gitea-fork)
out=$(sync 2>&1) && rc=0 || rc=$?
cd "$work/fork"
if [ "$rc" -eq 2 ] && [[ $out == *"src/lib.rs (not declared"* ]] && [ "$(git rev-parse sync)" = "$before" ] \
    && ! git rev-parse -q --verify MERGE_HEAD >/dev/null && [ "$(git branch --show-current)" = gitea-fork ]; then
    pass "an undeclared conflict aborts, names the file, commits nothing, returns to the starting branch"
else
    fail "undeclared conflict: rc=$rc, output: $out"
fi

# 4. fork-check fails on an undeclared product change even without a conflict,
#    and passes once it is declared as a carry.
setup
(cd "$work/fork" && echo "fn c() {}" >> src/lib.rs && git commit -qam "fork: product change")
cd "$work/fork"
if ! scripts/fork-check.sh >/dev/null 2>&1; then
    pass "fork-check refuses an undeclared change"
else
    fail "fork-check accepted an undeclared change"
fi
printf '## upstream PR #1\nsrc/lib.rs\n' >> .fork/carries && git commit -qam "declare"
if scripts/fork-check.sh >/dev/null 2>&1; then
    pass "fork-check accepts it once declared as a carry"
else
    fail "fork-check refused a declared carry"
fi

# 5. The carry lands upstream (squash-merged: same content, different commit).
#    The sync merges cleanly and drops the now-stale carry.
upstream_commit "upstream: the carried change, squashed" bash -c 'echo "fn c() {}" >> src/lib.rs'
if sync >/dev/null 2>&1; then
    cd "$work/fork"
    if ! grep -qx 'src/lib.rs' .fork/carries && scripts/fork-check.sh >/dev/null 2>&1; then
        pass "an upstreamed carry merges cleanly and is dropped from .fork/carries"
    else
        fail "stale carry not dropped: $(tr '\n' ' ' < .fork/carries)"
    fi
else
    fail "merge of an upstreamed carry did not succeed"
fi

# 6. A carry's upstream PR #7 landed reworked (same lines, different text).
#    `[take-upstream #7]` takes upstream's file once "(#7)" is upstream; a
#    different upstream change to those lines before then still stops the
#    sync, as does a conflicting carry without the marker.
case_carry() { # <marker> <upstream subject> <expect: take|stop>
    setup
    (cd "$work/fork" && sed -i 's/fn a() {}/fn a() { fork }/' src/lib.rs \
        && printf '## %supstream PR #7: a()\n## continued heading\nsrc/lib.rs\n' "$1" >> .fork/carries \
        && git commit -qam "fork: carry")
    upstream_commit "$2" sed -i 's/fn a() {}/fn a() { upstream }/' src/lib.rs
    out=$(sync 2>&1) && rc=0 || rc=$?
    cd "$work/fork"
    if [ "$3" = take ]; then
        [ "$rc" -eq 0 ] && grep -q 'fn a() { upstream }' src/lib.rs && ! grep -qx 'src/lib.rs' .fork/carries
    else
        [ "$rc" -eq 2 ] && [[ $out == *"src/lib.rs (declared carry"* ]]
    fi
}
if case_carry "[take-upstream #7] " "fix: a(), reviewed (#7)" take; then
    pass "a [take-upstream #7] carry resolves to upstream's rework once (#7) is upstream"
else fail "take-upstream after merge"; fi
if case_carry "[take-upstream #7] " "fix: something else touching a() (#8)" stop; then
    pass "before (#7) is upstream, a conflict on a take-upstream carry still stops the sync"
else fail "take-upstream before merge"; fi
if case_carry "" "fix: a(), reviewed (#7)" stop; then
    pass "a plain carry conflict stops the sync and says it is a carry"
else fail "plain carry conflict"; fi

# 7. Nothing new upstream: exit 3, no merge commit.
setup
out=$(sync 2>&1) && rc=0 || rc=$?
if [ "$rc" -eq 3 ]; then pass "up to date exits 3"; else fail "up to date: rc=$rc $out"; fi

echo
if [ "$failures" -eq 0 ]; then echo "all fork-sync tests passed"; else echo "$failures failure(s)"; exit 1; fi
