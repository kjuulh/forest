#!/bin/sh
# forest:git-init@v1 — `commands/init` implementation.
#
# Inputs (all optional except where noted):
#   INPUT_BRANCH       — branch name (default: main)
#   INPUT_USER_EMAIL   — git user.email (default: forest-bot@local)
#   INPUT_USER_NAME    — git user.name  (default: forest-bot)
#   INPUT_MESSAGE      — commit message (default: "initial commit")
#
# Context:
#   FOREST_WORK_DIR    — directory to init the repo in (set by runner)
#
# Outputs (written to $FOREST_OUTPUT, picked up by the engine):
#   branch=<name>
#   initial_commit_sha=<sha>

set -eu

work_dir="${FOREST_WORK_DIR:-/work}"
branch="${INPUT_BRANCH:-main}"
user_email="${INPUT_USER_EMAIL:-forest-bot@local}"
user_name="${INPUT_USER_NAME:-forest-bot}"
message="${INPUT_MESSAGE:-initial commit}"

cd "$work_dir"
if [ -d .git ]; then
    echo "git-init: $work_dir already a git repo — leaving alone" >&2
    sha=$(git rev-parse HEAD 2>/dev/null || echo "")
    echo "branch=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo "$branch")" >>"$FOREST_OUTPUT"
    echo "initial_commit_sha=$sha" >>"$FOREST_OUTPUT"
    echo "already_initialized=true" >>"$FOREST_OUTPUT"
    exit 0
fi

git init -q -b "$branch" .
git config user.email "$user_email"
git config user.name  "$user_name"
git commit --allow-empty -q -m "$message"

sha=$(git rev-parse HEAD)
{
    echo "branch=$branch"
    echo "initial_commit_sha=$sha"
    echo "already_initialized=false"
} >>"$FOREST_OUTPUT"
