# Releasing the forest CLI

Stable releases need nothing by hand: merging release-please's `chore(main):
release X.Y.Z` PR creates the `vX.Y.Z` GitHub release, and
[`release.yml`](.github/workflows/release.yml) builds and attaches the tarballs,
checksums and `install.sh`.

## Cutting a prerelease

A prerelease goes through the same workflow. Publishing it needs write access to
`understory-io/forest`.

First ship and install a stable release containing this feature. The already
published `v0.3.16` does not contain it. Choose an untagged core after the actual
feature-containing stable: if that stable is `v0.3.17`, an available next core
could be `0.3.18`, giving `0.3.18-rc.1`. Do not reuse the bootstrap stable's
core. The examples below assume main declares `0.3.17`, that stable tag exists,
and `v0.3.18` does not; adjust them to the versions actually released.

1. Pick a version a release-candidate is meant to precede: the next release's
   core plus a numbered suffix, e.g. `0.3.18-rc.1` after stable `0.3.17`.
   Check every existing stable tag, not just the proposed core's final tag:
   the core must be greater than all of them, even when the tagged commit
   declares an older version. A core equal to main's declaration is accepted
   only before that final or any higher stable tag exists. Never reuse a
   number: a rebuild is `rc.2`.
2. Create the GitHub release marked prerelease, on a commit on main:

   ```bash
   gh release create v0.3.18-rc.1 --repo understory-io/forest \
     --target <commit-sha> --prerelease --latest=false \
     --title v0.3.18-rc.1 --generate-notes
   ```

3. `release.yml` runs on `release: published`. It stamps `0.3.18-rc.1` into the
   build, checks the binary reports it with the commit, and uploads
   `forest-v0.3.18-rc.1-<target>.tar.gz` plus `.sha256` for each platform, then
   `install.sh`.

**Done** when all six assets and `install.sh` are attached and
`forest self update v0.3.18-rc.1` on a test machine prints
`forest 0.3.18-rc.1 (commit <commit-sha>) installed`.

Uploads never overwrite existing assets. Platform jobs publish independently,
not transactionally, so a failed run can leave a partial release and a rerun
can fail on assets already present. Use a new RC number rather than deleting
verified assets to force a green run.

**Refused** (the build fails before uploading anything) when:

- the tag has a prerelease suffix but the release is not marked prerelease, or
  the reverse. Set the release's prerelease flag to agree with its tag, then
  re-run the workflow;
- a stable tag differs from the version `Cargo.toml` declares at that commit;
- a prerelease core is at or below any existing stable tag's core. The guard
  checks all repository tag pages, including final tags with build metadata;
  lookup, pagination and malformed-response errors also fail before stamping
  or uploading. Tag existence is conservative: a final tag blocks an RC even
  before its GitHub release has finished publishing.

The command above passes `--latest=false` so the prerelease is not selected as
GitHub's latest release. Plain `install.sh`, and `forest self update` and the
update notice without `FOREST_PRERELEASE` set, stay on stable releases; the CI installers (`setup-forest`,
`forest-publish.yml`, `publish-build-component.yml`) also select only stable
releases by default. Explicit version arguments or CI pins opt into that version.

Creating the prerelease does not edit release-please's manifest or Cargo version
on main. [`release-please.yml`](.github/workflows/release-please.yml) runs on main
pushes using its own manifest and configuration. Its next hosted stable-release
run after a prerelease has not been verified by this source review.
