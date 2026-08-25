#!/usr/bin/env python3
"""Point Cargo.lock's `forest` entry at the version Cargo.toml declares.

release-please bumps `apps/forest/crates/forest/Cargo.toml` through its
`extra-files` config but cannot touch `apps/forest/Cargo.lock` — its Rust
strategy looks for a lockfile beside the package, and forest's lives up at the
workspace root. Left alone the lock falls a version behind on every release,
which breaks `cargo build --locked` on a fresh checkout.

`cargo update -p forest` is the obvious fix and the wrong one here: it wants to
load every source in the workspace, and forest-server depends on a *private*
git repo. On a developer machine that already sits in the cargo cache, so
`--offline` appears to work; on a clean CI runner it fails outright with

    can't checkout from 'https://github.com/understory-io/canopy-util-rs':
    you are in the offline mode (--offline)

Doing it as text needs no toolchain, no network and no credentials, and cannot
re-resolve anything by accident: the only line that can change is the `version`
of the workspace's own `forest` entry.
"""

from __future__ import annotations

import sys
import tomllib
from pathlib import Path

PACKAGE = "forest"


def declared_version(manifest: Path) -> str:
    with manifest.open("rb") as fh:
        return tomllib.load(fh)["package"]["version"]


def locked_version(lock: Path) -> str | None:
    with lock.open("rb") as fh:
        for entry in tomllib.load(fh).get("package", []):
            if entry.get("name") == PACKAGE:
                return entry.get("version")
    return None


def rewrite(lock: Path, want: str) -> bool:
    """Set the `forest` entry's version, leaving every other byte alone."""
    lines = lock.read_text().splitlines(keepends=True)

    in_block = False
    is_forest = False
    for i, line in enumerate(lines):
        stripped = line.strip()

        if stripped == "[[package]]":
            in_block, is_forest = True, False
            continue
        # Any other table header ends the block we were in.
        if stripped.startswith("[") and stripped != "[[package]]":
            in_block, is_forest = False, False
            continue

        if not in_block:
            continue

        if stripped == f'name = "{PACKAGE}"':
            is_forest = True
        elif is_forest and stripped.startswith("version = "):
            if stripped == f'version = "{want}"':
                return False
            lines[i] = line[: line.index("version")] + f'version = "{want}"\n'
            lock.write_text("".join(lines))
            return True

    raise SystemExit(f"no [[package]] entry named {PACKAGE!r} in {lock}")


def main() -> int:
    root = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(".")
    manifest = root / "apps/forest/crates/forest/Cargo.toml"
    lock = root / "apps/forest/Cargo.lock"

    want = declared_version(manifest)
    have = locked_version(lock)

    if not rewrite(lock, want):
        print(f"Cargo.lock already at {want}")
        return 0

    print(f"Cargo.lock: {have} -> {want}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
